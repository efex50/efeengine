//! Single-line text editing on top of `cosmic_text::Editor`: insertion, deletion, cursor and
//! word motions, selection, clipboard, `max_len`, password masking and the IME preedit.
//!
//! The editor owns the real text and the cursor / selection (byte offsets). What is drawn is a
//! separate *display* cell: the text with bullets for a password and the preedit string inserted
//! at the cursor; offsets are mapped between the two.

use cosmic_text::{Action, Buffer, Cursor, Edit, Editor, Metrics, Motion, Selection, Shaping};

use super::text::{LINE_HEIGHT, TextCell, TextSpec, TextSystem};

const BULLET: char = '\u{2022}';

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EditOp {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    SelectAll,
    Backspace,
    Delete,
    BackspaceWord,
    DeleteWord,
}

pub struct InputState {
    pub editor: Editor<'static>,
    /// The drawn text (masked / with preedit).
    pub display: TextCell,
    /// Horizontal scroll of the text inside the field, logical px.
    pub scroll_x: f32,
    /// IME composition string (drawn at the caret, not part of the value).
    pub preedit: String,
    /// The value the editor holds, to notice outside changes.
    pub value: String,
    /// The pointer is dragging a selection.
    pub selecting: bool,
    /// Time of the last click, for double-click word selection.
    pub last_click: Option<std::time::Instant>,
}

impl InputState {
    pub fn new(ts: &mut TextSystem, spec: &TextSpec, value: &str) -> Self {
        let mut buf = Buffer::new_empty(Metrics::new(spec.size, spec.size * LINE_HEIGHT));
        buf.set_size(None, None);
        let attrs = ts.attrs(spec);
        buf.set_text(value, &attrs, Shaping::Advanced, None);
        let mut editor = Editor::new(buf);
        editor.set_cursor(Cursor::new(0, value.len()));
        Self { editor, display: TextCell::new(), scroll_x: 0.0, preedit: String::new(), value: value.to_string(), selecting: false, last_click: None }
    }

    /// Replaces the text (cursor to the end, selection cleared).
    pub fn set_value(&mut self, ts: &mut TextSystem, spec: &TextSpec, value: &str) {
        let attrs = ts.attrs(spec);
        self.editor.with_buffer_mut(|b| b.set_text(value, &attrs, Shaping::Advanced, None));
        self.editor.set_cursor(Cursor::new(0, value.len()));
        self.editor.set_selection(Selection::None);
        self.value = value.to_string();
    }

    /// The whole text (lines joined with `\n`; an editable input has exactly one line).
    pub fn text(&self) -> String {
        self.editor.with_buffer(|b| {
            if b.lines.len() == 1 {
                b.lines[0].text().to_string()
            } else {
                b.lines.iter().map(|l| l.text()).collect::<Vec<_>>().join("\n")
            }
        })
    }

    /// The cursor at the very end of the text.
    fn end_cursor(&self) -> Cursor {
        self.editor.with_buffer(|b| {
            let last = b.lines.len().saturating_sub(1);
            Cursor::new(last, b.lines.get(last).map_or(0, |l| l.text().len()))
        })
    }

    pub fn cursor(&self) -> usize {
        self.editor.cursor().index
    }

