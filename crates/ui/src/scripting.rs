//! Script and mod access to the UI (API.md section 9): the `ui` host namespace, the `ui.signal`
//! script event and the rules that keep a mod inside its own widgets.
//!
//! # Why a queue
//!
//! Scripts run in parallel lanes against a frozen snapshot of the world; the [`UiTree`] is a
//! resource and the backend lives on the main thread. So host functions never touch the tree.
//! They validate against a small shared table ([`UiScripts`]: one [`ModState`] per module) and
//! queue an operation; `ui_scripts_apply` (PostUpdate, `LATE + 15`) applies the queued operations
//! on the main thread, in module order and call order, so the result does not depend on how the
//! lanes were scheduled.
//!
//! # Handles
//!
//! A widget handle is an `i64` that is positive and unique across modules:
//! `((module id + 1) << 32) | local`. The tree's `WidgetId` never reaches a script. Creating
//! functions return the handle at once (the node exists after the frame's apply step) or a
//! negative status (`-STATUS_*`). Other functions return a status (0 = ok). A handle of another
//! module, a removed node or a made-up number is `STATUS_BAD_HANDLE`.
//!
//! # Who may do what
//!
//! * A **mod** (a module limited to a grant) owns `Owner::Mod(<mod name>)` nodes. Its layers and
//!   keys are prefixed `mod:<mod name>/`. It can change and remove only nodes it created. The
//!   game can expose a node as a **mount point** ([`UiScripts::expose`]): `ui.find(key)` then
//!   returns a handle that is good as a `parent` for creating widgets, and for nothing else. At
//!   most [`UiScripts::set_max_nodes`] nodes per mod (default 256), no modal layers unless the
//!   game allows them, and layer `z` inside a range (default `0..=100`).
//! * A **game script** (not a mod) is trusted: owner `Owner::Plugin("script:<name>")`, plain keys,
//!   no limits, and `ui.find` reaches every keyed node of the tree.
//! * Nodes of a mod are removed when the mod unloads (or is replaced by a reload).
//!
//! Violations never panic and never trap: the call returns a status, the first one of each kind
//! per module is logged, and [`UiScripts::violations`] keeps the last ones for tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use efeengine_core::efevm::{ApiType as T, Caller, EventDesc, FieldDesc, HostFn, HostOk, Prim, StructDesc, Trap};
use efeengine_core::prelude::*;
use efeengine_core::scripting::{LogLevel, ModuleId, ScriptEvent, ScriptHost};

use crate::id::{Owner, WidgetId};
use crate::node::{Anchor, ImageFit, KindTag, Layer, NodeKind};
use crate::signal::{UiSignal, UiSignalKind, UiValue};
use crate::style::{Color, Direction, Edges, Size};
use crate::tree::UiTree;

/// Namespace of the host functions and the event.
pub const NS: &str = "ui";
/// Owner of the namespace in the API index.
pub const OWNER: &str = "efeengine-ui";
/// Name of the event struct (payload of `ui.signal`).
pub const SIGNAL_STRUCT: &str = "UiSignal";

/// Status codes of the host functions (0 is ok). Creating functions return `-code`.
pub const STATUS_OK: i32 = 0;
/// Unknown, stale, removed or foreign handle (also: no such key).
pub const STATUS_BAD_HANDLE: i32 = 1;
/// The call does not fit the widget (`set_text` on a slider, a child under a text, ...).
pub const STATUS_WRONG_KIND: i32 = 2;
/// The mod's node limit is reached.
pub const STATUS_LIMIT: i32 = 3;
/// Not allowed for this caller (changing a mount point, a modal layer for a mod).
pub const STATUS_DENIED: i32 = 4;
/// A bad argument (empty or taken key, unknown anchor).
pub const STATUS_BAD_ARG: i32 = 5;

/// `kind` values of the `ui.signal` payload.
pub mod signal_kind {
    /// A button was pressed and released.
    pub const CLICK: u32 = 1;
    /// A checkbox, slider or text input changed: `value` is 0 / 1, the slider value, or the text length in chars.
    pub const CHANGED: u32 = 2;
    /// Enter in a text input (read the text with `ui.get_text`).
    pub const SUBMIT: u32 = 3;
    /// Keyboard focus gained.
    pub const FOCUS_GAINED: u32 = 4;
    /// Keyboard focus lost.
    pub const FOCUS_LOST: u32 = 5;
    /// Pointer entered (nodes with `ui.set_hover_signals`).
    pub const HOVER_ENTER: u32 = 6;
    /// Pointer left.
    pub const HOVER_LEAVE: u32 = 7;
}

const SIGNAL_SIZE: usize = 16;

/// A violation a module caused, for tests and diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub module: String,
    pub function: &'static str,
    pub status: i32,
}

#[derive(Clone, Debug)]
enum Cache {
    None,
    Text(String),
    Bool(bool),
    F32(f32),
}

#[derive(Clone, Debug)]
struct Entry {
    widget: Option<WidgetId>,
    tag: KindTag,
    /// Created by this module (counts toward the limit, removed with it).
    own: bool,
    /// A mount point exposed by the game: only usable as a parent.
    mount: bool,
    cache: Cache,
}

struct ModState {
    name: String,
    trusted: bool,
    owner: Owner,
    next_local: u32,
    entries: HashMap<u32, Entry>,
    /// Script-side key (without prefix) to local handle.
    keys: HashMap<String, u32>,
    /// Mount points this module already got a handle for.
    mounts: HashMap<String, u32>,
    /// `(function, status)` pairs already logged.
    logged: Vec<(&'static str, i32)>,
}

impl ModState {
    fn new(name: &str, trusted: bool) -> Self {
        let owner = if trusted { Owner::Plugin(format!("script:{name}").into()) } else { Owner::Mod(name.into()) };
        ModState {
            name: name.to_string(),
            trusted,
            owner,
            next_local: 1,
            entries: HashMap::new(),
            keys: HashMap::new(),
            mounts: HashMap::new(),
            logged: Vec::new(),
        }
    }

