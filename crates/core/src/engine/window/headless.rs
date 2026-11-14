//! Window-less backend for tests and tools. Events come only from `GameWindowManager::inject`.

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};

use crate::engine::input::{InputEvent, InputEventKind, WindowEvent};
use crate::engine::window::InnerWindow;

pub struct HeadlessWindow {
    size: (u32, u32),
    title: String,
    sent_initial: bool,
    #[cfg(target_family = "wasm")]
    canvas_id: String,
}

impl HeadlessWindow {
    pub fn new(size: (u32, u32)) -> Self {
        Self {
            size,
            title: String::new(),
            sent_initial: false,
            #[cfg(target_family = "wasm")]
            canvas_id: "canvas".to_string(),
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

impl HasWindowHandle for HeadlessWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }
}

impl HasDisplayHandle for HeadlessWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }
}

impl InnerWindow for HeadlessWindow {
    #[cfg(target_family = "wasm")]
    fn set_canvas_id(&mut self, canvas_id: String) {
        self.canvas_id = canvas_id;
    }
    #[cfg(target_family = "wasm")]
    fn canvas(&self) -> Option<web_sys::HtmlCanvasElement> {
        None
    }
    #[cfg(target_family = "wasm")]
    fn canvas_id(&self) -> &str {
        &self.canvas_id
    }
    fn set_title(&mut self, title: String) {
        self.title = title;
    }
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn poll_events(&mut self, out: &mut Vec<InputEvent>) {
        if !self.sent_initial {
            self.sent_initial = true;
            out.push(InputEvent::new(InputEventKind::Window(
                WindowEvent::Resized {
                    width: self.size.0,
                    height: self.size.1,
                },
            )));
            out.push(InputEvent::new(InputEventKind::Window(
                WindowEvent::ScaleFactorChanged { scale: 1.0 },
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_events_once() {
        let mut w = HeadlessWindow::new((320, 200));
        let mut out = Vec::new();
        w.poll_events(&mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].kind,
            InputEventKind::Window(WindowEvent::Resized {
                width: 320,
                height: 200
            })
        );
        assert_eq!(
            out[1].kind,
            InputEventKind::Window(WindowEvent::ScaleFactorChanged { scale: 1.0 })
        );
        out.clear();
        w.poll_events(&mut out);
        assert!(out.is_empty());
    }
}
