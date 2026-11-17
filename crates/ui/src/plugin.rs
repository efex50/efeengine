//! The UI plugin: resources and the main-thread systems that drive the backend.
//!
//! Frame order:
//! - Engine input filter (before `InputState::apply`): every raw input event goes to
//!   `backend.input`; a `Filter::Captured` result counts into this frame's capture. The engine's
//!   render overlay calls `gpu_init` / `gpu_resize` / `render`.
//! - `PreUpdate`, `order::INPUT + 5`: window resizes to `backend.resize`.
//! - `PreUpdate`, `order::INPUT + 10`: `backend.drain_signals`, checks and value write-back
//!   ([`filter_signals`]); the signals become readable `Events<UiSignal>` for the rest of the
//!   frame (`Ctx::send_batch_now`); `InputState::set_capture` is written (backend capture, a
//!   visible modal layer, or an input event the backend consumed this frame).
//! - `PreUpdate`, `order::INPUT + 11`: view message systems (see [`crate::view`]).
//! - `PostUpdate`, `order::LATE + 10`: bindings evaluated, changed values written to the tree.
//! - `PostUpdate`, `order::LATE + 11`: view systems (after bindings).
//! - `PostUpdate`, `order::LATE + 20`: the journal goes to `backend.sync`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use efeengine_core::input::{InputCapture, InputEventKind, WindowEvent};
use efeengine_core::prelude::*;

use efeengine_core::input::{Filter, InputEvent, InputFilter};
use efeengine_core::render::{Gpu, Image, RenderOverlay};

use crate::backend::{UiBackend, UiBackendCx};
use crate::bindings::{BindCx, UiBindings};
use crate::headless::HeadlessBackend;
use crate::journal::UiChange;
use crate::node::NodeKind;
use crate::signal::{UiSignal, filter_signals};
use crate::theme::Theme;
use crate::tree::UiTree;

/// Adds the UI: the [`UiTree`] and [`UiBindings`] resources, the `Events<UiSignal>` queue and the
/// systems above. The default backend is headless (it also inserts a `HeadlessHandle`
/// resource); pick another with [`with_backend`](Self::with_backend).
pub struct UiPlugin {
    backend: RefCell<Option<Box<dyn UiBackend>>>,
    theme: Option<Theme>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    headless: bool,
}

impl Default for UiPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl UiPlugin {
    /// With the web backend on wasm32, else the headless one (tests use `UiTest`, which always
    /// runs headless).
    pub fn new() -> Self {
        Self { backend: RefCell::new(None), theme: None, headless: false }
    }

    /// The default backend for this engine: the iced backend on native when the `iced` feature
    /// is on and the engine has a window (not a headless config), else as [`new`](Self::new).
    fn default_backend(engine: &Engine, headless: bool) -> Option<Box<dyn UiBackend>> {
        #[cfg(all(feature = "iced", not(target_arch = "wasm32")))]
        if !headless && !engine.config().headless {
            return Some(Box::new(crate::native::IcedBackend::new()));
        }
        let _ = (engine, headless);
        None
    }

    /// Always the headless backend (and its `HeadlessHandle` resource), also on wasm32.
    pub fn headless() -> Self {
        Self { headless: true, ..Self::new() }
    }

    pub fn with_backend(backend: impl UiBackend) -> Self {
        Self { backend: RefCell::new(Some(Box::new(backend))), theme: None, headless: false }
    }

    /// Replaces the built-in dark theme.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }
}

impl Plugin for UiPlugin {
    fn name(&self) -> &str {
        "efeengine-ui"
    }

