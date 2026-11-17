//! Pure conversions from SDL3 types to engine input types. No window needed.

use sdl3::keyboard::{Keycode, Mod, Scancode};

use crate::engine::input::{Key, KeyCode, Modifiers, MouseButton, NamedKey};

/// Physical key from an SDL scancode. `None` and unmapped scancodes give `Unknown`.
pub(super) fn physical_key(sc: Option<Scancode>) -> KeyCode {
    let Some(sc) = sc else {
        return KeyCode::Unknown;
    };
    match sc {
        Scancode::Unknown => KeyCode::Unknown,
        Scancode::A => KeyCode::A,
        Scancode::B => KeyCode::B,
        Scancode::C => KeyCode::C,
        Scancode::D => KeyCode::D,
        Scancode::E => KeyCode::E,
        Scancode::F => KeyCode::F,
        Scancode::G => KeyCode::G,
        Scancode::H => KeyCode::H,
        Scancode::I => KeyCode::I,
        Scancode::J => KeyCode::J,
        Scancode::K => KeyCode::K,
        Scancode::L => KeyCode::L,
        Scancode::M => KeyCode::M,
        Scancode::N => KeyCode::N,
        Scancode::O => KeyCode::O,
        Scancode::P => KeyCode::P,
        Scancode::Q => KeyCode::Q,
        Scancode::R => KeyCode::R,
        Scancode::S => KeyCode::S,
        Scancode::T => KeyCode::T,
        Scancode::U => KeyCode::U,
        Scancode::V => KeyCode::V,
        Scancode::W => KeyCode::W,
        Scancode::X => KeyCode::X,
        Scancode::Y => KeyCode::Y,
        Scancode::Z => KeyCode::Z,
        Scancode::_1 => KeyCode::Key1,
        Scancode::_2 => KeyCode::Key2,
        Scancode::_3 => KeyCode::Key3,
        Scancode::_4 => KeyCode::Key4,
        Scancode::_5 => KeyCode::Key5,
        Scancode::_6 => KeyCode::Key6,
        Scancode::_7 => KeyCode::Key7,
        Scancode::_8 => KeyCode::Key8,
        Scancode::_9 => KeyCode::Key9,
        Scancode::_0 => KeyCode::Key0,

        // Özel Tuşlar
        Scancode::Return => KeyCode::Enter,
        Scancode::Escape => KeyCode::Escape,
        Scancode::Backspace => KeyCode::Backspace,
        Scancode::Tab => KeyCode::Tab,
        Scancode::Space => KeyCode::Space,
        Scancode::Minus => KeyCode::Minus,
        Scancode::Equals => KeyCode::Equal,
        Scancode::LeftBracket => KeyCode::LeftBracket,
        Scancode::RightBracket => KeyCode::RightBracket,
        Scancode::Backslash => KeyCode::Backslash,
        Scancode::Semicolon => KeyCode::Semicolon,
        Scancode::Apostrophe => KeyCode::Apostrophe,
        Scancode::Grave => KeyCode::Grave,
        Scancode::Comma => KeyCode::Comma,
        Scancode::Period => KeyCode::Period,
        Scancode::Slash => KeyCode::Slash,
        Scancode::CapsLock => KeyCode::CapsLock,

        // F Tuşları
        Scancode::F1 => KeyCode::F1,
        Scancode::F2 => KeyCode::F2,
        Scancode::F3 => KeyCode::F3,
        Scancode::F4 => KeyCode::F4,
        Scancode::F5 => KeyCode::F5,
        Scancode::F6 => KeyCode::F6,
        Scancode::F7 => KeyCode::F7,
        Scancode::F8 => KeyCode::F8,
        Scancode::F9 => KeyCode::F9,
        Scancode::F10 => KeyCode::F10,
        Scancode::F11 => KeyCode::F11,
        Scancode::F12 => KeyCode::F12,

        // Kontrol ve Navigasyon
        Scancode::ScrollLock => KeyCode::ScrollLock,
        Scancode::Pause => KeyCode::Pause,
        Scancode::Insert => KeyCode::Insert,
        Scancode::Home => KeyCode::Home,
        Scancode::PageUp => KeyCode::PageUp,
        Scancode::Delete => KeyCode::Delete,
        Scancode::End => KeyCode::End,
        Scancode::PageDown => KeyCode::PageDown,
        Scancode::Right => KeyCode::Right,
        Scancode::Left => KeyCode::Left,
        Scancode::Down => KeyCode::Down,
        Scancode::Up => KeyCode::Up,

        // Numpad
        Scancode::NumLockClear => KeyCode::NumLock,
        Scancode::KpDivide => KeyCode::NumpadDivide,
        Scancode::KpMultiply => KeyCode::NumpadMultiply,
        Scancode::KpMinus => KeyCode::NumpadSubtract,
        Scancode::KpPlus => KeyCode::NumpadAdd,
        Scancode::KpEnter => KeyCode::NumpadEnter,
        Scancode::Kp1 => KeyCode::Numpad1,
        Scancode::Kp2 => KeyCode::Numpad2,
        Scancode::Kp3 => KeyCode::Numpad3,
        Scancode::Kp4 => KeyCode::Numpad4,
        Scancode::Kp5 => KeyCode::Numpad5,
        Scancode::Kp6 => KeyCode::Numpad6,
        Scancode::Kp7 => KeyCode::Numpad7,
        Scancode::Kp8 => KeyCode::Numpad8,
        Scancode::Kp9 => KeyCode::Numpad9,
        Scancode::Kp0 => KeyCode::Numpad0,
        Scancode::KpPeriod => KeyCode::NumpadDecimal,

        // Modifiers (Değiştirici Tuşlar)
        Scancode::LCtrl => KeyCode::LCtrl,
        Scancode::LShift => KeyCode::LShift,
        Scancode::LAlt => KeyCode::LAlt,
        Scancode::LGui => KeyCode::LMeta,
        Scancode::RCtrl => KeyCode::RCtrl,
        Scancode::RShift => KeyCode::RShift,
        Scancode::RAlt => KeyCode::RAlt,
        Scancode::RGui => KeyCode::RMeta,

        Scancode::NonUsBackslash => KeyCode::IntlBackslash,
        // ISO keyboards: the key between Left Shift and Z (W3C "Backslash").
        Scancode::NonUsHash => KeyCode::Backslash,
        Scancode::International1 => KeyCode::IntlRo,
        Scancode::International3 => KeyCode::IntlYen,
        Scancode::Application | Scancode::Menu => KeyCode::ContextMenu,
        Scancode::PrintScreen => KeyCode::PrintScreen,
        Scancode::Power => KeyCode::Power,
        Scancode::Sleep => KeyCode::Sleep,
        Scancode::Wake => KeyCode::WakeUp,
        Scancode::Help => KeyCode::Help,
        Scancode::F13 => KeyCode::F13,
        Scancode::F14 => KeyCode::F14,
        Scancode::F15 => KeyCode::F15,
        Scancode::F16 => KeyCode::F16,
        Scancode::F17 => KeyCode::F17,
        Scancode::F18 => KeyCode::F18,
        Scancode::F19 => KeyCode::F19,
        Scancode::F20 => KeyCode::F20,
        Scancode::F21 => KeyCode::F21,
        Scancode::F22 => KeyCode::F22,
        Scancode::F23 => KeyCode::F23,
        Scancode::F24 => KeyCode::F24,
        Scancode::KpEquals => KeyCode::NumpadEqual,
        Scancode::KpComma => KeyCode::NumpadComma,
        Scancode::MediaPlayPause => KeyCode::MediaPlayPause,
        Scancode::MediaStop => KeyCode::MediaStop,
        Scancode::MediaNextTrack => KeyCode::MediaTrackNext,
        Scancode::MediaPreviousTrack => KeyCode::MediaTrackPrevious,
        Scancode::MediaSelect => KeyCode::MediaSelect,
        Scancode::MediaEject => KeyCode::Eject,
        Scancode::Mute => KeyCode::AudioVolumeMute,
        Scancode::VolumeUp => KeyCode::AudioVolumeUp,
        Scancode::VolumeDown => KeyCode::AudioVolumeDown,
        Scancode::AcBack => KeyCode::BrowserBack,
        Scancode::AcForward => KeyCode::BrowserForward,
        Scancode::AcRefresh => KeyCode::BrowserRefresh,
        Scancode::AcStop => KeyCode::BrowserStop,
        Scancode::AcSearch => KeyCode::BrowserSearch,
        Scancode::AcBookmarks => KeyCode::BrowserFavorites,
        Scancode::AcHome => KeyCode::BrowserHome,
        Scancode::Undo => KeyCode::Undo,
        Scancode::Cut => KeyCode::Cut,
        Scancode::Copy => KeyCode::Copy,
        Scancode::Paste => KeyCode::Paste,
        Scancode::Find => KeyCode::Find,
        Scancode::Again => KeyCode::Again,
        Scancode::Select => KeyCode::Select,
        Scancode::Lang1 => KeyCode::Lang1,
        Scancode::Lang2 => KeyCode::Lang2,
        Scancode::Lang3 => KeyCode::Lang3,
        Scancode::Lang4 => KeyCode::Lang4,
        Scancode::Lang5 => KeyCode::Lang5,
        Scancode::Execute
        | Scancode::Stop
        | Scancode::KpEqualsAs400
        | Scancode::International2
        | Scancode::International4
        | Scancode::International5
        | Scancode::International6
        | Scancode::International7
        | Scancode::International8
        | Scancode::International9
        | Scancode::Lang6
        | Scancode::Lang7
        | Scancode::Lang8
        | Scancode::Lang9
        | Scancode::AltErase
        | Scancode::SysReq
        | Scancode::Cancel
        | Scancode::Clear
        | Scancode::Prior
        | Scancode::Return2
        | Scancode::Separator
        | Scancode::Out
        | Scancode::Oper
        | Scancode::ClearAgain
        | Scancode::CrSel
        | Scancode::ExSel
        | Scancode::Kp00
        | Scancode::Kp000
        | Scancode::ThousandsSeparator
        | Scancode::DecimalSeparator
        | Scancode::CurrencyUnit
        | Scancode::CurrencySubunit
        | Scancode::KpLeftParen
        | Scancode::KpRightParen
        | Scancode::KpLeftBrace
        | Scancode::KpRightBrace
        | Scancode::KpTab
        | Scancode::KpBackspace
        | Scancode::KpA
        | Scancode::KpB
        | Scancode::KpC
        | Scancode::KpD
        | Scancode::KpE
        | Scancode::KpF
        | Scancode::KpXor
        | Scancode::KpPower
        | Scancode::KpPercent
        | Scancode::KpLess
        | Scancode::KpGreater
        | Scancode::KpAmpersand
        | Scancode::KpDblAmpersand
        | Scancode::KpVerticalBar
        | Scancode::KpDblVerticalBar
        | Scancode::KpColon
        | Scancode::KpHash
        | Scancode::KpSpace
        | Scancode::KpAt
        | Scancode::KpExclam
        | Scancode::KpMemStore
        | Scancode::KpMemRecall
        | Scancode::KpMemClear
        | Scancode::KpMemAdd
        | Scancode::KpMemSubtract
        | Scancode::KpMemMultiply
        | Scancode::KpMemDivide
        | Scancode::KpPlusMinus
        | Scancode::KpClear
        | Scancode::KpClearEntry
        | Scancode::KpBinary
        | Scancode::KpOctal
        | Scancode::KpDecimal
        | Scancode::KpHexadecimal
        | Scancode::Mode
        | Scancode::ChannelIncrement
        | Scancode::ChannelDecrement
        | Scancode::MediaPlay
        | Scancode::MediaPause
        | Scancode::MediaRecord
        | Scancode::MediaFastForward
        | Scancode::MediaRewind
        | Scancode::AcNew
        | Scancode::AcOpen
        | Scancode::AcClose
        | Scancode::AcExit
        | Scancode::AcSave
        | Scancode::AcPrint
        | Scancode::AcProperties
        | Scancode::SoftLeft
        | Scancode::SoftRight
        | Scancode::Call
        | Scancode::EndCall
        | Scancode::Reserved
        | Scancode::Count => KeyCode::Unknown,
    }
}

