//! `UiView`: the plugin that runs a view function and reconciles its result.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use efeengine_core::engine::resources::{Res, Resources};
use efeengine_core::engine::time::Time;
use efeengine_core::hecs::World;
use efeengine_core::prelude::*;

use crate::id::{Owner, WidgetId};
use crate::node::Layer;
use crate::signal::UiSignal;
use crate::tree::UiTree;
use crate::view::el::{El, Handlers};
use crate::view::reconcile::{Table, reconcile_root, validate};

/// What a view function may read (resources, the world and time), like a binding.
pub struct ViewCx<'a> {
    pub resources: &'a Resources,
    pub world: &'a World,
    pub time: &'a Time,
}

impl ViewCx<'_> {
    pub fn res<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        self.resources.get::<T>()
    }
}

type Check = Arc<dyn for<'a> Fn(&Ctx<'a>) -> bool + Send + Sync>;

/// When a view runs (besides its first run and the frames where one of its widgets sent a
/// message): true if any of the checks is. Combine with [`or`](Self::or) or by calling
/// [`UiView::when`] several times.
#[derive(Clone)]
pub struct RunCondition {
    checks: Vec<Check>,
}

/// True when resource `T` was inserted or changed since the view last ran (`Ctx::res_changed`).
pub fn changed<T: Send + Sync + 'static>() -> RunCondition {
    RunCondition { checks: vec![Arc::new(|cx: &Ctx| cx.res_changed::<T>())] }
}

impl RunCondition {
    /// A condition from any function of the system context.
    pub fn custom(f: impl for<'a> Fn(&Ctx<'a>) -> bool + Send + Sync + 'static) -> Self {
        Self { checks: vec![Arc::new(f)] }
    }

    /// True if `self` or `other` is.
    pub fn or(mut self, other: RunCondition) -> Self {
        self.checks.extend(other.checks);
        self
    }

    fn eval(&self, cx: &Ctx) -> bool {
        // Evaluate every check (no short circuit needed: they are pure reads).
        self.checks.iter().fold(false, |acc, c| c(cx) | acc)
    }
}

/// Which views are enabled; a resource inserted by the first [`UiView`]. Flip a view at runtime
/// with `cx.resources.get_mut::<UiViews>().unwrap().set_enabled("pause", true)`: a disabled view
/// hides its layer and does not run; enabling it shows the layer and runs it again at once.
#[derive(Default, Debug)]
pub struct UiViews {
    enabled: HashMap<Arc<str>, bool>,
}

impl UiViews {
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        self.enabled.insert(name.into(), enabled);
    }

    /// `true` for unknown names too.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.enabled.get(name).copied().unwrap_or(true)
    }

    pub fn toggle(&mut self, name: &str) {
        let v = !self.is_enabled(name);
        self.set_enabled(name, v);
    }
}

type BuildFn<Msg> = Box<dyn Fn(&ViewCx) -> Option<El<Msg>>>;

/// One view = one layer. Plugin name `efeengine-ui/view/<name>`, depends on `efeengine-ui`; the
/// layer key is `<name>`; every node of the view is owned by `Owner::Plugin(<plugin name>)`.
pub struct UiView<Msg> {
    name: String,
    plugin_name: String,
    build: RefCell<Option<BuildFn<Msg>>>,
    layer: Layer,
    when: Option<RunCondition>,
    enabled: bool,
}

impl<Msg: Clone + Send + Sync + 'static> UiView<Msg> {
    /// A view of resource `S`. While `S` is missing the view does nothing.
    pub fn new<S: Send + Sync + 'static>(name: &str, f: impl Fn(&S) -> El<Msg> + 'static) -> Self {
        Self::make(name, Box::new(move |cx| cx.res::<S>().map(|s| f(&s))))
    }

    /// A view that reads whatever it needs from a [`ViewCx`].
    pub fn with(name: &str, f: impl Fn(&ViewCx) -> El<Msg> + 'static) -> Self {
        Self::make(name, Box::new(move |cx| Some(f(cx))))
    }

    fn make(name: &str, build: BuildFn<Msg>) -> Self {
        Self {
            name: name.to_string(),
            plugin_name: format!("efeengine-ui/view/{name}"),
            build: RefCell::new(Some(build)),
            layer: Layer::fill(0),
            when: None,
            enabled: true,
        }
    }

    /// The layer data (z, anchor, modal, offset); default `Layer::fill(0)`.
    pub fn layer(mut self, layer: Layer) -> Self {
        self.layer = layer;
        self
    }

    /// Run only when `cond` holds. Repeated calls are OR-ed. Without it the view runs every frame.
    pub fn when(mut self, cond: RunCondition) -> Self {
        self.when = Some(match self.when.take() {
            Some(c) => c.or(cond),
            None => cond,
        });
        self
    }

    /// The initial enabled state (see [`UiViews`]).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

