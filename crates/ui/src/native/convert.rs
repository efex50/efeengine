//! Engine `InputEvent` -> iced `Event`.

use iced_core::keyboard::key::{Named, NativeCode, Physical};
use iced_core::keyboard::{self, Key, Location, Modifiers};
use iced_core::{Event, Point, SmolStr, input_method, mouse, touch, window};

use efeengine_core::input::{
    ElementState, InputEvent, InputEventKind, Key as EKey, MouseButton, NamedKey, TouchPhase, WHEEL_LINES_PER_PAGE,
    WHEEL_PIXELS_PER_LINE, WheelUnit, WindowEvent,
};

/// Keeps what conversion needs between events: the scale factor (positions are physical,
/// iced works in logical pixels) and the last modifier state.
pub struct Converter {
    pub scale: f32,
    mods: Modifiers,
}

impl Converter {
    pub fn new() -> Self {
        Self { scale: 1.0, mods: Modifiers::empty() }
    }

    fn point(&self, p: (f32, f32)) -> Point {
        Point::new(p.0 / self.scale, p.1 / self.scale)
    }

    /// The iced events for one engine event, plus the new cursor position when it carries one.
    pub fn convert(&mut self, ev: &InputEvent, out: &mut Vec<Event>) -> Option<Point> {
        let mut cursor = None;
        match &ev.kind {
            InputEventKind::MouseMotion { pos, .. } => {
                let p = self.point(*pos);
                cursor = Some(p);
                out.push(Event::Mouse(mouse::Event::CursorMoved { position: p }));
            }
            InputEventKind::MouseButton { button, state, pos } => {
                let p = self.point(*pos);
                cursor = Some(p);
                out.push(Event::Mouse(mouse::Event::CursorMoved { position: p }));
                let b = match button {
                    MouseButton::Left => mouse::Button::Left,
                    MouseButton::Right => mouse::Button::Right,
                    MouseButton::Middle => mouse::Button::Middle,
                    MouseButton::Back => mouse::Button::Back,
                    MouseButton::Forward => mouse::Button::Forward,
                    MouseButton::Other(n) => mouse::Button::Other(*n as u16),
                };
                out.push(Event::Mouse(match state {
                    ElementState::Pressed => mouse::Event::ButtonPressed(b),
                    ElementState::Released => mouse::Event::ButtonReleased(b),
                }));
            }
            InputEventKind::MouseWheel { delta, unit } => {
                let delta = match unit {
                    WheelUnit::Line => mouse::ScrollDelta::Lines { x: delta.0, y: delta.1 },
                    WheelUnit::Page => {
                        mouse::ScrollDelta::Lines { x: delta.0 * WHEEL_LINES_PER_PAGE, y: delta.1 * WHEEL_LINES_PER_PAGE }
                    }
                    WheelUnit::Pixel => mouse::ScrollDelta::Lines {
                        x: delta.0 / WHEEL_PIXELS_PER_LINE / self.scale,
                        y: delta.1 / WHEEL_PIXELS_PER_LINE / self.scale,
                    },
                };
                out.push(Event::Mouse(mouse::Event::WheelScrolled { delta }));
            }
            InputEventKind::Touch { id, phase, pos } => {
                let (id, position) = (touch::Finger(*id), self.point(*pos));
                out.push(Event::Touch(match phase {
                    TouchPhase::Started => touch::Event::FingerPressed { id, position },
                    TouchPhase::Moved => touch::Event::FingerMoved { id, position },
                    TouchPhase::Ended => touch::Event::FingerLifted { id, position },
                    TouchPhase::Cancelled => touch::Event::FingerLost { id, position },
                }));
            }
            InputEventKind::Key { logical, state, repeat, mods, .. } => {
                let m = modifiers(mods.bits());
                if m != self.mods {
                    self.mods = m;
                    out.push(Event::Keyboard(keyboard::Event::ModifiersChanged(m)));
                }
                let key = key(logical);
                let physical_key = Physical::Unidentified(NativeCode::Unidentified);
                out.push(Event::Keyboard(match state {
                    ElementState::Pressed => keyboard::Event::KeyPressed {
                        modified_key: key.clone(),
                        key,
                        physical_key,
                        location: Location::Standard,
                        modifiers: m,
                        // Typed text arrives as `Text` events (`InputMethod::Commit`).
                        text: None,
                        repeat: *repeat,
                    },
                    ElementState::Released => keyboard::Event::KeyReleased {
                        modified_key: key.clone(),
                        key,
                        physical_key,
                        location: Location::Standard,
                        modifiers: m,
                    },
                }));
            }
            InputEventKind::Text(t) => {
                out.push(Event::InputMethod(input_method::Event::Preedit(String::new(), None)));
                out.push(Event::InputMethod(input_method::Event::Commit(t.clone())));
            }
            InputEventKind::TextEditing { text, cursor, len } => {
                let range = (!text.is_empty()).then(|| {
                    let byte = |chars: i32| text.char_indices().nth(chars.max(0) as usize).map_or(text.len(), |(i, _)| i);
                    byte(*cursor)..byte(cursor.saturating_add(*len))
                });
                out.push(Event::InputMethod(input_method::Event::Preedit(text.clone(), range)));
            }
            InputEventKind::Window(WindowEvent::Focused(f)) => {
                out.push(Event::Window(if *f { window::Event::Focused } else { window::Event::Unfocused }));
            }
            InputEventKind::Window(_) => {}
        }
        cursor
    }
}

