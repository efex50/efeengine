//! Gamepad state (roadmap 1.12). Backends fill it once per frame by polling; it is not part of
//! the InputEvent stream. Buttons follow the W3C "standard" gamepad / SDL gamepad layout.
//! Axes are normalised: sticks -1.0..=1.0 with +x = right and +y = down (SDL and the W3C
//! standard mapping agree), triggers 0.0..=1.0.

/// Stable identifier of a connected pad, assigned by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GamepadId(pub u32);

/// Digital buttons on a pad, using the W3C "standard" / SDL gamepad layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamepadButton {
    /// Bottom face button (A on Xbox, Cross on PlayStation).
    South,
    /// Right face button (B on Xbox, Circle on PlayStation).
    East,
    /// Left face button (X on Xbox, Square on PlayStation).
    West,
    /// Top face button (Y on Xbox, Triangle on PlayStation).
    North,
    /// Back / Select / Share button.
    Back,
    /// Guide / Home / PS button.
    Guide,
    /// Start / Menu / Options button.
    Start,
    /// Left stick pressed in (L3).
    LeftStick,
    /// Right stick pressed in (R3).
    RightStick,
    /// Left shoulder bumper (LB / L1).
    LeftShoulder,
    /// Right shoulder bumper (RB / R1).
    RightShoulder,
    /// Left trigger as a digital button (LT / L2).
    LeftTrigger,
    /// Right trigger as a digital button (RT / R2).
    RightTrigger,
    /// D-pad up.
    DPadUp,
    /// D-pad down.
    DPadDown,
    /// D-pad left.
    DPadLeft,
    /// D-pad right.
    DPadRight,
}

impl GamepadButton {
    /// Every button, in declaration order.
    pub const ALL: [GamepadButton; 17] = [
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::West,
        GamepadButton::North,
        GamepadButton::Back,
        GamepadButton::Guide,
        GamepadButton::Start,
        GamepadButton::LeftStick,
        GamepadButton::RightStick,
        GamepadButton::LeftShoulder,
        GamepadButton::RightShoulder,
        GamepadButton::LeftTrigger,
        GamepadButton::RightTrigger,
        GamepadButton::DPadUp,
        GamepadButton::DPadDown,
        GamepadButton::DPadLeft,
        GamepadButton::DPadRight,
    ];
}

/// Analog axes on a pad. Sticks are -1.0..=1.0, triggers 0.0..=1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamepadAxis {
    /// Left stick, horizontal (+1 = right).
    LeftX,
    /// Left stick, vertical (+1 = down).
    LeftY,
    /// Right stick, horizontal (+1 = right).
    RightX,
    /// Right stick, vertical (+1 = down).
    RightY,
    /// Left trigger pull amount.
    LeftTrigger,
    /// Right trigger pull amount.
    RightTrigger,
}

impl GamepadAxis {
    /// Every axis, in declaration order.
    pub const ALL: [GamepadAxis; 6] = [
        GamepadAxis::LeftX,
        GamepadAxis::LeftY,
        GamepadAxis::RightX,
        GamepadAxis::RightY,
        GamepadAxis::LeftTrigger,
        GamepadAxis::RightTrigger,
    ];

    /// Whether this axis is a trigger (clamped to 0..=1) rather than a stick (-1..=1).
    fn is_trigger(self) -> bool {
        matches!(self, GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger)
    }
}

/// Bit mask for a button inside a `u32` button word.
fn button_bit(b: GamepadButton) -> u32 {
    1 << (b as u32)
}

/// One connected pad.
#[derive(Debug, Clone, PartialEq)]
pub struct Gamepad {
    id: GamepadId,
    name: String,
    /// One bit per [`GamepadButton`], indexed by its declaration order.
    buttons: u32,
    /// `buttons` as they were at the last `begin_frame`.
    prev_buttons: u32,
    /// Indexed by [`GamepadAxis`] declaration order.
    axes: [f32; 6],
}

impl Gamepad {
    fn new(id: GamepadId, name: String) -> Self {
        Self {
            id,
            name,
            buttons: 0,
            prev_buttons: 0,
            axes: [0.0; 6],
        }
    }

