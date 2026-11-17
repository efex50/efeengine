//! The native backend: the `UiTree` drawn by iced (`iced_wgpu`) on the engine's own device,
//! queue and surface texture (roadmap Phase 0 / Phase 3, API.md sections 5, 7, 10, 12, 13).
//!
//! No iced windowing: the engine owns the window, the surface and the event loop. What this
//! module does:
//! - `sync`: when the journal is not empty, rebuilds a [`Model`] (an owned copy of the visible
//!   tree with styles resolved against the theme). The model is the only thing iced's `Element`
//!   tree is built from, so iced keeps its own model (`UserInterface` + `Cache`) and the tree
//!   stays the source of truth.
//! - `input` (called from the engine's `InputFilter`): converts the event to iced events, builds
//!   the `UserInterface`, runs `update`, applies the produced messages to the model (so typing
//!   and dragging never wait a frame) and queues the `UiSignal`s. The verdict is iced's event
//!   status, or "the pointer is over a blocking node" / "a text input has focus".
//! - `render` (the engine's `RenderOverlay`): builds, updates (`RedrawRequested`), draws and
//!   presents into the engine's target view with `LoadOp::Load`.
//!
//! RefCell rule (see `plugin.rs`): the backend never calls back into the engine or the plugin;
//! signals and the text-input request are queued and picked up by the plugin's systems.

mod assets;
mod convert;
mod keyed;
mod model;
pub mod offscreen;
mod view;

pub use offscreen::{Screenshot, encode_png, render_offscreen, render_offscreen_represented};

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant as StdInstant;

use efeengine_core::input::{Filter, InputEvent, InputEventKind};
use efeengine_core::render::Gpu;
use iced_core::event::Status;
use iced_core::time::Instant;
use iced_core::widget::Operation;
use iced_core::widget::operation::{Focusable, Outcome, focusable};
use iced_core::clipboard;
use iced_core::{Color, Event, Rectangle, Size, Theme, mouse, renderer, shell, window};
use iced_runtime::user_interface::{self, UserInterface};
use iced_wgpu::graphics::{Shell, Viewport};
use iced_wgpu::{Engine, Renderer, wgpu};

use efeengine_core::window::CursorIcon;

use crate::backend::{TextInputArea, UiBackend, UiBackendCx};
use crate::capture::UiCapture;
use crate::id::{Owner, WidgetId};
use crate::journal::UiChange;
use crate::node::NodeKind;
use crate::signal::{UiSignal, UiSignalKind, UiValue};
use crate::theme::Theme as UiTheme;
use crate::tree::UiTree;

use convert::Converter;
use model::Model;
use assets::Assets;
use view::{Msg, iced_id};

struct GpuState {
    renderer: Renderer,
    viewport: Viewport,
}

/// Cost counters for `EFE_UI_STATS=1` (average CPU time of the overlay pass).
#[derive(Default)]
struct Stats {
    /// Frames that rebuilt, laid out and drew the interface.
    active: u32,
    active_total: std::time::Duration,
    /// Of `active_total`: build + layout + update + draw (everything before `present`).
    active_build: std::time::Duration,
    /// Frames that only presented the previous frame's primitives again.
    idle: u32,
    idle_total: std::time::Duration,
}

/// Keyboard focus navigation requested by a key press.
#[derive(Clone, Copy)]
enum Nav {
    Next,
    Previous,
    Blur,
}

pub struct IcedBackend {
    gpu: Option<GpuState>,
    /// The overlay could not be set up (no adapter): the backend stays inert.
    broken: bool,
    model: Model,
    cache: user_interface::Cache,
    conv: Converter,
    cursor: mouse::Cursor,
    /// Logical size from `UiBackendCx` / the viewport.
    logical: Size,
    signals: Vec<UiSignal>,
    /// Blocking nodes the pointer is inside (from `mouse_area` enter / exit messages).
    over: HashSet<WidgetId>,
    /// `hover_signals` nodes the pointer is inside.
    hovering: HashSet<WidgetId>,
    /// The text input with keyboard focus.
    focused: Option<WidgetId>,
    assets: Assets,
    /// The model, a message or an event changed something: rebuild, lay out and draw next frame.
    dirty: bool,
    /// What the last `update` asked for (caret blink, animations).
    redraw: window::RedrawRequest,
    /// The renderer holds a drawn frame that can be presented again.
    drawn: bool,
    /// Mouse cursor the UI wants, and the last one reported to the plugin.
    icon: CursorIcon,
    icon_sent: CursorIcon,
    icon_request: Option<CursorIcon>,
    /// Latest IME cursor rectangle (logical px) of the focused text input.
    ime_area: Option<Rectangle>,
    /// Text-input request not yet picked up by the plugin.
    text_request: Option<Option<TextInputArea>>,
    /// What the plugin was last told.
    text_sent: Option<TextInputArea>,
    stats: Stats,
    stats_t0: StdInstant,
    stats_on: bool,
}

