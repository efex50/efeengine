use std::{cell::{Cell, RefCell}, collections::{HashMap, HashSet, VecDeque}, ptr::NonNull, rc::Rc};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WebDisplayHandle, WebCanvasWindowHandle};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{
    AddEventListenerOptions, CompositionEvent, Event, EventTarget, FocusOptions, HtmlCanvasElement, HtmlElement,
    KeyboardEvent, PointerEvent, WheelEvent,
};

use crate::{
    engine::error::EngineError, debug_warn,
    input::{
        ElementState, GamepadAxis, GamepadButton, GamepadId, GamepadState, InputEvent, InputEventKind, Key, KeyCode, MouseButton, TouchPhase, WheelUnit,
        WindowEvent, WindowId,
    },
    window::InnerWindow,
};
use super::web_impls::{event_ns, is_editable_target, keyboard_event, now_ns, physical_key, should_prevent_default, text_for_key};

const DEFAULT_CANVAS_ID: &str = "canvas";

/// State shared between the DOM listeners and `WebWindow`.
/// The web build is single-threaded (requestAnimationFrame), so `Rc` + cells are enough.
struct Shared {
    /// Events collected by DOM listeners between two `poll_events` calls.
    queue: RefCell<VecDeque<InputEvent>>,
    /// Physical keys currently held down (`Unknown` is never tracked).
    held: RefCell<HashSet<KeyCode>>,
    /// Last focus state sent (or assumed at startup); used to drop duplicate focus events.
    focused: Cell<bool>,
    /// Whether keydown/compositionend produce `Text` events.
    text_input: Cell<bool>,
    /// Last mouse/pen position, for deltas while the pointer is not locked.
    last_pos: Cell<Option<(f32, f32)>>,
    /// Last devicePixelRatio sent as `ScaleFactorChanged` (0.0 = none yet).
    last_scale: Cell<f64>,
    /// Mouse/pen buttons currently held, as a `PointerEvent.buttons` mask (bit0 Left, bit1 Right,
    /// bit2 Middle, bit3 Back, bit4 Forward).
    buttons: Cell<u16>,
    /// Active touch pointer ids with their last position.
    touches: RefCell<HashMap<u64, (f32, f32)>>,
    /// Pointer lock wanted by `set_cursor_grab`; re-requested from a pointerdown (user activation).
    want_grab: Cell<bool>,
}

impl Shared {
    fn new() -> Self {
        Self {
            queue: RefCell::default(),
            held: RefCell::default(),
            focused: Cell::new(true),
            text_input: Cell::new(true),
            last_pos: Cell::new(None),
            last_scale: Cell::new(0.0),
            buttons: Cell::new(0),
            touches: RefCell::default(),
            want_grab: Cell::new(false),
        }
    }

    fn push(&self, timestamp_ns: u64, kind: InputEventKind) {
        self.queue.borrow_mut().push_back(InputEvent::with_time(kind, timestamp_ns, WindowId::MAIN));
    }

    fn track_key(&self, event: &InputEvent) {
        if let InputEventKind::Key { physical, state, .. } = &event.kind {
            if *physical == KeyCode::Unknown { return; }
            match state {
                ElementState::Pressed => { self.held.borrow_mut().insert(*physical); }
                ElementState::Released => { self.held.borrow_mut().remove(physical); }
            }
        }
    }

    /// Diff `new` (a `buttons` mask) against the pressed mask and emit a `MouseButton` event per changed bit.
    fn sync_buttons(&self, time: u64, new: u16, pos: (f32, f32)) {
        let new = new & 0x1f;
        let old = self.buttons.replace(new);
        for bit in 0..5u16 {
            let m = 1 << bit;
            if (old ^ new) & m == 0 { continue; }
            let button = match bit {
                0 => MouseButton::Left,
                1 => MouseButton::Right,
                2 => MouseButton::Middle,
                3 => MouseButton::Back,
                _ => MouseButton::Forward,
            };
            let state = if new & m != 0 { ElementState::Pressed } else { ElementState::Released };
            self.push(time, InputEventKind::MouseButton { button, state, pos });
        }
    }

