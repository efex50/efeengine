//! Input capture (API.md section 10).

/// Whether the UI used this frame's input: `pointer` is true while the pointer is over a widget
/// (or a modal layer is visible or the backend consumed a pointer event), `keyboard` while a
/// text input has focus (or a modal layer is visible or a key event was consumed).
///
/// This is core's [`InputCapture`](efeengine_core::input::InputCapture): the UI plugin writes
/// it every frame with `InputState::set_capture` at `order::INPUT + 10`, and `cx.game_input()`
/// filters by it. Read the current value with `cx.input.capture()` / `Engine::input().capture()`.
pub type UiCapture = efeengine_core::input::InputCapture;