struct Shared<Msg> {
    table: Table<Msg>,
    /// Run next time even if the condition says no.
    dirty: bool,
    /// The layer was last shown (enabled and run).
    shown: bool,
    layer: Option<WidgetId>,
}

impl<Msg: Clone + Send + Sync + 'static> Plugin for UiView<Msg> {
    fn name(&self) -> &str {
        &self.plugin_name
    }

    fn dependencies(&self) -> &[&'static str] {
        &["efeengine-ui"]
    }

    fn build(&self, engine: &mut Engine) {
        let build = self.build.borrow_mut().take().expect("UiView built twice");
        engine.add_event::<Msg>();
        if !engine.resources().contains::<UiViews>() {
            engine.insert_resource(UiViews::default());
        }
        {
            let views = engine.resources_mut().get_mut::<UiViews>().expect("inserted above");
            views.enabled.entry(self.name.as_str().into()).or_insert(self.enabled);
        }
        let shared = Rc::new(RefCell::new(Shared::<Msg> { table: HashMap::new(), dirty: true, shown: false, layer: None }));
        let owner = Owner::plugin(&self.plugin_name);
        let name = self.name.clone();

        // Signals -> messages, in the same frame.
        {
            let shared = shared.clone();
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let mut sh = shared.borrow_mut();
                    if sh.table.is_empty() {
                        return;
                    }
                    let msgs: Vec<Msg> = cx
                        .events::<UiSignal>()
                        .iter()
                        .filter_map(|s| sh.table.get(&s.widget).and_then(|h: &Handlers<Msg>| h.dispatch(&s.kind)))
                        .collect();
                    if msgs.is_empty() {
                        return;
                    }
                    sh.dirty = true;
                    drop(sh);
                    cx.send_batch_now(msgs);
                })
                .order(order::INPUT + 11)
                .name("ui_view_messages");
        }

        // Reconcile.
        let layer_cfg = self.layer;
        let when = self.when.clone();
        engine
            .add_system(Stage::PostUpdate, move |cx| {
                let enabled = cx.resources.get::<UiViews>().is_none_or(|v| v.is_enabled(&name));
                let mut sh = shared.borrow_mut();
                if !enabled {
                    if sh.shown {
                        sh.shown = false;
                        sh.table.clear();
                        if let Some(layer) = sh.layer
                            && let Some(tree) = cx.resources.get_mut::<UiTree>()
                        {
                            tree.set_visible(layer, false);
                        }
                    }
                    return;
                }
                let force = sh.dirty || !sh.shown;
                if !force && let Some(c) = &when && !c.eval(cx) {
                    return;
                }
                // Build the whole description first: a panic here applies nothing.
                let el = {
                    let vcx = ViewCx { resources: &*cx.resources, world: &*cx.world, time: cx.time };
                    build(&vcx)
                };
                let Some(el) = el else { return };
                validate(&el, &name, &mut HashSet::new());
                let Some(tree) = cx.resources.get_mut::<UiTree>() else { return };
                let mut table = Table::<Msg>::new();
                let layer = tree.with_owner(owner.clone(), |tree| {
                    let layer = tree.layer(&name, layer_cfg);
                    tree.set_visible(layer, true);
                    reconcile_root(tree, layer, &name, el, &mut table);
                    layer
                });
                sh.table = table;
                sh.layer = Some(layer);
                sh.shown = true;
                sh.dirty = false;
            })
            .order(order::LATE + 11)
            .name("ui_view");
    }
}