    /// Release every held non-modifier key (macOS sends no keyup for them while Meta is held).
    fn release_non_modifiers(&self, time: u64) {
        let keys: Vec<KeyCode> = {
            let mut held = self.held.borrow_mut();
            let keys: Vec<KeyCode> = held.iter().copied().filter(|k| !is_modifier_code(*k)).collect();
            for k in &keys { held.remove(k); }
            keys
        };
        for physical in keys {
            self.push(time, InputEventKind::key(
                physical, Key::Unidentified, ElementState::Released, false, Default::default(),
            ));
        }
    }

    /// Record a focus change. Duplicates are dropped. On focus loss every held key is
    /// released first, then mouse buttons and touches, since the browser won't deliver those
    /// releases when they happen while unfocused.
    fn set_focused(&self, focused: bool) {
        if self.focused.replace(focused) == focused { return; }
        let now = now_ns();
        if !focused {
            let held: Vec<KeyCode> = self.held.borrow_mut().drain().collect();
            for physical in held {
                self.push(now, InputEventKind::key(
                    physical, Key::Unidentified, ElementState::Released, false, Default::default(),
                ));
            }
            let pos = self.last_pos.get().unwrap_or((0.0, 0.0));
            self.sync_buttons(now, 0, pos);
            let touches: Vec<(u64, (f32, f32))> = self.touches.borrow_mut().drain().collect();
            for (id, pos) in touches {
                self.push(now, InputEventKind::Touch { id, phase: TouchPhase::Cancelled, pos });
            }
            self.last_pos.set(None);
        }
        self.push(now, InputEventKind::Window(WindowEvent::Focused(focused)));
    }

    /// Record a new devicePixelRatio; duplicates are dropped.
    fn set_scale(&self, scale: f64) {
        if scale != self.last_scale.replace(scale) {
            self.push(now_ns(), InputEventKind::Window(WindowEvent::ScaleFactorChanged { scale }));
        }
    }
}

fn is_modifier_code(k: KeyCode) -> bool {
    matches!(k, KeyCode::LShift | KeyCode::RShift | KeyCode::LCtrl | KeyCode::RCtrl
        | KeyCode::LAlt | KeyCode::RAlt | KeyCode::LMeta | KeyCode::RMeta)
}

/// A registered DOM listener; removed again when dropped from its owner explicitly.
struct Listener {
    target: EventTarget,
    kind: &'static str,
    closure: Closure<dyn FnMut(Event)>,
}

impl Listener {
    /// Registers `f` on `target`. `passive: Some(false)` registers a non-passive listener.
    fn add(target: EventTarget, kind: &'static str, passive: Option<bool>, f: impl FnMut(Event) + 'static) -> Option<Listener> {
        let closure = Closure::<dyn FnMut(Event)>::new(f);
        let result = match passive {
            None => target.add_event_listener_with_callback(kind, closure.as_ref().unchecked_ref()),
            Some(p) => {
                let opts = AddEventListenerOptions::new();
                opts.set_passive(p);
                target.add_event_listener_with_callback_and_add_event_listener_options(
                    kind, closure.as_ref().unchecked_ref(), &opts,
                )
            }
        };
        match result {
            Ok(()) => Some(Listener { target, kind, closure }),
            Err(_) => { debug_warn(&format!("could not add {kind} listener")); None }
        }
    }

    fn remove(self) {
        let _ = self.target.remove_event_listener_with_callback(self.kind, self.closure.as_ref().unchecked_ref());
    }
}

