//! The UI plugin: resources and the main-thread systems that drive the backend.
//!
//! One backend instance per window that has layers (`Layer::window`); the main window's exists
//! from the start, the others are made by the window-backend factory when their first layer and
//! the window both exist (see [`UiPlugin::with_window_backends`]). A headless backend handles
//! every window in one instance.
//!
//! Frame order:
//! - Engine input filter (before `InputState::apply`): every raw input event goes to the
//!   `backend.input` of its window; a `Filter::Captured` result counts into this frame's capture. The engine's
//!   render overlay calls `gpu_init` / `gpu_resize` / `render` per window.
//! - `PreUpdate`, `order::INPUT + 5`: window resizes to `backend.resize`.
//! - `PreUpdate`, `order::INPUT + 10`: `backend.drain_signals`, checks and value write-back
//!   ([`filter_signals`]); the signals become readable `Events<UiSignal>` for the rest of the
//!   frame (`Ctx::send_batch_now`); `InputState::set_capture` is written (backend capture, a
//!   visible modal layer, or an input event the backend consumed this frame).
//! - `PreUpdate`, `order::INPUT + 11`: view message systems (see [`crate::view`]).
//! - `PreUpdate`, `order::INPUT + 12`: this frame's signals become `ui.signal` script events
//!   (see [`crate::scripting`]).
//! - `PostUpdate`, `order::LATE + 10`: bindings evaluated, changed values written to the tree.
//! - `PostUpdate`, `order::LATE + 11`: view systems (after bindings).
//! - `PostUpdate`, `order::LATE + 15`: the operations scripts and mods queued are applied to the tree.
//! - `PostUpdate`, `order::LATE + 20`: the journal goes to `backend.sync`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use efeengine_core::input::{InputCapture, InputEventKind};
use efeengine_core::prelude::*;

use efeengine_core::input::{Filter, InputEvent, InputFilter};
use efeengine_core::render::{Gpu, Image, RenderOverlay};