    fn owned(&self) -> usize {
        self.entries.values().filter(|e| e.own).count()
    }

    /// The key as stored in the tree: prefixed for mods.
    fn full_key(&self, key: &str) -> String {
        if self.trusted { key.to_string() } else { format!("mod:{}/{key}", self.name) }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Layer { local: u32, key: String, layer: Layer },
    Create { local: u32, parent: u32, kind: NodeKind },
    Key { local: u32, key: String },
    Remove { local: u32 },
    Text { local: u32, text: String },
    Value { local: u32, value: UiValue },
    Visible { local: u32, on: bool },
    Enabled { local: u32, on: bool },
    Hover { local: u32, on: bool },
    Classes { local: u32, names: Vec<String> },
    AddClass { local: u32, name: String },
    RemoveClass { local: u32, name: String },
    Style { local: u32, change: StyleOp },
}

#[derive(Clone, Copy, Debug)]
enum StyleOp {
    Row(bool),
    Size(Size, Size),
    Padding(f32),
    Gap(f32),
    Background(Color),
    TextColor(Color),
    FontSize(f32),
}

struct Inner {
    mods: HashMap<u32, ModState>,
    /// `(module id, call order, op)`.
    ops: Vec<(u32, u64, Op)>,
    seq: u64,
    /// Mount points the game exposed, with the id they had at the last refresh.
    exposed: HashMap<String, Option<WidgetId>>,
    /// Every keyed node, refreshed only while a trusted script exists.
    all_keys: HashMap<String, WidgetId>,
    any_trusted: bool,
    max_nodes: usize,
    max_nodes_of: HashMap<String, usize>,
    allow_modal: bool,
    z_range: (i32, i32),
    violations: Vec<Violation>,
    pending_logs: Vec<String>,
}

impl Inner {
    fn new() -> Self {
        Inner {
            mods: HashMap::new(),
            ops: Vec::new(),
            seq: 0,
            exposed: HashMap::new(),
            all_keys: HashMap::new(),
            any_trusted: false,
            max_nodes: 256,
            max_nodes_of: HashMap::new(),
            allow_modal: false,
            z_range: (0, 100),
            violations: Vec::new(),
            pending_logs: Vec::new(),
        }
    }
}

/// The caller of a host function.
struct Who {
    id: u32,
    name: String,
    trusted: bool,
}

fn handle_of(module: u32, local: u32) -> i64 {
    (((module as u64 + 1) << 32) | local as u64) as i64
}

/// The module part and the local part of a handle, `None` for anything that cannot be one.
fn split_handle(h: i64) -> Option<(u32, u32)> {
    if h <= 0 {
        return None;
    }
    let hi = (h as u64) >> 32;
    let lo = h as u64 as u32;
    if hi == 0 || lo == 0 { None } else { Some((hi as u32 - 1, lo)) }
}

impl Inner {
    fn state(&mut self, w: &Who) -> &mut ModState {
        if w.trusted {
            self.any_trusted = true;
        }
        self.mods.entry(w.id).or_insert_with(|| ModState::new(&w.name, w.trusted))
    }

    /// Record and (once per module, function and status) log a refused call; returns `status`.
    fn deny(&mut self, w: &Who, function: &'static str, status: i32) -> i32 {
        if self.violations.len() >= 64 {
            self.violations.remove(0);
        }
        self.violations.push(Violation { module: w.name.clone(), function, status });
        let st = self.state(w);
        if !st.logged.contains(&(function, status)) {
            st.logged.push((function, status));
            self.pending_logs.push(format!("[{}] ui.{function} refused (status {status}); further identical refusals are not logged", w.name));
        }
        status
    }

    /// The entry behind `handle` for this caller. `modify`: a mount point is refused.
    fn lookup(&mut self, w: &Who, function: &'static str, handle: i64, modify: bool) -> Result<(u32, Entry), i32> {
        let found = match split_handle(handle) {
            Some((m, local)) if m == w.id => self.mods.get(&w.id).and_then(|s| s.entries.get(&local)).cloned().map(|e| (local, e)),
            _ => None,
        };
        match found {
            None => Err(self.deny(w, function, STATUS_BAD_HANDLE)),
            Some((_, e)) if modify && e.mount => Err(self.deny(w, function, STATUS_DENIED)),
            Some(x) => Ok(x),
        }
    }

    fn push(&mut self, w: &Who, op: Op) {
        self.seq += 1;
        self.ops.push((w.id, self.seq, op));
    }

    fn limit_for(&self, w: &Who) -> usize {
        if w.trusted { usize::MAX } else { self.max_nodes_of.get(&w.name).copied().unwrap_or(self.max_nodes) }
    }

    fn layer(&mut self, w: &Who, name: &str, z: i32, anchor: i32, modal: bool) -> i64 {
        const F: &str = "layer";
        let Some(anchor) = anchor_from(anchor) else { return -(self.deny(w, F, STATUS_BAD_ARG) as i64) };
        if name.is_empty() {
            return -(self.deny(w, F, STATUS_BAD_ARG) as i64);
        }
        if !w.trusted && modal && !self.allow_modal {
            return -(self.deny(w, F, STATUS_DENIED) as i64);
        }
        let z = if w.trusted { z } else { z.clamp(self.z_range.0, self.z_range.1) };
        let layer = Layer { z, modal, anchor, offset: (0.0, 0.0) };
        let limit = self.limit_for(w);
        let st = self.state(w);
        let key = st.full_key(name);
        let local = match st.keys.get(name).copied() {
            Some(l) => l,
            None => {
                if st.owned() >= limit {
                    return -(self.deny(w, F, STATUS_LIMIT) as i64);
                }
                let l = st.next_local;
                st.next_local += 1;
                st.keys.insert(name.to_string(), l);
                st.entries.insert(l, Entry { widget: None, tag: KindTag::Container, own: true, mount: false, cache: Cache::None });
                l
            }
        };
        self.push(w, Op::Layer { local, key, layer });
        handle_of(w.id, local)
    }