impl IcedBackend {
    pub fn new() -> Self {
        Self {
            gpu: None,
            broken: false,
            model: Model::default(),
            cache: user_interface::Cache::new(),
            conv: Converter::new(),
            cursor: mouse::Cursor::Unavailable,
            logical: Size::new(0.0, 0.0),
            signals: Vec::new(),
            over: HashSet::new(),
            hovering: HashSet::new(),
            focused: None,
            assets: Assets::new(),
            dirty: true,
            redraw: window::RedrawRequest::NextFrame,
            drawn: false,
            icon: CursorIcon::Default,
            icon_sent: CursorIcon::Default,
            icon_request: None,
            ime_area: None,
            text_request: None,
            text_sent: None,
            stats: Stats::default(),
            stats_t0: StdInstant::now(),
            stats_on: std::env::var_os("EFE_UI_STATS").is_some(),
        }
    }

    fn push(&mut self, widget: WidgetId, kind: UiSignalKind) {
        self.signals.push(UiSignal { widget, key: None, owner: Owner::Game, kind });
    }

    /// Builds the interface, feeds `events`, returns the statuses; applies produced messages
    /// and refreshes the focus. Draws when `draw` is given. `nav` runs a focus operation after
    /// the events (Tab / Shift+Tab / Escape).
    fn update(&mut self, events: &[Event], draw: Option<(&Gpu, &wgpu::TextureView)>, nav: Option<Nav>) -> Vec<Status> {
        let cursor = self.cursor;
        let Some(g) = self.gpu.as_mut() else { return Vec::new() };
        let mut bus = shell::Bus::new();
        let mut ui = UserInterface::build(view::view(&self.model, &self.assets), self.logical, std::mem::take(&mut self.cache), &mut g.renderer);
        let (mut state, statuses) = ui.update(&window::Headless, &shell::Waker::noop(), events, self.cursor, &mut g.renderer, &mut bus);
        // Clipboard requests: writes go out, reads are answered with a follow-up event.
        if let user_interface::State::Updated { clipboard, .. } = &mut state {
            if let Some(clipboard::Content::Text(t)) = clipboard.write.take() {
                let _ = efeengine_core::clipboard::set_clipboard_text(&t);
            }
            if !clipboard.reads.is_empty() {
                let answer = match efeengine_core::clipboard::clipboard_text() {
                    Some(t) => Ok(Arc::new(clipboard::Content::Text(t))),
                    None => Err(clipboard::Error::ContentNotAvailable),
                };
                let reply = [Event::Clipboard(clipboard::Event::Read(answer))];
                let (s2, _) = ui.update(&window::Headless, &shell::Waker::noop(), &reply, self.cursor, &mut g.renderer, &mut bus);
                state = s2;
            }
        }
        let nav_op: Option<Box<dyn Operation>> = match nav {
            Some(Nav::Next) => Some(Box::new(focusable::focus_next::<()>())),
            Some(Nav::Previous) => Some(Box::new(focusable::focus_previous::<()>())),
            Some(Nav::Blur) => Some(Box::new(focusable::unfocus::<()>())),
            None => None,
        };
        // `focus_next` / `focus_previous` count first and then focus: follow the chain.
        if let Some(mut op) = nav_op {
            loop {
                ui.operate(&g.renderer, op.as_mut());
                match op.finish() {
                    Outcome::Chain(next) => op = next,
                    _ => break,
                }
            }
        }
        let mut probe = FocusProbe(None);
        ui.operate(&g.renderer, &mut probe);
        if let Some((gpu, target)) = draw {
            ui.draw(&mut g.renderer, &Theme::Dark, &renderer::Style { text_color: Color::WHITE }, cursor);
            self.cache = ui.into_cache();
            self.stats.active_build += self.stats_t0.elapsed();
            let _ = g.renderer.present(None, gpu.surface_format, target, &g.viewport);
        } else {
            self.cache = ui.into_cache();
        }
        let focus_id = probe.0;
        if let user_interface::State::Updated { input_method, redraw_request, mouse_interaction, .. } = state {
            self.ime_area = match input_method {
                iced_core::InputMethod::Enabled { cursor, .. } => Some(cursor),
                iced_core::InputMethod::Disabled => None,
            };
            self.redraw = redraw_request;
            self.set_icon(mouse_interaction);
        } else {
            // Outdated: the widgets want a rebuild.
            self.dirty = true;
        }
        for m in bus {
            self.apply(m);
            self.dirty = true;
        }
        self.set_focus(focus_id.and_then(|id| self.model.text_inputs().into_iter().find(|n| iced_id(*n) == id)));
        statuses
    }

