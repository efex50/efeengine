use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::collections::VecDeque;

#[cfg(target_family = "wasm")]
pub mod web_backend;
#[cfg(target_family = "wasm")]
mod web_impls;

#[cfg(not(target_family = "wasm"))]
pub mod sdl_backend;
#[cfg(not(target_family = "wasm"))]
mod sdl_impls;
pub mod headless;
pub mod clipboard;

pub use clipboard::{clipboard_text, set_clipboard_text};

/// Mouse cursor shape (`Commands::set_cursor_icon`). SDL has no grab cursors: `Grab` shows the
/// pointer hand and `Grabbing` the move cursor there; the web uses the CSS `grab` / `grabbing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum CursorIcon {
    #[default]
    Default,
    Pointer,
    Text,
    Crosshair,
    Grab,
    Grabbing,
    NotAllowed,
    Wait,
    Move,
    ResizeEw,
    ResizeNs,
    ResizeNesw,
    ResizeNwse,
}

impl CursorIcon {
    /// The CSS `cursor` value.
    pub fn css(self) -> &'static str {
        match self {
            CursorIcon::Default => "default",
            CursorIcon::Pointer => "pointer",
            CursorIcon::Text => "text",
            CursorIcon::Crosshair => "crosshair",
            CursorIcon::Grab => "grab",
            CursorIcon::Grabbing => "grabbing",
            CursorIcon::NotAllowed => "not-allowed",
            CursorIcon::Wait => "wait",
            CursorIcon::Move => "move",
            CursorIcon::ResizeEw => "ew-resize",
            CursorIcon::ResizeNs => "ns-resize",
            CursorIcon::ResizeNesw => "nesw-resize",
            CursorIcon::ResizeNwse => "nwse-resize",
        }
    }
}

use crate::engine::config::EngineConfig;
use crate::engine::error::EngineError;
use crate::engine::input::{GamepadState, InputEvent, WindowId};

/// Where a secondary window lives on the web (ignored natively). See `docs/MULTI_WINDOW.md`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum WebTarget {
    /// A new canvas appended to `<body>` (a fixed `size` CSS box).
    #[default]
    Body,
    /// A new canvas created inside the first element matching this CSS selector. The canvas
    /// fills the element (`width: 100%; height: 100%`), the window's size follows the element's
    /// CSS size times `devicePixelRatio` (`ResizeObserver`), `size` is only the fallback while
    /// the element has no layout.
    Element(String),
    /// An existing `<canvas>` matching this CSS selector.
    Canvas(String),
}

/// Describes a secondary window for `Engine::create_window` / `Commands::create_window`.
///
/// ```
/// use efeengine_core::window::WindowDesc;
/// let d = WindowDesc::new("Map").web_element("#sidebar").size(320, 240);
/// assert_eq!(d.size, (320, 240));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct WindowDesc {
    pub title: String,
    /// Logical size (native: the window size; web: fallback, see [`WebTarget::Element`]).
    pub size: (u32, u32),
    pub resizable: bool,
    pub web: WebTarget,
}

impl WindowDesc {
    /// 640x480, resizable, a canvas in `<body>` on the web.
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), size: (640, 480), resizable: true, web: WebTarget::Body }
    }
    pub fn size(mut self, w: u32, h: u32) -> Self {
        self.size = (w, h);
        self
    }
    pub fn resizable(mut self, on: bool) -> Self {
        self.resizable = on;
        self
    }
    /// Web: create the canvas inside the element matching `selector`.
    pub fn web_element(mut self, selector: impl Into<String>) -> Self {
        self.web = WebTarget::Element(selector.into());
        self
    }
    /// Web: use the existing `<canvas>` matching `selector`.
    pub fn web_canvas(mut self, selector: impl Into<String>) -> Self {
        self.web = WebTarget::Canvas(selector.into());
        self
    }
}