    fn create(&mut self, w: &Who, function: &'static str, parent: i64, kind: NodeKind) -> i64 {
        let (plocal, pe) = match self.lookup(w, function, parent, false) {
            Ok(x) => x,
            Err(s) => return -(s as i64),
        };
        if matches!(pe.tag, KindTag::Text | KindTag::Image | KindTag::Slider | KindTag::TextInput | KindTag::Checkbox | KindTag::GpuView) {
            return -(self.deny(w, function, STATUS_WRONG_KIND) as i64);
        }
        let limit = self.limit_for(w);
        let st = self.state(w);
        if st.owned() >= limit {
            return -(self.deny(w, function, STATUS_LIMIT) as i64);
        }
        let local = st.next_local;
        st.next_local += 1;
        let cache = match &kind {
            NodeKind::Text { text } => Cache::Text(text.clone()),
            NodeKind::Button { label } => Cache::Text(label.clone()),
            NodeKind::Checkbox { label, checked } => {
                let _ = label;
                Cache::Bool(*checked)
            }
            NodeKind::Slider { value, .. } => Cache::F32(*value),
            NodeKind::TextInput { value, .. } => Cache::Text(value.clone()),
            _ => Cache::None,
        };
        st.entries.insert(local, Entry { widget: None, tag: kind.tag(), own: true, mount: false, cache });
        self.push(w, Op::Create { local, parent: plocal, kind });
        handle_of(w.id, local)
    }
}

fn anchor_from(i: i32) -> Option<Anchor> {
    Some(match i {
        0 => Anchor::Fill,
        1 => Anchor::Center,
        2 => Anchor::TopLeft,
        3 => Anchor::Top,
        4 => Anchor::TopRight,
        5 => Anchor::Left,
        6 => Anchor::Right,
        7 => Anchor::BottomLeft,
        8 => Anchor::Bottom,
        9 => Anchor::BottomRight,
        _ => return None,
    })
}

fn fit_from(i: i32) -> ImageFit {
    match i {
        1 => ImageFit::Cover,
        2 => ImageFit::Fill,
        3 => ImageFit::None,
        _ => ImageFit::Contain,
    }
}

fn size_from(px: f32) -> Size {
    if px.is_finite() && px >= 0.0 { Size::Px(px) } else { Size::Auto }
}

fn color_from(rgba: u32) -> Color {
    let c = |shift: u32| ((rgba >> shift) & 0xff) as f32 / 255.0;
    Color::rgba(c(24), c(16), c(8), c(0))
}

/// What the game holds to configure and inspect script access to the UI. Inserted as a resource
/// by `UiPlugin`; cheap to clone (a handle to the shared table).
#[derive(Clone)]
pub struct UiScripts {
    shared: Arc<Mutex<Inner>>,
}

impl UiScripts {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Let mods use the node `key` as a parent (a mount point): `ui.find(key)` gives them a
    /// handle for creating widgets under it, nothing more. Safe to call before the node exists.
    pub fn expose(&self, key: &str) {
        self.lock().exposed.entry(key.to_string()).or_insert(None);
    }

    /// Stop exposing `key` (handles mods already got stay usable until their node is gone).
    pub fn unexpose(&self, key: &str) {
        self.lock().exposed.remove(key);
    }

    /// Nodes per mod (default 256).
    pub fn set_max_nodes(&self, n: usize) {
        self.lock().max_nodes = n;
    }

    /// Nodes for one mod (by its id), overriding the default.
    pub fn set_max_nodes_for(&self, module: &str, n: usize) {
        self.lock().max_nodes_of.insert(module.to_string(), n);
    }

    /// Allow modal layers for mods (default: refused with `STATUS_DENIED`).
    pub fn allow_modal(&self, yes: bool) {
        self.lock().allow_modal = yes;
    }

    /// The `z` range mod layers are clamped to (default `0..=100`).
    pub fn set_z_range(&self, min: i32, max: i32) {
        self.lock().z_range = (min.min(max), min.max(max));
    }

    /// Nodes (layers included) the module called `module` created and still has.
    pub fn nodes_of(&self, module: &str) -> usize {
        self.lock().mods.values().filter(|m| m.name == module).map(ModState::owned).sum()
    }

    /// The last refused calls (at most 64), oldest first.
    pub fn violations(&self) -> Vec<Violation> {
        self.lock().violations.clone()
    }

    pub fn clear_violations(&self) {
        self.lock().violations.clear();
    }

    /// Apply the operations queued by scripts this frame (called by the plugin's system; public
    /// for tests that drive the tree by hand).
    pub fn apply(&self, tree: &mut UiTree, scripting: &efeengine_core::scripting::Scripting) {
        let mut g = self.lock();
        // Unloaded modules take their nodes with them.
        let gone: Vec<u32> = g.mods.keys().copied().filter(|&m| !scripting.is_loaded(ModuleId(m))).collect();
        for m in gone {
            if let Some(st) = g.mods.remove(&m) {
                tree.remove_owner(&st.owner);
            }
            g.ops.retain(|(om, _, _)| *om != m);
        }
        let mut ops = std::mem::take(&mut g.ops);
        ops.sort_by_key(|(m, seq, _)| (*m, *seq));
        let mut notes: Vec<String> = Vec::new();
        for (m, _, op) in ops {
            let Some(st) = g.mods.get_mut(&m) else { continue };
            let owner = st.owner.clone();
            let name = st.name.clone();
            let mut fail = |what: &str| notes.push(format!("[{name}] ui: {what}"));
            tree.with_owner(owner, |tree| apply_op(tree, st, op, &mut fail));
        }
        // Entries whose node is gone (removed with a parent, removed by the game, never created).
        for st in g.mods.values_mut() {
            let dead: Vec<u32> = st.entries.iter().filter(|(_, e)| e.widget.is_none_or(|w| !tree.contains(w))).map(|(l, _)| *l).collect();
            if !dead.is_empty() {
                for l in &dead {
                    st.entries.remove(l);
                }
                st.keys.retain(|_, l| !dead.contains(l));
                st.mounts.retain(|_, l| !dead.contains(l));
            }
        }
        // Mount points and, for trusted scripts, the key table the next frame's `find` reads.
        let keys: Vec<String> = g.exposed.keys().cloned().collect();
        for k in keys {
            let id = tree.find(&k);
            g.exposed.insert(k, id);
        }
        if g.any_trusted {
            g.all_keys = tree.iter().filter_map(|(id, n)| n.key().map(|k| (k.to_string(), id))).collect();
        }
        drop(g);
        for n in notes {
            global_warn(&n);
        }
    }

