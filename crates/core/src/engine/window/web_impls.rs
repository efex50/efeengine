//! Browser -> engine conversions (the web counterpart of `sdl_impls.rs`).
//! The mapping functions (`physical_key`, `logical_key`) are pure so they stay testable.

use wasm_bindgen::JsCast;
use web_sys::{Element, Event, HtmlElement, KeyboardEvent};

use crate::input::{ElementState, InputEvent, InputEventKind, Key, KeyCode, Modifiers, NamedKey, WindowId};

/// Longest multi-char `key` string still treated as one (composed) character.
const MAX_CHARACTER_KEY_CHARS: usize = 8;

/// Convert a DOM keyboard event into the engine's key event.
pub(super) fn keyboard_event(e: &KeyboardEvent, down: bool) -> InputEvent {
    let kind = InputEventKind::Key {
        physical: physical_key(&e.code()),
        logical: logical_key(&e.key(), e.location()),
        state: if down { ElementState::Pressed } else { ElementState::Released },
        repeat: down && e.repeat(),
        mods: modifiers(e),
    };
    InputEvent::with_time(kind, event_ns(e), WindowId::MAIN)
}

/// The text a keydown produces, if any: the `key` string when it is a character (not a named
/// key), and no Ctrl/Meta shortcut is in play (AltGraph combinations still type).
/// A hidden-input IME path (for composition on mobile/virtual keyboards) is out of scope;
/// composed text only arrives through `compositionend`.
pub(super) fn text_for_key(e: &KeyboardEvent) -> Option<String> {
    let key = e.key();
    if key != " " {
        character_key(&key)?;
    }
    let alt_graph = e.get_modifier_state("AltGraph");
    if !alt_graph && (e.ctrl_key() || e.meta_key()) {
        return None;
    }
    Some(key)
}

/// `Event.timeStamp` (ms, `performance.now()` origin) in nanoseconds.
pub(super) fn event_ns(e: &Event) -> u64 {
    (e.time_stamp().max(0.0) * 1_000_000.0) as u64
}

/// Current time in nanoseconds on the same clock as `Event.timeStamp` (`performance.now()`).
pub(super) fn now_ns() -> u64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0, |p| (p.now().max(0.0) * 1_000_000.0) as u64)
}

/// Whether a key event should have its default action (scrolling, tabbing, history back)
/// suppressed: only for keys games use, and never while typing in a page text field.
pub(super) fn should_prevent_default(e: &KeyboardEvent) -> bool {
    let game_key = matches!(
        e.code().as_str(),
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Space" | "Tab"
            | "PageUp" | "PageDown" | "Home" | "End" | "Backspace"
    );
    // Leave browser/OS shortcuts (Alt+Left = back, Ctrl+Tab, Ctrl+PageDown, ...) alone.
    let shortcut = e.ctrl_key() || e.alt_key() || e.meta_key();
    game_key && !shortcut && !is_editable_target(e)
}

pub(super) fn is_editable_target(e: &Event) -> bool {
    let Some(target) = e.target() else { return false };
    if let Some(el) = target.dyn_ref::<Element>() {
        if matches!(el.tag_name().to_ascii_lowercase().as_str(), "input" | "textarea" | "select") {
            return true;
        }
    }
    target.dyn_ref::<HtmlElement>().is_some_and(|el| el.is_content_editable())
}

/// Modifier state of a key event. The DOM only says "a shift key is down", not which one,
/// so the left bit is set, except when the event's own key is that modifier on the right.
fn modifiers(e: &KeyboardEvent) -> Modifiers {
    let right = e.location() == KeyboardEvent::DOM_KEY_LOCATION_RIGHT;
    let key = e.key();
    let mut m = Modifiers::NONE;
    let mut side = |down: bool, name: &str, l: Modifiers, r: Modifiers| {
        if down { m |= if right && key == name { r } else { l }; }
    };
    side(e.shift_key(), "Shift", Modifiers::LSHIFT, Modifiers::RSHIFT);
    side(e.ctrl_key(), "Control", Modifiers::LCTRL, Modifiers::RCTRL);
    side(e.alt_key(), "Alt", Modifiers::LALT, Modifiers::RALT);
    side(e.meta_key(), "Meta", Modifiers::LMETA, Modifiers::RMETA);
    if e.get_modifier_state("CapsLock") { m |= Modifiers::CAPS_LOCK; }
    if e.get_modifier_state("NumLock") { m |= Modifiers::NUM_LOCK; }
    if e.get_modifier_state("AltGraph") { m |= Modifiers::ALT_GRAPH; }
    m
}