/// The web counterpart of `SdlWindowManager`: a `<canvas>` instead of an OS window,
/// and DOM listeners feeding a local queue instead of an SDL event pump.
pub struct WebWindow{
    canvas_id:String,
    /// Cached lookup of `canvas_id`; cleared when the id changes.
    canvas:Option<HtmlCanvasElement>,
    title:String,
    shared:Rc<Shared>,
    /// Listeners on `window`/`document`.
    listeners:Vec<Listener>,
    /// Pointer, wheel and contextmenu listeners on the current canvas (replaced when the canvas id changes).
    canvas_listeners:Vec<Listener>,
    /// `change` listener on the current `(resolution: Ndppx)` media query.
    dpr_listener:Option<Listener>,
    /// The devicePixelRatio the media query listener was armed for.
    dpr_armed:f64,
    /// Remembered so it is re-applied when the canvas is re-resolved.
    cursor_visible:bool,
    cursor_icon: crate::engine::window::CursorIcon,
    /// Last size sent as `Resized`, in physical pixels.
    last_size:(u32,u32),
    /// `getGamepads()` failure already logged.
    gamepad_warned:bool,
    /// Gamepad indices already reported as non-standard-mapping (logged once).
    gamepad_ignored:HashSet<u32>,
}

impl WebWindow{
    /// `canvas_id: None` means `"canvas"`.
    pub fn new(title:&str, canvas_id:Option<String>) -> Result<Self, EngineError>{
        let Some(window) = web_sys::window() else {
            return Err(EngineError::Window("no `window` object (not running in a browser main thread)".into()));
        };
        if window.document().is_none() {
            return Err(EngineError::Window("no `document` object".into()));
        }
        let mut w = Self {
            canvas_id: canvas_id.unwrap_or_else(|| DEFAULT_CANVAS_ID.to_string()),
            canvas: None,
            title: title.to_string(),
            shared: Rc::new(Shared::new()),
            listeners: Vec::new(),
            canvas_listeners: Vec::new(),
            dpr_listener: None,
            dpr_armed: 0.0,
            cursor_visible: true,
            cursor_icon: crate::engine::window::CursorIcon::Default,
            last_size: (0,0),
            gamepad_warned: false,
            gamepad_ignored: HashSet::new(),
        };
        w.apply_title();
        w.shared.focused.set(document_focused());
        w.attach_keyboard_listeners();
        w.attach_focus_listeners();
        w.attach_composition_listener();
        w.attach_pointer_lock_listeners();
        w.setup_canvas();
        // Size the drawing buffer now, so the surface is created at the right size.
        w.sync_canvas_size();
        Ok(w)
    }

    fn canvas(&mut self) -> Option<HtmlCanvasElement> {
        if self.canvas.is_none() {
            self.canvas = find_canvas(&self.canvas_id);
        }
        self.canvas.clone()
    }

