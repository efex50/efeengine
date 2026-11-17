//! Bindings: read-only functions from game state to one node property (API.md section 8).

use std::sync::Arc;

use efeengine_core::engine::resources::{Res, Resources};
use efeengine_core::engine::time::Time;
use efeengine_core::hecs::{Component, Entity, World};
use efeengine_core::prelude::TextureHandle;

use crate::id::WidgetId;
use crate::signal::UiValue;
use crate::tree::UiTree;

/// What a binding function may read.
pub struct BindCx<'a> {
    pub resources: &'a Resources,
    pub world: &'a World,
    pub time: &'a Time,
}

impl BindCx<'_> {
    pub fn res<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        self.resources.get::<T>()
    }
}

/// A read-only function from game state to a value of type `T`. `None` (resource or component
/// missing) leaves the property as it is. `Send + Sync`.
pub struct Bind<T> {
    f: Arc<dyn Fn(&BindCx) -> Option<T> + Send + Sync>,
}

impl<T> Clone for Bind<T> {
    fn clone(&self) -> Self {
        Self { f: self.f.clone() }
    }
}

impl<T: 'static> Bind<T> {
    /// From resource `R`: `Bind::res(|s: &Score| format!("Score: {}", s.value))`.
    pub fn res<R: Send + Sync + 'static>(f: impl Fn(&R) -> T + Send + Sync + 'static) -> Self {
        Self { f: Arc::new(move |cx| cx.resources.get::<R>().map(|r| f(&r))) }
    }

    /// From component `C` of `entity` (`None` while the entity or component is missing):
    /// `Bind::query::<Health>(player, |h| h.hp / h.max)`.
    pub fn query<C: Component>(entity: Entity, f: impl Fn(&C) -> T + Send + Sync + 'static) -> Self {
        Self { f: Arc::new(move |cx| cx.world.get::<&C>(entity).ok().map(|c| f(&c))) }
    }

    /// From anything the [`BindCx`] reads (several resources, queries, time).
    pub fn with(f: impl Fn(&BindCx) -> Option<T> + Send + Sync + 'static) -> Self {
        Self { f: Arc::new(f) }
    }

    /// Evaluates the binding.
    pub fn eval(&self, cx: &BindCx) -> Option<T> {
        (self.f)(cx)
    }

    pub(crate) fn erase(self, wrap: fn(T) -> BoundValue) -> Erased {
        Arc::new(move |cx| (self.f)(cx).map(wrap))
    }
}

pub(crate) type Erased = Arc<dyn Fn(&BindCx) -> Option<BoundValue> + Send + Sync>;

/// A text given at node creation: a fixed string or a binding.
pub enum TextSrc {
    Static(String),
    Bound(Bind<String>),
}

impl TextSrc {
    pub(crate) fn split(self) -> (String, Option<Bind<String>>) {
        match self {
            TextSrc::Static(s) => (s, None),
            TextSrc::Bound(b) => (String::new(), Some(b)),
        }
    }
}

impl From<&str> for TextSrc {
    fn from(s: &str) -> Self {
        TextSrc::Static(s.to_string())
    }
}

impl From<String> for TextSrc {
    fn from(s: String) -> Self {
        TextSrc::Static(s)
    }
}

impl From<Bind<String>> for TextSrc {
    fn from(b: Bind<String>) -> Self {
        TextSrc::Bound(b)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BindTarget {
    Text,
    Value,
    Checked,
    Visible,
    Enabled,
    Class,
    Image,
}

/// A value a binding produced.
#[derive(Clone, PartialEq, Debug)]
pub enum BoundValue {
    Text(String),
    F32(f32),
    Bool(bool),
    Image(TextureHandle),
}

/// A changed binding result, to apply to the tree.
#[derive(Clone, PartialEq, Debug)]
pub struct BindUpdate {
    pub widget: WidgetId,
    pub target: BindTarget,
    pub value: BoundValue,
}

impl BindUpdate {
    /// Writes the value into the tree (a normal change: it reaches the journal only if the
    /// node actually changes). `false` if the id is stale or the value does not fit.
    pub fn apply(&self, tree: &mut UiTree) -> bool {
        let id = self.widget;
        match (self.target, &self.value) {
            (BindTarget::Text, BoundValue::Text(t)) => tree.set_text(id, t.clone()),
            (BindTarget::Value, BoundValue::F32(v)) => tree.set_value(id, UiValue::F32(*v)),
            (BindTarget::Checked, BoundValue::Bool(b)) => tree.set_value(id, UiValue::Bool(*b)),
            (BindTarget::Visible, BoundValue::Bool(b)) => tree.set_visible(id, *b),
            (BindTarget::Enabled, BoundValue::Bool(b)) => tree.set_enabled(id, *b),
            (BindTarget::Class, BoundValue::Text(c)) => tree.set_class(id, c),
            (BindTarget::Image, BoundValue::Image(t)) => tree.set_image(id, *t),
            _ => false,
        }
    }
}

struct Entry {
    id: WidgetId,
    target: BindTarget,
    eval: Erased,
    last: Option<BoundValue>,
}

/// All bindings, keyed by widget. A resource next to the [`UiTree`]; the UI plugin evaluates it
/// once per frame at `order::LATE + 10` in `PostUpdate`.
#[derive(Default)]
pub struct UiBindings {
    entries: Vec<Entry>,
}

impl UiBindings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `true` if `id` has any binding.
    pub fn is_bound(&self, id: WidgetId) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// Drops all bindings of a widget.
    pub fn remove(&mut self, id: WidgetId) {
        self.entries.retain(|e| e.id != id);
    }

    pub(crate) fn add(&mut self, id: WidgetId, target: BindTarget, eval: Erased) {
        // A second binding of the same property replaces the first.
        self.entries.retain(|e| !(e.id == id && e.target == target));
        self.entries.push(Entry { id, target, eval, last: None });
    }

    /// Takes the bindings registered on the tree since the last call and forgets the bindings
    /// of removed widgets.
    pub fn absorb(&mut self, tree: &mut UiTree) {
        for id in tree.take_removed() {
            self.remove(id);
        }
        for p in std::mem::take(&mut tree.pending_binds) {
            if tree.contains(p.id) {
                self.add(p.id, p.target, p.eval);
            }
        }
    }

    /// Evaluates every binding and returns the ones whose value changed since the last
    /// evaluation (the first evaluation always reports).
    pub fn evaluate(&mut self, cx: &BindCx) -> Vec<BindUpdate> {
        let mut out = Vec::new();
        for e in &mut self.entries {
            let Some(v) = (e.eval)(cx) else { continue };
            if e.last.as_ref() != Some(&v) {
                e.last = Some(v.clone());
                out.push(BindUpdate { widget: e.id, target: e.target, value: v });
            }
        }
        out
    }
}