/// Physical key from `KeyboardEvent.code` (layout independent, like an SDL scancode).
pub(super) fn physical_key(code: &str) -> KeyCode {
    use KeyCode::*;
    if code.is_empty() || code == "Unidentified" {
        return Unknown;
    }
    if let Some(c) = code.strip_prefix("Key").and_then(single_char) {
        return letter(c);
    }
    if let Some(c) = code.strip_prefix("Digit").and_then(single_char) {
        return digit(c);
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return KeyCode::function_key(n).unwrap_or(Unknown);
    }
    if let Some(rest) = code.strip_prefix("Numpad") {
        return match rest {
            "Add" => NumpadAdd,
            "Subtract" => NumpadSubtract,
            "Multiply" | "Star" => NumpadMultiply,
            "Divide" => NumpadDivide,
            "Enter" => NumpadEnter,
            "Decimal" => NumpadDecimal,
            "Equal" => NumpadEqual,
            "Comma" => NumpadComma,
            _ => single_char(rest).filter(char::is_ascii_digit).map_or(Unknown, numpad_digit),
        };
    }
    if let Some(n) = code.strip_prefix("Lang").and_then(|n| n.parse::<u8>().ok()) {
        return match n { 1 => Lang1, 2 => Lang2, 3 => Lang3, 4 => Lang4, 5 => Lang5, _ => Unknown };
    }
    match code {
        "ArrowUp" => Up,
        "ArrowDown" => Down,
        "ArrowLeft" => Left,
        "ArrowRight" => Right,
        "ShiftLeft" => LShift,
        "ShiftRight" => RShift,
        "ControlLeft" => LCtrl,
        "ControlRight" => RCtrl,
        "AltLeft" => LAlt,
        "AltRight" => RAlt,
        "MetaLeft" | "OSLeft" => LMeta,
        "MetaRight" | "OSRight" => RMeta,
        "Escape" => Escape,
        "Space" => Space,
        "Enter" => Enter,
        "Backspace" => Backspace,
        "Tab" => Tab,
        "Delete" => Delete,
        "Insert" => Insert,
        "Home" => Home,
        "End" => End,
        "PageUp" => PageUp,
        "PageDown" => PageDown,
        "CapsLock" => CapsLock,
        "ScrollLock" => ScrollLock,
        "NumLock" => NumLock,
        "Pause" => Pause,
        "Minus" => Minus,
        "Equal" => Equal,
        "BracketLeft" => LeftBracket,
        "BracketRight" => RightBracket,
        "Backslash" => Backslash,
        "Semicolon" => Semicolon,
        "Quote" => Apostrophe,
        "Comma" => Comma,
        "Period" => Period,
        "Slash" => Slash,
        "Backquote" => Grave,
        "IntlBackslash" => IntlBackslash,
        "IntlRo" => IntlRo,
        "IntlYen" => IntlYen,
        "PrintScreen" => PrintScreen,
        "ContextMenu" => ContextMenu,
        "Power" => Power,
        "Sleep" => Sleep,
        "WakeUp" => WakeUp,
        "Help" => Help,
        "MediaPlayPause" => MediaPlayPause,
        "MediaStop" => MediaStop,
        "MediaTrackNext" => MediaTrackNext,
        "MediaTrackPrevious" => MediaTrackPrevious,
        "MediaSelect" => MediaSelect,
        "Eject" => Eject,
        // Firefox used the short volume names for a long time.
        "AudioVolumeMute" | "VolumeMute" => AudioVolumeMute,
        "AudioVolumeUp" | "VolumeUp" => AudioVolumeUp,
        "AudioVolumeDown" | "VolumeDown" => AudioVolumeDown,
        "BrowserBack" => BrowserBack,
        "BrowserForward" => BrowserForward,
        "BrowserRefresh" => BrowserRefresh,
        "BrowserStop" => BrowserStop,
        "BrowserSearch" => BrowserSearch,
        "BrowserFavorites" => BrowserFavorites,
        "BrowserHome" => BrowserHome,
        "Undo" => Undo,
        "Cut" => Cut,
        "Copy" => Copy,
        "Paste" => Paste,
        "Find" => Find,
        "Again" => Again,
        "Select" => Select,
        "Fn" => Fn,
        _ => Unknown,
    }
}