    /// The pad's identifier.
    pub fn id(&self) -> GamepadId {
        self.id
    }

    /// The pad's human-readable name as reported by the backend.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether `b` is held down now.
    pub fn pressed(&self, b: GamepadButton) -> bool {
        self.buttons & button_bit(b) != 0
    }

    /// Whether `b` is down now but was up at the previous `begin_frame`.
    pub fn just_pressed(&self, b: GamepadButton) -> bool {
        self.pressed(b) && self.prev_buttons & button_bit(b) == 0
    }

    /// Whether `b` is up now but was down at the previous `begin_frame`.
    pub fn just_released(&self, b: GamepadButton) -> bool {
        !self.pressed(b) && self.prev_buttons & button_bit(b) != 0
    }

    /// Current value of `a` (sticks -1..=1, triggers 0..=1).
    pub fn axis(&self, a: GamepadAxis) -> f32 {
        self.axes[a as usize]
    }
}

/// All connected pads, sorted by id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GamepadState {
    pads: Vec<Gamepad>,
}

impl GamepadState {
    /// An empty state with no pads connected.
    pub fn new() -> Self {
        Self { pads: Vec::new() }
    }

    /// Start of a frame: remember the current buttons as "previous" for `just_*` queries.
    pub fn begin_frame(&mut self) {
        for pad in &mut self.pads {
            pad.prev_buttons = pad.buttons;
        }
    }

    /// Add a pad (no-op if present, but updates its name).
    pub fn connect(&mut self, id: GamepadId, name: impl Into<String>) {
        let name = name.into();
        match self.pads.binary_search_by_key(&id, |p| p.id) {
            Ok(index) => self.pads[index].name = name,
            Err(index) => self.pads.insert(index, Gamepad::new(id, name)),
        }
    }

    /// Remove the pad with `id`, if connected.
    pub fn disconnect(&mut self, id: GamepadId) {
        self.pads.retain(|p| p.id != id);
    }

    /// Keep only the pads whose id is in `ids` (backends that enumerate pads each frame).
    pub fn retain(&mut self, ids: &[GamepadId]) {
        self.pads.retain(|p| ids.contains(&p.id));
    }

    /// Set the state of button `b` on pad `id`. Values for unknown ids are ignored
    /// (connect the pad first).
    pub fn set_button(&mut self, id: GamepadId, b: GamepadButton, pressed: bool) {
        if let Some(pad) = self.pads.iter_mut().find(|p| p.id == id) {
            if pressed {
                pad.buttons |= button_bit(b);
            } else {
                pad.buttons &= !button_bit(b);
            }
        }
    }

    /// Set axis `a` on pad `id`. Sticks are clamped to -1..=1 and triggers to 0..=1;
    /// NaN becomes 0. Values for unknown ids are ignored (connect the pad first).
    pub fn set_axis(&mut self, id: GamepadId, a: GamepadAxis, value: f32) {
        let value = if value.is_nan() {
            0.0
        } else if a.is_trigger() {
            value.clamp(0.0, 1.0)
        } else {
            value.clamp(-1.0, 1.0)
        };
        if let Some(pad) = self.pads.iter_mut().find(|p| p.id == id) {
            pad.axes[a as usize] = value;
        }
    }

    /// The pad with `id`, if connected.
    pub fn get(&self, id: GamepadId) -> Option<&Gamepad> {
        self.pads.iter().find(|p| p.id == id)
    }

    /// All connected pads, sorted by id.
    pub fn pads(&self) -> impl Iterator<Item = &Gamepad> + '_ {
        self.pads.iter()
    }

    /// Number of connected pads.
    pub fn len(&self) -> usize {
        self.pads.len()
    }

    /// Whether no pads are connected.
    pub fn is_empty(&self) -> bool {
        self.pads.is_empty()
    }

    /// Convenience: the first pad (lowest id), for single-player games.
    pub fn first(&self) -> Option<&Gamepad> {
        self.pads.first()
    }
}

