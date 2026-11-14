//! Browser -> engine conversions (the web counterpart of `sdl_impls.rs`).

use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, KeyboardEvent};

use crate::window::events::{KeyCode, KeyMod, WindowEvents};

/// `KeyboardEvent.location` for keys on the numeric keypad.
const LOCATION_NUMPAD: u32 = 3;
/// `KeyboardEvent.location` for the right-hand copy of a modifier key.
const LOCATION_RIGHT: u32 = 2;

/// Convert a DOM keyboard event into the engine's key event.
/// `timestamp` is in nanoseconds, like SDL's.
pub(super) fn keyboard_event(e: &KeyboardEvent, down: bool) -> WindowEvents {
    let timestamp = (e.time_stamp().max(0.0) * 1_000_000.0) as u64;
    let key_code = logical_key(&e.key(), e.location());
    let scancode = physical_key(&e.code());
    let keymod = keymod(e);
    if down {
        WindowEvents::KeyDown { timestamp, window_id: 0, key_code, scancode, keymod, repeat: e.repeat(), raw: 0 }
    } else {
        WindowEvents::KeyUp { timestamp, window_id: 0, key_code, scancode, keymod, repeat: false, raw: 0 }
    }
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

fn is_editable_target(e: &KeyboardEvent) -> bool {
    let Some(target) = e.target() else { return false };
    if let Some(el) = target.dyn_ref::<Element>() {
        if matches!(el.tag_name().to_ascii_lowercase().as_str(), "input" | "textarea" | "select") {
            return true;
        }
    }
    target.dyn_ref::<HtmlElement>().is_some_and(|el| el.is_content_editable())
}

fn keymod(e: &KeyboardEvent) -> KeyMod {
    // The DOM only says "a shift key is down", not which one; use the side of
    // the key that fired this event when it is itself a modifier.
    let right = e.location() == LOCATION_RIGHT;
    let mut m = KeyMod::NONE;
    if e.shift_key() { m = m | if right && e.key() == "Shift" { KeyMod::RSHIFT } else { KeyMod::LSHIFT }; }
    if e.ctrl_key()  { m = m | if right && e.key() == "Control" { KeyMod::RCTRL } else { KeyMod::LCTRL }; }
    if e.alt_key()   { m = m | if right && e.key() == "Alt" { KeyMod::RALT } else { KeyMod::LALT }; }
    if e.meta_key()  { m = m | if right && e.key() == "Meta" { KeyMod::RMETA } else { KeyMod::LMETA }; }
    if e.get_modifier_state("CapsLock") { m = m | KeyMod::CAPS; }
    if e.get_modifier_state("NumLock") { m = m | KeyMod::NUM; }
    if e.get_modifier_state("AltGraph") { m = m | KeyMod::MODE; }
    m
}

/// Physical key from `KeyboardEvent.code` (layout independent, like an SDL scancode).
fn physical_key(code: &str) -> Option<KeyCode> {
    use KeyCode::*;
    if code.is_empty() || code == "Unidentified" {
        return None;
    }
    if let Some(c) = code.strip_prefix("Key").and_then(single_char) {
        return Some(letter(c));
    }
    if let Some(c) = code.strip_prefix("Digit").and_then(single_char) {
        return Some(digit(c));
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return Some(function_key(n));
    }
    if let Some(rest) = code.strip_prefix("Numpad") {
        return Some(match rest {
            "Add" => NumpadAdd,
            "Subtract" => NumpadSubtract,
            "Multiply" => NumpadMultiply,
            "Divide" => NumpadDivide,
            "Enter" => NumpadEnter,
            "Decimal" => NumpadDecimal,
            _ => single_char(rest).filter(char::is_ascii_digit).map_or(Unknown, numpad_digit),
        });
    }
    Some(match code {
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
        _ => Unknown,
    })
}

/// Logical key from `KeyboardEvent.key` (what the layout produces, like an SDL keycode).
fn logical_key(key: &str, location: u32) -> Option<KeyCode> {
    use KeyCode::*;
    if key.is_empty() || key == "Unidentified" || key == "Dead" {
        return None;
    }
    let right = location == LOCATION_RIGHT;
    if let Some(c) = single_char(key) {
        if location == LOCATION_NUMPAD {
            return Some(match c {
                '+' => NumpadAdd,
                '-' => NumpadSubtract,
                '*' => NumpadMultiply,
                '/' => NumpadDivide,
                '.' | ',' => NumpadDecimal,
                d if d.is_ascii_digit() => numpad_digit(d),
                _ => Unknown,
            });
        }
        return Some(match c {
            c if c.is_ascii_alphabetic() => letter(c),
            c if c.is_ascii_digit() => digit(c),
            ' ' => Space,
            '-' => Minus,
            '=' => Equal,
            '[' => LeftBracket,
            ']' => RightBracket,
            '\\' => Backslash,
            ';' => Semicolon,
            '\'' => Apostrophe,
            ',' => Comma,
            '.' => Period,
            '/' => Slash,
            '`' => Grave,
            _ => Unknown,
        });
    }
    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return Some(function_key(n));
    }
    Some(match key {
        "ArrowUp" => Up,
        "ArrowDown" => Down,
        "ArrowLeft" => Left,
        "ArrowRight" => Right,
        "Shift" => if right { RShift } else { LShift },
        "Control" => if right { RCtrl } else { LCtrl },
        "Alt" | "AltGraph" => if right || key == "AltGraph" { RAlt } else { LAlt },
        "Meta" | "OS" => if right { RMeta } else { LMeta },
        "Escape" => Escape,
        "Enter" => if location == LOCATION_NUMPAD { NumpadEnter } else { Enter },
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
        _ => Unknown,
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

fn function_key(n: u8) -> KeyCode {
    use KeyCode::*;
    const FKEYS: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    FKEYS.get((n as usize).wrapping_sub(1)).copied().unwrap_or(Unknown)
}
