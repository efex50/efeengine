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
use crate::engine::input::{GamepadState, InputEvent};

/// Platform window. Deliberately not `Send`/`Sync`: native (SDL) windows and the
/// event pump must stay on the main thread.
pub trait InnerWindow: HasWindowHandle + HasDisplayHandle{
    #[cfg(target_family = "wasm")]
    fn set_canvas_id(&mut self,canvas_id:String);
    #[cfg(target_family = "wasm")]
    fn canvas(&self) -> Option<web_sys::HtmlCanvasElement>;
    #[cfg(target_family = "wasm")]
    fn canvas_id(&self) -> &str;
    fn set_title(&mut self,title:String);
    /// Drawable size in physical pixels.
    fn size(&self) -> (u32, u32);
    /// Physical pixels per logical pixel (DPI scale / devicePixelRatio).
    fn scale_factor(&self) -> f64 { 1.0 }
    /// Size in logical pixels. Default: `size() / scale_factor()`, rounded.
    fn logical_size(&self) -> (u32, u32) {
        let (w, h) = self.size();
        let s = self.scale_factor();
        if s > 0.0 {
            ((w as f64 / s).round() as u32, (h as f64 / s).round() as u32)
        } else {
            (w, h)
        }
    }
    /// APPEND this poll's events to `out` (never clear it). The first call also emits the
    /// initial `Window(Resized)` + `Window(ScaleFactorChanged)`.
    fn poll_events(&mut self, out: &mut Vec<InputEvent>);
    /// Start/stop OS text input (Text events). Default no-op.
    fn set_text_input(&mut self, _enabled: bool) {}
    /// Where text is typed, in logical pixels, plus the caret offset (IME popup placement).
    /// Default no-op.
    fn set_text_input_area(&mut self, _x: i32, _y: i32, _w: i32, _h: i32, _cursor: i32) {}
    /// Show/hide the cursor over the window. Default no-op.
    fn set_cursor_visible(&mut self, _visible: bool) {}
    /// Change the cursor shape. Default no-op.
    fn set_cursor_icon(&mut self, _icon: CursorIcon) {}
    /// Grab the pointer: relative mouse mode (native) / pointer lock (web). Default no-op.
    fn set_cursor_grab(&mut self, _grab: bool) {}
    /// Write the current gamepad snapshot into `state` (connect/update/retain). Default no-op.
    fn poll_gamepads(&mut self, _state: &mut GamepadState) {}
}
pub trait WindowManager{
    fn size(&self) -> (u32,u32);
    fn scale_factor(&self) -> f64;
    fn logical_size(&self) -> (u32, u32);
    /// Injected events first, then the backend's own.
    fn poll_events(&mut self, out: &mut Vec<InputEvent>);
    fn set_title(&mut self,title:String);
    fn set_text_input(&mut self, enabled: bool);
    fn set_text_input_area(&mut self, x: i32, y: i32, w: i32, h: i32, cursor: i32);
    fn set_cursor_visible(&mut self, visible: bool);
    /// Change the cursor shape. Default no-op.
    fn set_cursor_icon(&mut self, _icon: CursorIcon) {}
    fn set_cursor_grab(&mut self, grab: bool);
    fn poll_gamepads(&mut self, state: &mut GamepadState);
}
pub struct GameWindowManager{
    inner: Box<dyn InnerWindow>,
    injected: VecDeque<InputEvent>,
    headless: bool,
    text_input_area: Option<(i32, i32, i32, i32, i32)>,
    cursor_icon: CursorIcon,
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
        })
    }
    /// Queue an event; drained into `out` before the backend's own events on the next poll.
    pub fn inject(&mut self, ev: InputEvent) {
        self.injected.push_back(ev);
    }
    /// The last `(x, y, w, h, cursor)` given to `set_text_input_area` (logical px).
    pub fn text_input_area(&self) -> Option<(i32, i32, i32, i32, i32)> {
        self.text_input_area
    }
    /// The last cursor icon given to `set_cursor_icon`.
    pub fn cursor_icon(&self) -> CursorIcon {
        self.cursor_icon
    }
    pub fn is_headless(&self) -> bool {
        self.headless
    }
    /// The canvas the window draws to (web only).
    #[cfg(target_family = "wasm")]
    pub fn canvas(&self) -> Option<web_sys::HtmlCanvasElement>{
        self.inner.canvas()
    }
    #[cfg(target_family = "wasm")]
    pub fn canvas_id(&self) -> &str{
        self.inner.canvas_id()
    }
    #[cfg(target_family = "wasm")]
    pub fn set_canvas_id(&mut self, canvas_id:String){
        self.inner.set_canvas_id(canvas_id);
    }
}

impl WindowManager for GameWindowManager {
    fn size(&self) -> (u32,u32) {
        self.inner.size()
    }

    fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }

    fn logical_size(&self) -> (u32, u32) {
        self.inner.logical_size()
    }

    fn poll_events(&mut self, out: &mut Vec<InputEvent>) {
        out.extend(self.injected.drain(..));
        self.inner.poll_events(out);
    }

    fn set_title(&mut self,title:String) {
        self.inner.set_title(title);
    }

    fn set_text_input(&mut self, enabled: bool) {
        self.inner.set_text_input(enabled);
    }

    fn set_text_input_area(&mut self, x: i32, y: i32, w: i32, h: i32, cursor: i32) {
        self.text_input_area = Some((x, y, w, h, cursor));
        self.inner.set_text_input_area(x, y, w, h, cursor);
    }

    fn set_cursor_visible(&mut self, visible: bool) {
        self.inner.set_cursor_visible(visible);
    }

    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            self.inner.set_cursor_icon(icon);
        }
    }

    fn set_cursor_grab(&mut self, grab: bool) {
        self.inner.set_cursor_grab(grab);
    }

    fn poll_gamepads(&mut self, state: &mut GamepadState) {
        self.inner.poll_gamepads(state);
    }
}

// GameWindow üzerinden handle'lara erişim sağlamak için delegasyon yapıyoruz
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