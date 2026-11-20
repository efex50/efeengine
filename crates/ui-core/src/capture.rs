//! Input capture (API.md section 10).

/// Whether the UI used this frame's input: `pointer` is true while the pointer is over a widget
/// (or a modal layer is visible or the backend consumed a pointer event), `keyboard` while a
/// text input has focus (or a modal layer is visible or a key event was consumed).
///
/// Adapters copy it into their engine's own capture type every frame.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct UiCapture {
    pub pointer: bool,
    pub keyboard: bool,
}
