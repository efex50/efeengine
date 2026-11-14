//! Key, modifier and button types.

/// A physical key: its position on the keyboard, independent of layout.
/// Names follow US-QWERTY positions. `Unknown` means the backend could not
/// identify the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyCode {
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Key0, Key1, Key2, Key3, Key4, Key5, Key6, Key7, Key8, Key9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
    Up, Down, Left, Right,
    LShift, RShift, LCtrl, RCtrl, LAlt, RAlt, LMeta, RMeta,
    Escape, Space, Enter, Backspace, Tab, Delete, Insert, Home, End, PageUp, PageDown,
    CapsLock, ScrollLock, NumLock, Pause,
    Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9,
    NumpadAdd, NumpadSubtract, NumpadMultiply, NumpadDivide, NumpadEnter, NumpadDecimal,
    NumpadEqual, NumpadComma,
    Minus, Equal, LeftBracket, RightBracket, Backslash, Semicolon, Apostrophe, Comma, Period,
    Slash, Grave,
    IntlBackslash, IntlRo, IntlYen,
    PrintScreen, ContextMenu, Power, Sleep, WakeUp, Help,
    MediaPlayPause, MediaStop, MediaTrackNext, MediaTrackPrevious, MediaSelect, Eject,
    AudioVolumeMute, AudioVolumeUp, AudioVolumeDown,
    BrowserBack, BrowserForward, BrowserRefresh, BrowserStop, BrowserSearch, BrowserFavorites,
    BrowserHome,
    Undo, Cut, Copy, Paste, Find, Again, Select,
    Lang1, Lang2, Lang3, Lang4, Lang5,
    Fn,
    /// The backend could not identify the key.
    Unknown,
}

impl KeyCode {
    /// Function key `F<n>` for `n` in `1..=24`.
    pub fn function_key(n: u8) -> Option<KeyCode> {
        use KeyCode::*;
        Some(match n {
            1 => F1, 2 => F2, 3 => F3, 4 => F4, 5 => F5, 6 => F6,
            7 => F7, 8 => F8, 9 => F9, 10 => F10, 11 => F11, 12 => F12,
            13 => F13, 14 => F14, 15 => F15, 16 => F16, 17 => F17, 18 => F18,
            19 => F19, 20 => F20, 21 => F21, 22 => F22, 23 => F23, 24 => F24,
            _ => return None,
        })
    }
}

/// Named (non-character) logical keys, following W3C `KeyboardEvent.key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Alt, AltGraph, CapsLock, Control, Fn, Meta, NumLock, ScrollLock, Shift,
    Enter, Tab, Space,
    ArrowDown, ArrowLeft, ArrowRight, ArrowUp, End, Home, PageDown, PageUp,
    Backspace, Delete, Insert, Clear, Escape, Pause, PrintScreen, ContextMenu, Help,
    Copy, Cut, Paste, Undo, Redo, Find, Select,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
    MediaPlayPause, MediaStop, MediaTrackNext, MediaTrackPrevious, Eject,
    AudioVolumeMute, AudioVolumeUp, AudioVolumeDown,
    BrowserBack, BrowserForward, BrowserRefresh, BrowserStop, BrowserSearch, BrowserFavorites,
    BrowserHome,
    Power, Sleep, WakeUp, Dead, Process,
}

impl NamedKey {
    /// Function key `F<n>` for `n` in `1..=24`.
    pub fn function_key(n: u8) -> Option<NamedKey> {
        use NamedKey::*;
        Some(match n {
            1 => F1, 2 => F2, 3 => F3, 4 => F4, 5 => F5, 6 => F6,
            7 => F7, 8 => F8, 9 => F9, 10 => F10, 11 => F11, 12 => F12,
            13 => F13, 14 => F14, 15 => F15, 16 => F16, 17 => F17, 18 => F18,
            19 => F19, 20 => F20, 21 => F21, 22 => F22, 23 => F23, 24 => F24,
            _ => return None,
        })
    }
}

/// A logical (layout-aware) key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Named(NamedKey),
    /// The lowercased string the key produces on the current layout.
    Character(String),
    Unidentified,
}

impl Key {
    /// Builds a `Character` key, lowercasing `s`.
    pub fn character(s: &str) -> Key {
        Key::Character(s.to_lowercase())
    }

    /// The character string, if this is a `Character` key.
    pub fn as_char_str(&self) -> Option<&str> {
        match self {
            Key::Character(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Modifier bit set with an engine-owned layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(pub u16);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const LSHIFT: Modifiers = Modifiers(1 << 0);
    pub const RSHIFT: Modifiers = Modifiers(1 << 1);
    pub const LCTRL: Modifiers = Modifiers(1 << 2);
    pub const RCTRL: Modifiers = Modifiers(1 << 3);
    pub const LALT: Modifiers = Modifiers(1 << 4);
    pub const RALT: Modifiers = Modifiers(1 << 5);
    pub const LMETA: Modifiers = Modifiers(1 << 6);
    pub const RMETA: Modifiers = Modifiers(1 << 7);
    pub const CAPS_LOCK: Modifiers = Modifiers(1 << 8);
    pub const NUM_LOCK: Modifiers = Modifiers(1 << 9);
    pub const ALT_GRAPH: Modifiers = Modifiers(1 << 10);
    pub const SHIFT: Modifiers = Modifiers(Self::LSHIFT.0 | Self::RSHIFT.0);
    pub const CTRL: Modifiers = Modifiers(Self::LCTRL.0 | Self::RCTRL.0);
    pub const ALT: Modifiers = Modifiers(Self::LALT.0 | Self::RALT.0);
    pub const META: Modifiers = Modifiers(Self::LMETA.0 | Self::RMETA.0);

    /// Either shift is held.
    pub const fn shift(self) -> bool { self.intersects(Self::SHIFT) }
    /// Either ctrl is held.
    pub const fn ctrl(self) -> bool { self.intersects(Self::CTRL) }
    /// Either alt is held.
    pub const fn alt(self) -> bool { self.intersects(Self::ALT) }
    /// Either meta (Windows/Command/Super) is held.
    pub const fn meta(self) -> bool { self.intersects(Self::META) }
    /// All bits of `other` are set.
    pub const fn contains(self, other: Modifiers) -> bool { self.0 & other.0 == other.0 }
    /// Any bit of `other` is set.
    pub const fn intersects(self, other: Modifiers) -> bool { self.0 & other.0 != 0 }
    pub const fn is_empty(self) -> bool { self.0 == 0 }
    pub const fn insert(&mut self, other: Modifiers) { self.0 |= other.0; }
    pub const fn remove(&mut self, other: Modifiers) { self.0 &= !other.0; }
    pub const fn bits(self) -> u16 { self.0 }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers { Modifiers(self.0 | rhs.0) }
}
impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Modifiers) { self.0 |= rhs.0; }
}
impl std::ops::BitAnd for Modifiers {
    type Output = Modifiers;
    fn bitand(self, rhs: Modifiers) -> Modifiers { Modifiers(self.0 & rhs.0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElementState {
    Pressed,
    Released,
}

impl ElementState {
    pub const fn is_pressed(self) -> bool { matches!(self, ElementState::Pressed) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    Other(u8),
}
