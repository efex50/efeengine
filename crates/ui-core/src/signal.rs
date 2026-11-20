//! Signals: what the user did (API.md section 7).

use std::sync::Arc;

use crate::id::{Owner, WidgetId};
use crate::node::NodeKind;
use crate::tree::UiTree;

#[derive(Clone, PartialEq, Debug)]
pub enum UiValue {
    Bool(bool),
    F32(f32),
    Text(String),
}

impl UiValue {
    pub fn as_bool(&self) -> Option<bool> {
        if let UiValue::Bool(b) = self { Some(*b) } else { None }
    }

    pub fn as_f32(&self) -> Option<f32> {
        if let UiValue::F32(v) = self { Some(*v) } else { None }
    }

    pub fn as_text(&self) -> Option<&str> {
        if let UiValue::Text(t) = self { Some(t) } else { None }
    }
}

impl From<bool> for UiValue {
    fn from(v: bool) -> Self {
        UiValue::Bool(v)
    }
}

impl From<f32> for UiValue {
    fn from(v: f32) -> Self {
        UiValue::F32(v)
    }
}

impl From<String> for UiValue {
    fn from(v: String) -> Self {
        UiValue::Text(v)
    }
}

impl From<&str> for UiValue {
    fn from(v: &str) -> Self {
        UiValue::Text(v.to_string())
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum UiSignalKind {
    /// Button: press and release inside.
    Click,
    /// Checkbox (`Bool`), Slider (`F32`), TextInput (`Text`), on every edit.
    Changed(UiValue),
    /// TextInput: Enter.
    Submit(String),
    /// Gained or lost keyboard focus.
    Focus(bool),
    /// Pointer entered or left (only nodes that opted in with `.hover_signals()`).
    Hover(bool),
    /// GpuView with `pointer`: the pointer moved over the view, a button changed, or the wheel
    /// turned. `pos` is in logical px relative to the view's top-left corner, `buttons` the
    /// pressed buttons ([`PointerEvent::LEFT`], ...), `wheel` the scroll delta of this event
    /// (zero for other events).
    Pointer { pos: (f32, f32), buttons: u8, wheel: (f32, f32) },
}

/// The data of a [`UiSignalKind::Pointer`] signal, as the view layer's `on_pointer` gets it.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PointerEvent {
    pub pos: (f32, f32),
    pub buttons: u8,
    pub wheel: (f32, f32),
}

impl PointerEvent {
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const MIDDLE: u8 = 4;

    pub fn left(&self) -> bool {
        self.buttons & Self::LEFT != 0
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct UiSignal {
    pub widget: WidgetId,
    /// Copied from the node, so handlers can match on keys.
    pub key: Option<Arc<str>>,
    pub owner: Owner,
    pub kind: UiSignalKind,
}

impl UiSignal {
    /// A signal for `widget` with key and owner read from the tree (a stale id gets none).
    pub fn new(tree: &UiTree, widget: WidgetId, kind: UiSignalKind) -> Self {
        let (key, owner) = match tree.get(widget) {
            Some(n) => (n.key.clone(), n.owner.clone()),
            None => (None, Owner::Game),
        };
        Self { widget, key, owner, kind }
    }

    /// `(key, kind)` with `""` for a node without a key, for `match` arms.
    pub fn key_kind(&self) -> (&str, &UiSignalKind) {
        (self.key.as_deref().unwrap_or(""), &self.kind)
    }
}

/// Validates the signals a backend produced against the tree and writes value changes back.
///
/// Dropped: stale widgets, widgets that are not shown or not enabled (own flag or an
/// ancestor's), widgets on a layer below a visible modal layer (lower `z`; API.md section 3),
/// kinds that do not fit the node (`Click` on a non-button, `Changed` with the
/// wrong value type, ...), `Hover` for nodes without `.hover_signals()`, `Pointer` for anything but a `GpuView` with `.pointer_signals()`. Kept `Changed` values are
/// sanitized (slider clamped, text cut to `max_len`), written into the tree WITHOUT a journal
/// entry (the backend already shows them), and the signal carries the sanitized value. Key and
/// owner are filled from the node.
pub fn filter_signals(tree: &mut UiTree, raw: Vec<UiSignal>) -> Vec<UiSignal> {
    let mut out = Vec::with_capacity(raw.len());
    for mut s in raw {
        let id = s.widget;
        let Some(node) = tree.get(id) else { continue };
        // A blur must always get through (the widget may have been disabled or hidden while it
        // had focus); everything else needs a shown, enabled widget that no modal layer covers.
        let blur = matches!(s.kind, UiSignalKind::Focus(false));
        if !blur && (!tree.is_shown(id) || !tree.is_enabled(id) || tree.is_below_modal(id)) {
            continue;
        }
        let kind = match s.kind {
            UiSignalKind::Click if matches!(node.kind, NodeKind::Button { .. }) => UiSignalKind::Click,
            UiSignalKind::Changed(v) => match tree.write_back(id, v) {
                Some(v) => UiSignalKind::Changed(v),
                None => continue,
            },
            UiSignalKind::Submit(t) if matches!(node.kind, NodeKind::TextInput { .. }) => UiSignalKind::Submit(t),
            UiSignalKind::Focus(f) => UiSignalKind::Focus(f),
            UiSignalKind::Hover(h) if node.hover_signals => UiSignalKind::Hover(h),
            k @ UiSignalKind::Pointer { .. } if matches!(node.kind, NodeKind::GpuView { pointer: true, .. }) => k,
            _ => continue,
        };
        let node = tree.get(id).expect("checked above");
        s.key = node.key.clone();
        s.owner = node.owner.clone();
        s.kind = kind;
        out.push(s);
    }
    out
}
