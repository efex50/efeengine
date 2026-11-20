//! Input capture (API.md section 10).

use efeengine_core::input::InputCapture;
pub use efeui_core::capture::UiCapture;

/// The engine's [`InputCapture`] for a [`UiCapture`]: the UI plugin writes it every frame with
/// `InputState::set_capture` at `order::INPUT + 10`, and `cx.game_input()` filters by it. Read
/// the current value with `cx.input.capture()` / `Engine::input().capture()`.
pub fn input_capture(c: UiCapture) -> InputCapture {
    InputCapture { pointer: c.pointer, keyboard: c.keyboard }
}

/// The [`UiCapture`] for an engine [`InputCapture`].
pub fn ui_capture(c: InputCapture) -> UiCapture {
    UiCapture { pointer: c.pointer, keyboard: c.keyboard }
}