/// Convert a signed 16-bit stick value (SDL range) to -1.0..=1.0.
/// `i16::MIN` maps to exactly -1.0.
pub fn axis_from_i16(v: i16) -> f32 {
    if v == i16::MIN {
        -1.0
    } else {
        (v as f32 / 32767.0).clamp(-1.0, 1.0)
    }
}

/// Convert an unsigned-range 16-bit trigger value (SDL range 0..=32767) to 0.0..=1.0.
/// Negative input maps to 0.0.
pub fn trigger_from_i16(v: i16) -> f32 {
    if v <= 0 {
        0.0
    } else {
        (v as f32 / 32767.0).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: GamepadId = GamepadId(1);
    const OTHER: GamepadId = GamepadId(2);

    #[test]
    fn connect_disconnect_retain() {
        let mut s = GamepadState::new();
        assert!(s.is_empty());
        s.connect(PAD, "A");
        s.connect(OTHER, "B");
        assert_eq!(s.len(), 2);
        assert!(s.get(PAD).is_some());

        s.disconnect(PAD);
        assert_eq!(s.len(), 1);
        assert!(s.get(PAD).is_none());

        s.connect(PAD, "A");
        s.retain(&[OTHER]);
        assert_eq!(s.len(), 1);
        assert!(s.get(PAD).is_none());
        assert_eq!(s.get(OTHER).unwrap().name(), "B");

        s.retain(&[]);
        assert!(s.is_empty());
    }

    #[test]
    fn connect_existing_updates_name() {
        let mut s = GamepadState::new();
        s.connect(PAD, "old");
        s.set_button(PAD, GamepadButton::South, true);
        s.connect(PAD, "new");
        assert_eq!(s.len(), 1);
        let pad = s.get(PAD).unwrap();
        assert_eq!(pad.name(), "new");
        assert_eq!(pad.id(), PAD);
        // Reconnecting must not reset the button state.
        assert!(pad.pressed(GamepadButton::South));
    }

    #[test]
    fn pads_sorted_by_id() {
        let mut s = GamepadState::new();
        s.connect(GamepadId(5), "e");
        s.connect(GamepadId(2), "b");
        s.connect(GamepadId(9), "i");
        s.connect(GamepadId(0), "a");
        let ids: Vec<u32> = s.pads().map(|p| p.id().0).collect();
        assert_eq!(ids, vec![0, 2, 5, 9]);
    }

    #[test]
    fn set_button_and_just_pressed_across_begin_frame() {
        let mut s = GamepadState::new();
        s.connect(PAD, "A");

        s.begin_frame();
        s.set_button(PAD, GamepadButton::South, true);
        let pad = s.get(PAD).unwrap();
        assert!(pad.pressed(GamepadButton::South));
        assert!(pad.just_pressed(GamepadButton::South));
        assert!(!pad.just_released(GamepadButton::South));
        assert!(!pad.pressed(GamepadButton::East));

        // Still held in the next frame: pressed, but no longer "just" pressed.
        s.begin_frame();
        let pad = s.get(PAD).unwrap();
        assert!(pad.pressed(GamepadButton::South));
        assert!(!pad.just_pressed(GamepadButton::South));
    }

    #[test]
    fn just_released() {
        let mut s = GamepadState::new();
        s.connect(PAD, "A");
        s.set_button(PAD, GamepadButton::Start, true);
        s.begin_frame();
        s.set_button(PAD, GamepadButton::Start, false);

        let pad = s.get(PAD).unwrap();
        assert!(!pad.pressed(GamepadButton::Start));
        assert!(pad.just_released(GamepadButton::Start));
        assert!(!pad.just_pressed(GamepadButton::Start));

        s.begin_frame();
        assert!(!s.get(PAD).unwrap().just_released(GamepadButton::Start));
    }

    #[test]
    fn button_bits_are_independent() {
        let mut s = GamepadState::new();
        s.connect(PAD, "A");
        for b in GamepadButton::ALL {
            s.set_button(PAD, b, true);
        }
        for b in GamepadButton::ALL {
            assert!(s.get(PAD).unwrap().pressed(b));
        }
        s.set_button(PAD, GamepadButton::DPadRight, false);
        let pad = s.get(PAD).unwrap();
        assert!(!pad.pressed(GamepadButton::DPadRight));
        assert!(pad.pressed(GamepadButton::DPadLeft));
        assert!(pad.pressed(GamepadButton::South));
    }

    #[test]
    fn set_axis_clamps_sticks() {
        let mut s = GamepadState::new();
        s.connect(PAD, "A");
        s.set_axis(PAD, GamepadAxis::LeftX, 3.0);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::LeftX), 1.0);
        s.set_axis(PAD, GamepadAxis::LeftX, -3.0);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::LeftX), -1.0);
        s.set_axis(PAD, GamepadAxis::RightY, -0.25);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::RightY), -0.25);
    }

    #[test]
    fn set_axis_clamps_triggers() {
        let mut s = GamepadState::new();
        s.connect(PAD, "A");
        s.set_axis(PAD, GamepadAxis::LeftTrigger, 2.0);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::LeftTrigger), 1.0);
        // Triggers do not go negative.
        s.set_axis(PAD, GamepadAxis::RightTrigger, -0.5);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::RightTrigger), 0.0);
        s.set_axis(PAD, GamepadAxis::RightTrigger, 0.5);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::RightTrigger), 0.5);
    }

    #[test]
    fn set_axis_nan_is_zero() {
        let mut s = GamepadState::new();
        s.connect(PAD, "A");
        s.set_axis(PAD, GamepadAxis::LeftY, 0.7);
        s.set_axis(PAD, GamepadAxis::LeftY, f32::NAN);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::LeftY), 0.0);
        s.set_axis(PAD, GamepadAxis::LeftTrigger, 0.7);
        s.set_axis(PAD, GamepadAxis::LeftTrigger, f32::NAN);
        assert_eq!(s.get(PAD).unwrap().axis(GamepadAxis::LeftTrigger), 0.0);
    }

    #[test]
    fn unknown_id_ignored() {
        let mut s = GamepadState::new();
        s.connect(PAD, "A");
        s.set_button(OTHER, GamepadButton::South, true);
        s.set_axis(OTHER, GamepadAxis::LeftX, 0.5);
        assert!(s.get(OTHER).is_none());
        assert_eq!(s.len(), 1);
        let pad = s.get(PAD).unwrap();
        assert!(!pad.pressed(GamepadButton::South));
        assert_eq!(pad.axis(GamepadAxis::LeftX), 0.0);
    }

    #[test]
    fn first_returns_lowest_id() {
        let mut s = GamepadState::new();
        assert!(s.first().is_none());
        s.connect(GamepadId(7), "seven");
        s.connect(GamepadId(3), "three");
        assert_eq!(s.first().unwrap().id(), GamepadId(3));
        s.disconnect(GamepadId(3));
        assert_eq!(s.first().unwrap().id(), GamepadId(7));
    }

    #[test]
    fn all_lists_are_complete_and_ordered() {
        assert_eq!(GamepadButton::ALL.len(), 17);
        assert_eq!(GamepadButton::ALL[0], GamepadButton::South);
        assert_eq!(GamepadButton::ALL[16], GamepadButton::DPadRight);
        assert_eq!(GamepadAxis::ALL.len(), 6);
        assert_eq!(GamepadAxis::ALL[5], GamepadAxis::RightTrigger);
    }

    #[test]
    fn axis_from_i16_bounds() {
        assert_eq!(axis_from_i16(0), 0.0);
        assert_eq!(axis_from_i16(i16::MAX), 1.0);
        assert_eq!(axis_from_i16(i16::MIN), -1.0);
        assert_eq!(axis_from_i16(-32767), -1.0);
        assert!((axis_from_i16(16384) - 0.5).abs() < 1e-3);
        assert!((axis_from_i16(-16384) + 0.5).abs() < 1e-3);
    }

    #[test]
    fn trigger_from_i16_bounds() {
        assert_eq!(trigger_from_i16(0), 0.0);
        assert_eq!(trigger_from_i16(i16::MAX), 1.0);
        assert_eq!(trigger_from_i16(-1), 0.0);
        assert_eq!(trigger_from_i16(i16::MIN), 0.0);
        assert!((trigger_from_i16(16384) - 0.5).abs() < 1e-3);
    }
}