/// Raw SDL keycodes with this bit set are scancode-derived (non-printable) keys.
const SCANCODE_MASK: i32 = 1 << 30;

fn named_key(k: Keycode) -> Option<NamedKey> {
    use NamedKey as N;
    Some(match k {
        Keycode::Return | Keycode::KpEnter => N::Enter,
        Keycode::Escape => N::Escape,
        Keycode::Backspace => N::Backspace,
        Keycode::Tab => N::Tab,
        Keycode::Space => N::Space,
        Keycode::Delete => N::Delete,
        Keycode::Insert => N::Insert,
        Keycode::Home => N::Home,
        Keycode::End => N::End,
        Keycode::PageUp => N::PageUp,
        Keycode::PageDown => N::PageDown,
        Keycode::Up => N::ArrowUp,
        Keycode::Down => N::ArrowDown,
        Keycode::Left => N::ArrowLeft,
        Keycode::Right => N::ArrowRight,
        Keycode::F1 => N::F1,
        Keycode::F2 => N::F2,
        Keycode::F3 => N::F3,
        Keycode::F4 => N::F4,
        Keycode::F5 => N::F5,
        Keycode::F6 => N::F6,
        Keycode::F7 => N::F7,
        Keycode::F8 => N::F8,
        Keycode::F9 => N::F9,
        Keycode::F10 => N::F10,
        Keycode::F11 => N::F11,
        Keycode::F12 => N::F12,
        Keycode::F13 => N::F13,
        Keycode::F14 => N::F14,
        Keycode::F15 => N::F15,
        Keycode::F16 => N::F16,
        Keycode::F17 => N::F17,
        Keycode::F18 => N::F18,
        Keycode::F19 => N::F19,
        Keycode::F20 => N::F20,
        Keycode::F21 => N::F21,
        Keycode::F22 => N::F22,
        Keycode::F23 => N::F23,
        Keycode::F24 => N::F24,
        Keycode::CapsLock => N::CapsLock,
        Keycode::NumLockClear => N::NumLock,
        Keycode::ScrollLock => N::ScrollLock,
        Keycode::PrintScreen => N::PrintScreen,
        Keycode::Pause => N::Pause,
        Keycode::Application | Keycode::Menu => N::ContextMenu,
        Keycode::Help => N::Help,
        Keycode::Undo => N::Undo,
        Keycode::Cut => N::Cut,
        Keycode::Copy => N::Copy,
        Keycode::Paste => N::Paste,
        Keycode::Find => N::Find,
        Keycode::Select => N::Select,
        Keycode::Clear => N::Clear,
        Keycode::Power => N::Power,
        Keycode::Sleep => N::Sleep,
        Keycode::Wake => N::WakeUp,
        Keycode::LShift | Keycode::RShift => N::Shift,
        Keycode::LCtrl | Keycode::RCtrl => N::Control,
        Keycode::LAlt | Keycode::RAlt => N::Alt,
        Keycode::LGui | Keycode::RGui => N::Meta,
        Keycode::Mode => N::AltGraph,
        Keycode::MediaPlayPause | Keycode::MediaPlay => N::MediaPlayPause,
        Keycode::MediaStop => N::MediaStop,
        Keycode::MediaNextTrack => N::MediaTrackNext,
        Keycode::MediaPreviousTrack => N::MediaTrackPrevious,
        Keycode::MediaEject => N::Eject,
        Keycode::Mute => N::AudioVolumeMute,
        Keycode::VolumeUp => N::AudioVolumeUp,
        Keycode::VolumeDown => N::AudioVolumeDown,
        Keycode::AcBack => N::BrowserBack,
        Keycode::AcForward => N::BrowserForward,
        Keycode::AcRefresh => N::BrowserRefresh,
        Keycode::AcStop => N::BrowserStop,
        Keycode::AcSearch => N::BrowserSearch,
        Keycode::AcBookmarks => N::BrowserFavorites,
        Keycode::AcHome => N::BrowserHome,
        _ => return None,
    })
}

