use std::{cell::{Cell, RefCell}, collections::{HashMap, VecDeque}, ptr::NonNull, rc::Rc};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WebDisplayHandle, WebCanvasWindowHandle};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Event, EventTarget, HtmlCanvasElement, HtmlElement, KeyboardEvent};

use crate::{Message, debug_warn, send_message, window::{InnerWindow, events::{KeyCode, KeyMod, WindowEvents}}};
use super::web_impls::{keyboard_event, now_ns, should_prevent_default};

const DEFAULT_CANVAS_ID: &str = "canvas";

/// Logical and physical key of a currently held key (needed to synthesize its `KeyUp`).
type HeldKey = (Option<KeyCode>, Option<KeyCode>);

/// State shared between the DOM listeners and `WebWindow`.
/// The web build is single-threaded (requestAnimationFrame), so `Rc` + cells are enough.
#[derive(Default)]
struct Shared {
    /// Events collected by DOM listeners between two `poll_events` calls.
    queue: RefCell<VecDeque<WindowEvents>>,
    /// Keys currently held down, keyed by physical key (falling back to the logical key).
    held: RefCell<HashMap<KeyCode, HeldKey>>,
    /// Last focus state sent (or assumed at startup); used to drop duplicate focus events.
    focused: Cell<bool>,
}

impl Shared {
    fn push(&self, event: WindowEvents) {
        self.queue.borrow_mut().push_back(event);
    }

    fn track_key(&self, event: &WindowEvents) {
        match event {
            WindowEvents::KeyDown { key_code, scancode, .. } => {
                if let Some(id) = scancode.or(*key_code) {
                    self.held.borrow_mut().insert(id, (*key_code, *scancode));
                }
            }
            WindowEvents::KeyUp { key_code, scancode, .. } => {
                if let Some(id) = scancode.or(*key_code) {
                    self.held.borrow_mut().remove(&id);
                }
            }
            _ => {}
        }
    }

    /// Record a focus change. Duplicates are dropped. On focus loss every held key is
    /// released first, since the browser won't deliver keyup for keys let go while unfocused.
    fn set_focused(&self, focused: bool) {
        if self.focused.replace(focused) == focused { return; }
        let mut queue = self.queue.borrow_mut();
        if !focused {
            let timestamp = now_ns();
            for (_, (key_code, scancode)) in self.held.borrow_mut().drain() {
                queue.push_back(WindowEvents::KeyUp {
                    timestamp, window_id: 0, key_code, scancode, keymod: KeyMod::NONE, repeat: false, raw: 0,
                });
            }
        }
        queue.push_back(WindowEvents::Focused { window_id: 0, focused });
    }
}

/// A registered DOM listener; removed again when the window is dropped.
struct Listener {
    target: EventTarget,
    kind: &'static str,
    closure: Closure<dyn FnMut(Event)>,
}

/// The web counterpart of `SdlWindowManager`: a `<canvas>` instead of an OS window,
/// and DOM listeners feeding a local queue instead of an SDL event pump.
pub struct WebWindow{
    canvas_id:String,
    /// Cached lookup of `canvas_id`; cleared when the id changes.
    canvas:Option<HtmlCanvasElement>,
    title:String,
    shared:Rc<Shared>,
    listeners:Vec<Listener>,
    /// `pointerdown` listener on the current canvas (replaced when the canvas id changes).
    canvas_listener:Option<Listener>,
    /// Last size sent as `Resized`, in physical pixels.
    last_size:(u32,u32),
    /// Last devicePixelRatio sent as `ScaleFactorChanged` (0.0 = none yet).
    last_scale:f64,
}

