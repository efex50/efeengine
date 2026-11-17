//! UI input capture: lets a UI layer claim the pointer and/or keyboard so game code can ignore
//! input the UI consumed.

use std::sync::atomic::{AtomicU8, Ordering};

use super::keys::{KeyCode, MouseButton};
use super::state::InputState;

/// Which devices a UI layer has captured this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputCapture {
    /// The pointer (mouse buttons, motion, wheel) is over / used by the UI.
    pub pointer: bool,
    /// The keyboard (keys, text) is used by the UI (e.g. a focused text field).
    pub keyboard: bool,
}

/// Interior-mutable, thread-safe storage of an [`InputCapture`], so the UI can set it through
/// the shared `&InputState` that systems hold. Bit 0 = pointer, bit 1 = keyboard.
#[derive(Debug, Default)]
pub(super) struct CaptureCell(AtomicU8);

impl CaptureCell {
    pub(super) fn get(&self) -> InputCapture {
        let v = self.0.load(Ordering::Relaxed);
        InputCapture { pointer: v & 1 != 0, keyboard: v & 2 != 0 }
    }

    pub(super) fn set(&self, c: InputCapture) {
        self.0.store(c.pointer as u8 | (c.keyboard as u8) << 1, Ordering::Relaxed);
    }
}

impl Clone for CaptureCell {
    fn clone(&self) -> Self {
        Self(AtomicU8::new(self.0.load(Ordering::Relaxed)))
    }
}

/// A view of [`InputState`] for game code: the same key / mouse queries, but a captured device
/// reports nothing pressed and no movement. Get one with [`InputState::game_input`].
#[derive(Debug, Clone, Copy)]
pub struct GameInput<'a> {
    input: &'a InputState,
    capture: InputCapture,
}

impl<'a> GameInput<'a> {
    pub(super) fn new(input: &'a InputState) -> Self {
        Self { input, capture: input.capture() }
    }

    /// The unfiltered state.
    pub fn raw(&self) -> &'a InputState {
        self.input
    }

    /// The capture flags this view filters by.
    pub fn capture(&self) -> InputCapture {
        self.capture
    }

    /// Key held (`false` while the keyboard is captured).
    pub fn pressed(&self, k: KeyCode) -> bool { !self.capture.keyboard && self.input.pressed(k) }
    /// Key pressed this frame (`false` while the keyboard is captured).
    pub fn just_pressed(&self, k: KeyCode) -> bool { !self.capture.keyboard && self.input.just_pressed(k) }
    /// Key released this frame (`false` while the keyboard is captured).
    pub fn just_released(&self, k: KeyCode) -> bool { !self.capture.keyboard && self.input.just_released(k) }
    /// Any key held (`false` while the keyboard is captured).
    pub fn any_pressed(&self) -> bool { !self.capture.keyboard && self.input.any_pressed() }
    /// Typed text this frame (empty while the keyboard is captured).
    pub fn text(&self) -> &'a str { if self.capture.keyboard { "" } else { self.input.text() } }

    /// Mouse button held (`false` while the pointer is captured).
    pub fn mouse_pressed(&self, b: MouseButton) -> bool { !self.capture.pointer && self.input.mouse_pressed(b) }
    /// Mouse button pressed this frame (`false` while the pointer is captured).
    pub fn mouse_just_pressed(&self, b: MouseButton) -> bool { !self.capture.pointer && self.input.mouse_just_pressed(b) }
    /// Mouse button released this frame (`false` while the pointer is captured).
    pub fn mouse_just_released(&self, b: MouseButton) -> bool { !self.capture.pointer && self.input.mouse_just_released(b) }
    /// Pointer position (`None` while the pointer is captured).
    pub fn mouse_pos(&self) -> Option<(f32, f32)> { if self.capture.pointer { None } else { self.input.mouse_pos() } }
    /// Pointer movement this frame (zero while the pointer is captured).
    pub fn mouse_delta(&self) -> (f32, f32) { if self.capture.pointer { (0.0, 0.0) } else { self.input.mouse_delta() } }
    /// Wheel movement in lines this frame (zero while the pointer is captured).
    pub fn wheel(&self) -> (f32, f32) { if self.capture.pointer { (0.0, 0.0) } else { self.input.wheel() } }
}