/// A lowercased `Character` key from a raw SDL keycode value, if it is printable.
fn character_from_raw(raw: i32) -> Option<Key> {
    if raw <= 0 || raw & SCANCODE_MASK != 0 {
        return None;
    }
    let c = char::from_u32(raw as u32)?;
    if c.is_control() {
        return None;
    }
    Some(Key::character(c.encode_utf8(&mut [0u8; 4])))
}

/// The character a keypad key produces regardless of layout or modifiers.
fn keypad_character(k: Keycode) -> Option<&'static str> {
    Some(match k {
        Keycode::Kp0 => "0",
        Keycode::Kp1 => "1",
        Keycode::Kp2 => "2",
        Keycode::Kp3 => "3",
        Keycode::Kp4 => "4",
        Keycode::Kp5 => "5",
        Keycode::Kp6 => "6",
        Keycode::Kp7 => "7",
        Keycode::Kp8 => "8",
        Keycode::Kp9 => "9",
        Keycode::KpPeriod => ".",
        Keycode::KpDivide => "/",
        Keycode::KpMultiply => "*",
        Keycode::KpMinus => "-",
        Keycode::KpPlus => "+",
        Keycode::KpEquals => "=",
        Keycode::KpComma => ",",
        _ => return None,
    })
}

/// The only modifier that may change the layout lookup for a scancode fallback: AltGr.
/// Shift/Caps must not alter the logical key, so press and release agree.
fn layout_lookup_mods(keymod: Mod) -> Mod {
    keymod & Mod::MODEMOD
}