fn modifiers(bits: u16) -> Modifiers {
    use efeengine_core::input::Modifiers as M;
    let m = M(bits);
    let mut out = Modifiers::empty();
    if m.shift() {
        out |= Modifiers::SHIFT;
    }
    if m.ctrl() {
        out |= Modifiers::CTRL;
    }
    if m.alt() {
        out |= Modifiers::ALT;
    }
    if m.meta() {
        out |= Modifiers::LOGO;
    }
    out
}

fn key(k: &EKey) -> Key {
    match k {
        EKey::Character(s) => Key::Character(SmolStr::new(s)),
        EKey::Unidentified => Key::Unidentified,
        EKey::Named(n) => match named(*n) {
            Some(n) => Key::Named(n),
            None => Key::Unidentified,
        },
    }
}

fn named(n: NamedKey) -> Option<Named> {
    use NamedKey as N;
    Some(match n {
        N::Alt => Named::Alt,
        N::AltGraph => Named::AltGraph,
        N::CapsLock => Named::CapsLock,
        N::Control => Named::Control,
        N::Shift => Named::Shift,
        N::Enter => Named::Enter,
        N::Tab => Named::Tab,
        N::Space => Named::Space,
        N::ArrowDown => Named::ArrowDown,
        N::ArrowLeft => Named::ArrowLeft,
        N::ArrowRight => Named::ArrowRight,
        N::ArrowUp => Named::ArrowUp,
        N::End => Named::End,
        N::Home => Named::Home,
        N::PageDown => Named::PageDown,
        N::PageUp => Named::PageUp,
        N::Backspace => Named::Backspace,
        N::Delete => Named::Delete,
        N::Insert => Named::Insert,
        N::Escape => Named::Escape,
        N::Copy => Named::Copy,
        N::Cut => Named::Cut,
        N::Paste => Named::Paste,
        N::Undo => Named::Undo,
        N::Redo => Named::Redo,
        N::F1 => Named::F1,
        N::F2 => Named::F2,
        N::F3 => Named::F3,
        N::F4 => Named::F4,
        N::F5 => Named::F5,
        N::F6 => Named::F6,
        N::F7 => Named::F7,
        N::F8 => Named::F8,
        N::F9 => Named::F9,
        N::F10 => Named::F10,
        N::F11 => Named::F11,
        N::F12 => Named::F12,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use efeengine_core::input::{KeyCode, Modifiers as M};

    fn conv(kind: InputEventKind, scale: f32) -> (Vec<Event>, Option<Point>) {
        let mut c = Converter::new();
        c.scale = scale;
        let mut out = Vec::new();
        let p = c.convert(&InputEvent::new(kind), &mut out);
        (out, p)
    }

    #[test]
    fn positions_are_logical() {
        let (ev, p) = conv(InputEventKind::MouseMotion { pos: (200.0, 100.0), delta: (0.0, 0.0) }, 2.0);
        assert_eq!(p, Some(Point::new(100.0, 50.0)));
        assert_eq!(ev, vec![Event::Mouse(mouse::Event::CursorMoved { position: Point::new(100.0, 50.0) })]);
    }

    #[test]
    fn click_moves_the_cursor_first() {
        let (ev, _) = conv(
            InputEventKind::MouseButton { button: MouseButton::Left, state: ElementState::Pressed, pos: (10.0, 20.0) },
            1.0,
        );
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[1], Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)));
    }

    #[test]
    fn text_commits_and_preedit_uses_byte_offsets() {
        let (ev, _) = conv(InputEventKind::Text("a".into()), 1.0);
        assert_eq!(ev.last(), Some(&Event::InputMethod(input_method::Event::Commit("a".into()))));
        let (ev, _) = conv(InputEventKind::TextEditing { text: "çay".into(), cursor: 1, len: 1 }, 1.0);
        assert_eq!(ev, vec![Event::InputMethod(input_method::Event::Preedit("çay".into(), Some(2..3)))]);
    }

    #[test]
    fn keys_carry_modifiers() {
        let kind = InputEventKind::key(
            KeyCode::Enter,
            EKey::Named(NamedKey::Enter),
            ElementState::Pressed,
            false,
            M::LCTRL,
        );
        let (ev, _) = conv(kind, 1.0);
        assert_eq!(ev[0], Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL)));
        let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, text, .. }) = &ev[1] else { panic!() };
        assert_eq!(*key, Key::Named(Named::Enter));
        assert_eq!(*modifiers, Modifiers::CTRL);
        assert!(text.is_none());
    }
}