/// Platform windows. Deliberately not `Send`/`Sync`: native (SDL) windows and the
/// event pump must stay on the main thread. `WindowId::MAIN` always exists; others are created
/// with `create_window`. A `window` argument that names no window is treated as the main one by
/// backends that cannot tell (they never receive one from `GameWindowManager`).
pub trait InnerWindow: HasWindowHandle + HasDisplayHandle{
    #[cfg(target_family = "wasm")]
    fn set_canvas_id(&mut self,canvas_id:String);
    /// The canvas of `window`.
    #[cfg(target_family = "wasm")]
    fn canvas(&self, window: WindowId) -> Option<web_sys::HtmlCanvasElement>;
    #[cfg(target_family = "wasm")]
    fn canvas_id(&self) -> &str;
    /// Open another window. Its first `poll_events` emits `Resized` + `ScaleFactorChanged` for it.
    fn create_window(&mut self, _window: WindowId, _desc: &WindowDesc) -> Result<(), EngineError> {
        Err(EngineError::Window("this window backend cannot open more windows".into()))
    }
    /// Close a secondary window (no-op for the main one and unknown ids).
    fn close_window(&mut self, _window: WindowId) {}
    fn set_title(&mut self, window: WindowId, title:String);
    /// The raw `SDL_Window*` of `window` (SDL mode creates an `SDL_Renderer` on it). Null when
    /// the backend is not SDL (headless) or the window does not exist.
    #[cfg(not(target_family = "wasm"))]
    fn sdl_window_ptr(&self, _window: WindowId) -> *mut sdl3::sys::video::SDL_Window { std::ptr::null_mut() }
    /// The raw handles of `window` (wgpu surface creation). `None` for headless / unknown.
    #[cfg(not(target_family = "wasm"))]
    fn raw_handles(&self, _window: WindowId) -> Option<(raw_window_handle::RawWindowHandle, raw_window_handle::RawDisplayHandle)> { None }
    /// Drawable size in physical pixels.
    fn size(&self, window: WindowId) -> (u32, u32);
    /// Physical pixels per logical pixel (DPI scale / devicePixelRatio).
    fn scale_factor(&self, _window: WindowId) -> f64 { 1.0 }
    /// Size in logical pixels. Default: `size() / scale_factor()`, rounded.
    fn logical_size(&self, window: WindowId) -> (u32, u32) {
        let (w, h) = self.size(window);
        let s = self.scale_factor(window);
        if s > 0.0 {
            ((w as f64 / s).round() as u32, (h as f64 / s).round() as u32)
        } else {
            (w, h)
        }
    }
    /// APPEND this poll's events to `out` (never clear it). The first call also emits the
    /// initial `Window(Resized)` + `Window(ScaleFactorChanged)` of every window not yet reported.
    fn poll_events(&mut self, out: &mut Vec<InputEvent>);
    /// Start/stop OS text input (Text events). Default no-op.
    fn set_text_input(&mut self, _window: WindowId, _enabled: bool) {}
    /// Where text is typed, in logical pixels, plus the caret offset (IME popup placement).
    /// Default no-op.
    fn set_text_input_area(&mut self, _window: WindowId, _x: i32, _y: i32, _w: i32, _h: i32, _cursor: i32) {}
    /// Show/hide the cursor over the window. Default no-op.
    fn set_cursor_visible(&mut self, _window: WindowId, _visible: bool) {}
    /// Change the cursor shape. Default no-op.
    fn set_cursor_icon(&mut self, _window: WindowId, _icon: CursorIcon) {}
    /// Grab the pointer: relative mouse mode (native) / pointer lock (web). Default no-op.
    fn set_cursor_grab(&mut self, _window: WindowId, _grab: bool) {}
    /// Fullscreen on or off. Default no-op.
    fn set_fullscreen(&mut self, _window: WindowId, _on: bool) {}
    /// Write the current gamepad snapshot into `state` (connect/update/retain). Default no-op.
    fn poll_gamepads(&mut self, _state: &mut GamepadState) {}
}

/// The window manager the engine owns: the platform backend plus injected events and the
/// bookkeeping of which windows exist. Methods that take a `WindowId` use the main window for
/// `WindowId::MAIN`.
pub struct GameWindowManager{
    inner: Box<dyn InnerWindow>,
    injected: VecDeque<InputEvent>,
    headless: bool,
    text_input_area: Option<(WindowId, (i32, i32, i32, i32, i32))>,
    cursor_icon: CursorIcon,
    /// Secondary windows that exist, in creation order.
    open: Vec<WindowId>,
}