    fn build(&self, engine: &mut Engine) {
        let mut backend: Box<dyn UiBackend> = match self.backend.borrow_mut().take() {
            Some(b) => b,
            #[cfg(target_arch = "wasm32")]
            None if !self.headless => Box::new(crate::web::WebBackend::new()),
            None if Self::default_backend(engine, self.headless).is_some() => {
                Self::default_backend(engine, self.headless).unwrap()
            }
            None => {
                let (b, handle) = HeadlessBackend::new();
                engine.insert_resource(handle);
                Box::new(b)
            }
        };
        let mut tree = UiTree::new();
        if let Some(t) = &self.theme {
            tree.set_theme(t.clone());
            tree.take_changes();
        }
        engine.insert_resource(tree);
        engine.insert_resource(UiBindings::new());
        engine.add_event::<UiSignal>();

        let mut bcx = UiBackendCx::new((0.0, 0.0), 1.0);
        backend.init(&mut bcx);
        let backend: Shared = Rc::new(RefCell::new(backend));
        let bcx = Rc::new(RefCell::new(bcx));
        // Pointer / keyboard input the backend consumed since the last `ui_signals` run.
        let consumed = Rc::new(Cell::new((false, false)));

        // Raw input: the engine's input filter hands every event to the backend before the game
        // sees it. `try_borrow_mut`: a filter call can never re-enter a system holding the
        // backend (it runs while events are applied, not inside a system), but never panic.
        engine.add_input_filter(UiInputFilter { backend: backend.clone(), consumed: consumed.clone() });

        // Native overlay: GPU init / resize / draw.
        engine.add_render_overlay(UiOverlay { backend: backend.clone() });

        // Window size changes (logical size and scale for the backend and `UiBackendCx`).
        {
            let backend = backend.clone();
            let bcx = bcx.clone();
            let physical = Cell::new((0u32, 0u32));
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let mut size_changed = false;
                    let mut scale = bcx.borrow().scale_factor;
                    for ev in cx.input.events() {
                        match &ev.kind {
                            InputEventKind::Window(WindowEvent::Resized { width, height }) => {
                                physical.set((*width, *height));
                                size_changed = true;
                            }
                            InputEventKind::Window(WindowEvent::ScaleFactorChanged { scale: s }) => {
                                scale = *s as f32;
                                size_changed = true;
                            }
                            _ => {}
                        }
                    }
                    if size_changed {
                        let (w, h) = physical.get();
                        let logical = (w as f32 / scale, h as f32 / scale);
                        let mut c = bcx.borrow_mut();
                        c.logical_size = logical;
                        c.scale_factor = scale;
                        backend.borrow_mut().resize(logical, scale);
                    }
                })
                .order(order::INPUT + 5)
                .name("ui_resize");
        }

        // Signals in, capture out.
        {
            let backend = backend.clone();
            let consumed = consumed.clone();
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let mut raw = Vec::new();
                    let (capture, text_input, cursor_icon) = {
                        let mut be = backend.borrow_mut();
                        be.drain_signals(&mut raw);
                        (be.capture(), be.take_text_input(), be.take_cursor_icon())
                    };
                    if let Some(icon) = cursor_icon {
                        cx.commands.set_cursor_icon(icon);
                    }
                    match text_input {
                        Some(Some(a)) => {
                            cx.commands.set_text_input(true);
                            cx.commands.set_text_input_area(a.x, a.y, a.w, a.h, a.cursor);
                        }
                        Some(None) => cx.commands.set_text_input(false),
                        None => {}
                    }
                    let (cp, ck) = consumed.replace((false, false));
                    let Some(tree) = cx.resources.get_mut::<UiTree>() else { return };
                    let signals = filter_signals(tree, raw);
                    let modal = tree.modal_active();
                    cx.send_batch_now(signals);
                    cx.input.set_capture(InputCapture {
                        pointer: capture.pointer || modal || cp,
                        keyboard: capture.keyboard || modal || ck,
                    });
                })
                .order(order::INPUT + 10)
                .name("ui_signals");
        }

        // Bindings. The `UiBindings` resource is borrowed in place and written (which stamps its
        // change tick) only when the tree has new registrations or removals.
        engine
            .add_system(Stage::PostUpdate, |cx| {
                if cx.resources.get::<UiTree>().is_some_and(|t| t.has_binding_work())
                    && let Some(mut tree) = cx.resources.remove::<UiTree>()
                {
                    if let Some(binds) = cx.resources.get_mut::<UiBindings>() {
                        binds.absorb(&mut tree);
                    }
                    cx.resources.insert(tree);
                }
                let updates = match cx.resources.get::<UiBindings>() {
                    Some(binds) if !binds.is_empty() => {
                        binds.evaluate(&BindCx { resources: &*cx.resources, world: &*cx.world, time: cx.time })
                    }
                    _ => return,
                };
                if !updates.is_empty()
                    && let Some(tree) = cx.resources.get_mut::<UiTree>()
                {
                    for u in &updates {
                        u.apply(tree);
                    }
                }
            })
            .order(order::LATE + 10)
            .name("ui_bindings");

        // Sync. Image textures: `seen` remembers the image (by `Arc`) each texture had when it
        // was last handed to the backend; a texture replaced in `Textures` since then makes
        // every `Image` node that shows it get a synthesized `UiChange::Kind` plus the new image
        // in `UiBackendCx::images`, exactly like a node whose image was just set.
        {
            let backend = backend.clone();
            let bcx = bcx.clone();
            let seen: RefCell<HashMap<TextureHandle, Arc<Image>>> = RefCell::new(HashMap::new());
            engine
                .add_system(Stage::PostUpdate, move |cx| {
                    let Some(tree) = cx.resources.get_mut::<UiTree>() else { return };
                    let mut changes = tree.take_changes();
                    let Some(tree) = cx.resources.get::<UiTree>() else { return };
                    let textures = cx.resources.get::<Textures>();
                    let mut seen = seen.borrow_mut();
                    let mut images: Vec<(TextureHandle, Arc<Image>)> = Vec::new();
                    if let Some(textures) = textures {
                        for ch in &changes {
                            let (UiChange::Created(id) | UiChange::Kind(id)) = ch else { continue };
                            if let Some(NodeKind::Image { texture, .. }) = tree.kind(*id)
                                && let Some(img) = textures.get(*texture)
                                && !images.iter().any(|(h, _)| h == texture)
                            {
                                images.push((*texture, img.clone()));
                            }
                        }
                        // Replaced (or removed) textures behind existing image nodes.
                        let stale: Vec<TextureHandle> = seen
                            .iter()
                            .filter(|(h, old)| textures.get(**h).is_none_or(|now| !Arc::ptr_eq(now, old)))
                            .map(|(h, _)| *h)
                            .collect();
                        for h in stale {
                            let _ = seen.remove(&h);
                            let Some(img) = textures.get(h) else { continue };
                            for (id, n) in tree.iter() {
                                if matches!(n.kind(), NodeKind::Image { texture, .. } if *texture == h) {
                                    if !changes.contains(&UiChange::Kind(id)) && !changes.contains(&UiChange::Created(id)) {
                                        changes.push(UiChange::Kind(id));
                                    }
                                    if !images.iter().any(|(t, _)| *t == h) {
                                        images.push((h, img.clone()));
                                    }
                                }
                            }
                        }
                    }
                    if changes.is_empty() {
                        return;
                    }
                    for (h, img) in &images {
                        let _ = seen.insert(*h, img.clone());
                    }
                    let mut bc = bcx.borrow_mut();
                    bc.images = images;
                    backend.borrow_mut().sync(&tree, &changes, tree.theme(), &mut bc);
                    bc.images.clear();
                })
                .order(order::LATE + 20)
                .name("ui_sync");
        }
    }
}

