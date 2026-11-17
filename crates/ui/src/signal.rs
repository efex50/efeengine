//! Signals: what the user did (API.md section 7).

use std::sync::Arc;

use efeengine_core::engine::events::Events;
use efeengine_core::prelude::{Ctx, ParCtx};

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

/// Signals readable this frame. The UI plugin refills it at `order::INPUT + 10` in `PreUpdate`,
/// so every later system of the same frame sees the signals of this frame's input (core's
/// `Events<T>` is only readable a frame after `send`).
#[derive(Default, Debug)]
pub struct UiSignals {
    events: Events<UiSignal>,
}

impl UiSignals {
    pub fn iter(&self) -> std::slice::Iter<'_, UiSignal> {
        self.events.iter()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Replaces the readable signals with `signals`.
    pub fn set(&mut self, signals: Vec<UiSignal>) {
        self.events.clear();
        self.events.extend_readable(signals);
    }
}

impl<'a> IntoIterator for &'a UiSignals {
    type Item = &'a UiSignal;
    type IntoIter = std::slice::Iter<'a, UiSignal>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
    }
}

/// `cx.ui_signals()` for systems. Returns a copy so the handler can use `cx` mutably while it
/// loops (signals are few).
pub trait UiCtxExt {
    fn ui_signals(&self) -> Vec<UiSignal>;
}

impl UiCtxExt for Ctx<'_> {
    fn ui_signals(&self) -> Vec<UiSignal> {
        self.resources.get::<UiSignals>().map(|s| s.iter().cloned().collect()).unwrap_or_default()
    }
}

/// Needs `Access::res::<UiSignals>()`.
impl UiCtxExt for ParCtx<'_> {
    fn ui_signals(&self) -> Vec<UiSignal> {
        self.res::<UiSignals>().iter().cloned().collect()
    }
}

/// Validates the signals a backend produced against the tree and writes value changes back.
///
/// Dropped: stale widgets, widgets that are not shown or not enabled (own flag or an
/// ancestor's), kinds that do not fit the node (`Click` on a non-button, `Changed` with the
/// wrong value type, ...), `Hover` for nodes without `.hover_signals()`. Kept `Changed` values are
/// sanitized (slider clamped, text cut to `max_len`), written into the tree WITHOUT a journal
/// entry (the backend already shows them), and the signal carries the sanitized value. Key and
/// owner are filled from the node.
pub fn filter_signals(tree: &mut UiTree, raw: Vec<UiSignal>) -> Vec<UiSignal> {
    let mut out = Vec::with_capacity(raw.len());
    for mut s in raw {
        let id = s.widget;
        let Some(node) = tree.get(id) else { continue };
        if !tree.is_shown(id) || !tree.is_enabled(id) {
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