    /// Update the caches and make the script events of this frame's signals.
    fn deliver(&self, signals: &[UiSignal]) -> Vec<ScriptEvent> {
        let mut g = self.lock();
        if g.mods.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for s in signals {
            let targets: Vec<(u32, u32)> = g
                .mods
                .iter()
                .flat_map(|(m, st)| st.entries.iter().filter(|(_, e)| e.widget == Some(s.widget) && !e.mount).map(|(l, _)| (*m, *l)))
                .collect();
            for (m, l) in targets {
                let (kind, value) = match &s.kind {
                    UiSignalKind::Click => (signal_kind::CLICK, 0.0),
                    UiSignalKind::Changed(v) => {
                        if let Some(e) = g.mods.get_mut(&m).and_then(|st| st.entries.get_mut(&l)) {
                            match v {
                                UiValue::Bool(b) => e.cache = Cache::Bool(*b),
                                UiValue::F32(f) => e.cache = Cache::F32(*f),
                                UiValue::Text(t) => e.cache = Cache::Text(t.clone()),
                            }
                        }
                        let value = match v {
                            UiValue::Bool(b) => f32::from(u8::from(*b)),
                            UiValue::F32(f) => *f,
                            UiValue::Text(t) => t.chars().count() as f32,
                        };
                        (signal_kind::CHANGED, value)
                    }
                    UiSignalKind::Submit(t) => {
                        if let Some(e) = g.mods.get_mut(&m).and_then(|st| st.entries.get_mut(&l)) {
                            e.cache = Cache::Text(t.clone());
                        }
                        (signal_kind::SUBMIT, 0.0)
                    }
                    UiSignalKind::Focus(true) => (signal_kind::FOCUS_GAINED, 0.0),
                    UiSignalKind::Focus(false) => (signal_kind::FOCUS_LOST, 0.0),
                    UiSignalKind::Hover(true) => (signal_kind::HOVER_ENTER, 0.0),
                    UiSignalKind::Hover(false) => (signal_kind::HOVER_LEAVE, 0.0),
                    _ => continue,
                };
                let mut payload = Vec::with_capacity(SIGNAL_SIZE);
                payload.extend_from_slice(&(handle_of(m, l) as u64).to_le_bytes());
                payload.extend_from_slice(&kind.to_le_bytes());
                payload.extend_from_slice(&value.to_le_bytes());
                out.push(ScriptEvent { event: format!("{NS}.signal"), payload });
            }
        }
        out
    }
}

fn apply_op(tree: &mut UiTree, st: &mut ModState, op: Op, fail: &mut dyn FnMut(&str)) {
    let widget = |st: &ModState, local: u32| st.entries.get(&local).and_then(|e| e.widget);
    match op {
        Op::Layer { local, key, layer } => {
            let existed = tree.find(&key);
            if existed.is_some_and(|id| st.entries.get(&local).is_some_and(|e| e.widget != Some(id))) {
                fail(&format!("layer key `{key}` is taken by another node"));
                return;
            }
            let id = tree.layer(&key, layer);
            if let Some(e) = st.entries.get_mut(&local) {
                e.widget = Some(id);
            }
        }
        Op::Create { local, parent, kind } => {
            let Some(p) = widget(st, parent).filter(|p| tree.contains(*p)) else {
                fail("parent is gone, node not created");
                return;
            };
            match tree.try_create(p, kind) {
                Ok(id) => {
                    if let Some(e) = st.entries.get_mut(&local) {
                        e.widget = Some(id);
                    }
                }
                Err(e) => fail(&format!("cannot create node: {e}")),
            }
        }
        Op::Key { local, key } => {
            let Some(id) = widget(st, local) else { return };
            if let Err(e) = tree.set_key(id, &key) {
                fail(&format!("cannot set key `{key}`: {e}"));
            }
        }
        Op::Remove { local } => {
            if let Some(id) = widget(st, local) {
                tree.remove(id);
            }
        }
        Op::Text { local, text } => {
            if let Some(id) = widget(st, local) {
                tree.set_text(id, text);
            }
        }
        Op::Value { local, value } => {
            if let Some(id) = widget(st, local) {
                tree.set_value(id, value);
            }
        }
        Op::Visible { local, on } => {
            if let Some(id) = widget(st, local) {
                tree.set_visible(id, on);
            }
        }
        Op::Enabled { local, on } => {
            if let Some(id) = widget(st, local) {
                tree.set_enabled(id, on);
            }
        }
        Op::Hover { local, on } => {
            if let Some(id) = widget(st, local) {
                tree.set_hover_signals(id, on);
            }
        }
        Op::Classes { local, names } => {
            if let Some(id) = widget(st, local) {
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                tree.set_classes(id, &refs);
            }
        }
        Op::AddClass { local, name } => {
            if let Some(id) = widget(st, local) {
                tree.add_class(id, &name);
            }
        }
        Op::RemoveClass { local, name } => {
            if let Some(id) = widget(st, local) {
                tree.remove_class(id, &name);
            }
        }
        Op::Style { local, change } => {
            if let Some(id) = widget(st, local) {
                tree.edit_style(id, |s| match change {
                    StyleOp::Row(row) => s.direction = Some(if row { Direction::Row } else { Direction::Column }),
                    StyleOp::Size(w, h) => {
                        s.width = Some(w);
                        s.height = Some(h);
                    }
                    StyleOp::Padding(p) => s.padding = Some(Edges::from(p)),
                    StyleOp::Gap(g) => s.gap = Some(g),
                    StyleOp::Background(c) => s.background = Some(c),
                    StyleOp::TextColor(c) => s.text_color = Some(c),
                    StyleOp::FontSize(f) => s.font_size = Some(f),
                });
            }
        }
    }
}

fn who(c: &mut Caller<'_, ScriptHost>) -> Result<Who, Trap> {
    let h = c.host();
    let id = h.current_module().ok_or_else(|| Trap::Host("ui: called outside a module".into()))?;
    let name = h.module_name(id).unwrap_or("?").to_string();
    Ok(Who { id: id.0, name, trusted: !h.is_mod(id) })
}

/// Run `f` on the shared table for the calling module, then log what it wanted to say.
fn run<R>(c: &mut Caller<'_, ScriptHost>, shared: &Arc<Mutex<Inner>>, f: impl FnOnce(&mut Inner, &Who) -> R) -> Result<R, Trap> {
    let w = who(c)?;
    let (r, logs) = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut g, &w);
        (r, std::mem::take(&mut g.pending_logs))
    };
    for l in logs {
        c.host().log(LogLevel::Warn, l);
    }
    Ok(r)
}

