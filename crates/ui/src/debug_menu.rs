//! A "Debug menu" window (a separate OS window). F5 opens or closes it. Not available on the web
//! yet (`create_window` is unsupported there).
//!
//! Built only with the UI API (`view::*`, theme classes), so it looks the same on every backend:
//! a bordered panel with a header (title, FPS readout), a tab bar made of buttons (`tab` and
//! `tab-active` classes; there is no tab widget) and the tab's body.
//!
//! - **Logs**: the latest lines, oldest first and newest at the bottom, one row each, a rule between
//!   rows. The text is a read-only selectable text input (select and Ctrl+C work). A multi-line
//!   line, or one the backend reports does not fit the row (`ellipsize` + `Truncated`), shows cut
//!   with `...` and gets a separate button (`v`, then `^`) that shows all of it, wrapped. "Copy all"
//!   and the "Auto scroll" checkbox sit in the tab bar. Auto scroll (on by default) keeps the view
//!   at the bottom; scrolling up turns it off, scrolling back to the bottom turns it on; while it
//!   is off nothing above the viewport moves (the ring buffer keeps its oldest lines meanwhile).
//! - **Edits**: [`setting_row`]s. First: the FPS limit (digits only, Enter applies it through
//!   `Commands::set_frame_rate`, 0 = unlimited; empty or invalid text falls back to the old value).
//! - **Scripts**: the state of scripts and mods, read-only. A summary (configured backend, whether
//!   the JIT is compiled in and how many modules run on it, the stats of the last run: lanes, write
//!   conflicts, spawn overflows, dropped events), then one two-line row per module slot: name
//!   (ellipsized like a log row), version, loaded / unloaded, JIT or interpreter, and a muted detail
//!   line (game module or mod package `id@version`, dependency count, budget used / total, memory
//!   page limit). The Backend row has the buttons Interpreter | Auto | JIT (the current one
//!   highlighted; `DebugMsg::SetBackend`, applied by an Update system through
//!   `Scripting::set_backend` and followed by an immediate refresh); without the `jit` feature it
//!   shows "JIT not compiled in (build with --jit)" instead. A hint line counts loaded mods that stay
//!   interpreted for lack of a `Grant::jit`. The data is a [`ScriptsInfo`] snapshot copied from `engine.scripting()` by an
//!   Update system at most twice a second, and only while the window shows this tab.
//!
//! Cost: the view runs only when something it shows changed (`UiView::when`: `DebugState`, the shown
//! FPS value, `ScriptsInfo`, a log tick bumped at most every 0.1 s when lines arrived) or one of its
//! widgets sent a message. With auto scroll on only the last 100 lines get rows.
//!
//! Keyboard: while the FPS field has focus the UI captures the keyboard (`UiCapture`, the engine's
//! input capture), so the game does not see the keys. Enter, Escape, a tab switch, a click
//! elsewhere and closing the window all end the focus (`UiTree::blur_window`).
//!
//! `engine.add_plugin(DebugMenuPlugin)` (after `UiPlugin`) installs a global logger that keeps
//! the last [`MAX_LINES`] lines and still forwards everything to the default logger, so
//! `global_info` and friends show up both in the terminal and in the window.

use std::collections::{HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};

use efeengine_core::log::{LogMsg, Logger, NewDefaultLogger, set_global_logger};
use efeengine_core::prelude::*;
use efeengine_core::scripting::{ModuleInfo, ScriptBackend, ScriptStats, Scripting};

use crate::view::*;
use crate::{Align, Layer, Size, TextAlign, UiTree, UiViews, column, row};
use efeengine_core::input::{InputEventKind, WindowEvent, WindowId};
use efeengine_core::window::WindowDesc;

/// How many log lines are kept.
pub const MAX_LINES: usize = 200;
/// While auto scroll is off the buffer may grow to this many lines (so the lines the user is reading
/// do not shift when the oldest would be dropped); it is trimmed again when auto scroll is back on.
const HELD_LINES: usize = 1000;
/// Set while auto scroll is off: the ring buffer keeps its oldest lines (see [`HELD_LINES`]).
static HOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone)]
struct Line {
    id: u64,
    text: String,
}

struct Store {
    next_id: u64,
    lines: VecDeque<Line>,
}

static LINES: OnceLock<Mutex<Store>> = OnceLock::new();

fn lines() -> &'static Mutex<Store> {
    LINES.get_or_init(|| Mutex::new(Store { next_id: 0, lines: VecDeque::new() }))
}

fn push(prefix: &str, msg: &str) {
    if let Ok(mut s) = lines().lock() {
        let limit = if HOLD.load(std::sync::atomic::Ordering::Relaxed) { HELD_LINES } else { MAX_LINES };
        while s.lines.len() >= limit {
            s.lines.pop_front();
        }
        let id = s.next_id;
        s.next_id += 1;
        s.lines.push_back(Line { id, text: format!("{prefix}{msg}") });
    }
}

/// A line with several lines in it is long whatever the width; a one-line text is long when the
/// backend reports it does not fit its row (`Truncated`).
fn is_multiline(t: &str) -> bool {
    t.contains('\n')
}

/// A multi-line text collapsed: its first line and `...`. (A one-line text is shown whole; the
/// backend cuts it to the row width.)
fn collapsed(t: &str) -> String {
    format!("{}...", t.lines().next().unwrap_or(""))
}

/// Forwards to the previous default logger and keeps a copy for the window.
struct Capture(Box<dyn Logger>);

impl Logger for Capture {
    fn logger_name(&self) -> &str {
        "debug menu logger"
    }
    fn info(&mut self, log: &str) {
        push("", log);
        self.0.info(log);
    }
    fn warn(&mut self, log: &str) {
        push("[warn] ", log);
        self.0.warn(log);
    }
    fn error(&mut self, log: &str) {
        push("[error] ", log);
        self.0.error(log);
    }
    fn alert(&mut self, log: &str) {
        push("[alert] ", log);
        self.0.alert(log);
    }
    fn log(&mut self, log: &LogMsg) {
        match log {
            LogMsg::Info(m) => self.info(m),
            LogMsg::Warn(m) => self.warn(m),
            LogMsg::Alert(m) => self.alert(m),
            LogMsg::Error(m) => self.error(m),
        }
    }
}

/// Smoothed frames per second (about half a second). A resource.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fps(pub f32);

/// The tabs of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugTab {
    /// The captured log lines.
    #[default]
    Logs,
    /// Settings, one [`setting_row`] each.
    Edits,
    /// Scripts, mods and the JIT (read-only, a [`ScriptsInfo`] snapshot).
    Scripts,
}

/// How often (seconds) the Scripts tab reads the engine.
const SCRIPTS_REFRESH: f32 = 0.5;

/// A copy of the scripting state for the Scripts tab, refreshed at most twice a second while that
/// tab is showing (so the view stays cheap). A resource.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptsInfo {
    /// Whether the snapshot was ever filled (false: nothing read yet).
    pub read: bool,
    pub backend: ScriptBackend,
    /// The `jit` feature is compiled in and the target supports it.
    pub jit_available: bool,
    pub stats: ScriptStats,
    /// Every module slot, loaded or not, in id order.
    pub modules: Vec<ModuleInfo>,
    /// Seconds since the last refresh, as of that refresh (so 0; infinite while the tab is not
    /// showing: the next show reads at once). Not advanced between refreshes.
    pub age: f32,
}

impl Default for ScriptsInfo {
    fn default() -> Self {
        ScriptsInfo { read: false, backend: ScriptBackend::default(), jit_available: false, stats: ScriptStats::default(), modules: Vec::new(), age: f32::INFINITY }
    }
}

impl ScriptsInfo {
    /// Loaded modules.
    pub fn loaded(&self) -> usize {
        self.modules.iter().filter(|m| m.loaded).count()
    }
    /// Loaded modules that run on the JIT.
    pub fn on_jit(&self) -> usize {
        self.modules.iter().filter(|m| m.loaded && m.jit).count()
    }
}

/// What the widgets send.
#[derive(Clone, Debug, PartialEq)]
pub enum DebugMsg {
    Tab(DebugTab),
    /// Expand or collapse a long log line.
    Toggle(u64),
    CopyAll,
    /// The Auto scroll checkbox.
    AutoScroll(bool),
    /// The user scrolled the log: `true` at the bottom. Away from the bottom turns auto scroll off,
    /// back to the bottom turns it on.
    Scrolled(bool),
    /// A collapsed line does not fit its row (`true`) or fits.
    Truncated(u64, bool),
    /// The FPS limit field changed (every edit; the update keeps digits only).
    FpsInput(String),
    /// Enter in the FPS limit field.
    FpsSubmit(String),
    /// The FPS limit field gained or lost keyboard focus.
    FpsFocus(bool),
    /// A backend button of the Scripts tab: switch the scripting backend (applied by `menu_update`).
    SetBackend(ScriptBackend),
}

/// What the window remembers. A resource.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugState {
    pub expanded: HashSet<u64>,
    pub tab: DebugTab,
    /// The log view stays at the bottom while lines arrive. On by default.
    pub auto_scroll: bool,
    /// Log lines (by id) whose one-line text is cut: they get an expand button.
    pub truncated: HashSet<u64>,
    /// The text of the FPS limit field (digits only).
    pub fps_text: String,
    /// The frame-rate cap last applied (0 = unlimited). The engine's own value when the plugin
    /// was built (60 by default).
    pub fps_limit: u32,
    /// The FPS limit field has keyboard focus (so Escape means "leave it").
    pub fps_focused: bool,
}

impl Default for DebugState {
    fn default() -> Self {
        DebugState { expanded: HashSet::new(), tab: DebugTab::Logs, auto_scroll: true, truncated: HashSet::new(), fps_text: "60".into(), fps_limit: 60, fps_focused: false }
    }
}

impl DebugState {
    /// A state whose FPS field starts at `limit` (0 = unlimited).
    pub fn with_limit(limit: u32) -> Self {
        DebugState { fps_text: limit.to_string(), fps_limit: limit, ..Default::default() }
    }
}