    fn apply_title(&self) {
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            doc.set_title(&self.title);
        }
    }

    fn add_listener(&mut self, target: EventTarget, kind: &'static str, f: impl FnMut(Event) + 'static) {
        if let Some(l) = Listener::add(target, kind, None, f) {
            self.listeners.push(l);
        }
    }

    /// Key events are listened for on `window`, so the canvas does not need focus.
    fn attach_keyboard_listeners(&mut self) {
        let Some(window) = web_sys::window() else { return };
        let target: EventTarget = window.into();
        for (kind, down) in [("keydown", true), ("keyup", false)] {
            let shared = self.shared.clone();
            self.add_listener(target.clone(), kind, move |e: Event| {
                let Ok(e) = e.dyn_into::<KeyboardEvent>() else { return };
                let physical = physical_key(&e.code());
                let held = physical != KeyCode::Unknown && shared.held.borrow().contains(&physical);
                if down {
                    // Ignore keys that are part of an IME composition, and typing in page inputs.
                    if e.is_composing() || is_editable_target(&e) { return; }
                } else if is_editable_target(&e) && !held {
                    // A release in a page input only matters for a key pressed in the game.
                    return;
                }
                // Stop the page from scrolling/navigating while playing, but leave text fields alone.
                if should_prevent_default(&e) { e.prevent_default(); }
                // A keyup for a key that is not held (its keydown was dropped, or happened while
                // unfocused) would be a stray release.
                if !down && physical != KeyCode::Unknown && !held { return; }
                let event = keyboard_event(&e, down);
                let time = event.timestamp_ns;
                shared.track_key(&event);
                shared.queue.borrow_mut().push_back(event);
                if !down && (matches!(physical, KeyCode::LMeta | KeyCode::RMeta) || e.key() == "Meta") {
                    shared.release_non_modifiers(time);
                }
                if down && shared.text_input.get() {
                    if let Some(text) = text_for_key(&e) {
                        shared.push(time, InputEventKind::Text(text));
                    }
                }
            });
        }
    }

    /// IME on `window`: `compositionupdate` gives `TextEditing` (the preedit string, caret at its
    /// end), `compositionend` gives an empty `TextEditing` (composition over) and then the
    /// committed text as `Text`. `compositionstart` needs nothing (the first update carries the
    /// text). A hidden-input path for virtual keyboards is out of scope.
    fn attach_composition_listener(&mut self) {
        let Some(window) = web_sys::window() else { return };
        let shared = self.shared.clone();
        self.add_listener(window.clone().into(), "compositionupdate", move |e: Event| {
            if !shared.text_input.get() || is_editable_target(&e) { return; }
            let time = event_ns(&e);
            let Ok(e) = e.dyn_into::<CompositionEvent>() else { return };
            let text = e.data().unwrap_or_default();
            let cursor = text.chars().count() as i32;
            shared.push(time, InputEventKind::TextEditing { text, cursor, len: 0 });
        });
        let shared = self.shared.clone();
        self.add_listener(window.into(), "compositionend", move |e: Event| {
            if !shared.text_input.get() || is_editable_target(&e) { return; }
            let time = event_ns(&e);
            let Ok(e) = e.dyn_into::<CompositionEvent>() else { return };
            shared.push(time, InputEventKind::TextEditing { text: String::new(), cursor: 0, len: 0 });
            if let Some(data) = e.data().filter(|d| !d.is_empty()) {
                shared.push(time, InputEventKind::Text(data));
            }
        });
    }

    /// `focus`/`blur` on window and `visibilitychange` on document (a hidden tab is unfocused).
    fn attach_focus_listeners(&mut self) {
        let Some(window) = web_sys::window() else { return };
        let document = window.document();
        let target: EventTarget = window.into();
        for (kind, focused) in [("focus", true), ("blur", false)] {
            let shared = self.shared.clone();
            self.add_listener(target.clone(), kind, move |_| {
                shared.set_focused(focused && !document_hidden());
            });
        }
        if let Some(document) = document {
            let shared = self.shared.clone();
            self.add_listener(document.into(), "visibilitychange", move |_| {
                shared.set_focused(document_focused());
            });
        }
    }

    /// `pointerlockchange` resets the motion baseline; `pointerlockerror` is only logged.
    fn attach_pointer_lock_listeners(&mut self) {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let shared = self.shared.clone();
        self.add_listener(document.clone().into(), "pointerlockchange", move |_| {
            shared.last_pos.set(None);
        });
        self.add_listener(document.into(), "pointerlockerror", |_| {
            log::warn!("pointer lock request failed");
        });
    }

    /// Make the canvas focusable, attach pointer/wheel/contextmenu listeners and re-apply the cursor style.
    fn setup_canvas(&mut self) {
        for l in self.canvas_listeners.drain(..) {
            l.remove();
        }
        let Some(canvas) = self.canvas() else { return };
        if !canvas.has_attribute("tabindex") {
            let _ = canvas.set_attribute("tabindex", "0");
        }
        self.apply_cursor();
        let target: EventTarget = canvas.clone().into();
        let mut added = Vec::new();

        // pointerdown: focus the canvas (so key events don't go elsewhere), capture the pointer.
        let shared = self.shared.clone();
        let el: HtmlElement = canvas.clone().into();
        added.extend(Listener::add(target.clone(), "pointerdown", None, move |e: Event| {
            let opts = FocusOptions::new();
            opts.set_prevent_scroll(true);
            let _ = el.focus_with_options(&opts);
            let Ok(e) = e.dyn_into::<PointerEvent>() else { return };
            // Pointer lock needs user activation: retry a wanted grab that is not active.
            if shared.want_grab.get() && !is_pointer_locked(&el) {
                el.request_pointer_lock();
            }
            if let Err(err) = el.set_pointer_capture(e.pointer_id()) {
                debug_warn(&format!("setPointerCapture failed: {err:?}"));
            }
            handle_pointer(&shared, &e, PointerPhase::Down);
        }));
        for (kind, phase) in [
            ("pointermove", PointerPhase::Move),
            ("pointerup", PointerPhase::Up),
            ("pointercancel", PointerPhase::Cancel),
        ] {
            let shared = self.shared.clone();
            added.extend(Listener::add(target.clone(), kind, None, move |e: Event| {
                let Ok(e) = e.dyn_into::<PointerEvent>() else { return };
                handle_pointer(&shared, &e, phase);
            }));
        }

        let shared = self.shared.clone();
        added.extend(Listener::add(target.clone(), "pointerleave", None, move |_| {
            shared.last_pos.set(None);
        }));

        // wheel must be non-passive, or preventDefault is ignored and the page scrolls.
        let shared = self.shared.clone();
        added.extend(Listener::add(target.clone(), "wheel", Some(false), move |e: Event| {
            e.prevent_default();
            let time = event_ns(&e);
            let Ok(e) = e.dyn_into::<WheelEvent>() else { return };
            let dpr = device_pixel_ratio();
            let (unit, scale) = match e.delta_mode() {
                WheelEvent::DOM_DELTA_LINE => (WheelUnit::Line, 1.0),
                WheelEvent::DOM_DELTA_PAGE => (WheelUnit::Page, 1.0),
                // Pixel deltas are CSS pixels; the engine wants physical pixels.
                _ => (WheelUnit::Pixel, dpr),
            };
            // DOM: deltaY > 0 scrolls down; engine: +y is up/away. deltaX > 0 is right in both.
            let delta = ((e.delta_x() * scale) as f32, (-e.delta_y() * scale) as f32);
            shared.push(time, InputEventKind::MouseWheel { delta, unit });
        }));

        added.extend(Listener::add(target, "contextmenu", None, |e: Event| e.prevent_default()));
        self.canvas_listeners = added;
    }

    /// Cursor visibility via the canvas' CSS `cursor`.
    fn apply_cursor(&mut self) {
        let Some(canvas) = self.canvas() else { return };
        let value = if self.cursor_visible { self.cursor_icon.css() } else { "none" };
        if canvas.style().set_property("cursor", value).is_err() {
            debug_warn("could not set canvas cursor style");
        }
    }

    /// Report devicePixelRatio changes (browser zoom, moving between monitors) as `ScaleFactorChanged`
    /// and keep the `matchMedia` listener armed for the current ratio.
    fn sync_scale_factor(&mut self) {
        let scale = device_pixel_ratio();
        self.shared.set_scale(scale);
        if scale != self.dpr_armed {
            self.arm_dpr_listener(scale);
        }
    }

    /// `(resolution: Ndppx)` stops matching as soon as the ratio changes, so the listener fires once
    /// per change and has to be re-armed for the new ratio (done from the next poll).
    fn arm_dpr_listener(&mut self, dpr: f64) {
        if let Some(l) = self.dpr_listener.take() {
            l.remove();
        }
        self.dpr_armed = dpr;
        let Some(window) = web_sys::window() else { return };
        let mql = match window.match_media(&format!("(resolution: {dpr}dppx)")) {
            Ok(Some(m)) => m,
            _ => { debug_warn("matchMedia(resolution) unavailable; relying on per-frame DPR polling"); return; }
        };
        let shared = self.shared.clone();
        self.dpr_listener = Listener::add(mql.into(), "change", None, move |_| {
            shared.set_scale(device_pixel_ratio());
        });
    }

    /// A canvas' drawing buffer does not follow its CSS size, so match it to
    /// CSS size * devicePixelRatio every frame and report changes as `Resized`
    /// (the same physical-pixel size SDL's PixelSizeChanged gives).
    fn sync_canvas_size(&mut self) {
        self.sync_scale_factor();
        // A removed canvas is dropped, and a canvas that appears later gets its listeners.
        if self.canvas.as_ref().is_some_and(|c| !c.is_connected()) {
            self.canvas = None;
            self.last_size = (0,0);
            self.shared.last_pos.set(None);
            self.setup_canvas();
        } else if self.canvas_listeners.is_empty() && self.canvas().is_some() {
            self.setup_canvas();
        }
        let Some(canvas) = self.canvas() else { return };
        let dpr = device_pixel_ratio();
        let width = (canvas.client_width().max(0) as f64 * dpr).round() as u32;
        let height = (canvas.client_height().max(0) as f64 * dpr).round() as u32;
        // 0x0 means hidden or not laid out yet; keep the old buffer.
        if width == 0 || height == 0 { return; }
        if canvas.width() != width { canvas.set_width(width); }
        if canvas.height() != height { canvas.set_height(height); }
        if self.last_size != (width, height) {
            self.last_size = (width, height);
            self.shared.push(now_ns(), InputEventKind::Window(WindowEvent::Resized { width, height }));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PointerPhase { Down, Move, Up, Cancel }

/// `MouseEvent` coordinates as f64: Chromium/Firefox expose fractional values on the
/// property, while the generated `client_x()` getter truncates to i32.
fn event_f64(e: &PointerEvent, name: &str, fallback: i32) -> f64 {
    js_sys::Reflect::get(e, &JsValue::from_str(name))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(fallback as f64)
}

/// Translate a pointer event: touch -> `Touch`, mouse and pen -> `MouseMotion`/`MouseButton`.
/// Pointer Events only fire down/up for the first/last button of a chord, so the mouse/pen
/// buttons are derived by diffing `buttons` against the pressed mask on every event.
fn handle_pointer(shared: &Shared, e: &PointerEvent, phase: PointerPhase) {
    let dpr = device_pixel_ratio();
    let target = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
    let (left, top) = target.as_ref().map_or((0.0, 0.0), |t| {
        let r = t.get_bounding_client_rect();
        (r.left(), r.top())
    });
    let pos = (
        ((event_f64(e, "clientX", e.client_x()) - left) * dpr) as f32,
        ((event_f64(e, "clientY", e.client_y()) - top) * dpr) as f32,
    );
    let time = event_ns(e);

    if e.pointer_type() == "touch" {
        let id = e.pointer_id().max(0) as u64;
        let phase = match phase {
            PointerPhase::Down => TouchPhase::Started,
            PointerPhase::Move => TouchPhase::Moved,
            PointerPhase::Up => TouchPhase::Ended,
            PointerPhase::Cancel => TouchPhase::Cancelled,
        };
        match phase {
            TouchPhase::Ended | TouchPhase::Cancelled => { shared.touches.borrow_mut().remove(&id); }
            _ => { shared.touches.borrow_mut().insert(id, pos); }
        }
        shared.push(time, InputEventKind::Touch { id, phase, pos });
        return;
    }

    // Mouse or pen.
    match phase {
        PointerPhase::Move => {
            let locked = target.as_ref().is_some_and(is_pointer_locked);
            let delta = if locked {
                (
                    (event_f64(e, "movementX", e.movement_x()) * dpr) as f32,
                    (event_f64(e, "movementY", e.movement_y()) * dpr) as f32,
                )
            } else {
                shared.last_pos.get().map_or((0.0, 0.0), |p| (pos.0 - p.0, pos.1 - p.1))
            };
            shared.last_pos.set(Some(pos));
            shared.push(time, InputEventKind::MouseMotion { pos, delta });
            shared.sync_buttons(time, e.buttons(), pos);
        }
        PointerPhase::Down | PointerPhase::Up => {
            shared.last_pos.set(Some(pos));
            shared.sync_buttons(time, e.buttons(), pos);
        }
        // The pointer was taken over by the browser: no further up will arrive.
        PointerPhase::Cancel => shared.sync_buttons(time, 0, pos),
    }
}

fn is_pointer_locked(canvas: &web_sys::Element) -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.pointer_lock_element())
        .is_some_and(|locked| &locked == canvas)
}