impl GameWindowManager {
    /// Headless when `cfg.headless`, else SDL (native) / the web canvas (wasm).
    pub fn new(cfg: &EngineConfig) -> Result<Self, EngineError> {
        let inner: Box<dyn InnerWindow> = if cfg.headless {
            Box::new(headless::HeadlessWindow::new(cfg.size))
        } else {
            #[cfg(target_family = "wasm")]
            { Box::new(web_backend::WebWindow::new(&cfg.title, cfg.canvas_id.clone())?) }
            #[cfg(not(target_family = "wasm"))]
            { Box::new(sdl_backend::SdlWindowManager::new(&cfg.title, cfg.size)?) }
        };
        Ok(Self {
            inner,
            injected: VecDeque::new(),
            headless: cfg.headless,
            text_input_area: None,
            cursor_icon: CursorIcon::Default,
            open: Vec::new(),
        })
    }
    /// Queue an event; drained into `out` before the backend's own events on the next poll.
    pub fn inject(&mut self, ev: InputEvent) {
        self.injected.push_back(ev);
    }
    /// The last `(x, y, w, h, cursor)` given to `set_text_input_area` (logical px).
    pub fn text_input_area(&self) -> Option<(i32, i32, i32, i32, i32)> {
        self.text_input_area.map(|(_, a)| a)
    }
    /// The last cursor icon given to `set_cursor_icon`.
    pub fn cursor_icon(&self) -> CursorIcon {
        self.cursor_icon
    }
    /// The raw `SDL_Window*` of a window (null when headless or unknown).
    #[cfg(not(target_family = "wasm"))]
    pub fn sdl_window_ptr(&self, window: WindowId) -> *mut sdl3::sys::video::SDL_Window {
        self.inner.sdl_window_ptr(window)
    }
    /// The raw window and display handles of a window (native, not headless).
    #[cfg(not(target_family = "wasm"))]
    pub fn raw_handles(&self, window: WindowId) -> Option<(raw_window_handle::RawWindowHandle, raw_window_handle::RawDisplayHandle)> {
        self.inner.raw_handles(window)
    }
    pub fn is_headless(&self) -> bool {
        self.headless
    }
    /// The canvas a window draws to (web only).
    #[cfg(target_family = "wasm")]
    pub fn canvas(&self, window: WindowId) -> Option<web_sys::HtmlCanvasElement>{
        self.inner.canvas(window)
    }
    #[cfg(target_family = "wasm")]
    pub fn canvas_id(&self) -> &str{
        self.inner.canvas_id()
    }
    #[cfg(target_family = "wasm")]
    pub fn set_canvas_id(&mut self, canvas_id:String){
        self.inner.set_canvas_id(canvas_id);
    }

    /// Open a secondary window under `id` (the engine hands the ids out).
    pub fn create_window(&mut self, id: WindowId, desc: &WindowDesc) -> Result<(), EngineError> {
        if id.is_main() || self.open.contains(&id) {
            return Err(EngineError::Window(format!("window {} already exists", id.0)));
        }
        self.inner.create_window(id, desc)?;
        self.open.push(id);
        Ok(())
    }
    /// Close a secondary window; `false` when it did not exist (the main window cannot be closed here).
    pub fn close_window(&mut self, id: WindowId) -> bool {
        let Some(i) = self.open.iter().position(|w| *w == id) else { return false };
        self.open.remove(i);
        self.inner.close_window(id);
        true
    }
    /// The secondary windows that exist, in creation order.
    pub fn secondary(&self) -> &[WindowId] {
        &self.open
    }
    pub fn exists(&self, id: WindowId) -> bool {
        id.is_main() || self.open.contains(&id)
    }

    pub fn size(&self, window: WindowId) -> (u32,u32) {
        self.inner.size(window)
    }
    pub fn scale_factor(&self, window: WindowId) -> f64 {
        self.inner.scale_factor(window)
    }
    pub fn logical_size(&self, window: WindowId) -> (u32, u32) {
        self.inner.logical_size(window)
    }
    /// Injected events first, then the backend's own.
    pub fn poll_events(&mut self, out: &mut Vec<InputEvent>) {
        out.extend(self.injected.drain(..));
        self.inner.poll_events(out);
    }
    pub fn set_title(&mut self, window: WindowId, title:String) {
        self.inner.set_title(window, title);
    }
    pub fn set_text_input(&mut self, window: WindowId, enabled: bool) {
        self.inner.set_text_input(window, enabled);
    }
    pub fn set_text_input_area(&mut self, window: WindowId, x: i32, y: i32, w: i32, h: i32, cursor: i32) {
        self.text_input_area = Some((window, (x, y, w, h, cursor)));
        self.inner.set_text_input_area(window, x, y, w, h, cursor);
    }
    pub fn set_cursor_visible(&mut self, window: WindowId, visible: bool) {
        self.inner.set_cursor_visible(window, visible);
    }
    /// Change the cursor shape over `window`.
    pub fn set_cursor_icon(&mut self, window: WindowId, icon: CursorIcon) {
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            self.inner.set_cursor_icon(window, icon);
        }
    }
    pub fn set_cursor_grab(&mut self, window: WindowId, grab: bool) {
        self.inner.set_cursor_grab(window, grab);
    }
    /// Fullscreen on or off (no-op for headless windows).
    pub fn set_fullscreen(&mut self, window: WindowId, on: bool) {
        if !self.headless {
            self.inner.set_fullscreen(window, on);
        }
    }
    pub fn poll_gamepads(&mut self, state: &mut GamepadState) {
        self.inner.poll_gamepads(state);
    }
}

// Handles of the MAIN window (wgpu's GL instance needs the display handle at creation).
impl HasWindowHandle for GameWindowManager {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for GameWindowManager {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.inner.display_handle()
    }
}