impl WebWindow{
    pub fn new(title:String) -> Self{
        let mut w = Self {
            canvas_id: crate::config::startup_config()
                .canvas_id.clone()
                .unwrap_or_else(|| DEFAULT_CANVAS_ID.to_string()),
            canvas: None,
            title,
            shared: Rc::default(),
            listeners: Vec::new(),
            canvas_listener: None,
            last_size: (0,0),
            last_scale: 0.0,
        };
        w.apply_title();
        w.shared.focused.set(document_focused());
        w.attach_keyboard_listeners();
        w.attach_focus_listeners();
        w.setup_canvas();
        // Size the drawing buffer now, so the surface is created at the right size.
        w.sync_canvas_size();
        w
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
        let closure = Closure::<dyn FnMut(Event)>::new(f);
        match target.add_event_listener_with_callback(kind, closure.as_ref().unchecked_ref()) {
            Ok(()) => self.listeners.push(Listener { target, kind, closure }),
            Err(_) => debug_warn(&format!("could not add {kind} listener")),
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
                // Ignore keys that are part of an IME composition.
                if e.is_composing() { return; }
                // Stop the page from scrolling/navigating while playing, but leave text fields alone.
                if should_prevent_default(&e) { e.prevent_default(); }
                let event = keyboard_event(&e, down);
                shared.track_key(&event);
                shared.push(event);
            });
        }
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

    /// Make the canvas focusable and focus it on `pointerdown`, so key events
    /// don't go to something else on the page after clicking the game.
    fn setup_canvas(&mut self) {
        if let Some(l) = self.canvas_listener.take() {
            let _ = l.target.remove_event_listener_with_callback(l.kind, l.closure.as_ref().unchecked_ref());
        }
        let Some(canvas) = self.canvas() else { return };
        if !canvas.has_attribute("tabindex") {
            let _ = canvas.set_attribute("tabindex", "0");
        }
        let el: HtmlElement = canvas.clone().into();
        let closure = Closure::<dyn FnMut(Event)>::new(move |_| { let _ = el.focus(); });
        let target: EventTarget = canvas.into();
        match target.add_event_listener_with_callback("pointerdown", closure.as_ref().unchecked_ref()) {
            Ok(()) => self.canvas_listener = Some(Listener { target, kind: "pointerdown", closure }),
            Err(_) => debug_warn("could not add pointerdown listener"),
        }
    }

    /// Report devicePixelRatio changes (browser zoom, moving between monitors) as `ScaleFactorChanged`.
    fn sync_scale_factor(&mut self) {
        let scale = self.device_pixel_ratio();
        if scale != self.last_scale {
            self.last_scale = scale;
            self.shared.push(WindowEvents::ScaleFactorChanged { window_id: 0, scale });
        }
    }

    fn device_pixel_ratio(&self) -> f64 {
        web_sys::window().map_or(1.0, |w| w.device_pixel_ratio())
    }

    /// A canvas' drawing buffer does not follow its CSS size, so match it to
    /// CSS size * devicePixelRatio every frame and report changes as `Resized`
    /// (the same physical-pixel size SDL's PixelSizeChanged gives).
    fn sync_canvas_size(&mut self) {
        self.sync_scale_factor();
        let Some(canvas) = self.canvas() else { return };
        let dpr = self.device_pixel_ratio();
        let width = (canvas.client_width().max(0) as f64 * dpr).round() as u32;
        let height = (canvas.client_height().max(0) as f64 * dpr).round() as u32;
        // 0x0 means hidden or not laid out yet; keep the old buffer.
        if width == 0 || height == 0 { return; }
        if canvas.width() != width { canvas.set_width(width); }
        if canvas.height() != height { canvas.set_height(height); }
        if self.last_size != (width, height) {
            self.last_size = (width, height);
            self.shared.push(WindowEvents::Resized { window_id: 0, width, height });
        }
    }
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
            let _ = l.target.remove_event_listener_with_callback(l.kind, l.closure.as_ref().unchecked_ref());
        }
        if let Some(l) = self.canvas_listener.take() {
            let _ = l.target.remove_event_listener_with_callback(l.kind, l.closure.as_ref().unchecked_ref());
        }
    }
}

impl InnerWindow for WebWindow{
    fn set_canvas_id(&mut self,canvas_id:String) {
        self.canvas_id = canvas_id;
        self.canvas = None;
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
        self.device_pixel_ratio()
    }

    fn poll_events(&mut self) {
        self.sync_canvas_size();
        // Take the queue first, so a listener firing during send_message can't hit a borrow.
        let events = std::mem::take(&mut *self.shared.queue.borrow_mut());
        for event in events {
            send_message(Message::WindowEvent(event));
        }
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