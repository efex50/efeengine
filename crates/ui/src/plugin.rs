//! The UI plugin: resources and the main-thread systems that drive the backend.
//!
//! Frame order:
//! - `PreUpdate`, `order::INPUT + 5`: raw input events go to `backend.input`, window resizes to
//!   `backend.resize`.
//! - `PreUpdate`, `order::INPUT + 10`: `backend.drain_signals`, checks and value write-back
//!   ([`filter_signals`]), [`UiSignals`] refilled, [`UiCapture`] updated.
//! - `PostUpdate`, `order::LATE + 10`: bindings evaluated, changed values written to the tree.
//! - `PostUpdate`, `order::LATE + 20`: the journal goes to `backend.sync`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use efeengine_core::input::{InputEventKind, WindowEvent};
use efeengine_core::prelude::*;

use crate::backend::{UiBackend, UiBackendCx};
use crate::bindings::{BindCx, UiBindings};
use crate::capture::UiCapture;
use crate::headless::HeadlessBackend;
use crate::signal::{UiSignals, filter_signals};
use crate::theme::Theme;
use crate::tree::UiTree;

/// Adds the UI: the [`UiTree`], [`UiBindings`], [`UiSignals`] and [`UiCapture`] resources and the
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
        engine.insert_resource(UiSignals::default());
        engine.insert_resource(UiCapture::default());

        let mut bcx = UiBackendCx { logical_size: (0.0, 0.0), scale_factor: 1.0 };
        backend.init(&mut bcx);
        let backend = Rc::new(RefCell::new(backend));
        let bcx = Rc::new(RefCell::new(bcx));

        // Raw input and window changes.
        {
            let backend = backend.clone();
            let bcx = bcx.clone();
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
                        be.input(ev, &tree);
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
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let mut raw = Vec::new();
                    let capture = {
                        let mut be = backend.borrow_mut();
                        be.drain_signals(&mut raw);
                        be.capture()
                    };
                    let Some(tree) = cx.resources.get_mut::<UiTree>() else { return };
                    let signals = filter_signals(tree, raw);
                    let modal = tree.modal_active();
                    if let Some(s) = cx.resources.get_mut::<UiSignals>() {
                        s.set(signals);
                    }
                    if let Some(c) = cx.resources.get_mut::<UiCapture>() {
                        *c = UiCapture { pointer: capture.pointer || modal, keyboard: capture.keyboard || modal };
                    }
                })
                .order(order::INPUT + 10)
                .name("ui_signals");
        }

        // Bindings.
        engine
            .add_system(Stage::PostUpdate, |cx| {
                let Some(mut binds) = cx.resources.remove::<UiBindings>() else { return };
                if let Some(tree) = cx.resources.get_mut::<UiTree>() {
                    binds.absorb(tree);
                }
                let updates = binds.evaluate(&BindCx { resources: &*cx.resources, world: &*cx.world, time: cx.time });
                cx.resources.insert(binds);
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