fn device_pixel_ratio() -> f64 {
    web_sys::window().map_or(1.0, |w| w.device_pixel_ratio())
}

fn document_hidden() -> bool {
    web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden())
}

/// Visible tab whose document has focus.
fn document_focused() -> bool {
    web_sys::window().and_then(|w| w.document()).is_some_and(|d| !d.hidden() && d.has_focus().unwrap_or(true))
}

fn find_canvas(id: &str) -> Option<HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .dyn_into::<HtmlCanvasElement>()
        .ok()
}

impl Drop for WebWindow {
    fn drop(&mut self) {
        for l in self.listeners.drain(..) {
            l.remove();
        }
        for l in self.canvas_listeners.drain(..) {
            l.remove();
        }
        if let Some(l) = self.dpr_listener.take() {
            l.remove();
        }
    }
}

impl InnerWindow for WebWindow{
    fn set_canvas_id(&mut self,canvas_id:String) {
        self.canvas_id = canvas_id;
        self.canvas = None;
        self.shared.last_pos.set(None);
        // Force a Resized for the new canvas on the next poll.
        self.last_size = (0,0);
        self.setup_canvas();
    }

    fn canvas(&self) -> Option<HtmlCanvasElement> {
        self.canvas.clone().or_else(|| find_canvas(&self.canvas_id))
    }