    fn set_icon(&mut self, i: mouse::Interaction) {
        use mouse::Interaction as I;
        self.icon = match i {
            I::Pointer | I::Alias | I::Copy | I::ContextMenu | I::Help | I::Cell | I::ZoomIn | I::ZoomOut => CursorIcon::Pointer,
            I::Text => CursorIcon::Text,
            I::Crosshair => CursorIcon::Crosshair,
            I::Grab => CursorIcon::Grab,
            I::Grabbing => CursorIcon::Grabbing,
            I::NotAllowed | I::NoDrop => CursorIcon::NotAllowed,
            I::Wait | I::Progress => CursorIcon::Wait,
            I::Move | I::AllScroll => CursorIcon::Move,
            I::ResizingHorizontally | I::ResizingColumn => CursorIcon::ResizeEw,
            I::ResizingVertically | I::ResizingRow => CursorIcon::ResizeNs,
            I::ResizingDiagonallyUp => CursorIcon::ResizeNesw,
            I::ResizingDiagonallyDown => CursorIcon::ResizeNwse,
            I::None | I::Idle | I::Hidden => CursorIcon::Default,
        };
        if self.icon != self.icon_sent {
            self.icon_sent = self.icon;
            self.icon_request = Some(self.icon);
        }
    }

    fn set_focus(&mut self, now: Option<WidgetId>) {
        if now == self.focused {
            return;
        }
        if let Some(old) = self.focused {
            self.push(old, UiSignalKind::Focus(false));
        }
        if let Some(new) = now {
            self.push(new, UiSignalKind::Focus(true));
        }
        self.focused = now;
        if now.is_none() {
            self.ime_area = None;
        }
        self.refresh_text_request();
    }

    /// Queues `SetTextInput` / area when the wanted state differs from what was sent.
    fn refresh_text_request(&mut self) {
        let want = self.focused.map(|_| {
            let r = self.ime_area.unwrap_or(Rectangle::new(iced_core::Point::ORIGIN, Size::ZERO));
            TextInputArea { x: r.x as i32, y: r.y as i32, w: r.width.ceil() as i32, h: r.height.ceil() as i32, cursor: 0 }
        });
        if want != self.text_sent {
            self.text_sent = want;
            self.text_request = Some(want);
        }
    }

