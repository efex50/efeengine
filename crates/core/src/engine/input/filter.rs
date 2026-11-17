//! Input pre-filter: lets a UI layer claim raw events before game code sees them.

use super::events::InputEvent;

/// A filter's verdict on one raw event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Game code may use the event.
    Pass,
    /// A UI consumed it: `InputEvent::captured` is set, `InputState::game_input()` ignores it.
    Captured,
}

/// Asked about every raw [`InputEvent`] (in the order filters were added with
/// `Engine::add_input_filter`) before `InputState::apply`. Every filter is called for every
/// event; any `Captured` marks the event. Main thread only (no `Send` needed).
pub trait InputFilter: 'static {
    fn filter(&mut self, ev: &InputEvent) -> Filter;
}