    /// The selection as byte offsets `(start, end)`, `None` when empty.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.editor.selection_bounds().filter(|(a, b)| a != b).map(|(a, b)| (a.index, b.index))
    }

    /// Inserts `s` (newlines become spaces, control characters are dropped) at the cursor,
    /// replacing the selection, cut to `max_len` characters (0 = unlimited). Returns whether the
    /// text changed.
    pub fn insert(&mut self, ts: &mut TextSystem, s: &str, max_len: u32) -> bool {
        let mut clean: String = s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return false;
        }
        if max_len > 0 {
            let selected = self.selection().map_or(0, |(a, b)| self.text()[a..b].chars().count());
            let room = (max_len as usize + selected).saturating_sub(self.text().chars().count());
            clean = clean.chars().take(room).collect();
            if clean.is_empty() {
                return false;
            }
        }
        let before = self.text();
        self.editor.insert_string(&clean, None);
        let _ = ts;
        self.text() != before
    }

    /// Runs a motion or edit. Returns whether the text changed. `select` extends the selection.
    pub fn apply(&mut self, ts: &mut TextSystem, op: EditOp, select: bool) -> bool {
        let before = self.text();
        let motion = match op {
            EditOp::Left => Some(Motion::Left),
            EditOp::Right => Some(Motion::Right),
            EditOp::WordLeft => Some(Motion::LeftWord),
            EditOp::WordRight => Some(Motion::RightWord),
            EditOp::Home => Some(Motion::Home),
            EditOp::End => Some(Motion::End),
            _ => None,
        };
        if let Some(m) = motion {
            if select {
                if matches!(self.editor.selection(), Selection::None) {
                    self.editor.set_selection(Selection::Normal(self.editor.cursor()));
                }
            } else if let Some((a, b)) = self.selection() {
                // A plain arrow collapses the selection to its edge.
                let c = match m {
                    Motion::Left | Motion::LeftWord => Some(Cursor::new(0, a)),
                    Motion::Right | Motion::RightWord => Some(Cursor::new(0, b)),
                    _ => None,
                };
                self.editor.set_selection(Selection::None);
                if let Some(c) = c {
                    self.editor.set_cursor(c);
                    return false;
                }
            } else {
                self.editor.set_selection(Selection::None);
            }
            self.editor.borrow_with(&mut ts.fs).action(Action::Motion(m));
            return false;
        }
        match op {
            EditOp::SelectAll => {
                let end = self.end_cursor();
                self.editor.set_selection(Selection::Normal(Cursor::new(0, 0)));
                self.editor.set_cursor(end);
            }
            EditOp::Backspace => {
                self.editor.borrow_with(&mut ts.fs).action(Action::Backspace);
            }
            EditOp::Delete => {
                self.editor.borrow_with(&mut ts.fs).action(Action::Delete);
            }
            EditOp::BackspaceWord | EditOp::DeleteWord => {
                if self.selection().is_none() {
                    let back = op == EditOp::BackspaceWord;
                    self.editor.set_selection(Selection::Normal(self.editor.cursor()));
                    self.editor
                        .borrow_with(&mut ts.fs)
                        .action(Action::Motion(if back { Motion::LeftWord } else { Motion::RightWord }));
                }
                self.editor.delete_selection();
                self.editor.set_selection(Selection::None);
            }
            _ => {}
        }
        self.text() != before
    }

    /// Selected text, for copy.
    pub fn selected_text(&self) -> Option<String> {
        self.editor.copy_selection().filter(|s| !s.is_empty())
    }

    /// Deletes the selection; whether there was one.
    pub fn delete_selection(&mut self) -> bool {
        self.editor.delete_selection()
    }

    /// Puts the cursor at byte offset `idx`; `extend` keeps / starts a selection from the old one.
    pub fn place_cursor(&mut self, idx: usize, extend: bool) {
        let len = self.text().len();
        let c = Cursor::new(0, idx.min(len));
        if extend {
            if matches!(self.editor.selection(), Selection::None) {
                self.editor.set_selection(Selection::Normal(self.editor.cursor()));
            }
        } else {
            self.editor.set_selection(Selection::None);
        }
        self.editor.set_cursor(c);
        self.drop_empty_selection();
    }

    /// Multi-line text: puts the cursor at `c` (a hit on the display buffer); `extend` keeps or
    /// starts a selection from the old cursor.
    pub fn place_cursor_at(&mut self, c: Cursor, extend: bool) {
        if extend {
            if matches!(self.editor.selection(), Selection::None) {
                self.editor.set_selection(Selection::Normal(self.editor.cursor()));
            }
        } else {
            self.editor.set_selection(Selection::None);
        }
        self.editor.set_cursor(c);
        self.drop_empty_selection();
    }

    /// An anchor that is the cursor selects nothing, but the editor would still take the next
    /// Backspace / Delete as "delete the selection" and swallow it: a click (press, then the
    /// release's drag update) must leave no such selection.
    fn drop_empty_selection(&mut self) {
        if let Selection::Normal(a) = self.editor.selection()
            && a == self.editor.cursor()
        {
            self.editor.set_selection(Selection::None);
        }
    }

    /// Multi-line text: selects the word at `c`.
    pub fn select_word_at(&mut self, c: Cursor) {
        self.editor.set_cursor(c);
        self.editor.set_selection(Selection::Word(c));
        if let Some((a, b)) = self.editor.selection_bounds() {
            self.editor.set_selection(Selection::Normal(a));
            self.editor.set_cursor(b);
        }
    }

    /// Multi-line text: the selection as cursors `(start, end)`, `None` when empty.
    pub fn selection_cursors(&self) -> Option<(Cursor, Cursor)> {
        self.editor.selection_bounds().filter(|(a, b)| a != b)
    }

    /// Multi-line text: lays the display out wrapped at `width` (logical px), no password mask
    /// and no preedit.
    pub fn refresh_wrapped(&mut self, ts: &mut TextSystem, spec: &TextSpec, width: f32) {
        let mut spec = spec.clone();
        spec.text = self.text();
        spec.wrap = true;
        ts.layout(&mut self.display, &spec, Some(width.max(1.0)));
    }

    /// Selects the word around byte offset `idx`.
    pub fn select_word(&mut self, idx: usize) {
        let len = self.text().len();
        let c = Cursor::new(0, idx.min(len));
        self.editor.set_cursor(c);
        self.editor.set_selection(Selection::Word(c));
        if let Some((a, b)) = self.editor.selection_bounds() {
            // The cursor goes to the end of the word, the selection is anchored at its start.
            self.editor.set_selection(Selection::Normal(a));
            self.editor.set_cursor(b);
        }
    }

    /// Re-applies the display text and shaping for the current state. `password` shows bullets.
    /// Returns the display text.
    pub fn refresh_display(&mut self, ts: &mut TextSystem, spec: &TextSpec, password: bool, replace: Option<&str>) {
        let text = self.text();
        let cur = self.cursor().min(text.len());
        let shown = |s: &str| -> String { if password { s.chars().map(|_| BULLET).collect() } else { s.to_string() } };
        let mut display = shown(&text[..cur]);
        display.push_str(&self.preedit);
        display.push_str(&shown(&text[cur..]));
        if let Some(r) = replace {
            display = r.to_string();
        }
        let mut spec = spec.clone();
        spec.text = display;
        spec.wrap = false;
        ts.layout(&mut self.display, &spec, None);
    }

    /// Byte offset in the display text of real byte offset `idx`.
    pub fn display_offset(&self, idx: usize, password: bool) -> usize {
        let text = self.text();
        let cur = self.cursor().min(text.len());
        let idx = idx.min(text.len());
        let map = |i: usize| if password { text[..i].chars().count() * BULLET.len_utf8() } else { i };
        let pre = if idx > cur { self.preedit.len() } else { 0 };
        map(idx) + pre
    }

    /// Real byte offset of display byte offset `d` (a hit on the display text).
    pub fn real_offset(&self, d: usize, password: bool) -> usize {
        let text = self.text();
        let cur = self.cursor().min(text.len());
        let cur_d = self.display_offset(cur, password);
        let d = if d > cur_d && d < cur_d + self.preedit.len() {
            return cur;
        } else if d >= cur_d + self.preedit.len() {
            d - self.preedit.len()
        } else {
            d
        };
        if password {
            let chars = d / BULLET.len_utf8();
            text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
        } else {
            d.min(text.len())
        }
    }
}