type Shared = Rc<RefCell<Box<dyn UiBackend>>>;

struct UiInputFilter {
    backend: Shared,
    consumed: Rc<Cell<(bool, bool)>>,
}

impl InputFilter for UiInputFilter {
    fn filter(&mut self, ev: &InputEvent) -> Filter {
        let Ok(mut be) = self.backend.try_borrow_mut() else { return Filter::Pass };
        let f = be.input(ev);
        if f == Filter::Captured {
            let (mut p, mut k) = self.consumed.get();
            match &ev.kind {
                InputEventKind::Key { .. } | InputEventKind::Text(_) | InputEventKind::TextEditing { .. } => k = true,
                InputEventKind::MouseMotion { .. }
                | InputEventKind::MouseButton { .. }
                | InputEventKind::MouseWheel { .. }
                | InputEventKind::Touch { .. } => p = true,
                InputEventKind::Window(_) => {}
            }
            self.consumed.set((p, k));
        }
        f
    }
}

struct UiOverlay {
    backend: Shared,
}

impl RenderOverlay for UiOverlay {
    fn init(&mut self, gpu: &Gpu) {
        self.backend.borrow_mut().gpu_init(gpu);
    }
    fn resize(&mut self, gpu: &Gpu) {
        self.backend.borrow_mut().gpu_resize(gpu);
    }
    fn render(&mut self, gpu: &Gpu, target: &efeengine_core::wgpu::TextureView) {
        self.backend.borrow_mut().render(gpu, target);
    }
}
