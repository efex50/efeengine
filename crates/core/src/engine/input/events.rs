//! Platform-neutral input events.
//!
//! Conventions (identical on every backend):
//! - Positions and deltas are physical pixels, origin top-left of the
//!   window/canvas, y down.
//! - Wheel: `+y` = scroll up / away from the user, `+x` = right.
//! - `repeat` is always `false` for `Released`.
//! - `timestamp_ns` is monotonic nanoseconds with a platform-defined origin
//!   (never wall-clock); comparable only within one run.

use super::keys::{ElementState, Key, KeyCode, Modifiers, MouseButton};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

impl WindowId {
    /// The main window. Events without a window (touch) use this.
    pub const MAIN: WindowId = WindowId(0);
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    pub timestamp_ns: u64,
    pub window: WindowId,
    pub kind: InputEventKind,
}

impl InputEvent {
    /// Timestamp 0, main window. Mostly for tests.
    pub fn new(kind: InputEventKind) -> Self {
        Self { timestamp_ns: 0, window: WindowId::MAIN, kind }
    }

    pub fn with_time(kind: InputEventKind, timestamp_ns: u64, window: WindowId) -> Self {
        Self { timestamp_ns, window, kind }
    }
}

impl From<InputEventKind> for InputEvent {
    fn from(kind: InputEventKind) -> Self {
        InputEvent::new(kind)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputEventKind {
    Key { physical: KeyCode, logical: Key, state: ElementState, repeat: bool, mods: Modifiers },
    Text(String),
    MouseMotion { pos: (f32, f32), delta: (f32, f32) },
    MouseButton { button: MouseButton, state: ElementState, pos: (f32, f32) },
    MouseWheel { delta: (f32, f32), unit: WheelUnit },
    Touch { id: u64, phase: TouchPhase, pos: (f32, f32) },
    Window(WindowEvent),
}

impl InputEventKind {
    pub fn key(
        physical: KeyCode,
        logical: Key,
        state: ElementState,
        repeat: bool,
        mods: Modifiers,
    ) -> Self {
        InputEventKind::Key { physical, logical, state, repeat, mods }
    }

    /// Non-repeat press with unidentified logical key and no modifiers.
    pub fn key_pressed(physical: KeyCode) -> Self {
        Self::key(physical, Key::Unidentified, ElementState::Pressed, false, Modifiers::NONE)
    }

    /// Release with unidentified logical key and no modifiers.
    pub fn key_released(physical: KeyCode) -> Self {
        Self::key(physical, Key::Unidentified, ElementState::Released, false, Modifiers::NONE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowEvent {
    /// Physical pixels.
    Resized { width: u32, height: u32 },
    ScaleFactorChanged { scale: f64 },
    Focused(bool),
    CloseRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelUnit {
    Line,
    Pixel,
    Page,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}
