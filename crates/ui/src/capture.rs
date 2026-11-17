//! Input capture (API.md section 10).

/// Whether the UI used this frame's input. Updated each frame by the UI plugin from the backend
/// (and visible modal layers); game systems skip captured devices.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct UiCapture {
    /// The pointer is over a widget, or a modal layer is visible.
    pub pointer: bool,
    /// A text input has focus, or a modal layer is visible.
    pub keyboard: bool,
}
