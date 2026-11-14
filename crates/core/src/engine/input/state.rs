//! Per-frame input state folded from [`InputEvent`]s.

use std::collections::HashSet;

use super::events::{InputEvent, InputEventKind, TouchPhase, WheelUnit, WindowEvent};
use super::keys::{ElementState, KeyCode, Modifiers, MouseButton};

/// Pixels per wheel line (`Pixel` units are divided by this).
pub const WHEEL_PIXELS_PER_LINE: f32 = 40.0;
/// Lines per wheel page (`Page` units are multiplied by this).
pub const WHEEL_LINES_PER_PAGE: f32 = 20.0;

#[derive(Debug, Clone)]
pub struct InputState {
    held: HashSet<KeyCode>,
    just_pressed: HashSet<KeyCode>,
    just_released: HashSet<KeyCode>,
    mouse_held: HashSet<MouseButton>,
    mouse_just_pressed: HashSet<MouseButton>,
    mouse_just_released: HashSet<MouseButton>,
    mouse_pos: Option<(f32, f32)>,
    mouse_delta: (f32, f32),
    wheel: (f32, f32),
    touches: Vec<(u64, (f32, f32))>,
    text: String,
    modifiers: Modifiers,
    focused: bool,
    events: Vec<InputEvent>,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl InputState {
    pub fn new() -> Self {
        Self {
            held: HashSet::new(),
            just_pressed: HashSet::new(),
            just_released: HashSet::new(),
            mouse_held: HashSet::new(),
            mouse_just_pressed: HashSet::new(),
            mouse_just_released: HashSet::new(),
            mouse_pos: None,
            mouse_delta: (0.0, 0.0),
            wheel: (0.0, 0.0),
            touches: Vec::new(),
            text: String::new(),
            modifiers: Modifiers::NONE,
            focused: true,
            events: Vec::new(),
        }
    }

    /// Clears per-frame state. Held keys/buttons, mouse position, touches,
    /// modifiers and focus persist.
    pub fn begin_frame(&mut self) {
        self.just_pressed.clear();
        self.just_released.clear();
        self.mouse_just_pressed.clear();
        self.mouse_just_released.clear();
        self.mouse_delta = (0.0, 0.0);
        self.wheel = (0.0, 0.0);
        self.text.clear();
        self.events.clear();
    }

    pub fn apply(&mut self, ev: &InputEvent) {
        self.events.push(ev.clone());
        match &ev.kind {
            InputEventKind::Key { physical, state, repeat, mods, .. } => {
                self.modifiers = *mods;
                if *physical == KeyCode::Unknown {
                    return;
                }
                match state {
                    ElementState::Pressed => {
                        let newly = self.held.insert(*physical);
                        if newly && !*repeat {
                            self.just_pressed.insert(*physical);
                        }
                    }
                    ElementState::Released => {
                        if self.held.remove(physical) {
                            self.just_released.insert(*physical);
                        }
                    }
                }
            }
            InputEventKind::Text(s) => self.text.push_str(s),
            InputEventKind::MouseMotion { pos, delta } => {
                self.mouse_pos = Some(*pos);
                self.mouse_delta.0 += delta.0;
                self.mouse_delta.1 += delta.1;
            }
            InputEventKind::MouseButton { button, state, pos } => {
                self.mouse_pos = Some(*pos);
                match state {
                    ElementState::Pressed => {
                        if self.mouse_held.insert(*button) {
                            self.mouse_just_pressed.insert(*button);
                        }
                    }
                    ElementState::Released => {
                        if self.mouse_held.remove(button) {
                            self.mouse_just_released.insert(*button);
                        }
                    }
                }
            }
            InputEventKind::MouseWheel { delta, unit } => {
                let k = match unit {
                    WheelUnit::Line => 1.0,
                    WheelUnit::Pixel => 1.0 / WHEEL_PIXELS_PER_LINE,
                    WheelUnit::Page => WHEEL_LINES_PER_PAGE,
                };
                self.wheel.0 += delta.0 * k;
                self.wheel.1 += delta.1 * k;
            }
            InputEventKind::Touch { id, phase, pos } => match phase {
                TouchPhase::Started | TouchPhase::Moved => {
                    if let Some(t) = self.touches.iter_mut().find(|t| t.0 == *id) {
                        t.1 = *pos;
                    } else {
                        self.touches.push((*id, *pos));
                    }
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    self.touches.retain(|t| t.0 != *id);
                }
            },
            InputEventKind::Window(w) => match w {
                WindowEvent::Focused(true) => self.focused = true,
                WindowEvent::Focused(false) => {
                    self.focused = false;
                    self.just_released.extend(self.held.drain());
                    self.mouse_just_released.extend(self.mouse_held.drain());
                    self.modifiers = Modifiers::NONE;
                    self.touches.clear();
                }
                WindowEvent::Resized { .. }
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::CloseRequested => {}
            },
        }
    }

    pub fn pressed(&self, k: KeyCode) -> bool { self.held.contains(&k) }
    pub fn just_pressed(&self, k: KeyCode) -> bool { self.just_pressed.contains(&k) }
    pub fn just_released(&self, k: KeyCode) -> bool { self.just_released.contains(&k) }
    pub fn any_pressed(&self) -> bool { !self.held.is_empty() }
    pub fn pressed_keys(&self) -> impl Iterator<Item = KeyCode> + '_ { self.held.iter().copied() }

    pub fn mouse_pressed(&self, b: MouseButton) -> bool { self.mouse_held.contains(&b) }
    pub fn mouse_just_pressed(&self, b: MouseButton) -> bool { self.mouse_just_pressed.contains(&b) }
    pub fn mouse_just_released(&self, b: MouseButton) -> bool { self.mouse_just_released.contains(&b) }
    /// `None` until the first motion/button event.
    pub fn mouse_pos(&self) -> Option<(f32, f32)> { self.mouse_pos }
    /// Summed over the frame.
    pub fn mouse_delta(&self) -> (f32, f32) { self.mouse_delta }
    /// Summed over the frame, in lines.
    pub fn wheel(&self) -> (f32, f32) { self.wheel }

    pub fn touches(&self) -> impl Iterator<Item = (u64, (f32, f32))> + '_ {
        self.touches.iter().copied()
    }
    /// All `Text` events of the frame, concatenated.
    pub fn text(&self) -> &str { &self.text }
    pub fn modifiers(&self) -> Modifiers { self.modifiers }
    pub fn focused(&self) -> bool { self.focused }
    /// Raw events applied since `begin_frame`, in order.
    pub fn events(&self) -> &[InputEvent] { &self.events }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::input::keys::Key;