/// X (logical px, from the text origin) of display byte offset `idx` in a single-line buffer.
pub fn caret_x(buf: &Buffer, idx: usize) -> f32 {
    let Some(run) = buf.layout_runs().next() else { return 0.0 };
    let mut x = 0.0;
    for g in run.glyphs {
        if idx <= g.start {
            return g.x;
        }
        if idx < g.end {
            return g.x;
        }
        x = g.x + g.w;
    }
    x
}

/// Display byte offset nearest to `x` (logical px from the text origin).
pub fn offset_at(buf: &Buffer, x: f32) -> usize {
    let Some(run) = buf.layout_runs().next() else { return 0 };
    let mut last_end = 0;
    for g in run.glyphs {
        if x < g.x + g.w * 0.5 {
            return g.start;
        }
        last_end = g.end;
    }
    last_end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(t: &str) -> TextSpec {
        TextSpec { text: t.into(), size: 16.0, font: None, bold: false, align: crate::style::TextAlign::Start, wrap: false }
    }

    #[test]
    fn insert_delete_and_max_len() {
        let mut ts = TextSystem::new(false);
        let mut s = InputState::new(&mut ts, &spec(""), "");
        assert!(s.insert(&mut ts, "hello", 0));
        assert_eq!(s.text(), "hello");
        assert!(s.apply(&mut ts, EditOp::Backspace, false));
        assert_eq!(s.text(), "hell");
        // max_len 6 allows two more characters
        assert!(s.insert(&mut ts, "o!?", 6));
        assert_eq!(s.text(), "hello!");
        assert!(!s.insert(&mut ts, "x", 6));
        // newlines become spaces
        let mut s = InputState::new(&mut ts, &spec(""), "");
        s.insert(&mut ts, "a\nb", 0);
        assert_eq!(s.text(), "a b");
    }

    #[test]
    fn selection_replace_and_collapse() {
        let mut ts = TextSystem::new(false);
        let mut s = InputState::new(&mut ts, &spec("hello world"), "hello world");
        s.apply(&mut ts, EditOp::SelectAll, false);
        assert_eq!(s.selection(), Some((0, 11)));
        assert_eq!(s.selected_text().as_deref(), Some("hello world"));
        s.insert(&mut ts, "x", 0);
        assert_eq!(s.text(), "x");
        // shift+left selects the character before the cursor
        s.apply(&mut ts, EditOp::Left, true);
        assert_eq!(s.selection(), Some((0, 1)));
        // plain Left collapses to the start
        s.apply(&mut ts, EditOp::Left, false);
        assert_eq!(s.selection(), None);
        assert_eq!(s.cursor(), 0);
    }

    #[test]
    fn word_motion_and_delete_word() {
        let mut ts = TextSystem::new(false);
        let mut s = InputState::new(&mut ts, &spec("one two"), "one two");
        s.apply(&mut ts, EditOp::BackspaceWord, false);
        assert_eq!(s.text(), "one ");
        s.apply(&mut ts, EditOp::Home, false);
        assert_eq!(s.cursor(), 0);
        s.apply(&mut ts, EditOp::DeleteWord, false);
        assert_eq!(s.text(), " ");
    }

    #[test]
    fn password_display_maps_offsets() {
        let mut ts = TextSystem::new(false);
        let mut s = InputState::new(&mut ts, &spec("abc"), "abc");
        s.refresh_display(&mut ts, &spec("abc"), true, None);
        // bullets are three bytes each
        assert_eq!(s.display_offset(2, true), 6);
        assert_eq!(s.real_offset(6, true), 2);
        assert_eq!(s.real_offset(9, true), 3);
    }
}
