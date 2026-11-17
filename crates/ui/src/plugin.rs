//! The UI plugin: resources and the main-thread systems that drive the backend.
//!
//! Frame order:
//! - `PreUpdate`, `order::INPUT + 5`: raw input events go to `backend.input`, window resizes to
//!   `backend.resize`. A `Filter::Captured` result counts into this frame's capture.
//! - `PreUpdate`, `order::INPUT + 10`: `backend.drain_signals`, checks and value write-back
//!   ([`filter_signals`]); the signals become readable `Events<UiSignal>` for the rest of the
//!   frame (`Ctx::send_batch_now`); `InputState::set_capture` is written (backend capture, a
//!   visible modal layer, or an input event the backend consumed this frame).
//! - `PreUpdate`, `order::INPUT + 11`: view message systems (see [`crate::view`]).
//! - `PostUpdate`, `order::LATE + 10`: bindings evaluated, changed values written to the tree.
//! - `PostUpdate`, `order::LATE + 11`: view systems (after bindings).
//! - `PostUpdate`, `order::LATE + 20`: the journal goes to `backend.sync`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use efeengine_core::input::{InputCapture, InputEventKind, WindowEvent};
use efeengine_core::prelude::*;

use crate::backend::{Filter, UiBackend, UiBackendCx};
use crate::bindings::{BindCx, UiBindings};
use crate::headless::HeadlessBackend;
use crate::signal::{UiSignal, filter_signals};
use crate::theme::Theme;
use crate::tree::UiTree;

/// Adds the UI: the [`UiTree`] and [`UiBindings`] resources, the `Events<UiSignal>` queue and the
/// systems above. The default backend is headless (it also inserts a `HeadlessHandle`
/// resource); pick another with [`with_backend`](Self::with_backend).
pub struct UiPlugin {
    backend: RefCell<Option<Box<dyn UiBackend>>>,
    theme: Option<Theme>,
}

impl Default for UiPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl UiPlugin {
    /// With the headless backend.
    pub fn new() -> Self {
        Self { backend: RefCell::new(None), theme: None }
    }

    pub fn with_backend(backend: impl UiBackend) -> Self {
        Self { backend: RefCell::new(Some(Box::new(backend))), theme: None }
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

        let mut bcx = UiBackendCx { logical_size: (0.0, 0.0), scale_factor: 1.0 };
        backend.init(&mut bcx);
        let backend = Rc::new(RefCell::new(backend));
        let bcx = Rc::new(RefCell::new(bcx));
        // Pointer / keyboard input the backend consumed since the last `ui_signals` run.
        let consumed = Rc::new(Cell::new((false, false)));

        // Raw input and window changes.
        {
            let backend = backend.clone();
            let bcx = bcx.clone();
            let consumed = consumed.clone();
            let physical = Cell::new((0u32, 0u32));
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let Some(tree) = cx.resources.get::<UiTree>() else { return };
                    let mut be = backend.borrow_mut();
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
                        if be.input(ev, &tree) == Filter::Captured {
                            let (mut p, mut k) = consumed.get();
                            match &ev.kind {
                                InputEventKind::Key { .. } | InputEventKind::Text(_) => k = true,
                                InputEventKind::MouseMotion { .. }
                                | InputEventKind::MouseButton { .. }
                                | InputEventKind::MouseWheel { .. }
                                | InputEventKind::Touch { .. } => p = true,
                                InputEventKind::Window(_) => {}
                            }
                            consumed.set((p, k));
                        }
                    }
                    if size_changed {
                        let (w, h) = physical.get();
                        let logical = (w as f32 / scale, h as f32 / scale);
                        let mut c = bcx.borrow_mut();
                        c.logical_size = logical;
                        c.scale_factor = scale;
                        be.resize(logical, scale);
                    }
                })
                .order(order::INPUT + 5)
                .name("ui_input");
        }

        // Signals in, capture out.
        {
            let backend = backend.clone();
            let consumed = consumed.clone();
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let mut raw = Vec::new();
                    let capture = {
                        let mut be = backend.borrow_mut();
                        be.drain_signals(&mut raw);
                        be.capture()
                    };
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

        // Sync.
        {
            let backend = backend.clone();
            let bcx = bcx.clone();
            engine
                .add_system(Stage::PostUpdate, move |cx| {
                    let Some(tree) = cx.resources.get_mut::<UiTree>() else { return };
                    let changes = tree.take_changes();
                    if changes.is_empty() {
                        return;
                    }
                    let Some(tree) = cx.resources.get::<UiTree>() else { return };
                    backend.borrow_mut().sync(&tree, &changes, tree.theme(), &mut bcx.borrow_mut());
                })
                .order(order::LATE + 20)
                .name("ui_sync");
        }
    }
}