/// `s` without everything that is not an ASCII digit.
fn digits_only(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// The frame-rate cap a submitted text means: digits only, 0 = unlimited. Empty text, anything
/// else and numbers that do not fit give `None` (the field then falls back to the old value).
fn parse_limit(s: &str) -> Option<u32> {
    let t = s.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

fn menu_update(cx: &mut Ctx) {
    let msgs: Vec<DebugMsg> = cx.events::<DebugMsg>().iter().cloned().collect();
    // A backend switch: applied here (between frames, `Ctx` has the scripting), then the snapshot
    // is read at once so the module rows show the new JIT state without waiting for the timer.
    if let Some(b) = msgs.iter().rev().find_map(|m| if let DebugMsg::SetBackend(b) = m { Some(*b) } else { None }) {
        let jit = cx.scripting.jit_config();
        cx.scripting.set_backend(b, jit);
        refresh_scripts(cx);
    }
    // Escape leaves the field even when no widget saw it (the key is captured by the UI while a
    // text input has focus, but the raw input still reports it).
    let escape = cx.input.just_pressed(KeyCode::Escape);
    let mut blur = false;
    let mut apply_fps = None;
    // `get_mut` stamps the resource changed, and that wakes the view: only take it when there is
    // something to write.
    let esc_blur = escape && cx.resources.get::<DebugState>().is_some_and(|s| s.fps_focused);
    if (esc_blur || !msgs.is_empty())
        && let Some(st) = cx.resources.get_mut::<DebugState>()
    {
        if escape && st.fps_focused {
            st.fps_text = st.fps_limit.to_string();
            st.fps_focused = false;
            blur = true;
        }
        for m in msgs {
            match m {
                DebugMsg::Tab(t) => {
                    st.tab = t;
                    // Switching tabs ends any edit: keys go back to the game.
                    st.fps_text = st.fps_limit.to_string();
                    st.fps_focused = false;
                    blur = true;
                }
                DebugMsg::Toggle(id) => {
                    if !st.expanded.remove(&id) {
                        st.expanded.insert(id);
                    } else {
                        // Collapsed again: the new one-line text reports whether it is cut at the
                        // width the window has now.
                        st.truncated.remove(&id);
                    }
                }
                DebugMsg::CopyAll => {
                    let all = lines().lock().map(|s| s.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                    let _ = efeengine_core::clipboard::set_clipboard_text(&all);
                }
                DebugMsg::AutoScroll(on) | DebugMsg::Scrolled(on) => st.auto_scroll = on,
                DebugMsg::Truncated(id, cut) => {
                    if cut {
                        st.truncated.insert(id);
                    } else {
                        st.truncated.remove(&id);
                    }
                }
                DebugMsg::FpsInput(t) => st.fps_text = digits_only(&t),
                DebugMsg::FpsSubmit(t) => {
                    match parse_limit(&t) {
                        Some(n) => {
                            st.fps_limit = n;
                            apply_fps = Some(n);
                        }
                        None => {} // empty or invalid: the old value stays
                    }
                    st.fps_text = st.fps_limit.to_string();
                    st.fps_focused = false;
                    blur = true;
                }
                DebugMsg::SetBackend(_) => {} // applied above
                DebugMsg::FpsFocus(on) => {
                    st.fps_focused = on;
                    if !on {
                        // Focus lost without Enter (a click elsewhere): the edit is dropped.
                        st.fps_text = st.fps_limit.to_string();
                    }
                }
            }
        }
    }
    if let Some(st) = cx.resources.get::<DebugState>() {
        // Auto scroll off: keep the oldest lines, so nothing above the viewport shifts.
        HOLD.store(!st.auto_scroll, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(oldest) = lines().lock().ok().and_then(|s| s.lines.front().map(|l| l.id))
        && cx.resources.get::<DebugState>().is_some_and(|st| st.truncated.iter().any(|id| *id < oldest))
        && let Some(st) = cx.resources.get_mut::<DebugState>()
    {
        st.truncated.retain(|id| *id >= oldest);
    }
    if let Some(n) = apply_fps {
        // The engine's own frame-rate path; 0 (not a positive rate) removes the cap.
        cx.commands.set_frame_rate(n as f32);
    }
    if blur {
        let window = cx.resources.get::<DebugWindow>().and_then(|w| w.0);
        if let Some(tree) = cx.resources.get_mut::<UiTree>() {
            match window {
                Some(id) => tree.blur_window(u32::from(id)),
                None => tree.blur(),
            }
        }
    }
}

/// Copies the scripting state into [`ScriptsInfo`]: only while the window shows the Scripts tab, and
/// at most every [`SCRIPTS_REFRESH`] seconds (at once when the tab was just opened).
fn scripts_update(cx: &mut Ctx) {
    let shown = cx.resources.get::<UiViews>().is_some_and(|v| v.is_enabled("debug_menu"))
        && cx.resources.get::<DebugState>().is_some_and(|s| s.tab == DebugTab::Scripts);
    let dt = cx.time.delta();
    // The age lives in a private resource: writing `ScriptsInfo` every frame would wake the view.
    let age = {
        let Some(clock) = cx.resources.get_mut::<ScriptsClock>() else { return };
        clock.0 = if shown { clock.0 + dt } else { f32::INFINITY };
        clock.0
    };
    if !shown || age < SCRIPTS_REFRESH {
        // Hidden: the next show reads at once (the view is woken when the tab or window changes).
        if !shown && cx.resources.get::<ScriptsInfo>().is_some_and(|i| i.age.is_finite()) {
            if let Some(info) = cx.resources.get_mut::<ScriptsInfo>() {
                info.age = f32::INFINITY;
            }
        }
        return;
    }
    refresh_scripts(cx);
}

/// Reads the scripting state into [`ScriptsInfo`] and restarts the refresh timer.
fn refresh_scripts(cx: &mut Ctx) {
    if let Some(clock) = cx.resources.get_mut::<ScriptsClock>() {
        clock.0 = 0.0;
    }
    let Some(info) = cx.resources.get_mut::<ScriptsInfo>() else { return };
    info.age = 0.0;
    let sc = &*cx.scripting;
    info.read = true;
    info.backend = sc.backend();
    info.jit_available = Scripting::jit_available();
    info.stats = sc.stats();
    info.modules = sc.modules();
}

fn fps_update(cx: &mut Ctx) {
    let dt = cx.time.delta();
    if dt <= 0.0 {
        return;
    }
    let k = 1.0 - (-2.0 * dt).exp();
    let Some(avg) = cx.resources.get_mut::<FpsSmooth>() else { return };
    avg.0 = if avg.0 <= 0.0 { 1.0 / dt } else { avg.0 + (1.0 / dt - avg.0) * k };
    let now = avg.0;
    // `Fps` is written only when the shown (rounded) value changes: every write wakes the view.
    if cx.resources.get::<Fps>().is_some_and(|f| f.0.round() != now.round()) {
        if let Some(f) = cx.resources.get_mut::<Fps>() {
            f.0 = now;
        }
    }
}

/// The unrounded smoothed FPS (private; [`Fps`] follows it when the rounded value changes).
#[derive(Default)]
struct FpsSmooth(f32);

/// Seconds since the Scripts tab last read the engine.
struct ScriptsClock(f32);

/// Bumped when new log lines arrived, at most [`LOG_REFRESH`] seconds apart: the view's wake-up for
/// the log (a burst of lines is one rebuild, not one per line).
#[derive(Default)]
struct LogTick(u64);

/// The shortest time between two log-driven rebuilds of the view.
const LOG_REFRESH: f32 = 0.1;

struct LogWatch {
    /// `Store::next_id` at the last bump.
    seen: u64,
    /// Seconds since the last bump (infinite at the start: the first line shows at once).
    since: f32,
}

fn log_update(cx: &mut Ctx) {
    let dt = cx.time.delta();
    let next = lines().lock().map_or(0, |s| s.next_id);
    let Some(w) = cx.resources.get_mut::<LogWatch>() else { return };
    w.since = (w.since + dt).min(1.0e6);
    if next == w.seen || w.since < LOG_REFRESH {
        return;
    }
    w.seen = next;
    w.since = 0.0;
    if let Some(t) = cx.resources.get_mut::<LogTick>() {
        t.0 += 1;
    }
}

/// The OS window the menu lives in, while it is open. A resource.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DebugWindow(pub Option<WindowId>);

fn set_shown(cx: &mut Ctx, on: bool) {
    if let Some(views) = cx.resources.get_mut::<UiViews>() {
        views.set_enabled("debug_menu", on);
    }
}

/// F5 opens or closes the window (the raw input, so it works behind a modal menu too). A close
/// button on the window itself closes it as well: the engine removes it and sends `Closed`.
fn window_update(cx: &mut Ctx) {
    let Some(open) = cx.resources.get::<DebugWindow>().map(|w| w.0) else { return };
    let closed = open.is_some_and(|id| {
        cx.input.events().iter().any(|e| e.window == id && matches!(e.kind, InputEventKind::Window(WindowEvent::Closed)))
    });
    let mut now = if closed { None } else { open };
    if cx.input.just_pressed(KeyCode::F5) {
        now = match now {
            Some(id) => {
                cx.commands.close_window(id);
                None
            }
            None => Some(cx.commands.create_window(WindowDesc::new("Debug menu").size(520, 460))),
        };
    }
    if now != open {
        if let Some(w) = cx.resources.get_mut::<DebugWindow>() {
            w.0 = now;
        }
        // A closed window takes its focused field with it: no stale "editing" state.
        if let Some(st) = cx.resources.get_mut::<DebugState>() {
            st.fps_focused = false;
            st.fps_text = st.fps_limit.to_string();
        }
        set_shown(cx, now.is_some());
    }
}

/// The view rebuilds its layer from the fixed `Layer` every frame, so the window is put on the
/// layer afterwards (after `ui_view`, before `ui_sync`).
fn place(cx: &mut Ctx) {
    let Some(id) = cx.resources.get::<DebugWindow>().and_then(|w| w.0) else { return };
    if let Some(tree) = cx.resources.get_mut::<UiTree>() {
        tree.layer_in(u32::from(id), "debug_menu", layer());
    }
}

fn layer() -> Layer {
    Layer::fill(50)
}

/// A settings row: `label` on the left, `control` on the right, a rule below. New settings are
/// one call. `key` names the row (the control keeps its own key inside it).
///
/// ```ignore
/// setting_row("fps", "Fps limit here (0=infinite)", text_input(&s.text).class("wire-field").key("input"))
/// ```
pub fn setting_row<Msg>(key: &str, label: &str, control: El<Msg>) -> El<Msg> {
    column![
        row![text(label).key("label").width(Size::Fill), control].class("wire-row").align(Align::Center).width(Size::Fill),
        container().class("separator").key("sep"),
    ]
    .key(key)
    .width(Size::Fill)
}

fn tab_button(label: &str, key: &str, tab: DebugTab, current: DebugTab) -> El<DebugMsg> {
    let b = button(label).key(key).class("tab").on_click(DebugMsg::Tab(tab));
    if tab == current { b.class("tab-active") } else { b }
}

/// One log row: the selectable text and, for a long line, the expand button beside it. The text
/// and the button are separate widgets, so selecting text never toggles the row.
fn log_row(l: &Line, open: bool, truncated: bool) -> El<DebugMsg> {
    let id = l.id;
    let multi = is_multiline(&l.text);
    // Open: the whole text, wrapped over several lines. Collapsed: one line, cut by the backend to
    // the row's width (a multi-line text shows its first line and `...`).
    // An open row is long by definition (it was opened with the button).
    let long = multi || truncated || open;
    let body = if open {
        selectable_text(l.text.clone(), true).key("text_open")
    } else {
        let shown = if multi { collapsed(&l.text) } else { l.text.clone() };
        selectable_text(shown, false).ellipsize(true).on_truncated(move |cut| DebugMsg::Truncated(id, cut)).key("text")
    };
    // The button column is always there (same width with or without the button), so the text
    // width does not depend on whether the button shows.
    let mut slot = container().width(Size::Px(40.0));
    if long {
        slot = slot.child(button(if open { "^" } else { "v" }).key("expand").width(Size::Fill).on_click(DebugMsg::Toggle(id)));
    }
    let r = row().class("wire-row").align(Align::Start).width(Size::Fill).key("row").child(body.class("wire-text").width(Size::Fill)).child(slot);
    column![r, container().class("separator").key("sep")].width(Size::Fill)
}

fn logs_tab(recent: &[Line], state: &DebugState) -> El<DebugMsg> {
    let mut log = column![].width(Size::Fill);
    if recent.is_empty() {
        log = log.child(text("(no log lines yet)").class("muted").class("wire-row").key("log_empty"));
    }
    for l in recent {
        log = log.child(log_row(l, state.expanded.contains(&l.id), state.truncated.contains(&l.id)).key(&format!("log{}", l.id)));
    }
    scroll().width(Size::Fill).height(Size::Fill).key("log_scroll").follow_end(state.auto_scroll).on_scroll(DebugMsg::Scrolled).child(log)
}

fn edits_tab(state: &DebugState) -> El<DebugMsg> {
    let field = text_input(state.fps_text.clone())
        .class("wire-field")
        .key("input")
        .max_len(9)
        .on_input(DebugMsg::FpsInput)
        .on_submit(DebugMsg::FpsSubmit)
        .on_focus(DebugMsg::FpsFocus);
    scroll().width(Size::Fill).height(Size::Fill).key("edits").child(column![setting_row("fps", "Fps limit here (0=infinite)", field)].width(Size::Fill))
}

/// One module slot: a line with the name, version, state and engine, and a muted detail line.
/// Both texts are read-only selectable inputs that ellipsize like a log row.
fn module_row(m: &ModuleInfo) -> El<DebugMsg> {
    let state = if m.loaded { "loaded" } else { "unloaded" };
    let engine = match (m.loaded, m.jit, m.jit_allowed) {
        (false, ..) => "-",
        (true, true, _) => "JIT",
        (true, false, _) => "interp",
    };
    let kind = match &m.package {
        Some((id, ver)) => format!("mod {id}@{ver}, {} deps", m.dependencies),
        None => "game module / bare mod".to_string(),
    };
    let jit_note = if m.jit_allowed { "" } else { ", no jit grant" };
    let used = m.budget.saturating_sub(m.budget_left);
    let detail = format!("{kind}{jit_note}, budget {used}/{}, {} pages max", m.budget, m.memory_pages);
    let head = row![
        selectable_text(m.name.clone(), false).ellipsize(true).key("name").class("wire-text").width(Size::Fill),
        text(format!("v{}", m.version)).key("version").width(Size::Px(44.0)),
        text(state).key("state").width(Size::Px(76.0)),
        text(engine).key("jit").width(Size::Px(52.0)),
    ]
    .class("wire-row")
    .align(Align::Center)
    .width(Size::Fill)
    .key("head");
    let info = selectable_text(detail, false).ellipsize(true).key("detail").class("wire-text").class("muted").width(Size::Fill);
    column![head, info, container().class("separator").key("sep")].width(Size::Fill)
}

fn scripts_tab(info: &ScriptsInfo) -> El<DebugMsg> {
    let jit = if !info.jit_available {
        "not compiled in".to_string()
    } else {
        format!("compiled in, {} of {} modules active", info.on_jit(), info.loaded())
    };
    let st = &info.stats;
    let choose = |label: &str, key: &str, b: ScriptBackend| {
        let btn = button(label).key(key).class("tab").on_click(DebugMsg::SetBackend(b));
        if info.backend == b { btn.class("tab-active") } else { btn }
    };
    // Without the JIT every choice runs the interpreter, so the switch is replaced by a note.
    let backend_control = if info.jit_available {
        row![
            choose("Interpreter", "b_interp", ScriptBackend::Interpreter),
            choose("Auto", "b_auto", ScriptBackend::Auto),
            choose("JIT", "b_jit", ScriptBackend::Jit),
        ]
        .key("control")
    } else {
        row![text("JIT not compiled in (build with --jit)").class("muted").key("no_jit")].key("control")
    };
    let no_grant = info.modules.iter().filter(|m| m.loaded && !m.jit_allowed).count();
    let mut col = column![
        setting_row("backend", "Backend", backend_control),
        setting_row("jit", "JIT", text(jit)),
        setting_row("lanes", "Lanes (last run)", text(st.lanes.to_string())),
        setting_row("conflicts", "Write conflicts", text(st.write_conflicts.to_string())),
        setting_row("overflows", "Spawn overflows", text(st.spawn_overflows.to_string())),
        setting_row("dropped", "Dropped events", text(st.dropped_events.to_string())),
        text(format!("Modules ({} loaded of {})", info.loaded(), info.modules.len())).key("modules_title").class("wire-row").bold(true),
        container().class("separator").key("sep_modules"),
    ]
    .width(Size::Fill);
    if info.modules.is_empty() {
        col = col.child(text("(no scripts or mods loaded)").class("muted").class("wire-row").key("empty"));
    }
    if info.jit_available && info.backend != ScriptBackend::Interpreter && no_grant > 0 {
        col = col.child(text(format!("{no_grant} mod(s) stay interpreted: no jit grant (Grant::jit)")).class("muted").class("wire-row").key("no_grant"));
    }
    for m in &info.modules {
        col = col.child(module_row(m).key(&format!("m{}", m.id.0)));
    }
    scroll().width(Size::Fill).height(Size::Fill).key("scripts").child(col)
}

/// The whole window. `recent`: log lines, newest first.
fn menu_view(fps: f32, state: &DebugState, recent: &[Line], scripts: &ScriptsInfo) -> El<DebugMsg> {
    let header = row![
        text(format!("{fps:.0} fps")).class("muted").key("fps").width(Size::Px(90.0)),
        text("Debug Menu").key("title").width(Size::Fill).text_align(TextAlign::Center).font_size(18.0).bold(true),
        container().width(Size::Px(90.0)),
    ]
    .class("wire-header")
    .width(Size::Fill);
    let mut tabs = row![
        tab_button("Logs", "tab_logs", DebugTab::Logs, state.tab),
        container().class("separator-v").key("tab_sep1"),
        tab_button("Edits", "tab_edits", DebugTab::Edits, state.tab),
        container().class("separator-v").key("tab_sep2"),
        tab_button("Scripts", "tab_scripts", DebugTab::Scripts, state.tab),
        container().class("separator-v").key("tab_sep3"),
        container().width(Size::Fill),
    ]
    .width(Size::Fill);
    if state.tab == DebugTab::Logs {
        tabs = tabs
            .child(checkbox("Auto scroll", state.auto_scroll).key("auto_scroll").on_toggle(DebugMsg::AutoScroll))
            .child(button("Copy all").key("copy_all").class("tab").on_click(DebugMsg::CopyAll));
    }
    column![
        header,
        container().class("separator").key("sep_header"),
        tabs,
        container().class("separator").key("sep_tabs"),
        match state.tab {
            DebugTab::Logs => logs_tab(recent, state),
            DebugTab::Edits => edits_tab(state),
            DebugTab::Scripts => scripts_tab(scripts),
        },
    ]
    .class("wire-panel")
    .width(Size::Fill)
    .height(Size::Fill)
}

/// How many log rows the view builds while auto scroll is on.
const PINNED_ROWS: usize = 100;

/// How often the view function ran (tests).
#[cfg(test)]
static VIEW_RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn view(cx: &ViewCx) -> El<DebugMsg> {
    let fps = cx.res::<Fps>().map_or(0.0, |f| f.0);
    #[cfg(test)]
    VIEW_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let state = cx.res::<DebugState>().map(|s| s.clone()).unwrap_or_default();
    // Oldest first, newest at the bottom, like a terminal. With auto scroll on the view is pinned to
    // the bottom, so only the last `PINNED_ROWS` lines get rows (the ring keeps more); with it off
    // all the held lines are shown, so what the user reads does not move.
    let recent: Vec<Line> = lines()
        .lock()
        .map(|s| {
            let skip = if state.auto_scroll { s.lines.len().saturating_sub(PINNED_ROWS) } else { 0 };
            s.lines.iter().skip(skip).cloned().collect()
        })
        .unwrap_or_default();
    let scripts = cx.res::<ScriptsInfo>().map(|s| s.clone()).unwrap_or_default();
    menu_view(fps, &state, &recent, &scripts)
}

/// Adds the Debug menu window. Add it after `UiPlugin`. Hidden until F5 is pressed.
pub struct DebugMenuPlugin;

impl Plugin for DebugMenuPlugin {
    fn build(&self, engine: &mut Engine) {
        // Once per process: another plugin instance (tests, a second engine) must not wrap the
        // logger again.
        static LOGGER: std::sync::Once = std::sync::Once::new();
        LOGGER.call_once(|| set_global_logger(Box::new(Capture(Box::new(NewDefaultLogger())))));
        // The engine has a logger of its own (`Engine::logger`): script and mod `engine.log`
        // lines, the mod load report and `Message::Say` go through it, not through the global
        // one. Wrap it too, once per engine.
        let previous = std::mem::replace(&mut engine.logger, Box::new(NewDefaultLogger()));
        engine.logger = Box::new(Capture(previous));
        // The FPS field starts at the engine's current cap (None or invalid = unlimited = 0).
        let limit = engine.config().max_fps.filter(|f| f.is_finite() && *f > 0.0).map_or(0, |f| f.round() as u32);
        engine.insert_resource(Fps::default());
        engine.insert_resource(FpsSmooth::default());
        engine.insert_resource(ScriptsClock(f32::INFINITY));
        engine.insert_resource(LogTick::default());
        engine.insert_resource(LogWatch { seen: lines().lock().map_or(0, |s| s.next_id), since: f32::INFINITY });
        engine.add_system(Stage::Update, log_update).name("debug_menu_log");
        engine.insert_resource(DebugState::with_limit(limit));
        engine.insert_resource(ScriptsInfo::default());
        engine.add_system(Stage::Update, scripts_update).name("debug_menu_scripts");
        engine.add_system(Stage::Update, menu_update).order(order::EARLY).name("debug_menu_update");
        engine.add_system(Stage::Update, fps_update).name("debug_menu_fps");
        engine.insert_resource(DebugWindow::default());
        engine.add_system(Stage::Update, window_update).order(order::EARLY).name("debug_menu_window");
        engine.add_system(Stage::PostUpdate, place).order(order::LATE + 12).name("debug_menu_place");
        engine
            .add_plugin(
                // Only what the view shows wakes it (plus its first run and its own messages).
                UiView::with("debug_menu", view)
                    .layer(layer())
                    .enabled(false)
                    .when(changed::<DebugState>().or(changed::<Fps>()).or(changed::<ScriptsInfo>()).or(changed::<LogTick>())),
            )
            .expect("debug menu view");
    }
}

/// The log store is one global: tests that look at it take this lock (kept until the test thread
/// ends) and start from an empty store.
#[cfg(test)]
fn serial() {
    use std::sync::{Mutex, MutexGuard};
    static LOCK: Mutex<()> = Mutex::new(());
    thread_local! {
        static GUARD: std::cell::RefCell<Option<MutexGuard<'static, ()>>> = const { std::cell::RefCell::new(None) };
    }
    GUARD.with(|g| {
        let mut g = g.borrow_mut();
        if g.is_none() {
            *g = Some(LOCK.lock().unwrap_or_else(|e| e.into_inner()));
            HOLD.store(false, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut s) = lines().lock() {
                s.lines.clear();
            }
        }
    });
}

/// A tiny module (one exported function), for the Scripts tab tests.
#[cfg(test)]
fn tiny_module() -> Vec<u8> {
    use efeengine_core::efevm::ValType::I64;
    use efeengine_core::efevm::{FuncType, FunctionBuilder, ModuleBuilder};
    let mut mb = ModuleBuilder::new();
    let mut f = FunctionBuilder::new(4);
    f.const_i64(1, 7);
    f.ret(1);
    let id = mb.add_func(FuncType::new([], [I64]), f);
    mb.export_func("entry", id);
    mb.finish().unwrap().encode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::UiTest;
    use crate::{NodeKind, UiSignalKind, UiValue};
    use efeengine_core::input::{ElementState, InputEvent, Key, Modifiers, NamedKey};

    const FPS_INPUT: &str = "debug_menu/edits/fps/input";

    fn menu() -> UiTest {
        serial();
        let mut t = UiTest::new(|e| {
            e.add_plugin(DebugMenuPlugin).expect("debug menu");
        });
        t.resource_mut::<UiViews>().set_enabled("debug_menu", true);
        t.step(2);
        t
    }

    fn view_runs() -> usize {
        VIEW_RUNS.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[test]
    fn an_idle_window_does_not_run_the_view() {
        let mut t = menu();
        log("one line");
        t.step(30);
        let before = view_runs();
        t.step(180);
        assert_eq!(view_runs(), before, "no new lines, no state change: the view stays asleep");
        // ... and wakes for a change of state or a new line
        edits(&mut t);
        assert!(view_runs() > before);
        let b = view_runs();
        log("a new line");
        t.step(10);
        assert!(view_runs() > b);
        assert!(t.ui().find("debug_menu/log_scroll").is_none() || t.ui().find("debug_menu/edits").is_some());
    }

    #[test]
    fn a_burst_of_lines_is_a_few_rebuilds_a_second() {
        let mut t = menu();
        log("first");
        t.step(30);
        let before = view_runs();
        for f in 0..60 {
            for i in 0..50 {
                push("", &format!("burst {f}/{i}"));
            }
            t.step(1);
        }
        let runs = view_runs() - before;
        assert!(runs >= 1 && runs <= 11, "60 frames (1 s) of 50 lines each: {runs} rebuilds");
        // the last line shows up once the burst ends
        t.step(10);
        let last = lines().lock().unwrap().lines.back().unwrap().id;
        assert!(t.ui().find(&key(last, "text")).is_some());
    }

    #[test]
    fn auto_scroll_builds_only_the_last_rows_and_off_shows_all_held_lines() {
        let mut t = menu();
        for i in 0..150 {
            push("", &format!("row {i}"));
        }
        t.step(10);
        let (first, end) = {
            let s = lines().lock().unwrap();
            (s.lines.front().unwrap().id, s.next_id)
        };
        let rows = |t: &UiTest| (first..end).filter(|i| t.ui().find(&key(*i, "text")).is_some()).count();
        assert!(rows(&t) <= PINNED_ROWS, "{} rows", rows(&t));
        assert!(t.ui().find(&key(first, "text")).is_none());
        t.set_checked("debug_menu/auto_scroll", false);
        t.step(3);
        assert_eq!(rows(&t), 150);
    }

    /// Milliseconds per frame of `run_frames` with the window open on the Logs tab, `fill` lines in
    /// the log, `per_frame` new lines every frame, 300 frames (`open` false: the window is closed, the
    /// engine's own floor).
    fn bench(fill: usize, per_frame: usize, open: bool) -> f64 {
        serial();
        let mut t = UiTest::new(|e| {
            e.add_plugin(DebugMenuPlugin).expect("debug menu");
        });
        t.resource_mut::<UiViews>().set_enabled("debug_menu", open);
        for i in 0..fill {
            push("", &format!("fill line {i} with some text to make it a realistic log row"));
        }
        t.step(10);
        let start = std::time::Instant::now();
        for f in 0..300 {
            for i in 0..per_frame {
                push("", &format!("spam {f}/{i} with some text to make it a realistic log row"));
            }
            t.step(1);
        }
        start.elapsed().as_secs_f64() * 1000.0 / 300.0
    }

    #[test]
    #[ignore = "benchmark: run in release with --nocapture"]
    fn bench_the_log_view() {
        let floor = bench(200, 0, false);
        println!("closed window (engine floor): {floor:.3} ms/frame");
        for (fill, n) in [(200, 0), (200, 5), (200, 50), (1000, 0), (1000, 5), (1000, 50)] {
            println!("fill {fill:4} new/frame {n:2}: {:.3} ms/frame", bench(fill, n, true));
        }
    }

    /// Pushes a unique log line and returns its id.
    fn log(text: &str) -> u64 {
        push("", text);
        lines().lock().unwrap().lines.iter().rev().find(|l| l.text.ends_with(text)).expect("line").id
    }

    fn key(id: u64, leaf: &str) -> String {
        format!("debug_menu/log_scroll/log{id}/row/{leaf}")
    }

    fn edits(t: &mut UiTest) {
        t.click("debug_menu/tab_edits");
        t.step(2);
    }

    fn focus_fps(t: &mut UiTest) {
        t.focus(FPS_INPUT, true);
        t.step(1);
        assert!(t.capture().keyboard, "a focused field captures the keyboard");
        assert!(t.resource::<DebugState>().fps_focused);
    }

    #[test]
    fn the_view_builds_for_both_tabs() {
        let mut t = menu();
        // header, tab bar, rule lines
        assert_eq!(t.text("debug_menu/title"), "Debug Menu");
        assert!(t.text("debug_menu/fps").ends_with(" fps"));
        for k in ["tab_logs", "tab_edits", "copy_all", "sep_header", "sep_tabs", "log_scroll"] {
            assert!(t.ui().find(&format!("debug_menu/{k}")).is_some(), "Logs tab has `{k}`");
        }
        let logs = t.id("debug_menu/tab_logs");
        let edits_tab = t.id("debug_menu/tab_edits");
        assert!(t.ui().has_class(logs, "tab-active") && t.ui().has_class(logs, "tab"));
        assert!(t.ui().has_class(edits_tab, "tab") && !t.ui().has_class(edits_tab, "tab-active"));
        assert!(t.ui().find("debug_menu/edits").is_none());

        edits(&mut t);
        assert_eq!(t.resource::<DebugState>().tab, DebugTab::Edits);
        assert!(t.ui().has_class(t.id("debug_menu/tab_edits"), "tab-active"));
        assert!(!t.ui().has_class(t.id("debug_menu/tab_logs"), "tab-active"));
        assert_eq!(t.text("debug_menu/edits/fps/label"), "Fps limit here (0=infinite)");
        assert_eq!(t.text(FPS_INPUT), t.resource::<DebugState>().fps_limit.to_string());
        assert!(t.ui().find("debug_menu/log_scroll").is_none() && t.ui().find("debug_menu/copy_all").is_none());
        assert!(t.ui().has_class(t.id(FPS_INPUT), "wire-field"));

        t.click("debug_menu/tab_logs");
        t.step(2);
        assert!(t.ui().find("debug_menu/log_scroll").is_some() && t.ui().find(FPS_INPUT).is_none());
    }

    fn scripts(t: &mut UiTest) {
        t.click("debug_menu/tab_scripts");
        t.step(2);
    }

    #[test]
    fn the_scripts_tab_builds_and_the_three_tabs_switch() {
        let mut t = menu();
        assert!(t.ui().find("debug_menu/tab_scripts").is_some());
        scripts(&mut t);
        assert_eq!(t.resource::<DebugState>().tab, DebugTab::Scripts);
        let tab = t.id("debug_menu/tab_scripts");
        assert!(t.ui().has_class(tab, "tab") && t.ui().has_class(tab, "tab-active"));
        assert!(!t.ui().has_class(t.id("debug_menu/tab_logs"), "tab-active"));
        assert!(!t.ui().has_class(t.id("debug_menu/tab_edits"), "tab-active"));
        for k in ["scripts", "scripts/backend", "scripts/jit", "scripts/lanes", "scripts/conflicts", "scripts/overflows", "scripts/dropped", "scripts/modules_title"] {
            assert!(t.ui().find(&format!("debug_menu/{k}")).is_some(), "Scripts tab has `{k}`");
        }
        assert!(t.ui().find("debug_menu/log_scroll").is_none() && t.ui().find("debug_menu/edits").is_none() && t.ui().find("debug_menu/copy_all").is_none());
        assert!(t.ui().has_class(t.id("debug_menu/scripts/backend/sep"), "separator"));
        let want = efeengine_core::scripting::ScriptBackend::default();
        assert_eq!(t.text("debug_menu/scripts/backend/label"), "Backend");
        assert_eq!(t.resource::<ScriptsInfo>().backend, want);
        assert!(t.resource::<ScriptsInfo>().read);
        // back to the others
        edits(&mut t);
        assert!(t.ui().find(FPS_INPUT).is_some() && t.ui().find("debug_menu/scripts").is_none());
        t.click("debug_menu/tab_logs");
        t.step(2);
        assert!(t.ui().find("debug_menu/log_scroll").is_some());
    }

    /// A test request: sent as a `DebugMsg` on the next step, like a button click would.
    struct Req(Option<ScriptBackend>);

    fn send_req(cx: &mut Ctx) {
        let b = cx.resources.get_mut::<Req>().and_then(|r| r.0.take());
        if let Some(b) = b {
            cx.send(DebugMsg::SetBackend(b));
        }
    }

    #[test]
    fn a_backend_request_switches_the_backend_and_refreshes_the_snapshot_at_once() {
        let mut t = menu();
        scripts(&mut t);
        t.engine.insert_resource(Req(None));
        t.engine.add_system(Stage::Update, send_req).name("test_send_req");
        let id = t.engine.load_module("switch_mod", &tiny_module()).expect("module");
        t.step(3);
        let jit = Scripting::jit_available();
        assert_eq!(t.resource::<ScriptsInfo>().jit_available, jit);
        let has_buttons = t.ui().find("debug_menu/scripts/backend/control/b_auto").is_some();
        assert_eq!(has_buttons, jit, "the switch shows only when the JIT is compiled in");
        assert_eq!(t.ui().find("debug_menu/scripts/backend/control/no_jit").is_some(), !jit);
        // Interpreter, then Auto (the choices that work with and without the jit feature).
        for want in [ScriptBackend::Interpreter, ScriptBackend::Auto] {
            t.resource_mut::<Req>().0 = Some(want);
            t.step(3); // far below the 0.5 s refresh: the switch refreshes by itself
            assert_eq!(t.engine.scripting().backend(), want);
            assert_eq!(t.resource::<ScriptsInfo>().backend, want);
        }
        let info = t.resource::<ScriptsInfo>().clone();
        let m = info.modules.iter().find(|m| m.id == id).expect("module row");
        assert_eq!(m.jit, t.engine.scripting().runs_on_jit(id));
        if jit {
            // The buttons are real: a click on the current backend's button keeps it highlighted.
            t.click("debug_menu/scripts/backend/control/b_interp");
            t.step(3);
            assert_eq!(t.engine.scripting().backend(), ScriptBackend::Interpreter);
            assert!(t.ui().has_class(t.id("debug_menu/scripts/backend/control/b_interp"), "tab-active"));
            assert!(!t.ui().has_class(t.id("debug_menu/scripts/backend/control/b_auto"), "tab-active"));
        }
    }

    #[test]
    fn the_scripts_tab_is_empty_without_modules_and_lists_a_loaded_one() {
        let mut t = menu();
        scripts(&mut t);
        assert_eq!(t.text("debug_menu/scripts/empty"), "(no scripts or mods loaded)");
        assert!(t.ui().has_class(t.id("debug_menu/scripts/empty"), "muted"));

        let id = t.engine.load_module("tiny_mod", &tiny_module()).expect("module");
        // read within half a second, not at once (the snapshot is throttled)
        t.step(40);
        assert!(t.ui().find("debug_menu/scripts/empty").is_none());
        let m = format!("debug_menu/scripts/m{}", id.0);
        assert_eq!(t.text(&format!("{m}/head/name")), "tiny_mod");
        assert_eq!(t.text(&format!("{m}/head/version")), "v1");
        assert_eq!(t.text(&format!("{m}/head/state")), "loaded");
        assert!(t.text(&format!("{m}/detail")).contains("game module"));
        assert!(t.text(&format!("{m}/detail")).contains("pages max"));
        assert!(t.ui().find(&format!("{m}/sep")).is_some());
        assert!(t.text("debug_menu/scripts/modules_title").contains("1 loaded of 1"));
        let jit = t.text(&format!("{m}/head/jit"));
        assert!(jit == "JIT" || jit == "interp", "{jit}");
        // The jit row follows the build.
        let line = t.text("debug_menu/scripts/jit/label");
        assert_eq!(line, "JIT");

        // unloaded slots stay listed
        t.engine.unload_module(id).expect("unload");
        t.step(40);
        assert_eq!(t.text(&format!("{m}/head/state")), "unloaded");
        assert_eq!(t.text(&format!("{m}/head/jit")), "-");
    }

    #[test]
    fn the_scripts_snapshot_is_throttled() {
        let mut t = menu();
        scripts(&mut t);
        let a = t.engine.load_module("late_mod", &tiny_module()).expect("module");
        // two frames (about 0.03 s) after the read: not refreshed yet
        t.step(2);
        assert!(t.ui().find(&format!("debug_menu/scripts/m{}", a.0)).is_none(), "not read every frame");
        t.step(40);
        assert!(t.ui().find(&format!("debug_menu/scripts/m{}", a.0)).is_some());
        // not on the tab: the engine is not read at all
        edits(&mut t);
        let b = t.engine.load_module("unseen_mod", &tiny_module()).expect("module");
        t.step(40);
        assert!(t.resource::<ScriptsInfo>().modules.iter().all(|m| m.id != b));
        scripts(&mut t);
        assert!(t.resource::<ScriptsInfo>().modules.iter().any(|m| m.id == b), "opening the tab reads at once");
    }

    #[test]
    fn a_long_module_name_is_ellipsized_like_a_log_row() {
        let mut t = menu();
        scripts(&mut t);
        let long = format!("module_{}", "n".repeat(120));
        let id = t.engine.load_module(&long, &tiny_module()).expect("module");
        t.step(40);
        let name = format!("debug_menu/scripts/m{}/head/name", id.0);
        assert_eq!(t.text(&name), long);
        assert!(matches!(t.ui().kind(t.id(&name)), Some(NodeKind::TextInput { read_only: true, ellipsize: true, .. })));
        assert!(t.ui().has_class(t.id(&name), "wire-text"));
        let detail = format!("debug_menu/scripts/m{}/detail", id.0);
        assert!(matches!(t.ui().kind(t.id(&detail)), Some(NodeKind::TextInput { ellipsize: true, .. })));
    }

    #[test]
    fn switching_through_the_scripts_tab_releases_the_focus() {
        let mut t = menu();
        edits(&mut t);
        focus_fps(&mut t);
        t.set_text(FPS_INPUT, "77");
        t.step(1);
        scripts(&mut t);
        assert!(!t.capture().keyboard, "keys go back to the game");
        assert_eq!(t.backend().focused(), None);
        assert!(!t.resource::<DebugState>().fps_focused);
        t.click("debug_menu/tab_logs");
        t.step(2);
        scripts(&mut t);
        edits(&mut t);
        assert_eq!(t.text(FPS_INPUT), "60", "the unsent edit did not survive");
        focus_fps(&mut t);
        scripts(&mut t);
        assert!(!t.capture().keyboard);
        assert_eq!(t.resource::<DebugState>().tab, DebugTab::Scripts);
    }

    #[test]
    fn the_fps_field_starts_at_the_engines_cap() {
        let t = menu();
        let want = t.engine.config().max_fps.filter(|f| f.is_finite() && *f > 0.0).map_or(0, |f| f.round() as u32);
        assert_eq!(t.resource::<DebugState>().fps_limit, want);
        assert_eq!(DebugState::default().fps_limit, 60, "the engine default is a 60 fps cap");
        assert_eq!(DebugState::with_limit(0).fps_text, "0");
    }

    #[test]
    fn short_lines_show_as_is_long_ones_collapse_and_expand() {
        let mut t = menu();
        let short = log("short line for the menu test");
        let long_text = format!("long line {}", "x".repeat(200));
        let long = log(&long_text);
        let multi = log("multi first\nmulti second\nmulti third");
        t.step(2);

        // short: the text as is, one line, no button
        assert_eq!(t.text(&key(short, "text")), "short line for the menu test");
        assert!(t.ui().find(&key(short, "expand")).is_none());
        assert!(matches!(t.ui().kind(t.id(&key(short, "text"))), Some(NodeKind::TextInput { read_only: true, multiline: false, .. })));

        // A one-line text is long only when the backend says it does not fit (`Truncated`): until
        // then the whole text is the value and there is no button.
        assert_eq!(t.text(&key(long, "text")), long_text);
        assert!(t.ui().find(&key(long, "expand")).is_none());
        assert!(matches!(t.ui().kind(t.id(&key(long, "text"))), Some(NodeKind::TextInput { ellipsize: true, .. })));
        t.signal(&key(long, "text"), UiSignalKind::Truncated(true));
        t.step(2);
        assert_eq!(t.text(&key(long, "expand")), "v");
        assert!(t.ui().find(&key(short, "expand")).is_none(), "a line that fits never gets the button");
        // multi-line: first line then `...`
        assert_eq!(t.text(&key(multi, "text")), "multi first...");

        // expand: the whole text, multi-line, wrapped; the button shows `^`
        t.click(&key(long, "expand"));
        t.step(2);
        assert!(t.resource::<DebugState>().expanded.contains(&long));
        assert_eq!(t.text(&key(long, "text_open")), long_text);
        assert!(matches!(t.ui().kind(t.id(&key(long, "text_open"))), Some(NodeKind::TextInput { read_only: true, multiline: true, .. })));
        assert!(t.ui().find(&key(long, "text")).is_none());
        assert_eq!(t.text(&key(long, "expand")), "^");

        t.click(&key(multi, "expand"));
        t.step(2);
        assert_eq!(t.text(&key(multi, "text_open")), "multi first\nmulti second\nmulti third");

        // collapse again
        t.click(&key(long, "expand"));
        t.step(2);
        assert!(!t.resource::<DebugState>().expanded.contains(&long));
        assert_eq!(t.text(&key(long, "text")), long_text, "the backend cuts it again at the current width");
        assert!(t.ui().find(&key(long, "expand")).is_none(), "until the new text reports that it is cut");
        t.signal(&key(long, "text"), UiSignalKind::Truncated(true));
        t.step(2);
        assert_eq!(t.text(&key(long, "expand")), "v");
        assert!(t.resource::<DebugState>().expanded.contains(&multi), "the other row is untouched");
    }

    fn child_pos(t: &UiTest, id: u64) -> usize {
        let col = t.ui().children(t.id("debug_menu/log_scroll"))[0];
        let w = t.id(&format!("debug_menu/log_scroll/log{id}"));
        t.ui().children(col).iter().position(|c| *c == w).expect("row")
    }

    #[test]
    fn lines_are_oldest_first_newest_at_the_bottom() {
        let mut t = menu();
        let a = log("order A");
        let b = log("order B");
        let c = log("order C");
        t.step(2);
        assert!(child_pos(&t, a) < child_pos(&t, b) && child_pos(&t, b) < child_pos(&t, c));
        // a new line goes to the end
        let d = log("order D");
        // log-driven rebuilds are at most 0.1 s apart
        t.step(8);
        assert!(child_pos(&t, c) < child_pos(&t, d));
        assert_eq!(child_pos(&t, d), t.ui().children(t.ui().children(t.id("debug_menu/log_scroll"))[0]).len() - 1);
    }

    #[test]
    fn auto_scroll_is_on_by_default_and_follows_the_checkbox() {
        let mut t = menu();
        let follows = |t: &UiTest| matches!(t.ui().kind(t.id("debug_menu/log_scroll")), Some(NodeKind::Scroll { follow_end: true }));
        assert!(t.resource::<DebugState>().auto_scroll);
        assert!(follows(&t));
        assert_eq!(t.value("debug_menu/auto_scroll"), Some(UiValue::Bool(true)));
        t.set_checked("debug_menu/auto_scroll", false);
        t.step(2);
        assert!(!t.resource::<DebugState>().auto_scroll);
        assert!(!follows(&t));
        t.set_checked("debug_menu/auto_scroll", true);
        t.step(2);
        assert!(follows(&t));
        // the checkbox lives in the tab bar of the Logs tab only
        edits(&mut t);
        assert!(t.ui().find("debug_menu/auto_scroll").is_none());
    }

    #[test]
    fn scrolling_up_turns_auto_scroll_off_and_back_to_the_bottom_turns_it_on() {
        let mut t = menu();
        t.signal("debug_menu/log_scroll", UiSignalKind::Scrolled { at_end: false });
        t.step(2);
        assert!(!t.resource::<DebugState>().auto_scroll);
        assert_eq!(t.value("debug_menu/auto_scroll"), Some(UiValue::Bool(false)), "the checkbox follows");
        assert!(matches!(t.ui().kind(t.id("debug_menu/log_scroll")), Some(NodeKind::Scroll { follow_end: false })));
        t.signal("debug_menu/log_scroll", UiSignalKind::Scrolled { at_end: true });
        t.step(2);
        assert!(t.resource::<DebugState>().auto_scroll);
        assert_eq!(t.value("debug_menu/auto_scroll"), Some(UiValue::Bool(true)));
        assert!(matches!(t.ui().kind(t.id("debug_menu/log_scroll")), Some(NodeKind::Scroll { follow_end: true })));
    }

    #[test]
    fn while_auto_scroll_is_off_the_oldest_lines_are_kept() {
        let mut t = menu();
        t.set_checked("debug_menu/auto_scroll", false);
        t.step(2);
        let first = log("held first");
        for i in 0..MAX_LINES + 50 {
            push("", &format!("held {i}"));
        }
        let (n, oldest) = {
            let s = lines().lock().unwrap();
            (s.lines.len(), s.lines.front().unwrap().id)
        };
        assert_eq!(oldest, first, "nothing above the viewport was dropped");
        assert!(n > MAX_LINES);
        // back on: the buffer is trimmed to its usual size by the next line
        t.set_checked("debug_menu/auto_scroll", true);
        t.step(2);
        push("", "trim");
        assert_eq!(lines().lock().unwrap().lines.len(), MAX_LINES);
    }

    #[test]
    fn the_expand_button_is_not_the_text() {
        let mut t = menu();
        let long = log(&format!("separate widgets {}", "y".repeat(150)));
        t.step(2);
        t.signal(&key(long, "text"), UiSignalKind::Truncated(true));
        t.step(2);
        let (text, button) = (t.id(&key(long, "text")), t.id(&key(long, "expand")));
        assert_ne!(text, button);
        // the text and the button column are siblings in the row
        let col = t.ui().parent(button).unwrap();
        assert_eq!(t.ui().parent(text), t.ui().parent(col), "separate children of the row");
        assert_eq!(t.ui().get(button).unwrap().tag(), crate::KindTag::Button);
        assert_eq!(t.ui().get(text).unwrap().tag(), crate::KindTag::TextInput);
        // Working with the text (focus, selection edits, a click on it) never toggles the row.
        t.focus(&key(long, "text"), true);
        t.signal(&key(long, "text"), UiSignalKind::Changed(UiValue::Text("typed".into())));
        t.click(&key(long, "text"));
        t.step(2);
        assert!(t.resource::<DebugState>().expanded.is_empty());
        assert!(t.text(&key(long, "text")).starts_with("separate widgets"), "typing into a log changes nothing");
        // Only the button toggles.
        t.click(&key(long, "expand"));
        t.step(1);
        assert!(t.resource::<DebugState>().expanded.contains(&long));
    }

    #[test]
    fn the_fps_field_takes_digits_only() {
        assert_eq!(digits_only("6a0 b-1.5"), "6015");
        assert_eq!(parse_limit("144"), Some(144));
        assert_eq!(parse_limit(" 30 "), Some(30));
        assert_eq!(parse_limit("0"), Some(0));
        for bad in ["", "  ", "abc", "-5", "1.5", "1e3", "99999999999999999999"] {
            assert_eq!(parse_limit(bad), None, "{bad:?}");
        }

        let mut t = menu();
        edits(&mut t);
        focus_fps(&mut t);
        t.type_text(FPS_INPUT, "x");
        t.step(2);
        assert_eq!(t.text(FPS_INPUT), "60", "a letter is refused and the field is put back");
        t.type_text(FPS_INPUT, "5");
        t.step(2);
        assert_eq!(t.text(FPS_INPUT), "605");
        assert_eq!(t.resource::<DebugState>().fps_text, "605");
        assert_eq!(t.resource::<DebugState>().fps_limit, 60, "nothing is applied before Enter");
    }

    #[test]
    fn submit_applies_the_limit_and_releases_the_keyboard() {
        let mut t = menu();
        edits(&mut t);
        focus_fps(&mut t);
        t.set_text(FPS_INPUT, "144");
        t.step(1);
        t.submit(FPS_INPUT);
        t.step(1);
        assert_eq!(t.resource::<DebugState>().fps_limit, 144);
        assert_eq!(t.text(FPS_INPUT), "144");
        t.step(1);
        assert!(!t.capture().keyboard, "keys go back to the game");
        assert_eq!(t.backend().focused(), None);
        assert!(!t.resource::<DebugState>().fps_focused);
    }

    #[test]
    fn zero_means_unlimited() {
        let mut t = menu();
        edits(&mut t);
        focus_fps(&mut t);
        t.set_text(FPS_INPUT, "0");
        t.step(1);
        t.submit(FPS_INPUT);
        t.step(2);
        // 0 is kept as the applied value; `set_frame_rate(0.0)` is "no cap" in the engine
        // (`sanitize_fps` maps a non-positive rate to `None`).
        assert_eq!(t.resource::<DebugState>().fps_limit, 0);
        assert_eq!(t.text(FPS_INPUT), "0");
        assert!(!t.capture().keyboard);
    }

    #[test]
    fn an_empty_or_invalid_submit_falls_back_to_the_old_value() {
        let mut t = menu();
        edits(&mut t);
        for bad in ["", "abc", "12x", "99999999999999999999"] {
            focus_fps(&mut t);
            t.signal(FPS_INPUT, UiSignalKind::Submit(bad.to_string()));
            t.step(3);
            let s = t.resource::<DebugState>().clone();
            assert_eq!(s.fps_limit, 60, "{bad:?}");
            assert_eq!(s.fps_text, "60", "{bad:?}");
            assert_eq!(t.text(FPS_INPUT), "60", "{bad:?}");
            assert!(!t.capture().keyboard, "{bad:?}: the focus is released anyway");
        }
        // an empty field shown to the user, then Enter
        focus_fps(&mut t);
        t.set_text(FPS_INPUT, "");
        t.step(1);
        assert_eq!(t.text(FPS_INPUT), "");
        t.submit(FPS_INPUT);
        t.step(3);
        assert_eq!(t.text(FPS_INPUT), "60");
        assert_eq!(t.resource::<DebugState>().fps_limit, 60);
    }

    #[test]
    fn escape_releases_the_focus_and_drops_the_edit() {
        let mut t = menu();
        edits(&mut t);
        focus_fps(&mut t);
        t.set_text(FPS_INPUT, "75");
        t.step(1);
        t.engine.inject(InputEvent::with_time(InputEventKind::key(KeyCode::Escape, Key::Named(NamedKey::Escape), ElementState::Pressed, false, Modifiers::NONE), 0, WindowId::MAIN));
        t.step(3);
        assert!(!t.capture().keyboard);
        assert_eq!(t.backend().focused(), None);
        assert_eq!(t.resource::<DebugState>().fps_limit, 60);
        assert_eq!(t.text(FPS_INPUT), "60", "the edit is dropped");
    }

    #[test]
    fn switching_tabs_releases_the_focus() {
        let mut t = menu();
        edits(&mut t);
        focus_fps(&mut t);
        t.set_text(FPS_INPUT, "90");
        t.step(1);
        t.click("debug_menu/tab_logs");
        t.step(3);
        assert!(!t.capture().keyboard);
        assert_eq!(t.backend().focused(), None);
        assert_eq!(t.resource::<DebugState>().tab, DebugTab::Logs);
        edits(&mut t);
        assert_eq!(t.text(FPS_INPUT), "60", "the unsent edit did not survive the tab switch");
    }

    #[test]
    fn losing_the_focus_elsewhere_releases_the_keyboard() {
        // What every backend reports when the user clicks outside the field.
        let mut t = menu();
        edits(&mut t);
        focus_fps(&mut t);
        t.set_text(FPS_INPUT, "33");
        t.step(1);
        t.focus(FPS_INPUT, false);
        t.step(2);
        assert!(!t.capture().keyboard);
        assert!(!t.resource::<DebugState>().fps_focused);
        assert_eq!(t.text(FPS_INPUT), "60");
    }

    #[test]
    fn setting_rows_are_one_call() {
        // The helper alone: label left, control right, a rule below, keys as given.
        let el: El<DebugMsg> = setting_row("vsync", "V-sync", button("on").key("toggle").on_click(DebugMsg::CopyAll));
        let mut ui = UiTree::new();
        let root = ui.layer("v", Layer::fill(0));
        let mut table = Default::default();
        crate::view::reconcile::reconcile_root(&mut ui, root, "v", el, &mut table);
        assert_eq!(ui.text_of(ui.find("v/vsync/label").unwrap()), Some("V-sync"));
        assert!(ui.find("v/vsync/toggle").is_some() && ui.find("v/vsync/sep").is_some());
        assert!(ui.has_class(ui.find("v/vsync/sep").unwrap(), "separator"));
    }

    #[test]
    fn the_engine_logger_is_captured_too() {
        // Script and mod `engine.log` lines go through `Engine::logger`, not the global logger.
        let mut e = menu();
        let msg = "logged through the engine logger";
        e.engine.logger.info(msg);
        assert_eq!(lines().lock().unwrap().lines.iter().filter(|l| l.text == msg).count(), 1);
    }

    #[test]
    fn only_one_logger_is_installed_per_process() {
        // Building the plugin twice must not wrap the logger twice (it would double every line).
        let _a = menu();
        let _b = menu();
        let msg = "logged once through the global logger";
        global_info(msg);
        // The global logger runs on its own thread: wait for the line, then for any duplicate.
        let count = || lines().lock().unwrap().lines.iter().filter(|l| l.text == msg).count();
        for _ in 0..200 {
            if count() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(count(), 1);
    }
}

/// The menu on the fallback backend: the real engine loop (plugin, view, signals, capture) with
/// input events injected into the engine (its input filter feeds the backend), plus screenshots.
#[cfg(all(test, feature = "fallback"))]
mod fallback_tests {
    use super::*;
    use crate::backend::{Filter, UiBackend, UiBackendCx};
    use crate::fallback::FallbackBackend;
    use crate::journal::UiChange;
    use crate::plugin::UiPlugin;
    use crate::signal::UiSignal;
    use crate::theme::Theme;
    use crate::{UiCapture, WidgetId};
    use efeengine_core::input::{ElementState, InputEvent, Key, Modifiers, MouseButton, NamedKey};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Lets the test keep a handle on the backend the plugin owns.
    struct Shared(Rc<RefCell<FallbackBackend>>);

    impl UiBackend for Shared {
        fn init(&mut self, cx: &mut UiBackendCx) {
            self.0.borrow_mut().init(cx)
        }
        fn input(&mut self, ev: &InputEvent) -> Filter {
            self.0.borrow_mut().input(ev)
        }
        fn sync(&mut self, tree: &UiTree, changes: &[UiChange], theme: &Theme, cx: &mut UiBackendCx) {
            self.0.borrow_mut().sync(tree, changes, theme, cx)
        }
        fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
            self.0.borrow_mut().drain_signals(out)
        }
        fn capture(&self) -> UiCapture {
            self.0.borrow().capture()
        }
        fn resize(&mut self, logical: (f32, f32), scale: f32) {
            self.0.borrow_mut().resize(logical, scale)
        }
    }

    struct Rig {
        engine: Engine,
        fb: Rc<RefCell<FallbackBackend>>,
    }

    impl Rig {
        fn new() -> Rig {
            super::serial();
            let fb = Rc::new(RefCell::new(FallbackBackend::new()));
            fb.borrow_mut().set_caret_blink(false);
            fb.borrow_mut().set_atlas_texture(efeengine_core::prelude::TextureHandle { id: 4242 });
            let mut engine = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("engine");
            engine.add_plugin(UiPlugin::with_backend(Shared(fb.clone()))).expect("ui");
            engine.add_plugin(DebugMenuPlugin).expect("menu");
            engine.resources_mut().get_mut::<UiViews>().unwrap().set_enabled("debug_menu", true);
            let mut r = Rig { engine, fb };
            r.frames(3);
            // The window of the screenshots (after the plugin gave the backend the engine window's size).
            r.fb.borrow_mut().resize((520.0, 460.0), 1.0);
            r.frames(3);
            r
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.engine.run_frames(1, 1.0 / 60.0);
                // Layout and a drawn frame, as the window's overlay does every frame (hit tests
                // and the backend's reports need them).
                let mut fb = self.fb.borrow_mut();
                fb.prepare();
                fb.draw(&mut efeengine_core::render::DrawList::new());
            }
        }

        fn id(&self, key: &str) -> WidgetId {
            self.engine.resources().get::<UiTree>().unwrap().find(&format!("debug_menu/{key}")).unwrap_or_else(|| panic!("no `{key}`"))
        }

        fn rect(&self, key: &str) -> (f32, f32, f32, f32) {
            let id = self.id(key);
            self.fb.borrow_mut().widget_rect(id).unwrap_or_else(|| panic!("`{key}` has no rect"))
        }

        fn event(&mut self, kind: InputEventKind) {
            // The engine hands the event to the plugin's input filter (the backend) itself.
            self.engine.inject(InputEvent::with_time(kind, 0, WindowId::MAIN));
        }

        fn click_at(&mut self, x: f32, y: f32) {
            for state in [ElementState::Pressed, ElementState::Released] {
                self.event(InputEventKind::MouseButton { button: MouseButton::Left, state, pos: (x, y) });
            }
            self.frames(2);
        }

        fn click(&mut self, key: &str) {
            let (x, y, w, h) = self.rect(key);
            self.click_at(x + w * 0.5, y + h * 0.5);
        }

        fn named(&mut self, k: NamedKey, code: KeyCode) {
            for state in [ElementState::Pressed, ElementState::Released] {
                self.event(InputEventKind::key(code, Key::Named(k), state, false, Modifiers::NONE));
            }
            self.frames(3);
        }

        fn text(&mut self, t: &str) {
            self.event(InputEventKind::Text(t.to_string()));
            self.frames(2);
        }

        fn keyboard(&self) -> bool {
            self.engine.input().capture().keyboard
        }

        fn state(&self) -> DebugState {
            self.engine.resources().get::<DebugState>().unwrap().clone()
        }

        fn value(&self, key: &str) -> String {
            let t = self.engine.resources().get::<UiTree>().unwrap();
            t.text_of(self.id(key)).unwrap().to_string()
        }
    }

    const INPUT: &str = "edits/fps/input";

    #[test]
    fn typing_in_the_fps_field_blocks_the_game_and_enter_gives_the_keys_back() {
        let mut r = Rig::new();
        assert!(!r.keyboard());
        r.click("tab_edits");
        assert_eq!(r.state().tab, DebugTab::Edits);
        assert_eq!(r.value(INPUT), "60");

        r.click(INPUT);
        assert!(r.keyboard(), "a focused field captures the keyboard");
        assert!(r.state().fps_focused);
        // typed text: digits only (backspace the 60 away first)
        r.named(NamedKey::Backspace, KeyCode::Backspace);
        r.named(NamedKey::Backspace, KeyCode::Backspace);
        r.text("1x2a0");
        assert_eq!(r.value(INPUT), "120");
        assert!(r.keyboard());
        r.named(NamedKey::Enter, KeyCode::Enter);
        assert_eq!(r.state().fps_limit, 120);
        assert!(!r.keyboard(), "Enter releases the focus: keys go back to the game at once");
        assert!(!r.state().fps_focused);
        assert_eq!(r.value(INPUT), "120");
    }

    #[test]
    fn escape_a_click_outside_and_a_tab_switch_release_the_focus() {
        let mut r = Rig::new();
        r.click("tab_edits");

        // Escape (the backend blurs, and so does the menu's own raw-key check)
        r.click(INPUT);
        assert!(r.keyboard());
        r.text("9");
        r.named(NamedKey::Escape, KeyCode::Escape);
        assert!(!r.keyboard());
        assert_eq!(r.value(INPUT), "60", "Escape drops the edit");

        // a click outside the field (on the title)
        r.click(INPUT);
        assert!(r.keyboard());
        r.click("title");
        assert!(!r.keyboard());
        assert!(!r.state().fps_focused);

        // switching tabs
        r.click(INPUT);
        assert!(r.keyboard());
        r.click("tab_logs");
        assert!(!r.keyboard());
        assert_eq!(r.state().tab, DebugTab::Logs);
    }

    #[test]
    fn a_selected_log_line_blocks_the_keys_until_the_focus_ends() {
        let mut r = Rig::new();
        let long = log_line(&format!("selectable {}", "z".repeat(150)));
        r.frames(5);
        let text = format!("log_scroll/log{long}/row/text");
        let button = format!("log_scroll/log{long}/row/expand");
        // a click in the text focuses it (selection, Ctrl+C), the row does not expand
        r.click(&text);
        assert!(r.keyboard());
        assert!(r.state().expanded.is_empty());
        // typing into it changes nothing
        r.text("abc");
        assert!(r.value(&text).starts_with("selectable "));
        // the separate button expands, and takes the focus away from the text
        r.click(&button);
        assert!(r.state().expanded.contains(&long));
        assert!(!r.keyboard());
        assert!(r.value(&format!("log_scroll/log{long}/row/text_open")).starts_with("selectable "));
    }

    /// ms per frame (engine frame + the backend's layout, paint and draw) with `fill` lines in the log
    /// and `per_frame` new ones every frame, 120 frames, the window open on the Logs tab.
    fn bench_fb(fill: usize, per_frame: usize) -> f64 {
        let mut r = Rig::new();
        for i in 0..fill {
            super::push("", &format!("fill line {i} with some text to make it a realistic log row"));
        }
        r.frames(10);
        let start = std::time::Instant::now();
        for f in 0..120 {
            for i in 0..per_frame {
                super::push("", &format!("spam {f}/{i} with some text to make it a realistic log row"));
            }
            r.frames(1);
        }
        start.elapsed().as_secs_f64() * 1000.0 / 120.0
    }

    #[test]
    #[ignore = "benchmark: run in release with --nocapture"]
    fn bench_the_log_view_fallback() {
        for (fill, n) in [(200, 0), (200, 1), (200, 5), (200, 50), (1000, 5)] {
            println!("fallback fill {fill:4} new/frame {n:2}: {:.3} ms/frame", bench_fb(fill, n));
        }
    }

    fn log_line(text: &str) -> u64 {
        push("", text);
        lines().lock().unwrap().lines.iter().rev().find(|l| l.text.ends_with(text)).expect("line").id
    }

    // ----- width-based collapsing ------------------------------------------------------------

    fn row_key(id: u64, leaf: &str) -> String {
        format!("log_scroll/log{id}/row/{leaf}")
    }

    fn has(r: &Rig, key: &str) -> bool {
        r.engine.resources().get::<UiTree>().unwrap().find(&format!("debug_menu/{key}")).is_some()
    }

    #[test]
    fn only_lines_that_do_not_fit_get_the_expand_button_and_a_resize_changes_it() {
        let mut r = Rig::new();
        let fits = log_line("[warn] Row 3: selectable text, select it and press Ctrl+C");
        let wide = log_line(&format!("wide line {}", "w".repeat(70)));
        let multi = log_line("first line\nsecond line");
        r.frames(5);
        assert!(!has(&r, &row_key(fits, "expand")), "a line that fits one row is shown as is");
        assert!(has(&r, &row_key(wide, "expand")), "a line wider than the row is cut and expandable");
        assert!(has(&r, &row_key(multi, "expand")), "a multi-line line always is");
        // wider window: the wide line fits now, the multi-line one still has its button
        r.fb.borrow_mut().resize((1400.0, 460.0), 1.0);
        r.frames(5);
        assert!(!has(&r, &row_key(wide, "expand")), "it fits the wider row");
        assert!(has(&r, &row_key(multi, "expand")));
        // narrow again
        r.fb.borrow_mut().resize((520.0, 460.0), 1.0);
        r.frames(5);
        assert!(has(&r, &row_key(wide, "expand")));
        // the text stays selectable in the collapsed state
        r.click(&row_key(wide, "text"));
        assert!(r.keyboard());
        assert!(r.state().expanded.is_empty(), "selecting never toggles");
    }

    // ----- auto scroll -----------------------------------------------------------------------

    fn wheel(r: &mut Rig, lines: f32) {
        let (x, y, w, h) = r.rect("log_scroll");
        r.event(InputEventKind::MouseMotion { pos: (x + w * 0.5, y + h * 0.5), delta: (0.0, 0.0) });
        r.event(InputEventKind::MouseWheel { delta: (0.0, lines), unit: efeengine_core::input::WheelUnit::Line });
        r.frames(3);
    }

    fn fill(n: usize, tag: &str) -> Vec<u64> {
        (0..n).map(|i| log_line(&format!("{tag} line {i}"))).collect()
    }

    #[test]
    fn auto_scroll_follows_the_newest_line_and_scrolling_up_stops_it() {
        let mut r = Rig::new();
        let ids = fill(40, "follow");
        r.frames(5);
        let last = *ids.last().unwrap();
        let (_, y, _, h) = r.rect(&row_key(last, "text"));
        assert!(y + h <= 460.0 && y > 100.0, "the newest line is in view at the bottom: y {y}");
        let first = r.rect(&row_key(ids[0], "text"));
        assert!(first.1 < 78.0, "the oldest line is scrolled away");
        // a new line stays in view
        let more = fill(3, "more");
        r.frames(5);
        let (_, y2, _, h2) = r.rect(&row_key(*more.last().unwrap(), "text"));
        assert!(y2 + h2 <= 460.0 && y2 > 100.0, "{y2}");
        assert!(r.state().auto_scroll);

        // scroll up: auto scroll turns itself off, the checkbox follows
        wheel(&mut r, 6.0);
        assert!(!r.state().auto_scroll, "scrolling away from the bottom turns it off");
        assert!(!matches!(r.engine.resources().get::<UiTree>().unwrap().value(r.id("auto_scroll")), Some(crate::UiValue::Bool(true))));
        let reading = more.first().copied().unwrap();
        let before = r.rect(&row_key(reading, "text"));

        // lines arrive, even enough to push the ring buffer over: nothing in view moves
        fill(5, "later");
        r.frames(4);
        assert_eq!(r.rect(&row_key(reading, "text")), before, "the line being read stays put");
        fill(MAX_LINES + 20, "flood");
        r.frames(4);
        assert_eq!(r.rect(&row_key(reading, "text")), before, "also when the buffer would drop old lines");
        assert!(lines().lock().unwrap().lines.iter().any(|l| l.id == ids[0]), "the oldest line was kept");

        // back to the bottom: on again, and the view follows the next line
        wheel(&mut r, -400.0);
        assert!(r.state().auto_scroll, "scrolling to the very bottom turns it back on");
        let newest = log_line("after the bottom");
        r.frames(5);
        let (_, y3, _, h3) = r.rect(&row_key(newest, "text"));
        assert!(y3 + h3 <= 460.0 && y3 > 100.0, "{y3}");
    }

    #[test]
    fn the_checkbox_turns_following_off_and_on() {
        let mut r = Rig::new();
        let ids = fill(40, "box");
        r.frames(5);
        r.click("auto_scroll");
        assert!(!r.state().auto_scroll);
        let probe = ids[30];
        let before = r.rect(&row_key(probe, "text"));
        fill(4, "off");
        r.frames(4);
        assert_eq!(r.rect(&row_key(probe, "text")), before);
        r.click("auto_scroll");
        assert!(r.state().auto_scroll);
        r.frames(3);
        let newest = log_line("checked again");
        r.frames(4);
        let (_, y, _, h) = r.rect(&row_key(newest, "text"));
        assert!(y + h <= 460.0, "{y}");
    }

    // ----- screenshots ---------------------------------------------------------------------

    fn shoot(r: &mut Rig, sh: &mut Option<crate::fallback::offscreen::SdlShooter>, name: &str) -> Option<crate::fallback::offscreen::Shot> {
        if sh.is_none() {
            *sh = crate::fallback::offscreen::SdlShooter::new((520, 460)).ok();
        }
        let shooter = match sh {
            Some(s) => s,
            None => {
                eprintln!("no software renderer here, screenshot `{name}` skipped");
                return None;
            }
        };
        r.frames(2);
        let s = shooter.shoot(&mut r.fb.borrow_mut(), [0.05, 0.05, 0.07]).ok()?;
        if let Some(dir) = std::env::var_os("EFE_DEBUG_MENU_SHOTS") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).expect("shot dir");
            std::fs::write(dir.join(format!("{name}.png")), s.png()).expect("write png");
        }
        Some(s)
    }

    /// Pixels close to the wireframe line colour (0xb4b9c8).
    fn line_pixels(s: &crate::fallback::offscreen::Shot, x0: u32, x1: u32, y: u32) -> usize {
        (x0..x1).filter(|x| s.pixel(*x, y)[..3].iter().zip([0xb4, 0xb9, 0xc8]).all(|(a, b)| (*a as i32 - b).abs() < 6)).count()
    }

    #[test]
    fn screenshots() {
        let mut shooter = None;
        // collapsed rows: a short line, a line that fits, wide lines, a multi-line one
        let mut r = Rig::new();
        log_line("Row 1: Small log");
        log_line("The quick brown fox jumps over the lazy dog and keeps running through the whole engine log until it is far too long for one row.");
        log_line("[warn] Row 3: selectable text, select it and press Ctrl+C");
        let multi = log_line("[error] first line of a multi-line log\nsecond line\nthird line");
        let wide = log_line("Another wide line: sphinx of black quartz, judge my vow, then keep going for a long while past the edge.");
        r.frames(6);
        let collapsed = shoot(&mut r, &mut shooter, "logs_collapsed");
        // expand the multi-line one and the wide one
        r.click(&row_key(multi, "expand"));
        r.click(&row_key(wide, "expand"));
        r.frames(3);
        let expanded = shoot(&mut r, &mut shooter, "logs_expanded");
        r.click("tab_edits");
        let edits = shoot(&mut r, &mut shooter, "edits");
        // the Scripts tab: a few modules, one with a very long name
        r.engine.load_module("coin_dash", &super::tiny_module()).expect("module");
        r.engine.load_module(&format!("a_module_with_a_really_long_name_{}", "x".repeat(80)), &super::tiny_module()).expect("module");
        let gone = r.engine.load_module("unloaded_one", &super::tiny_module()).expect("module");
        r.engine.unload_module(gone).expect("unload");
        r.click("tab_scripts");
        r.frames(40);
        assert_eq!(r.state().tab, DebugTab::Scripts);
        let scripts = shoot(&mut r, &mut shooter, "scripts");
        if let (Some(a), Some(b), Some(c), Some(d)) = (&collapsed, &expanded, &edits, &scripts) {
            for s in [a, b, c, d] {
                assert!(line_pixels(s, 4, 516, 0) > 400, "top border");
                assert!(line_pixels(s, 4, 516, 459) > 400, "bottom border");
                let rules = (1..120).filter(|y| line_pixels(s, 4, 516, *y) > 400).count();
                assert!(rules >= 2, "rules under the header and the tab bar: {rules}");
            }
            assert_ne!(a.rgba, b.rgba);
        }

        // auto scroll on with more lines than fit
        let mut r = Rig::new();
        for i in 0..40 {
            log_line(&format!("[info] line {i:02}: auto scroll keeps the newest line in view"));
        }
        r.frames(6);
        shoot(&mut r, &mut shooter, "autoscroll_on");
        // scrolled up: off
        wheel(&mut r, 8.0);
        shoot(&mut r, &mut shooter, "autoscroll_off");
        assert!(!r.state().auto_scroll);
    }
}