/// Logical (layout-aware) key. `keycode` is what SDL resolved for the event; when it is
/// `None` (layouts whose keys are outside the sdl3 `Keycode` enum, e.g. Turkish) the
/// scancode is resolved through `SDL_GetKeyFromScancode`.
pub(super) fn logical_key(
    keycode: Option<Keycode>,
    scancode: Option<Scancode>,
    keymod: Mod,
) -> Key {
    if let Some(k) = keycode {
        if matches!(k, Keycode::Again) {
            // No W3C named key (Redo is not Again).
            return Key::Unidentified;
        }
        if let Some(c) = keypad_character(k) {
            return Key::character(c);
        }
        if let Some(n) = named_key(k) {
            return Key::Named(n);
        }
        return character_from_raw(k as i32).unwrap_or(Key::Unidentified);
    }
    let Some(sc) = scancode else {
        return Key::Unidentified;
    };
    // SAFETY: pure lookup in SDL's keymap; no pointers involved.
    let raw = unsafe {
        sdl3::sys::keyboard::SDL_GetKeyFromScancode(
            sc.into(),
            sdl3::sys::keycode::SDL_Keymod(layout_lookup_mods(keymod).bits()),
            false,
        )
    };
    // u32 -> i32 wraps for values with the high bits set, which then fail the checks.
    match i32::try_from(raw.0) {
        Ok(v) => character_from_raw(v).unwrap_or(Key::Unidentified),
        Err(_) => Key::Unidentified,
    }
}