    fn ev(k: InputEventKind) -> InputEvent { InputEvent::new(k) }
    fn press(k: KeyCode) -> InputEvent { ev(InputEventKind::key_pressed(k)) }
    fn release(k: KeyCode) -> InputEvent { ev(InputEventKind::key_released(k)) }
    fn repeat(k: KeyCode) -> InputEvent {
        ev(InputEventKind::key(k, Key::Unidentified, ElementState::Pressed, true, Modifiers::NONE))
    }
    fn focus(b: bool) -> InputEvent { ev(InputEventKind::Window(WindowEvent::Focused(b))) }
    fn mbtn(b: MouseButton, s: ElementState) -> InputEvent {
        ev(InputEventKind::MouseButton { button: b, state: s, pos: (1.0, 2.0) })
    }
    fn wheel(d: (f32, f32), unit: WheelUnit) -> InputEvent {
        ev(InputEventKind::MouseWheel { delta: d, unit })
    }

    #[test]
    fn rule1_press_sets_held_and_just_pressed() {
        let mut s = InputState::new();
        s.apply(&press(KeyCode::A));
        assert!(s.pressed(KeyCode::A) && s.just_pressed(KeyCode::A));
        assert!(s.any_pressed());
        assert_eq!(s.pressed_keys().collect::<Vec<_>>(), vec![KeyCode::A]);
    }