    /// Applies one widget message to the model and queues its signal (value write-back
    /// semantics of API.md section 7: edit locally, report, the plugin writes the tree).
    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Click(id) => {
                if self.model.find(id).is_some_and(|n| n.enabled) {
                    self.push(id, UiSignalKind::Click);
                }
            }
            Msg::Toggle(id, v) => {
                let Some(n) = self.model.find_mut(id).filter(|n| n.enabled) else { return };
                if let NodeKind::Checkbox { checked, .. } = &mut n.kind
                    && *checked != v
                {
                    *checked = v;
                    self.push(id, UiSignalKind::Changed(UiValue::Bool(v)));
                }
            }
            Msg::Slide(id, v) => {
                let Some(n) = self.model.find_mut(id).filter(|n| n.enabled) else { return };
                if let NodeKind::Slider { min, max, value, .. } = &mut n.kind {
                    let v = v.clamp(*min, *max);
                    if v != *value {
                        *value = v;
                        self.push(id, UiSignalKind::Changed(UiValue::F32(v)));
                    }
                }
            }
            Msg::Input(id, mut s) => {
                let Some(n) = self.model.find_mut(id).filter(|n| n.enabled) else { return };
                if let NodeKind::TextInput { value, max_len, .. } = &mut n.kind {
                    if *max_len > 0 && s.chars().count() > *max_len as usize {
                        s = s.chars().take(*max_len as usize).collect();
                    }
                    if s != *value {
                        value.clone_from(&s);
                        self.push(id, UiSignalKind::Changed(UiValue::Text(s)));
                    }
                }
            }
            Msg::Submit(id) => {
                let Some(n) = self.model.find(id).filter(|n| n.enabled) else { return };
                if let NodeKind::TextInput { value, .. } = &n.kind {
                    let v = value.clone();
                    self.push(id, UiSignalKind::Submit(v));
                }
            }
            Msg::Enter(id) => {
                let Some(n) = self.model.find(id) else { return };
                let (blocking, hover) = (n.blocking, n.hover_signals);
                if blocking {
                    let _ = self.over.insert(id);
                }
                if hover && self.hovering.insert(id) {
                    self.push(id, UiSignalKind::Hover(true));
                }
            }
            Msg::Exit(id) => {
                let _ = self.over.remove(&id);
                if self.hovering.remove(&id) {
                    self.push(id, UiSignalKind::Hover(false));
                }
            }
        }
    }

    /// Tab / Shift+Tab move the focus between text inputs, Escape blurs the focused one.
    fn nav_for(&self, ev: &InputEvent) -> Option<Nav> {
        use efeengine_core::input::{ElementState, Key as EKey, NamedKey};
        let InputEventKind::Key { logical: EKey::Named(k), state: ElementState::Pressed, mods, .. } = &ev.kind else { return None };
        match k {
            NamedKey::Tab if !self.model.text_inputs().is_empty() => Some(if mods.shift() { Nav::Previous } else { Nav::Next }),
            NamedKey::Escape if self.focused.is_some() => Some(Nav::Blur),
            _ => None,
        }
    }

    fn pointer_over(&self) -> bool {
        !self.over.is_empty() || self.model.modal()
    }

    fn set_viewport(&mut self, gpu: &Gpu) {
        let scale = gpu.scale_factor.max(0.01);
        let vp = Viewport::with_physical_size(
            Size::new(gpu.size.0.max(1), gpu.size.1.max(1)),
            renderer::Scale { window: scale, application: 1.0 },
        );
        self.logical = vp.logical_size();
        self.conv.scale = scale;
        self.dirty = true;
        if let Some(g) = self.gpu.as_mut() {
            g.viewport = vp;
        }
    }
}

impl Default for IcedBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Finds the focused widget that has an id (text inputs; their ids come from `iced_id`).
struct FocusProbe(Option<iced_core::widget::Id>);

impl Operation for FocusProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&iced_core::widget::Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if state.is_focused() && id.is_some() {
            self.0 = id.cloned();
        }
    }

    fn finish(&self) -> Outcome<()> {
        Outcome::None
    }
}

impl UiBackend for IcedBackend {
    fn init(&mut self, cx: &mut UiBackendCx) {
        self.conv.scale = cx.scale_factor.max(0.01);
        self.logical = Size::new(cx.logical_size.0, cx.logical_size.1);
    }