/// SDL modifier flags to the engine layout.
pub(super) fn modifiers(m: Mod) -> Modifiers {
    let table = [
        (Mod::LSHIFTMOD, Modifiers::LSHIFT),
        (Mod::RSHIFTMOD, Modifiers::RSHIFT),
        (Mod::LCTRLMOD, Modifiers::LCTRL),
        (Mod::RCTRLMOD, Modifiers::RCTRL),
        (Mod::LALTMOD, Modifiers::LALT),
        (Mod::RALTMOD, Modifiers::RALT),
        (Mod::LGUIMOD, Modifiers::LMETA),
        (Mod::RGUIMOD, Modifiers::RMETA),
        (Mod::CAPSMOD, Modifiers::CAPS_LOCK),
        (Mod::NUMMOD, Modifiers::NUM_LOCK),
        (Mod::MODEMOD, Modifiers::ALT_GRAPH),
    ];
    let mut out = Modifiers::NONE;
    for (sdl, ours) in table {
        if m.contains(sdl) {
            out |= ours;
        }
    }
    out
}

pub(super) fn mouse_button(b: sdl3::mouse::MouseButton) -> MouseButton {
    use sdl3::mouse::MouseButton as S;
    match b {
        S::Left => MouseButton::Left,
        S::Middle => MouseButton::Middle,
        S::Right => MouseButton::Right,
        S::X1 => MouseButton::Back,
        S::X2 => MouseButton::Forward,
        S::Unknown => MouseButton::Other(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_mapping() {
        assert_eq!(physical_key(None), KeyCode::Unknown);
        assert_eq!(physical_key(Some(Scancode::A)), KeyCode::A);
        assert_eq!(
            physical_key(Some(Scancode::NonUsBackslash)),
            KeyCode::IntlBackslash
        );
        assert_eq!(physical_key(Some(Scancode::Menu)), KeyCode::ContextMenu);
        assert_eq!(physical_key(Some(Scancode::F20)), KeyCode::F20);
        assert_eq!(
            physical_key(Some(Scancode::AcBookmarks)),
            KeyCode::BrowserFavorites
        );
        assert_eq!(physical_key(Some(Scancode::Execute)), KeyCode::Unknown);
        assert_eq!(physical_key(Some(Scancode::NonUsHash)), KeyCode::Backslash);
    }

    #[test]
    fn keypad_logical_keys() {
        let none = Mod::empty();
        assert_eq!(
            logical_key(Some(Keycode::Kp0), None, none),
            Key::character("0")
        );
        assert_eq!(
            logical_key(Some(Keycode::Kp9), None, none),
            Key::character("9")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpPeriod), None, none),
            Key::character(".")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpDivide), None, none),
            Key::character("/")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpMultiply), None, none),
            Key::character("*")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpMinus), None, none),
            Key::character("-")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpPlus), None, none),
            Key::character("+")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpEquals), None, none),
            Key::character("=")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpComma), None, none),
            Key::character(",")
        );
        assert_eq!(
            logical_key(Some(Keycode::KpEnter), None, none),
            Key::Named(NamedKey::Enter)
        );
    }

    #[test]
    fn layout_lookup_keeps_only_altgr() {
        let m = Mod::LSHIFTMOD | Mod::CAPSMOD | Mod::RALTMOD | Mod::MODEMOD;
        assert_eq!(layout_lookup_mods(m), Mod::MODEMOD);
        assert_eq!(layout_lookup_mods(Mod::LSHIFTMOD), Mod::empty());
    }

    #[test]
    fn logical_mapping() {
        let none = Mod::empty();
        assert_eq!(
            logical_key(Some(Keycode::Return), None, none),
            Key::Named(NamedKey::Enter)
        );
        assert_eq!(
            logical_key(Some(Keycode::RShift), None, none),
            Key::Named(NamedKey::Shift)
        );
        assert_eq!(
            logical_key(Some(Keycode::Again), None, none),
            Key::Unidentified
        );
        assert_eq!(
            logical_key(Some(Keycode::A), None, none),
            Key::character("a")
        );
        assert_eq!(
            logical_key(Some(Keycode::_1), None, none),
            Key::character("1")
        );
        assert_eq!(logical_key(None, None, none), Key::Unidentified);
    }

    #[test]
    fn raw_characters() {
        assert_eq!(character_from_raw('ş' as i32), Some(Key::character("ş")));
        assert_eq!(character_from_raw('Ş' as i32), Some(Key::character("ş")));
        assert_eq!(character_from_raw(SCANCODE_MASK | 4), None);
        assert_eq!(character_from_raw(0), None);
    }

    #[test]
    fn modifier_mapping() {
        let m = modifiers(Mod::LSHIFTMOD | Mod::RGUIMOD | Mod::CAPSMOD | Mod::MODEMOD);
        assert_eq!(
            m,
            Modifiers::LSHIFT | Modifiers::RMETA | Modifiers::CAPS_LOCK | Modifiers::ALT_GRAPH
        );
        assert_eq!(modifiers(Mod::empty()), Modifiers::NONE);
    }

    #[test]
    fn button_mapping() {
        use sdl3::mouse::MouseButton as S;
        assert_eq!(mouse_button(S::X1), MouseButton::Back);
        assert_eq!(mouse_button(S::X2), MouseButton::Forward);
        assert_eq!(mouse_button(S::Unknown), MouseButton::Other(0));
    }
}