    #[test]
    fn rule2_repeat_never_just_pressed() {
        let mut s = InputState::new();
        s.apply(&repeat(KeyCode::A));
        assert!(s.pressed(KeyCode::A));
        assert!(!s.just_pressed(KeyCode::A));
    }

    #[test]
    fn rule3_duplicate_press_not_just_pressed() {
        let mut s = InputState::new();
        s.apply(&press(KeyCode::A));
        s.begin_frame();
        s.apply(&press(KeyCode::A));
        assert!(s.pressed(KeyCode::A));
        assert!(!s.just_pressed(KeyCode::A));
    }

    #[test]
    fn rule4_release_only_just_released_if_held() {
        let mut s = InputState::new();
        s.apply(&release(KeyCode::A));
        assert!(!s.just_released(KeyCode::A));
        s.apply(&press(KeyCode::A));
        s.begin_frame();
        s.apply(&release(KeyCode::A));
        assert!(!s.pressed(KeyCode::A) && s.just_released(KeyCode::A));
    }

    #[test]
    fn rule5_press_and_release_same_frame() {
        let mut s = InputState::new();
        s.apply(&press(KeyCode::A));
        s.apply(&release(KeyCode::A));
        assert!(s.just_pressed(KeyCode::A) && s.just_released(KeyCode::A));
        assert!(!s.pressed(KeyCode::A));
    }

    #[test]
    fn rule6_unknown_never_tracked() {
        let mut s = InputState::new();
        s.apply(&press(KeyCode::Unknown));
        assert!(!s.pressed(KeyCode::Unknown) && !s.just_pressed(KeyCode::Unknown));
        assert!(!s.any_pressed());
        assert_eq!(s.events().len(), 1);
    }

    #[test]
    fn rule7_mods_replace_modifiers() {
        let mut s = InputState::new();
        let k = |m| {
            ev(InputEventKind::key(KeyCode::A, Key::Unidentified, ElementState::Pressed, false, m))
        };
        s.apply(&k(Modifiers::LSHIFT | Modifiers::RCTRL));
        assert!(s.modifiers().shift() && s.modifiers().ctrl());
        s.apply(&k(Modifiers::LALT));
        assert_eq!(s.modifiers(), Modifiers::LALT);
    }

    #[test]
    fn rule8_focus_lost_releases_everything() {
        let mut s = InputState::new();
        s.apply(&ev(InputEventKind::key(
            KeyCode::A, Key::Unidentified, ElementState::Pressed, false, Modifiers::SHIFT,
        )));
        s.apply(&mbtn(MouseButton::Left, ElementState::Pressed));
        s.apply(&ev(InputEventKind::Touch { id: 1, phase: TouchPhase::Started, pos: (0.0, 0.0) }));
        s.begin_frame();
        s.apply(&focus(false));
        assert!(!s.focused());
        assert!(!s.pressed(KeyCode::A) && s.just_released(KeyCode::A));
        assert!(!s.mouse_pressed(MouseButton::Left) && s.mouse_just_released(MouseButton::Left));
        assert!(s.modifiers().is_empty());
        assert_eq!(s.touches().count(), 0);
        s.begin_frame();
        s.apply(&release(KeyCode::A));
        assert!(!s.just_released(KeyCode::A));
        s.apply(&focus(true));
        assert!(s.focused());
    }

    #[test]
    fn rule9_wheel_units_to_lines() {
        let mut s = InputState::new();
        s.apply(&wheel((1.0, 2.0), WheelUnit::Line));
        assert_eq!(s.wheel(), (1.0, 2.0));
        s.begin_frame();
        s.apply(&wheel((80.0, -40.0), WheelUnit::Pixel));
        assert_eq!(s.wheel(), (2.0, -1.0));
        s.begin_frame();
        s.apply(&wheel((0.0, 1.0), WheelUnit::Page));
        assert_eq!(s.wheel(), (0.0, WHEEL_LINES_PER_PAGE));
        s.apply(&wheel((0.0, 1.0), WheelUnit::Line));
        assert_eq!(s.wheel(), (0.0, WHEEL_LINES_PER_PAGE + 1.0));
    }