    fn input(&mut self, ev: &InputEvent) -> Filter {
        if self.broken {
            return Filter::Pass;
        }
        let mut events = Vec::new();
        if let Some(p) = self.conv.convert(ev, &mut events) {
            self.cursor = mouse::Cursor::Available(p);
        }
        if events.is_empty() || self.model.is_empty() || self.gpu.is_none() {
            return Filter::Pass;
        }
        let nav = self.nav_for(ev);
        self.dirty = true;
        let statuses = self.update(&events, None, nav);
        let used = statuses.contains(&Status::Captured);
        let captured = match &ev.kind {
            InputEventKind::MouseMotion { .. }
            | InputEventKind::MouseButton { .. }
            | InputEventKind::MouseWheel { .. }
            | InputEventKind::Touch { .. } => used || self.pointer_over(),
            InputEventKind::Key { .. } | InputEventKind::Text(_) | InputEventKind::TextEditing { .. } => {
                used || nav.is_some() || self.focused.is_some() || self.model.modal()
            }
            InputEventKind::Window(_) => false,
        };
        if captured { Filter::Captured } else { Filter::Pass }
    }

    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], _theme: &UiTheme, cx: &mut UiBackendCx) {
        for (h, img) in &cx.images {
            self.assets.set_image(*h, img);
        }
        for ch in changes {
            if let UiChange::Font(id) = ch
                && let Some(f) = tree.font(*id)
                && self.assets.load_font(*id, &f.bytes).is_none()
            {
                efeengine_core::prelude::global_warn(&format!("[efeengine-ui] font \"{}\" has no usable face", f.name));
            }
        }
        self.dirty = true;
        self.model = Model::build(tree);
        // Forget ids that are gone or no longer interesting; a surviving widget keeps its state
        // in iced's cache, so its enter / exit pairs stay consistent.
        let m = &self.model;
        self.over.retain(|id| m.find(*id).is_some_and(|n| n.blocking));
        self.hovering.retain(|id| m.find(*id).is_some_and(|n| n.hover_signals));
        if self.focused.is_some_and(|f| m.find(f).is_none()) {
            self.focused = None;
            self.ime_area = None;
            self.refresh_text_request();
        }
    }

    fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
        out.append(&mut self.signals);
    }

    fn capture(&self) -> UiCapture {
        UiCapture { pointer: self.pointer_over(), keyboard: self.focused.is_some() || self.model.modal() }
    }

    fn resize(&mut self, logical: (f32, f32), scale: f32) {
        self.conv.scale = scale.max(0.01);
        self.logical = Size::new(logical.0, logical.1);
        self.dirty = true;
    }

    fn gpu_init(&mut self, gpu: &Gpu) {
        let engine = Engine::new(gpu.adapter, gpu.device.clone(), gpu.queue.clone(), gpu.surface_format, None, Shell::headless());
        let renderer = Renderer::new(engine, renderer::Settings::default());
        let viewport = Viewport::with_physical_size(Size::new(1, 1), renderer::Scale { window: 1.0, application: 1.0 });
        self.gpu = Some(GpuState { renderer, viewport });
        self.set_viewport(gpu);
    }

    fn gpu_resize(&mut self, gpu: &Gpu) {
        self.set_viewport(gpu);
    }

    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView) {
        if self.model.is_empty() || self.gpu.is_none() {
            return;
        }
        let t0 = StdInstant::now();
        self.stats_t0 = t0;
        let due = match self.redraw {
            window::RedrawRequest::NextFrame => true,
            window::RedrawRequest::At(t) => Instant::now() >= t,
            window::RedrawRequest::Wait => false,
        };
        let active = self.dirty || due || !self.drawn;
        if active {
            // `RedrawRequested` lets widgets refresh their status and request the input method
            // and the next redraw; the same interface is then drawn.
            self.dirty = false;
            self.redraw = window::RedrawRequest::Wait;
            let _ = self.update(&[Event::Window(window::Event::RedrawRequested(Instant::now()))], Some((gpu, target)), None);
            self.drawn = true;
            self.refresh_text_request();
        } else if let Some(g) = self.gpu.as_mut() {
            // Nothing changed: no rebuild, no layout, no draw; the renderer still holds the
            // primitives of the last frame, the engine cleared the target, so present again.
            let _ = g.renderer.present(None, gpu.surface_format, target, &g.viewport);
        }
        if self.stats_on {
            let el = t0.elapsed();
            if active {
                self.stats.active += 1;
                self.stats.active_total += el;
            } else {
                self.stats.idle += 1;
                self.stats.idle_total += el;
            }
            if self.stats.active + self.stats.idle == 300 {
                let s = &self.stats;
                let avg = |t: std::time::Duration, n: u32| if n == 0 { 0.0 } else { t.as_secs_f64() * 1000.0 / n as f64 };
                let (a, i) = (avg(s.active_total, s.active), avg(s.idle_total, s.idle));
                let build = avg(s.active_build, s.active);
                efeengine_core::prelude::global_info(&format!(
                    "[efeengine-ui] iced overlay over 300 frames: active {a:.3} ms ({} frames, {build:.3} build+layout+update+draw, {:.3} present), idle {i:.3} ms ({} frames)",
                    s.active,
                    a - build,
                    s.idle
                ));
                self.stats = Stats::default();
            }
        }
    }

    fn take_text_input(&mut self) -> Option<Option<TextInputArea>> {
        self.text_request.take()
    }

    fn take_cursor_icon(&mut self) -> Option<CursorIcon> {
        self.icon_request.take()
    }
}
