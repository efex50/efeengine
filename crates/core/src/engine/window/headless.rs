//! Window-less backend for tests and tools. Events come only from `GameWindowManager::inject`.
//! Secondary windows are virtual: they have a size and a title and report their initial
//! `Resized` / `ScaleFactorChanged`, nothing else.

use std::collections::BTreeMap;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};

use crate::engine::error::EngineError;
use crate::engine::input::{InputEvent, InputEventKind, WindowEvent, WindowId};
use crate::engine::window::{InnerWindow, WindowDesc};

struct Virtual {
    size: (u32, u32),
    title: String,
    sent_initial: bool,
}

pub struct HeadlessWindow {
    main: Virtual,
    others: BTreeMap<u32, Virtual>,
    #[cfg(target_family = "wasm")]
    canvas_id: String,
}

impl HeadlessWindow {
    pub fn new(size: (u32, u32)) -> Self {
        Self {
            main: Virtual { size, title: String::new(), sent_initial: false },
            others: BTreeMap::new(),
            #[cfg(target_family = "wasm")]
            canvas_id: "canvas".to_string(),
        }
    }

    /// The main window's title.
    pub fn title(&self) -> &str {
        &self.main.title
    }

    fn get(&self, w: WindowId) -> &Virtual {
        self.others.get(&w.0).unwrap_or(&self.main)
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
    fn canvas(&self, _window: WindowId) -> Option<web_sys::HtmlCanvasElement> {
        None
    }
    #[cfg(target_family = "wasm")]
    fn canvas_id(&self) -> &str {
        &self.canvas_id
    }
    fn create_window(&mut self, window: WindowId, desc: &WindowDesc) -> Result<(), EngineError> {
        self.others.insert(window.0, Virtual { size: desc.size, title: desc.title.clone(), sent_initial: false });
        Ok(())
    }
    fn close_window(&mut self, window: WindowId) {
        self.others.remove(&window.0);
    }
    fn set_title(&mut self, window: WindowId, title: String) {
        match self.others.get_mut(&window.0) {
            Some(v) => v.title = title,
            None => self.main.title = title,
        }
    }
    fn size(&self, window: WindowId) -> (u32, u32) {
        self.get(window).size
    }
    fn poll_events(&mut self, out: &mut Vec<InputEvent>) {
        let main = std::iter::once((0u32, &mut self.main));
        for (id, v) in main.chain(self.others.iter_mut().map(|(k, v)| (*k, v))) {
            if v.sent_initial {
                continue;
            }
            v.sent_initial = true;
            let window = WindowId(id);
            let mk = |k| InputEvent::with_time(InputEventKind::Window(k), 0, window);
            out.push(mk(WindowEvent::Resized { width: v.size.0, height: v.size.1 }));
            out.push(mk(WindowEvent::ScaleFactorChanged { scale: 1.0 }));
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

    #[test]
    fn secondary_windows_report_their_own_initial_events() {
        let mut w = HeadlessWindow::new((320, 200));
        let mut out = Vec::new();
        w.poll_events(&mut out);
        out.clear();
        w.create_window(WindowId(1), &WindowDesc::new("Map").size(100, 50)).unwrap();
        w.poll_events(&mut out);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| e.window == WindowId(1)));
        assert_eq!(w.size(WindowId(1)), (100, 50));
        assert_eq!(w.size(WindowId::MAIN), (320, 200));
        w.close_window(WindowId(1));
        out.clear();
        w.poll_events(&mut out);
        assert!(out.is_empty());
    }
}