    #[test]
    fn rule10_mouse_motion_and_button_set_pos() {
        let mut s = InputState::new();
        assert_eq!(s.mouse_pos(), None);
        s.apply(&ev(InputEventKind::MouseMotion { pos: (5.0, 6.0), delta: (1.0, 2.0) }));
        s.apply(&ev(InputEventKind::MouseMotion { pos: (6.0, 8.0), delta: (1.0, 2.0) }));
        assert_eq!(s.mouse_pos(), Some((6.0, 8.0)));
        assert_eq!(s.mouse_delta(), (2.0, 4.0));
        s.apply(&mbtn(MouseButton::Right, ElementState::Pressed));
        assert_eq!(s.mouse_pos(), Some((1.0, 2.0)));
        assert!(s.mouse_pressed(MouseButton::Right) && s.mouse_just_pressed(MouseButton::Right));
        s.apply(&mbtn(MouseButton::Right, ElementState::Released));
        assert!(!s.mouse_pressed(MouseButton::Right) && s.mouse_just_released(MouseButton::Right));
    }

    #[test]
    fn rule11_touch_upsert_and_remove() {
        let mut s = InputState::new();
        let t = |phase, pos| ev(InputEventKind::Touch { id: 7, phase, pos });
        s.apply(&t(TouchPhase::Started, (1.0, 1.0)));
        s.apply(&t(TouchPhase::Moved, (2.0, 3.0)));
        assert_eq!(s.touches().collect::<Vec<_>>(), vec![(7, (2.0, 3.0))]);
        s.apply(&t(TouchPhase::Ended, (2.0, 3.0)));
        assert_eq!(s.touches().count(), 0);
        s.apply(&t(TouchPhase::Started, (1.0, 1.0)));
        s.apply(&t(TouchPhase::Cancelled, (1.0, 1.0)));
        assert_eq!(s.touches().count(), 0);
    }

    #[test]
    fn rule12_window_events_do_not_change_state() {
        let mut s = InputState::new();
        s.apply(&press(KeyCode::A));
        s.apply(&ev(InputEventKind::Window(WindowEvent::Resized { width: 1, height: 2 })));
        s.apply(&ev(InputEventKind::Window(WindowEvent::ScaleFactorChanged { scale: 2.0 })));
        s.apply(&ev(InputEventKind::Window(WindowEvent::CloseRequested)));
        assert!(s.pressed(KeyCode::A) && s.focused());
        assert_eq!(s.events().len(), 4);
    }

    #[test]
    fn begin_frame_clears_just_but_keeps_held() {
        let mut s = InputState::new();
        s.apply(&press(KeyCode::A));
        s.apply(&mbtn(MouseButton::Left, ElementState::Pressed));
        s.apply(&ev(InputEventKind::Text("hi".into())));
        s.apply(&ev(InputEventKind::MouseMotion { pos: (3.0, 3.0), delta: (1.0, 1.0) }));
        s.apply(&ev(InputEventKind::Touch { id: 1, phase: TouchPhase::Started, pos: (0.0, 0.0) }));
        assert_eq!(s.text(), "hi");
        s.begin_frame();
        assert!(s.pressed(KeyCode::A) && !s.just_pressed(KeyCode::A));
        assert!(s.mouse_pressed(MouseButton::Left) && !s.mouse_just_pressed(MouseButton::Left));
        assert_eq!(s.text(), "");
        assert_eq!(s.mouse_delta(), (0.0, 0.0));
        assert_eq!(s.wheel(), (0.0, 0.0));
        assert!(s.events().is_empty());
        assert_eq!(s.mouse_pos(), Some((3.0, 3.0)));
        assert_eq!(s.touches().count(), 1);
    }
}