    fn canvas_id(&self) -> &str {
        &self.canvas_id
    }

    fn set_title(&mut self,title:String) {
        self.title = title;
        self.apply_title();
    }

    /// Drawing-buffer size in physical pixels.
    fn size(&self) -> (u32, u32) {
        self.canvas.clone()
            .or_else(|| find_canvas(&self.canvas_id))
            .map_or((0,0), |c| (c.width(), c.height()))
    }

    fn scale_factor(&self) -> f64 {
        device_pixel_ratio()
    }

    fn poll_events(&mut self, out: &mut Vec<InputEvent>) {
        self.sync_canvas_size();
        // Take the queue first, so a listener firing meanwhile can't hit a borrow.
        let events = std::mem::take(&mut *self.shared.queue.borrow_mut());
        out.extend(events);
    }

    fn set_text_input(&mut self, enabled: bool) {
        self.shared.text_input.set(enabled);
    }

    fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
        self.apply_cursor();
    }

    fn set_cursor_icon(&mut self, icon: crate::engine::window::CursorIcon) {
        self.cursor_icon = icon;
        self.apply_cursor();
    }

    /// The Fullscreen API needs a user gesture; a request outside one is refused by the browser
    /// (a rejected promise, ignored here).
    fn set_fullscreen(&mut self, on: bool) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        if on {
            if let Some(el) = doc.document_element() {
                let _ = el.request_fullscreen();
            }
        } else if doc.fullscreen_element().is_some() {
            doc.exit_fullscreen();
        }
    }

    fn set_cursor_grab(&mut self, grab: bool) {
        self.shared.want_grab.set(grab);
        if grab {
            match self.canvas() {
                Some(canvas) => canvas.request_pointer_lock(),
                None => debug_warn("set_cursor_grab: canvas not found"),
            }
        } else if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            doc.exit_pointer_lock();
        }
    }

    /// Only pads with the W3C "standard" mapping are interpreted (button/axis indices
    /// below); other pads are skipped, since their inputs would land on the wrong slots.
    fn poll_gamepads(&mut self, state: &mut GamepadState) {
        const BUTTONS: [(usize, GamepadButton); 17] = [
            (0, GamepadButton::South),
            (1, GamepadButton::East),
            (2, GamepadButton::West),
            (3, GamepadButton::North),
            (4, GamepadButton::LeftShoulder),
            (5, GamepadButton::RightShoulder),
            (6, GamepadButton::LeftTrigger),
            (7, GamepadButton::RightTrigger),
            (8, GamepadButton::Back),
            (9, GamepadButton::Start),
            (10, GamepadButton::LeftStick),
            (11, GamepadButton::RightStick),
            (12, GamepadButton::DPadUp),
            (13, GamepadButton::DPadDown),
            (14, GamepadButton::DPadLeft),
            (15, GamepadButton::DPadRight),
            (16, GamepadButton::Guide),
        ];
        let Some(window) = web_sys::window() else { return };
        let pads = match window.navigator().get_gamepads() {
            Ok(p) => p,
            Err(_) => {
                if !self.gamepad_warned {
                    self.gamepad_warned = true;
                    debug_warn("navigator.getGamepads() failed; gamepads disabled");
                }
                return;
            }
        };
        let mut ids = Vec::new();
        for entry in pads.iter() {
            let Ok(pad) = entry.dyn_into::<web_sys::Gamepad>() else { continue };
            if !pad.connected() {
                continue;
            }
            if pad.mapping() != web_sys::GamepadMappingType::Standard {
                if self.gamepad_ignored.insert(pad.index()) {
                    log::info!("gamepad {} ignored: non-standard mapping ({:?})", pad.index(), pad.id());
                }
                continue;
            }
            let id = GamepadId(pad.index());
            ids.push(id);
            state.connect(id, pad.id());
            let buttons = pad.buttons();
            let button = |i: usize| buttons.get(i as u32).dyn_into::<web_sys::GamepadButton>().ok();
            for (i, b) in BUTTONS {
                if let Some(btn) = button(i) {
                    state.set_button(id, b, btn.pressed());
                }
            }
            let axes = pad.axes();
            let axis = |i: u32| axes.get(i).as_f64().unwrap_or(0.0) as f32;
            state.set_axis(id, GamepadAxis::LeftX, axis(0));
            state.set_axis(id, GamepadAxis::LeftY, axis(1));
            state.set_axis(id, GamepadAxis::RightX, axis(2));
            state.set_axis(id, GamepadAxis::RightY, axis(3));
            let trigger = |i: usize| button(i).map_or(0.0, |b| b.value() as f32);
            state.set_axis(id, GamepadAxis::LeftTrigger, trigger(6));
            state.set_axis(id, GamepadAxis::RightTrigger, trigger(7));
        }
        state.retain(&ids);
    }
}


// Web için boş handle implementasyonları (WGPU create_surface_from_canvas kullanacağımız için burası kritik değil)
impl HasWindowHandle for WebWindow {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // Burada gerçek bir handle döndürmek yerine dummy dönüyoruz, renderer'da cfg ile çözeceğiz.
        let mut handle = WebCanvasWindowHandle::new(NonNull::dangling());
        unsafe { Ok(raw_window_handle::WindowHandle::borrow_raw(RawWindowHandle::WebCanvas(handle))) }
    }
}

impl HasDisplayHandle for WebWindow {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        let handle = WebDisplayHandle::new();
        unsafe { Ok(raw_window_handle::DisplayHandle::borrow_raw(RawDisplayHandle::Web(handle))) }
    }
}