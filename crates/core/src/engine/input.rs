//! Platform-neutral input: key/button types, input events and per-frame input state.
//!
//! Backends translate platform events into [`InputEvent`]s; [`InputState`]
//! folds them into queryable per-frame state. Nothing here depends on a
//! windowing system.

pub mod capture;
pub mod events;
pub mod gamepad;
pub mod keys;
pub mod state;

pub use capture::{GameInput, InputCapture};
pub use events::*;
pub use gamepad::*;
pub use keys::*;
pub use state::*;