/// Logical key from `KeyboardEvent.key` (what the layout produces, like an SDL keycode).
pub(super) fn logical_key(key: &str, _location: u32) -> Key {
    if let Some(named) = named_key(key) {
        return Key::Named(named);
    }
    match character_key(key) {
        Some(c) => Key::character(c),
        None => Key::Unidentified,
    }
}

/// `key` when it is a printable character string (not a named key, not empty/"Unidentified").
/// One char is the common case; a short non-ASCII-alphanumeric multi-char string
/// (a composed grapheme such as "e\u{301}") is accepted too.
fn character_key(key: &str) -> Option<&str> {
    if key.is_empty() || key == "Unidentified" || named_key(key).is_some() {
        return None;
    }
    let n = key.chars().count();
    if n == 1 {
        return Some(key);
    }
    // W3C key names are ASCII alphanumerics ("Hiragana", "F25"); anything else that is short
    // is treated as one grapheme.
    let looks_named = key.chars().all(|c| c.is_ascii_alphanumeric());
    (n <= MAX_CHARACTER_KEY_CHARS && !looks_named).then_some(key)
}

fn named_key(key: &str) -> Option<NamedKey> {
    use NamedKey::*;
    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return NamedKey::function_key(n);
    }
    Some(match key {
        " " => Space,
        "Alt" => Alt,
        "AltGraph" => AltGraph,
        "CapsLock" => CapsLock,
        "Control" => Control,
        "Fn" => Fn,
        "OS" | "Meta" | "Super" | "Hyper" => Meta,
        "NumLock" => NumLock,
        "ScrollLock" => ScrollLock,
        "Shift" => Shift,
        "Enter" => Enter,
        "Tab" => Tab,
        "ArrowDown" => ArrowDown,
        "ArrowLeft" => ArrowLeft,
        "ArrowRight" => ArrowRight,
        "ArrowUp" => ArrowUp,
        "End" => End,
        "Home" => Home,
        "PageDown" => PageDown,
        "PageUp" => PageUp,
        "Backspace" => Backspace,
        "Delete" => Delete,
        "Insert" => Insert,
        "Clear" => Clear,
        "Escape" => Escape,
        "Pause" => Pause,
        "PrintScreen" => PrintScreen,
        "ContextMenu" => ContextMenu,
        "Help" => Help,
        "Copy" => Copy,
        "Cut" => Cut,
        "Paste" => Paste,
        "Undo" => Undo,
        "Redo" => Redo,
        "Find" => Find,
        "Select" => Select,
        "MediaPlayPause" => MediaPlayPause,
        "MediaStop" => MediaStop,
        "MediaTrackNext" => MediaTrackNext,
        "MediaTrackPrevious" => MediaTrackPrevious,
        "Eject" => Eject,
        "AudioVolumeMute" => AudioVolumeMute,
        "AudioVolumeUp" => AudioVolumeUp,
        "AudioVolumeDown" => AudioVolumeDown,
        "BrowserBack" => BrowserBack,
        "BrowserForward" => BrowserForward,
        "BrowserRefresh" => BrowserRefresh,
        "BrowserStop" => BrowserStop,
        "BrowserSearch" => BrowserSearch,
        "BrowserFavorites" => BrowserFavorites,
        "BrowserHome" => BrowserHome,
        "Power" => Power,
        "Sleep" => Sleep,
        "WakeUp" => WakeUp,
        "Dead" => Dead,
        "Process" => Process,
        _ => return None,
    })
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

fn letter(c: char) -> KeyCode {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z];
    let c = c.to_ascii_uppercase();
    if c.is_ascii_uppercase() { LETTERS[(c as u8 - b'A') as usize] } else { Unknown }
}

fn digit(c: char) -> KeyCode {
    use KeyCode::*;
    const DIGITS: [KeyCode; 10] = [Key0, Key1, Key2, Key3, Key4, Key5, Key6, Key7, Key8, Key9];
    c.to_digit(10).map_or(Unknown, |d| DIGITS[d as usize])
}

fn numpad_digit(c: char) -> KeyCode {
    use KeyCode::*;
    const DIGITS: [KeyCode; 10] = [Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9];
    c.to_digit(10).map_or(Unknown, |d| DIGITS[d as usize])
}