fn status_ok(r: Result<(), i32>) -> i32 {
    r.err().unwrap_or(STATUS_OK)
}

/// Registers the namespace, the event and every host function. Returns the handle the game uses
/// (also inserted as a resource by the plugin). Fails softly (a warning) when API registration is
/// already closed.
pub(crate) fn register(engine: &mut Engine) -> Option<UiScripts> {
    if !engine.api_open() {
        global_warn("[ui] scripting API not registered: API registration is closed (add UiPlugin before the game loop starts)");
        return None;
    }
    match register_all(engine) {
        Ok(s) => Some(s),
        Err(e) => {
            global_warn(&format!("[ui] scripting API not registered: {e}"));
            None
        }
    }
}

fn register_all(engine: &mut Engine) -> Result<UiScripts, String> {
    let e = |r: Result<(), efeengine_core::efevm::RegistryError>| r.map_err(|e| e.to_string());
    let shared = Arc::new(Mutex::new(Inner::new()));
    e(engine.define_namespace(
        NS,
        1,
        OWNER,
        "Game UI for scripts and mods. Widgets are `i64` handles (positive, valid only for the module that created or found them). Creating functions return a handle or a negative status; the others return a status: 0 ok, 1 bad handle, 2 wrong widget kind, 3 node limit, 4 denied, 5 bad argument. A mod can only change and remove widgets it created; the game may expose mount points (`find`) that are good as parents only. Changes apply at the end of the frame. Needs the `ui` namespace in the mod's grant; the event `ui.signal` (payload UiSignal) reports Click, Changed, Submit, Focus and Hover of the module's own widgets.",
    ))?;
    engine
        .register_struct(
            StructDesc::new(SIGNAL_STRUCT, SIGNAL_SIZE as u32, 8)
                .doc("Payload of ui.signal: what the user did to one of the module's widgets.")
                .field(FieldDesc::new("widget", 0, Prim::I64, 1).with_doc("The widget handle."))
                .field(FieldDesc::new("kind", 8, Prim::U32, 1).with_doc("1 click, 2 changed, 3 submit, 4 focus gained, 5 focus lost, 6 hover enter, 7 hover leave."))
                .field(FieldDesc::new("value", 12, Prim::F32, 1).with_doc("Changed: checkbox 0 or 1, slider value, text input length in chars. Otherwise 0.")),
        )
        .map_err(|e| e.to_string())?;
    e(engine.register_event(
        EventDesc::new(NS, "signal")
            .payload(SIGNAL_STRUCT)
            .doc("A user action on one of the module's own widgets (readable the same frame; the text of a text input is read with ui.get_text)."),
    ))?;

    macro_rules! reg {
        ($desc:expr, $f:expr) => {
            e(engine.register_host_fn($desc, $f))?
        };
    }
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "layer")
            .doc("Create (or get, if it exists) the layer `name`: a root container drawn at `z` (higher is on top; a mod's z is clamped to the game's range, default 0..=100), placed at `anchor` (0 fill, 1 center, 2 top-left, 3 top, 4 top-right, 5 left, 6 right, 7 bottom-left, 8 bottom, 9 bottom-right). A mod's layer is called `mod:<mod>/<name>` in the tree. `modal` captures all input below it; refused (4) for mods unless the game allows it. Returns the handle or a negative status.")
            .param("name", T::Str)
            .param("z", T::I32)
            .param("anchor", T::I32)
            .param("modal", T::Bool)
            .result("widget", T::I64),
        move |c, a, r| {
            let (name, z, anchor, modal) = (c.read_str(a.str(0)?)?, a.i32(1)?, a.i32(2)?, a.bool(3)?);
            let h = run(c, &s, |g, w| g.layer(w, &name, z, anchor, modal))?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    for (name, row, doc) in [
        ("column", false, "Create a container that stacks its children top to bottom under `parent`."),
        ("row", true, "Create a container that places its children left to right under `parent`."),
    ] {
        let s = shared.clone();
        let function: &'static str = name;
        reg!(
            HostFn::new(NS, name).doc(format!("{doc} Returns the handle or a negative status.")).param("parent", T::I64).result("widget", T::I64),
            move |c, a, r| {
                let parent = a.i64(0)?;
                let h = run(c, &s, |g, w| {
                    let h = g.create(w, function, parent, NodeKind::Container);
                    if h > 0 {
                        let (_, local) = split_handle(h).expect("fresh handle");
                        g.push(w, Op::Style { local, change: StyleOp::Row(row) });
                    }
                    h
                })?;
                r.set_i64(0, h)?;
                Ok(HostOk::Continue)
            }
        );
    }
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "text")
            .doc("Create a text label under `parent`. Returns the handle or a negative status.")
            .param("parent", T::I64)
            .param("text", T::Str)
            .result("widget", T::I64),
        move |c, a, r| {
            let (parent, text) = (a.i64(0)?, c.read_str(a.str(1)?)?);
            let h = run(c, &s, |g, w| g.create(w, "text", parent, NodeKind::Text { text }))?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "button")
            .doc("Create a button with `label` under `parent`; clicks arrive as ui.signal kind 1. Returns the handle or a negative status.")
            .param("parent", T::I64)
            .param("label", T::Str)
            .result("widget", T::I64),
        move |c, a, r| {
            let (parent, label) = (a.i64(0)?, c.read_str(a.str(1)?)?);
            let h = run(c, &s, |g, w| g.create(w, "button", parent, NodeKind::Button { label }))?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "checkbox")
            .doc("Create a checkbox with `label` and the initial `checked` state under `parent`; changes arrive as ui.signal kind 2 (value 0 or 1). Returns the handle or a negative status.")
            .param("parent", T::I64)
            .param("label", T::Str)
            .param("checked", T::Bool)
            .result("widget", T::I64),
        move |c, a, r| {
            let (parent, label, checked) = (a.i64(0)?, c.read_str(a.str(1)?)?, a.bool(2)?);
            let h = run(c, &s, |g, w| g.create(w, "checkbox", parent, NodeKind::Checkbox { label, checked }))?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "slider")
            .doc("Create a slider from `min` to `max` with the initial `value` under `parent`; changes arrive as ui.signal kind 2 (value = the slider value). Returns the handle or a negative status.")
            .param("parent", T::I64)
            .param("min", T::F32)
            .param("max", T::F32)
            .param("value", T::F32)
            .result("widget", T::I64),
        move |c, a, r| {
            let (parent, min, max, value) = (a.i64(0)?, a.f32(1)?, a.f32(2)?, a.f32(3)?);
            let h = run(c, &s, |g, w| g.create(w, "slider", parent, NodeKind::slider(min, max, value)))?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "text_input")
            .doc("Create a single-line text input with the initial `text` under `parent`; edits arrive as ui.signal kind 2 and Enter as kind 3 (read the text with ui.get_text). Returns the handle or a negative status.")
            .param("parent", T::I64)
            .param("text", T::Str)
            .result("widget", T::I64),
        move |c, a, r| {
            let (parent, text) = (a.i64(0)?, c.read_str(a.str(1)?)?);
            let h = run(c, &s, |g, w| g.create(w, "text_input", parent, NodeKind::text_input(text)))?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "image")
            .doc("Create an image of texture `texture` (an id the game gave the module, for example from its own assets) under `parent`; `fit` 0 contain, 1 cover, 2 fill, 3 none. Returns the handle or a negative status.")
            .param("parent", T::I64)
            .param("texture", T::I32)
            .param("fit", T::I32)
            .result("widget", T::I64),
        move |c, a, r| {
            let (parent, texture, fit) = (a.i64(0)?, a.i32(1)?, a.i32(2)?);
            let h = run(c, &s, |g, w| {
                let kind = NodeKind::Image { texture: TextureHandle { id: texture.max(0) as u32 }, fit: fit_from(fit) };
                g.create(w, "image", parent, kind)
            })?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "remove")
            .doc("Remove `widget` and its children. Only widgets the module created (a layer included).")
            .param("widget", T::I64)
            .result("status", T::I32),
        move |c, a, r| {
            let h = a.i64(0)?;
            let st = run(c, &s, |g, w| {
                status_ok(g.lookup(w, "remove", h, true).map(|(local, _)| {
                    g.push(w, Op::Remove { local });
                }))
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "set_key")
            .doc("Give `widget` the key `key` (unique among the module's keys; the tree key of a mod's widget is `mod:<mod>/<key>`), so `find` returns it later. Status 5 for an empty or taken key.")
            .param("widget", T::I64)
            .param("key", T::Str)
            .result("status", T::I32),
        move |c, a, r| {
            let (h, key) = (a.i64(0)?, c.read_str(a.str(1)?)?);
            let st = run(c, &s, |g, w| {
                let (local, _) = match g.lookup(w, "set_key", h, true) {
                    Ok(x) => x,
                    Err(s) => return s,
                };
                let st = g.state(w);
                if key.is_empty() || st.keys.get(&key).is_some_and(|l| *l != local) {
                    return g.deny(w, "set_key", STATUS_BAD_ARG);
                }
                st.keys.retain(|_, l| *l != local);
                st.keys.insert(key.clone(), local);
                let full = st.full_key(&key);
                g.push(w, Op::Key { local, key: full });
                STATUS_OK
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "find")
            .doc("The handle of the widget with key `key`: one of the module's own keys, else a mount point the game exposed (good as a parent only). A game script (not a mod) also finds any keyed node of the tree, as it was at the end of the last frame. Negative status 1 when there is none.")
            .param("key", T::Str)
            .result("widget", T::I64),
        move |c, a, r| {
            let key = c.read_str(a.str(0)?)?;
            let h = run(c, &s, |g, w| {
                let st = g.state(w);
                if let Some(l) = st.keys.get(&key) {
                    return handle_of(w.id, *l);
                }
                if let Some(l) = st.mounts.get(&key) {
                    return handle_of(w.id, *l);
                }
                let (found, mount) = if w.trusted {
                    (g.all_keys.get(&key).copied(), false)
                } else {
                    (g.exposed.get(&key).copied().flatten(), true)
                };
                let Some(id) = found else { return -(g.deny(w, "find", STATUS_BAD_HANDLE) as i64) };
                let st = g.state(w);
                let l = st.next_local;
                st.next_local += 1;
                st.entries.insert(l, Entry { widget: Some(id), tag: KindTag::Container, own: false, mount, cache: Cache::None });
                if mount {
                    st.mounts.insert(key.clone(), l);
                } else {
                    st.keys.insert(key.clone(), l);
                }
                handle_of(w.id, l)
            })?;
            r.set_i64(0, h)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "set_text")
            .doc("Set the text of a text label, the label of a button or checkbox, or the value of a text input. Status 2 for other widgets.")
            .param("widget", T::I64)
            .param("text", T::Str)
            .result("status", T::I32),
        move |c, a, r| {
            let (h, text) = (a.i64(0)?, c.read_str(a.str(1)?)?);
            let st = run(c, &s, |g, w| {
                let (local, e) = match g.lookup(w, "set_text", h, true) {
                    Ok(x) => x,
                    Err(s) => return s,
                };
                if !matches!(e.tag, KindTag::Text | KindTag::Button | KindTag::Checkbox | KindTag::TextInput) {
                    return g.deny(w, "set_text", STATUS_WRONG_KIND);
                }
                if let Some(en) = g.state(w).entries.get_mut(&local) {
                    if matches!(en.cache, Cache::Text(_)) {
                        en.cache = Cache::Text(text.clone());
                    }
                }
                g.push(w, Op::Text { local, text });
                STATUS_OK
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "get_text")
            .doc("Copy the text of a text label, the label of a button or checkbox, or the value of a text input (as the module last set it or the user last changed it) into `buf`, as much as fits. Returns the full length in bytes, or a negative status.")
            .param("widget", T::I64)
            .param("buf", T::Bytes)
            .result("len", T::I32),
        move |c, a, r| {
            let (h, buf) = (a.i64(0)?, a.bytes(1)?);
            if buf.ptr > i64::MAX as u64 || buf.len > i64::MAX as u64 {
                return Err(Trap::OutOfBounds);
            }
            let text = run(c, &s, |g, w| match g.lookup(w, "get_text", h, true) {
                Ok((_, e)) => match e.cache {
                    Cache::Text(t) => Ok(t),
                    _ => Err(g.deny(w, "get_text", STATUS_WRONG_KIND)),
                },
                Err(s) => Err(s),
            })?;
            match text {
                Ok(t) => {
                    let n = (buf.len as usize).min(t.len());
                    if n > 0 {
                        c.write_bytes(buf.ptr, &t.as_bytes()[..n])?;
                    }
                    r.set_i32(0, t.len().min(i32::MAX as usize) as i32)?;
                }
                Err(st) => r.set_i32(0, -st)?,
            }
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "set_value")
            .doc("Set the value of a slider (clamped to its range). Status 2 for other widgets.")
            .param("widget", T::I64)
            .param("value", T::F32)
            .result("status", T::I32),
        move |c, a, r| {
            let (h, value) = (a.i64(0)?, a.f32(1)?);
            let st = run(c, &s, |g, w| {
                let (local, e) = match g.lookup(w, "set_value", h, true) {
                    Ok(x) => x,
                    Err(s) => return s,
                };
                if e.tag != KindTag::Slider {
                    return g.deny(w, "set_value", STATUS_WRONG_KIND);
                }
                if let Some(en) = g.state(w).entries.get_mut(&local) {
                    en.cache = Cache::F32(value);
                }
                g.push(w, Op::Value { local, value: UiValue::F32(value) });
                STATUS_OK
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "value")
            .doc("The value of a slider as the module last set it or the user last changed it; 0 for a bad handle or another widget.")
            .param("widget", T::I64)
            .result("value", T::F32),
        move |c, a, r| {
            let h = a.i64(0)?;
            let v = run(c, &s, |g, w| match g.lookup(w, "value", h, true) {
                Ok((_, Entry { cache: Cache::F32(v), .. })) => v,
                _ => 0.0,
            })?;
            r.set_f32(0, v)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "set_checked")
            .doc("Set the state of a checkbox. Status 2 for other widgets.")
            .param("widget", T::I64)
            .param("checked", T::Bool)
            .result("status", T::I32),
        move |c, a, r| {
            let (h, on) = (a.i64(0)?, a.bool(1)?);
            let st = run(c, &s, |g, w| {
                let (local, e) = match g.lookup(w, "set_checked", h, true) {
                    Ok(x) => x,
                    Err(s) => return s,
                };
                if e.tag != KindTag::Checkbox {
                    return g.deny(w, "set_checked", STATUS_WRONG_KIND);
                }
                if let Some(en) = g.state(w).entries.get_mut(&local) {
                    en.cache = Cache::Bool(on);
                }
                g.push(w, Op::Value { local, value: UiValue::Bool(on) });
                STATUS_OK
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        }
    );
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "checked")
            .doc("The state of a checkbox as the module last set it or the user last changed it; false for a bad handle or another widget.")
            .param("widget", T::I64)
            .result("checked", T::Bool),
        move |c, a, r| {
            let h = a.i64(0)?;
            let v = run(c, &s, |g, w| matches!(g.lookup(w, "checked", h, true), Ok((_, Entry { cache: Cache::Bool(true), .. }))))?;
            r.set_bool(0, v)?;
            Ok(HostOk::Continue)
        }
    );
    for (name, doc) in [
        ("set_visible", "Show or hide `widget` (and its children)."),
        ("set_enabled", "Enable or disable `widget` (a disabled widget and its children send no signals)."),
        ("set_hover_signals", "Make `widget` send ui.signal kinds 6 and 7 when the pointer enters and leaves it."),
    ] {
        let s = shared.clone();
        let function: &'static str = name;
        reg!(HostFn::new(NS, name).doc(doc).param("widget", T::I64).param("on", T::Bool).result("status", T::I32), move |c, a, r| {
            let (h, on) = (a.i64(0)?, a.bool(1)?);
            let st = run(c, &s, |g, w| {
                status_ok(g.lookup(w, function, h, true).map(|(local, _)| {
                    g.push(
                        w,
                        match function {
                            "set_visible" => Op::Visible { local, on },
                            "set_enabled" => Op::Enabled { local, on },
                            _ => Op::Hover { local, on },
                        },
                    );
                }))
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        });
    }
    for (name, doc) in [
        ("set_class", "Replace the classes of `widget` with the single class `name` (styled by the game's theme)."),
        ("add_class", "Add the class `name` to `widget`."),
        ("remove_class", "Remove the class `name` from `widget`."),
    ] {
        let s = shared.clone();
        let function: &'static str = name;
        reg!(HostFn::new(NS, name).doc(doc).param("widget", T::I64).param("name", T::Str).result("status", T::I32), move |c, a, r| {
            let (h, class) = (a.i64(0)?, c.read_str(a.str(1)?)?);
            let st = run(c, &s, |g, w| {
                status_ok(g.lookup(w, function, h, true).map(|(local, _)| {
                    g.push(
                        w,
                        match function {
                            "set_class" => Op::Classes { local, names: vec![class] },
                            "add_class" => Op::AddClass { local, name: class },
                            _ => Op::RemoveClass { local, name: class },
                        },
                    );
                }))
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        });
    }
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "set_size")
            .doc("Set the width and height of `widget` in logical pixels; a negative value means automatic.")
            .param("widget", T::I64)
            .param("width", T::F32)
            .param("height", T::F32)
            .result("status", T::I32),
        move |c, a, r| {
            let (h, wd, ht) = (a.i64(0)?, a.f32(1)?, a.f32(2)?);
            let st = run(c, &s, |g, w| {
                status_ok(g.lookup(w, "set_size", h, true).map(|(local, _)| {
                    g.push(w, Op::Style { local, change: StyleOp::Size(size_from(wd), size_from(ht)) });
                }))
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        }
    );
    for (name, doc) in [
        ("set_padding", "Set the padding of `widget` (all sides) in logical pixels."),
        ("set_gap", "Set the gap between the children of `widget` in logical pixels."),
        ("set_font_size", "Set the font size of `widget` (inherited by its children) in logical pixels."),
    ] {
        let s = shared.clone();
        let function: &'static str = name;
        reg!(HostFn::new(NS, name).doc(doc).param("widget", T::I64).param("px", T::F32).result("status", T::I32), move |c, a, r| {
            let (h, px) = (a.i64(0)?, a.f32(1)?);
            let st = run(c, &s, |g, w| {
                status_ok(g.lookup(w, function, h, true).map(|(local, _)| {
                    let px = if px.is_finite() { px.clamp(0.0, 4096.0) } else { 0.0 };
                    let change = match function {
                        "set_padding" => StyleOp::Padding(px),
                        "set_gap" => StyleOp::Gap(px),
                        _ => StyleOp::FontSize(px),
                    };
                    g.push(w, Op::Style { local, change });
                }))
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        });
    }
    for (name, doc) in [
        ("set_background", "Set the background colour of `widget` as 0xRRGGBBAA."),
        ("set_text_color", "Set the text colour of `widget` (inherited by its children) as 0xRRGGBBAA."),
    ] {
        let s = shared.clone();
        let function: &'static str = name;
        reg!(HostFn::new(NS, name).doc(doc).param("widget", T::I64).param("rgba", T::I32).result("status", T::I32), move |c, a, r| {
            let (h, rgba) = (a.i64(0)?, a.i32(1)? as u32);
            let st = run(c, &s, |g, w| {
                status_ok(g.lookup(w, function, h, true).map(|(local, _)| {
                    let col = color_from(rgba);
                    let change = if function == "set_background" { StyleOp::Background(col) } else { StyleOp::TextColor(col) };
                    g.push(w, Op::Style { local, change });
                }))
            })?;
            r.set_i32(0, st)?;
            Ok(HostOk::Continue)
        });
    }
    let s = shared.clone();
    reg!(
        HostFn::new(NS, "node_count")
            .doc("How many widgets (layers included) the calling module has right now, counting ones created this frame.")
            .result("count", T::I32),
        move |c, _a, r| {
            let n = run(c, &s, |g, w| g.state(w).owned())?;
            r.set_i32(0, n.min(i32::MAX as usize) as i32)?;
            Ok(HostOk::Continue)
        }
    );

    Ok(UiScripts { shared })
}

/// Systems of the script side: signals to script events (`INPUT + 12`) and the apply step
/// (`LATE + 15`).
pub(crate) fn add_systems(engine: &mut Engine, scripts: UiScripts) {
    engine.insert_resource(scripts.clone());
    let s = scripts.clone();
    engine
        .add_system(Stage::PreUpdate, move |cx| {
            let signals: Vec<UiSignal> = cx.events::<UiSignal>().iter().cloned().collect();
            if signals.is_empty() {
                return;
            }
            let events = s.deliver(&signals);
            if !events.is_empty() {
                cx.send_batch_now(events);
            }
        })
        .order(order::INPUT + 12)
        .name("ui_script_signals");
    engine
        .add_system(Stage::PostUpdate, move |cx| {
            let Some(tree) = cx.resources.get_mut::<UiTree>() else { return };
            scripts.apply(tree, cx.scripting);
        })
        .order(order::LATE + 15)
        .name("ui_scripts_apply");
}
