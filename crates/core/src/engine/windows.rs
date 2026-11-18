//! `Windows`: the per-window state of every window the engine has open, kept up to date from
//! window events (`WindowEvent::Resized`, `ScaleFactorChanged`, `Focused`, `FullscreenChanged`,
//! `Created`, `Closed`). Systems read it through `Ctx::windows`; the main window's entry is also
//! `Ctx::window`.

use std::collections::BTreeMap;

use crate::engine::input::{WindowEvent, WindowId};

/// Last known state of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowState {
    /// Drawable size in physical pixels.
    pub size: (u32, u32),
    pub scale_factor: f64,
    pub focused: bool,
    pub fullscreen: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { size: (0, 0), scale_factor: 1.0, focused: true, fullscreen: false }
    }
}

impl WindowState {
    /// Size in logical pixels (`size / scale_factor`).
    pub fn logical_size(&self) -> (f32, f32) {
        let s = if self.scale_factor.is_finite() && self.scale_factor > 0.0 { self.scale_factor as f32 } else { 1.0 };
        (self.size.0 as f32 / s, self.size.1 as f32 / s)
    }
}

/// The state of the main window and every secondary window. The main window always exists.
#[derive(Debug, Clone, Default)]
pub struct Windows {
    main: WindowState,
    others: BTreeMap<u32, WindowState>,
}

impl Windows {
    pub fn new() -> Self {
        Self::default()
    }

    /// The main window (`WindowId::MAIN`).
    pub fn main(&self) -> &WindowState {
        &self.main
    }

    pub fn get(&self, id: WindowId) -> Option<&WindowState> {
        if id.is_main() { Some(&self.main) } else { self.others.get(&id.0) }
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.get(id).is_some()
    }

    /// All windows, the main one first, then by id.
    pub fn iter(&self) -> impl Iterator<Item = (WindowId, &WindowState)> {
        std::iter::once((WindowId::MAIN, &self.main)).chain(self.others.iter().map(|(k, v)| (WindowId(*k), v)))
    }

    /// Number of open windows (at least 1).
    pub fn len(&self) -> usize {
        1 + self.others.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// A window that has focus (the main one when none does). `InputState::focus_window` is the
    /// one that had it last, which is what the engine's cursor and text-input commands use.
    pub fn focused(&self) -> WindowId {
        self.iter().find(|(_, s)| s.focused).map_or(WindowId::MAIN, |(id, _)| id)
    }

    fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        if id.is_main() { Some(&mut self.main) } else { self.others.get_mut(&id.0) }
    }

    /// Register a secondary window (unfocused, size not known yet).
    pub(crate) fn insert(&mut self, id: WindowId, size: (u32, u32)) {
        if !id.is_main() {
            self.others.insert(id.0, WindowState { size, focused: false, ..WindowState::default() });
        }
    }

    pub(crate) fn remove(&mut self, id: WindowId) -> bool {
        !id.is_main() && self.others.remove(&id.0).is_some()
    }

    /// Fold a window event into the state. Events of unknown windows are ignored.
    pub(crate) fn apply(&mut self, id: WindowId, ev: &WindowEvent) {
        let Some(w) = self.get_mut(id) else { return };
        match *ev {
            WindowEvent::Resized { width, height } => w.size = (width, height),
            WindowEvent::ScaleFactorChanged { scale } => w.scale_factor = scale,
            WindowEvent::Focused(f) => w.focused = f,
            WindowEvent::FullscreenChanged(f) => w.fullscreen = f,
            WindowEvent::CloseRequested | WindowEvent::Created | WindowEvent::Closed => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_each_window_separately() {
        let mut w = Windows::new();
        w.insert(WindowId(1), (10, 10));
        w.apply(WindowId(1), &WindowEvent::Resized { width: 320, height: 240 });
        w.apply(WindowId::MAIN, &WindowEvent::Resized { width: 800, height: 600 });
        w.apply(WindowId(1), &WindowEvent::ScaleFactorChanged { scale: 2.0 });
        assert_eq!(w.main().size, (800, 600));
        assert_eq!(w.get(WindowId(1)).unwrap().size, (320, 240));
        assert_eq!(w.get(WindowId(1)).unwrap().logical_size(), (160.0, 120.0));
        assert_eq!(w.len(), 2);
        // unknown windows are ignored
        w.apply(WindowId(9), &WindowEvent::Resized { width: 1, height: 1 });
        assert!(w.get(WindowId(9)).is_none());
        assert!(w.remove(WindowId(1)));
        assert!(!w.remove(WindowId::MAIN));
        assert_eq!(w.len(), 1);
    }
}