use crate::backend::{UiBackend, UiBackendCx};
use crate::capture::UiCapture;
use crate::backend::ImageIdExt;
use crate::bindings::{UiBindings, engine_bind_cx};
use crate::headless::HeadlessBackend;
use efeengine_core::engine::config::GraphicsMode;
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
    factory: RefCell<Option<WindowBackends>>,
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
        Self { backend: RefCell::new(None), factory: RefCell::new(None), theme: None, headless: false }
    }

    /// Whether the native default applies: a native build, a window, and some native backend
    /// compiled in. The backend itself is chosen later, when the graphics mode is known (see
    /// [`native_backend_for`](Self::native_backend_for)).
    fn defers_native(engine: &Engine, headless: bool) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if !headless && !engine.config().headless {
            return cfg!(any(feature = "iced", feature = "fallback"));
        }
        let _ = (engine, headless);
        false
    }

    /// The native default for the graphics mode the engine came up with: iced on wgpu, else the
    /// fallback backend (SDL mode, or no iced compiled in). `None` = nothing compiled in.
    #[cfg(not(target_arch = "wasm32"))]
    fn native_backend_for(mode: GraphicsMode) -> Option<Box<dyn UiBackend>> {
        #[cfg(feature = "iced")]
        if mode == GraphicsMode::Wgpu {
            efeengine_core::prelude::global_info("[efeengine-ui] UI backend: iced (wgpu)");
            return Some(Box::new(crate::native::IcedBackend::new()));
        }
        #[cfg(feature = "fallback")]
        {
            efeengine_core::prelude::global_info(&format!("[efeengine-ui] UI backend: fallback ({mode:?} mode)"));
            return Some(Box::new(crate::fallback::FallbackBackend::new()));
        }
        #[allow(unreachable_code)]
        {
            efeengine_core::prelude::global_warn(&format!(
                "[efeengine-ui] no UI backend for {mode:?} mode (enable the `fallback` feature for SDL mode); no UI is drawn"
            ));
            None
        }
    }

    /// The default way to make a secondary window's backend (see [`with_window_backends`](Self::with_window_backends)).
    fn default_factory(engine: &Engine, headless: bool) -> Option<WindowBackends> {
        #[cfg(target_arch = "wasm32")]
        if !headless && !engine.config().headless {
            return Some(Box::new(|_| Some(Box::new(crate::web::WebBackend::new()) as Box<dyn UiBackend>)));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if Self::defers_native(engine, headless) {
            // Secondary windows exist in the wgpu modes only (SDL mode closes them), so this is
            // iced when compiled in; the fallback otherwise.
            return Some(Box::new(|_| {
                #[cfg(feature = "iced")]
                return Some(Box::new(crate::native::IcedBackend::new()) as Box<dyn UiBackend>);
                #[allow(unreachable_code)]
                #[cfg(feature = "fallback")]
                return Some(Box::new(crate::fallback::FallbackBackend::new()) as Box<dyn UiBackend>);
                #[allow(unreachable_code)]
                None
            }));
        }
        let _ = (engine, headless);
        None
    }

    /// The fallback backend (taffy + cosmic-text through `Draw2D`): works in the wgpu and in the
    /// SDL rendering mode. Needs the `fallback` feature and a native build; otherwise it is the
    /// same as [`new`](Self::new).
    pub fn fallback() -> Self {
        #[cfg(all(feature = "fallback", not(target_arch = "wasm32")))]
        {
            let p = Self::with_backend(crate::fallback::FallbackBackend::new());
            *p.factory.borrow_mut() = Some(Box::new(|_| Some(Box::new(crate::fallback::FallbackBackend::new()) as Box<dyn UiBackend>)));
            return p;
        }
        #[allow(unreachable_code)]
        Self::new()
    }

    /// Always the headless backend (and its `HeadlessHandle` resource), also on wasm32.
    pub fn headless() -> Self {
        Self { headless: true, ..Self::new() }
    }

    pub fn with_backend(backend: impl UiBackend) -> Self {
        Self { backend: RefCell::new(Some(Box::new(backend))), factory: RefCell::new(None), theme: None, headless: false }
    }

    /// How secondary windows get their backend instance: called with the window id once the window
    /// exists and has a layer; `None` = no UI in that window. The default makes a web backend
    /// (wasm) or an iced backend (native, `iced` feature), and none for a headless engine or
    /// plugin.
    pub fn with_window_backends(self, f: impl Fn(u32) -> Option<Box<dyn UiBackend>> + 'static) -> Self {
        *self.factory.borrow_mut() = Some(Box::new(f));
        self
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
        let mut headless_handle = None;
        let mut deferred = false;
        let mut backend: Box<dyn UiBackend> = match self.backend.borrow_mut().take() {
            Some(b) => b,
            #[cfg(target_arch = "wasm32")]
            None if !self.headless => Box::new(crate::web::WebBackend::new()),
            // Native default: the real backend is chosen when graphics is up (hook below).
            None if Self::defers_native(engine, self.headless) => {
                deferred = true;
                Box::new(PendingBackend)
            }
            None => {
                let (b, handle) = HeadlessBackend::new();
                headless_handle = Some(handle);
                Box::new(b)
            }
        };
        if let Some(h) = headless_handle {
            engine.insert_resource(h);
        }
        let mut tree = UiTree::new();
        if let Some(t) = &self.theme {
            tree.set_theme(t.clone());
            tree.take_changes();
        }
        engine.insert_resource(tree);
        engine.insert_resource(UiBindings::new());
        engine.add_event::<UiSignal>();
        if let Some(scripts) = crate::scripting::register(engine) {
            crate::scripting::add_systems(engine, scripts);
        }

        let mut bcx = UiBackendCx::new((0.0, 0.0), 1.0);
        backend.init(&mut bcx);
        let painters = crate::painter::EngineUiExt::ui_painters(engine);
        backend.set_painters(&painters);
        let factory: Option<WindowBackends> = match self.factory.borrow_mut().take() {
            Some(f) => Some(f),
            None => Self::default_factory(engine, self.headless),
        };
        let backends: Shared = Rc::new(RefCell::new(Backends {
            slots: vec![Slot { window: 0, be: backend, bcx, gpu_ready: false, consumed: (false, false) }],
            factory,
            painters,
            replay_main: false,
        }));

        // Raw input: the engine's input filter hands every event to its window's backend before
        // the game sees it. `try_borrow_mut`: a filter call can never re-enter a system holding
        // the backends (it runs while events are applied, not inside a system), but never panic.
        engine.add_input_filter(UiInputFilter { backends: backends.clone() });

        // Native overlay: GPU init / resize / draw, per window.
        // Draw2D backends (the fallback) draw through a Draw2D overlay instead, which also works
        // in SDL mode.
        if deferred {
            // The mode is known only once graphics is up (`auto` may fall back to SDL): pick the
            // backend then, replay the tree into it and register its kind of overlay.
            #[cfg(not(target_arch = "wasm32"))]
            {
                let backends = backends.clone();
                engine.on_graphics_ready(move |engine, mode| {
                    let Some(mut be) = Self::native_backend_for(mode) else { return };
                    {
                        let mut b = backends.borrow_mut();
                        let painters = b.painters.clone();
                        let slot = &mut b.slots[0];
                        be.init(&mut slot.bcx);
                        be.set_painters(&painters);
                        if slot.bcx.logical_size.0 > 0.0 {
                            be.resize(slot.bcx.logical_size, slot.bcx.scale_factor);
                        }
                        slot.be = be;
                        slot.gpu_ready = false;
                        b.replay_main = true;
                    }
                    if backends.borrow().slots[0].be.draws_with_draw2d() {
                        engine.add_draw2d_overlay(UiDraw2D { backends: backends.clone() });
                    } else {
                        engine.add_render_overlay(UiOverlay { backends: backends.clone() });
                    }
                });
            }
        } else if backends.borrow().slots[0].be.draws_with_draw2d() {
            engine.add_draw2d_overlay(UiDraw2D { backends: backends.clone() });
        } else {
            engine.add_render_overlay(UiOverlay { backends: backends.clone() });
        }

        // Window size changes (logical size and scale for each backend and its `UiBackendCx`).
        {
            let backends = backends.clone();
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let mut b = backends.borrow_mut();
                    for slot in &mut b.slots {
                        let Some(w) = cx.windows.get(efeengine_core::input::WindowId(slot.window)) else { continue };
                        let scale = if w.scale_factor.is_finite() && w.scale_factor > 0.0 { w.scale_factor as f32 } else { 1.0 };
                        let logical = (w.size.0 as f32 / scale, w.size.1 as f32 / scale);
                        if logical != slot.bcx.logical_size || scale != slot.bcx.scale_factor {
                            slot.bcx.logical_size = logical;
                            slot.bcx.scale_factor = scale;
                            slot.be.resize(logical, scale);
                        }
                    }
                })
                .order(order::INPUT + 5)
                .name("ui_resize");
        }

        // Signals in, capture out.
        {
            let backends = backends.clone();
            engine
                .add_system(Stage::PreUpdate, move |cx| {
                    let mut raw = Vec::new();
                    let (mut capture, mut text_input, mut cursor_icon) = (UiCapture::default(), None, None);
                    let (mut cp, mut ck) = (false, false);
                    {
                        let mut b = backends.borrow_mut();
                        for slot in &mut b.slots {
                            slot.be.drain_signals(&mut raw);
                            let c = slot.be.capture();
                            capture.pointer |= c.pointer;
                            capture.keyboard |= c.keyboard;
                            text_input = slot.be.take_text_input().or(text_input);
                            cursor_icon = slot.be.take_cursor_icon().or(cursor_icon);
                            let (p, k) = std::mem::take(&mut slot.consumed);
                            cp |= p;
                            ck |= k;
                        }
                    }
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
                        engine_bind_cx(&*cx.resources, &*cx.world, cx.time, |bcx| binds.evaluate(bcx))
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
        // was last handed to a backend; a texture replaced in `Textures` since then makes
        // every `Image` node that shows it get a synthesized `UiChange::Kind` plus the new image
        // in `UiBackendCx::images`, exactly like a node whose image was just set.
        // Every backend instance gets the whole journal and skips what is not in its window;
        // an instance made this frame (a window's first layer) gets the whole tree instead.
        {
            let backends = backends.clone();
            let seen: RefCell<HashMap<TextureHandle, Arc<Image>>> = RefCell::new(HashMap::new());
            engine
                .add_system(Stage::PostUpdate, move |cx| {
                    let Some(tree) = cx.resources.get_mut::<UiTree>() else { return };
                    let mut changes = tree.take_changes();
                    let Some(tree) = cx.resources.get::<UiTree>() else { return };
                    let textures = cx.resources.get::<Textures>();
                    let mut b = backends.borrow_mut();
                    let fresh = b.ensure_slots(&tree, cx.windows);
                    let mut seen = seen.borrow_mut();
                    // Images of the nodes the given changes created or changed.
                    let images_of = |changes: &[UiChange]| -> Vec<(TextureHandle, Arc<Image>)> {
                        let mut images: Vec<(TextureHandle, Arc<Image>)> = Vec::new();
                        if let Some(textures) = &textures {
                            for ch in changes {
                                let (UiChange::Created(id) | UiChange::Kind(id)) = ch else { continue };
                                if let Some(NodeKind::Image { texture, .. }) = tree.kind(*id)
                                    && let Some(img) = textures.get(texture.texture())
                                    && !images.iter().any(|(h, _)| *h == texture.texture())
                                {
                                    images.push((texture.texture(), img.clone()));
                                }
                            }
                        }
                        images
                    };
                    let mut images = images_of(&changes);
                    if let Some(textures) = &textures {
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
                                if matches!(n.kind(), NodeKind::Image { texture, .. } if texture.texture() == h) {
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
                    // Backends created this frame start from the whole tree.
                    if !fresh.is_empty() {
                        let snapshot = tree.snapshot_changes();
                        let snap_images = images_of(&snapshot);
                        for (h, img) in &snap_images {
                            let _ = seen.insert(*h, img.clone());
                        }
                        for slot in b.slots.iter_mut().filter(|s| fresh.contains(&s.window)) {
                            slot.bcx.images = snap_images.clone();
                            slot.be.sync(&tree, &snapshot, tree.theme(), &mut slot.bcx);
                            slot.bcx.images.clear();
                        }
                    }
                    if changes.is_empty() {
                        return;
                    }
                    for (h, img) in &images {
                        let _ = seen.insert(*h, img.clone());
                    }
                    for slot in b.slots.iter_mut().filter(|s| !fresh.contains(&s.window)) {
                        slot.bcx.images = images.clone();
                        slot.be.sync(&tree, &changes, tree.theme(), &mut slot.bcx);
                        slot.bcx.images.clear();
                    }
                })
                .order(order::LATE + 20)
                .name("ui_sync");
        }
    }
}

/// Makes the backend for a secondary window (`None` = no UI there).
type WindowBackends = Box<dyn Fn(u32) -> Option<Box<dyn UiBackend>>>;

/// One backend instance and what the plugin keeps next to it.
struct Slot {
    window: u32,
    be: Box<dyn UiBackend>,
    bcx: UiBackendCx,
    /// `gpu_init` ran for the window's surface.
    gpu_ready: bool,
    /// Pointer / keyboard input this instance consumed since the last `ui_signals` run.
    consumed: (bool, bool),
}

struct Backends {
    /// Slot 0 is the main window's (or the headless backend that handles every window).
    slots: Vec<Slot>,
    factory: Option<WindowBackends>,
    painters: crate::painter::UiPainters,
    /// Slot 0's backend was swapped in (deferred native choice): it gets the whole tree next sync.
    replay_main: bool,
}

/// Slot 0's stand-in until graphics is up and the real native backend is chosen: ignores everything.
struct PendingBackend;

impl UiBackend for PendingBackend {
    fn init(&mut self, _cx: &mut UiBackendCx) {}
    fn sync(&mut self, _tree: &UiTree, _changes: &[UiChange], _theme: &Theme, _cx: &mut UiBackendCx) {}
    fn drain_signals(&mut self, _out: &mut Vec<UiSignal>) {}
    fn capture(&self) -> crate::capture::UiCapture {
        Default::default()
    }
}

impl Backends {
    /// The slot that takes events and drawing of `window`.
    fn slot_mut(&mut self, window: u32) -> Option<&mut Slot> {
        let all = self.slots[0].be.handles_all_windows();
        let i = self.slots.iter().position(|s| s.window == window).or(if all { Some(0) } else { None })?;
        self.slots.get_mut(i)
    }

    /// Drops the instances of closed windows and makes one for every open secondary window that has
    /// a layer. Returns the windows whose instance was made now.
    fn ensure_slots(&mut self, tree: &UiTree, windows: &efeengine_core::ecs::Windows) -> Vec<u32> {
        use efeengine_core::input::WindowId;
        self.slots.retain(|s| s.window == 0 || windows.contains(WindowId(s.window)));
        let mut fresh = Vec::new();
        if std::mem::take(&mut self.replay_main) {
            fresh.push(0);
        }
        if self.slots[0].be.handles_all_windows() {
            return fresh;
        }
        for window in tree.layer_windows() {
            let Some(w) = windows.get(WindowId(window)) else { continue };
            if window == 0 || self.slots.iter().any(|s| s.window == window) {
                continue;
            }
            let Some(mut be) = self.factory.as_ref().and_then(|f| f(window)) else { continue };
            let scale = if w.scale_factor.is_finite() && w.scale_factor > 0.0 { w.scale_factor as f32 } else { 1.0 };
            let logical = (w.size.0 as f32 / scale, w.size.1 as f32 / scale);
            let mut bcx = UiBackendCx::for_window(window, logical, scale);
            be.init(&mut bcx);
            be.set_painters(&self.painters);
            be.resize(logical, scale);
            self.slots.push(Slot { window, be, bcx, gpu_ready: false, consumed: (false, false) });
            fresh.push(window);
        }
        fresh
    }
}

type Shared = Rc<RefCell<Backends>>;

struct UiInputFilter {
    backends: Shared,
}

impl InputFilter for UiInputFilter {
    fn filter(&mut self, ev: &InputEvent) -> Filter {
        let Ok(mut b) = self.backends.try_borrow_mut() else { return Filter::Pass };
        let Some(slot) = b.slot_mut(ev.window.0) else { return Filter::Pass };
        let f = slot.be.input(ev);
        if f == Filter::Captured {
            let (mut p, mut k) = slot.consumed;
            match &ev.kind {
                InputEventKind::Key { .. } | InputEventKind::Text(_) | InputEventKind::TextEditing { .. } => k = true,
                InputEventKind::MouseMotion { .. }
                | InputEventKind::MouseButton { .. }
                | InputEventKind::MouseWheel { .. }
                | InputEventKind::Touch { .. } => p = true,
                InputEventKind::Window(_) => {}
            }
            slot.consumed = (p, k);
        }
        f
    }
}

struct UiOverlay {
    backends: Shared,
}

impl UiOverlay {
    /// Runs `gpu_init` once per instance (the instance may be made after the window's surface).
    fn ready(slot: &mut Slot, gpu: &Gpu) {
        if !slot.gpu_ready {
            slot.gpu_ready = true;
            slot.be.gpu_init(gpu);
        }
    }
}

impl RenderOverlay for UiOverlay {
    fn init(&mut self, gpu: &Gpu) {
        if let Some(slot) = self.backends.borrow_mut().slot_mut(gpu.window) {
            Self::ready(slot, gpu);
        }
    }
    fn resize(&mut self, gpu: &Gpu) {
        if let Some(slot) = self.backends.borrow_mut().slot_mut(gpu.window) {
            if slot.gpu_ready {
                slot.be.gpu_resize(gpu);
            } else {
                Self::ready(slot, gpu);
            }
        }
    }
    fn render(&mut self, gpu: &Gpu, target: &efeengine_core::wgpu::TextureView) {
        if let Some(slot) = self.backends.borrow_mut().slot_mut(gpu.window) {
            Self::ready(slot, gpu);
            slot.be.render(gpu, target);
        }
    }
}

struct UiDraw2D {
    backends: Shared,
}

impl efeengine_core::render::Draw2DOverlay for UiDraw2D {
    fn render(&mut self, cx: &mut efeengine_core::render::Overlay2D) {
        if let Some(slot) = self.backends.borrow_mut().slot_mut(cx.window) {
            slot.be.draw2d(cx);
        }
    }
}
