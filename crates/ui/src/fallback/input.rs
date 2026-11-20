//! Interaction: hit testing, hover / press / focus, the widget state machines (button,
//! checkbox, slider, text input, scroll) and the signals they produce.
//!
//! Positions arrive in physical pixels and are converted to logical ones (the layout's unit).
//! Hit testing walks the last layout: layers from the top (a modal layer ends the walk),
//! children in reverse tree order, `Scroll` nodes clip and shift their children. The verdict
//! (`Filter`) follows API.md section 10: pointer events are captured while the pointer is over a
//! blocking node or a modal layer is visible, keyboard events while a text input has focus (or
//! for keys the UI used: Tab, Enter / Space on a focused button, ...).

use std::collections::{HashMap, HashSet};

use efeengine_core::input::{
    ElementState, InputEvent, InputEventKind, Key, KeyCode, Modifiers, MouseButton, NamedKey, TouchPhase, WHEEL_LINES_PER_PAGE, WHEEL_PIXELS_PER_LINE, WheelUnit,
    WindowEvent,
};
use efeengine_core::input::Filter;
use efeengine_core::window::CursorIcon;

use super::FallbackBackend;
use super::edit::{EditOp, offset_at};
use super::model::{LRect, Model, VNode};
use super::paint::{padding, slider_track, thumb_rect};
use crate::backend::TextInputArea;
use crate::id::{Owner, WidgetId};
use crate::node::NodeKind;
use crate::signal::{UiSignal, UiSignalKind, UiValue};
use crate::style::Direction;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Drag {
    Slider(WidgetId),
    /// Scrollbar thumb: the node and where inside the thumb the pointer grabbed it.
    Thumb(WidgetId, f32),
    Select(WidgetId),
}

#[derive(Default)]
pub struct Interaction {
    /// Pointer position, logical px.
    pub pointer: Option<(f32, f32)>,
    /// The interactive widget under the pointer (enabled ones).
    pub hovered: Option<WidgetId>,
    /// The topmost interactive widget under the pointer, enabled or not.
    pub target: Option<WidgetId>,
    /// The pointer is over a blocking node.
    pub over: bool,
    path: Vec<WidgetId>,
    hover_set: HashSet<WidgetId>,
    pub pressed: Option<WidgetId>,
    pub focus: Option<WidgetId>,
    /// Focus came from the keyboard (draw the focus ring on buttons and checkboxes).
    pub focus_visible: bool,
    pub drag: Option<Drag>,
    pub scroll: HashMap<WidgetId, (f32, f32)>,
    /// Scroll nodes with `follow_end` as of the last sync.
    pub following: HashSet<WidgetId>,
    /// Followers that are at the end and stick to it as content grows (a user scroll away from
    /// the end removes the node until it is scrolled back to the end).
    pub pinned: HashSet<WidgetId>,
    pub mods: Modifiers,
    buttons: u8,
    /// Physical keys whose press the UI used (their release is used too).
    held: HashSet<KeyCode>,
}

impl Interaction {
    pub fn retain(&mut self, m: &Model) {
        let alive = |id: &WidgetId| m.find(*id).is_some();
        self.hover_set.retain(&alive);
        self.scroll.retain(|id, _| alive(id));
        self.following.retain(&alive);
        self.pinned.retain(&alive);
        if self.pressed.is_some_and(|i| !alive(&i)) {
            self.pressed = None;
        }
        if self.hovered.is_some_and(|i| !alive(&i)) {
            self.hovered = None;
        }
        if self.target.is_some_and(|i| !alive(&i)) {
            self.target = None;
        }
        if self.drag.is_some_and(|d| !alive(&drag_id(d))) {
            self.drag = None;
        }
        if self.focus.is_some_and(|i| !alive(&i)) {
            self.focus = None;
        }
    }

    /// Follows the end of every pinned scroll node (content may have grown or the node resized).
    pub fn clamp_scroll(&mut self, m: &Model) {
        for id in &self.pinned {
            self.scroll.entry(*id).or_default();
        }
        for (id, off) in self.scroll.iter_mut() {
            if let Some(n) = m.find(*id) {
                off.0 = off.0.clamp(0.0, (n.content.0 - n.rect.w).max(0.0));
                off.1 = off.1.clamp(0.0, (n.content.1 - n.rect.h).max(0.0));
                if self.pinned.contains(id) {
                    off.1 = (n.content.1 - n.rect.h).max(0.0);
                }
            }
        }
    }
}

fn drag_id(d: Drag) -> WidgetId {
    match d {
        Drag::Slider(i) | Drag::Thumb(i, _) | Drag::Select(i) => i,
    }
}

fn interactive(n: &VNode) -> bool {
    matches!(n.kind, NodeKind::Button { .. } | NodeKind::Checkbox { .. } | NodeKind::Slider { .. } | NodeKind::TextInput { .. } | NodeKind::GpuView { pointer: true, .. })
}

/// The chain of nodes (outer to inner) under `p` on the topmost branch; empty when none.
fn hit_node(n: &VNode, origin: (f32, f32), clip: Option<LRect>, scroll: &HashMap<WidgetId, (f32, f32)>, p: (f32, f32), path: &mut Vec<WidgetId>) -> bool {
    let r = n.rect.offset(origin.0, origin.1);
    let vis = clip.map_or(r, |c| c.intersect(&r));
    let is_scroll = matches!(n.kind, NodeKind::Scroll { .. });
    if is_scroll && !vis.contains(p) {
        return false;
    }
    let (child_origin, child_clip) = if is_scroll {
        let off = scroll.get(&n.id).copied().unwrap_or((0.0, 0.0));
        ((r.x - off.0, r.y - off.1), Some(vis))
    } else {
        ((r.x, r.y), clip)
    };
    path.push(n.id);
    for c in n.children.iter().rev() {
        if hit_node(c, child_origin, child_clip, scroll, p, path) {
            return true;
        }
    }
    if n.blocking && vis.contains(p) {
        return true;
    }
    path.pop();
    false
}

fn abs_rect_in(n: &VNode, origin: (f32, f32), scroll: &HashMap<WidgetId, (f32, f32)>, id: WidgetId) -> Option<LRect> {
    let r = n.rect.offset(origin.0, origin.1);
    if n.id == id {
        return Some(r);
    }
    let off = if matches!(n.kind, NodeKind::Scroll { .. }) { scroll.get(&n.id).copied().unwrap_or((0.0, 0.0)) } else { (0.0, 0.0) };
    n.children.iter().find_map(|c| abs_rect_in(c, (r.x - off.0, r.y - off.1), scroll, id))
}

/// Ancestors of `id` (outer to inner, not including it) and the node.
fn chain<'a>(n: &'a VNode, id: WidgetId, out: &mut Vec<&'a VNode>) -> bool {
    if n.id == id {
        return true;
    }
    out.push(n);
    if n.children.iter().any(|c| chain(c, id, out)) {
        return true;
    }
    out.pop();
    false
}

impl FallbackBackend {
    fn push(&mut self, widget: WidgetId, kind: UiSignalKind) {
        self.signals.push(UiSignal { widget, key: None, owner: Owner::Game, kind });
    }

    pub(super) fn abs_rect(&self, id: WidgetId) -> Option<LRect> {
        self.model.layers.iter().find_map(|l| abs_rect_in(&l.root, (0.0, 0.0), &self.ui.scroll, id))
    }

    fn logical(&self, p: (f32, f32)) -> (f32, f32) {
        (p.0 / self.scale, p.1 / self.scale)
    }

    /// Recomputes what is under the pointer; sends hover signals; updates the cursor icon.
    pub(super) fn refresh_hover(&mut self) {
        let mut path = Vec::new();
        if let Some(p) = self.ui.pointer {
            for l in self.model.layers.iter().rev() {
                path.clear();
                if hit_node(&l.root, (0.0, 0.0), None, &self.ui.scroll, p, &mut path) {
                    break;
                }
                path.clear();
                if l.layer.modal {
                    break;
                }
            }
        }
        let over = !path.is_empty();
        let target = path.iter().rev().copied().find(|id| self.model.find(*id).is_some_and(interactive));
        let hovered = target.filter(|id| self.model.find(*id).is_some_and(|n| n.enabled));
        let now: HashSet<WidgetId> = path.iter().copied().filter(|id| self.model.find(*id).is_some_and(|n| n.hover_signals)).collect();
        let entered: Vec<_> = now.difference(&self.ui.hover_set).copied().collect();
        let left: Vec<_> = self.ui.hover_set.difference(&now).copied().collect();
        for id in left {
            self.push(id, UiSignalKind::Hover(false));
        }
        for id in entered {
            self.push(id, UiSignalKind::Hover(true));
        }
        if hovered != self.ui.hovered {
            self.dirty = true;
        }
        self.ui.hover_set = now;
        self.ui.hovered = hovered;
        self.ui.target = target;
        self.ui.over = over;
        self.ui.path = path;
        let icon = match hovered.and_then(|id| self.model.find(id)).map(|n| &n.kind) {
            Some(NodeKind::TextInput { .. }) => CursorIcon::Text,
            Some(NodeKind::Button { .. } | NodeKind::Checkbox { .. } | NodeKind::Slider { .. }) => CursorIcon::Pointer,
            _ => CursorIcon::Default,
        };
        self.icon = icon;
        if icon != self.icon_sent {
            self.icon_sent = icon;
            self.icon_request = Some(icon);
        }
    }

    pub(super) fn handle_input(&mut self, ev: &InputEvent) -> Filter {
        if self.model.is_empty() {
            return Filter::Pass;
        }
        self.ensure_layout();
        let modal = self.model.modal();
        let captured = match &ev.kind {
            InputEventKind::MouseMotion { pos, .. } => {
                self.pointer_moved(self.logical(*pos));
                self.ui.over || modal || self.ui.drag.is_some()
            }
            InputEventKind::MouseButton { button, state, pos } => {
                self.pointer_moved(self.logical(*pos));
                let active = self.ui.pressed.is_some() || self.ui.drag.is_some();
                let bit = match button {
                    MouseButton::Left => 1,
                    MouseButton::Right => 2,
                    MouseButton::Middle => 4,
                    _ => 0,
                };
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => self.press(),
                    (MouseButton::Left, ElementState::Released) => self.release(),
                    _ => {}
                }
                match state {
                    ElementState::Pressed => self.ui.buttons |= bit,
                    ElementState::Released => self.ui.buttons &= !bit,
                }
                self.gpu_pointer((0.0, 0.0));
                self.ui.over || modal || active
            }
            InputEventKind::Touch { phase, pos, .. } => {
                let p = self.logical(*pos);
                self.pointer_moved(p);
                let active = self.ui.pressed.is_some() || self.ui.drag.is_some();
                match phase {
                    TouchPhase::Started => self.press(),
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        self.release();
                        self.ui.pointer = None;
                        self.refresh_hover();
                    }
                    TouchPhase::Moved => {}
                }
                self.ui.over || modal || active
            }
            InputEventKind::MouseWheel { delta, unit } => {
                let k = match unit {
                    WheelUnit::Pixel => 1.0 / self.scale,
                    WheelUnit::Line => WHEEL_PIXELS_PER_LINE,
                    WheelUnit::Page => WHEEL_PIXELS_PER_LINE * WHEEL_LINES_PER_PAGE,
                };
                let d = (delta.0 * k, delta.1 * k);
                self.wheel(d);
                self.ui.over || modal
            }
            InputEventKind::Key { physical, logical, state, repeat, mods } => self.key(*physical, logical, *state, *repeat, *mods),
            InputEventKind::Text(t) => {
                let id = self.focused_input();
                if let Some(id) = id {
                    self.clear_preedit(id);
                    self.type_text(id, t);
                }
                id.is_some() || modal
            }
            InputEventKind::TextEditing { text, .. } => {
                let id = self.focused_input();
                if let Some(id) = id
                    && let Some(s) = self.inputs.get_mut(&id)
                {
                    s.preedit.clone_from(text);
                    self.dirty = true;
                }
                id.is_some() || modal
            }
            InputEventKind::Window(w) => {
                match w {
                    WindowEvent::Focused(false) => {
                        self.ui.pressed = None;
                        self.ui.drag = None;
                        self.dirty = true;
                    }
                    WindowEvent::Resized { .. } | WindowEvent::ScaleFactorChanged { .. } => {}
                    _ => {}
                }
                false
            }
        };
        if captured { Filter::Captured } else { Filter::Pass }
    }

    pub(super) fn focused_input(&self) -> Option<WidgetId> {
        let id = self.ui.focus?;
        self.model.find(id).filter(|n| n.enabled && matches!(n.kind, NodeKind::TextInput { .. })).map(|n| n.id)
    }

    fn pointer_moved(&mut self, p: (f32, f32)) {
        self.ui.pointer = Some(p);
        self.refresh_hover();
        match self.ui.drag {
            Some(Drag::Slider(id)) => self.slide_to(id, p.0),
            Some(Drag::Thumb(id, grab)) => self.drag_thumb(id, p.1, grab),
            Some(Drag::Select(id)) => self.place_caret(id, p, true),
            None => {}
        }
        self.gpu_pointer((0.0, 0.0));
    }

    /// `Pointer` signals for a `GpuView` that asked for them.
    fn gpu_pointer(&mut self, wheel: (f32, f32)) {
        let (Some(id), Some(p)) = (self.ui.target, self.ui.pointer) else { return };
        let Some(n) = self.model.find(id) else { return };
        if !n.enabled || !matches!(n.kind, NodeKind::GpuView { pointer: true, .. }) {
            return;
        }
        let Some(r) = self.abs_rect(id) else { return };
        let (buttons, pos) = (self.ui.buttons, (p.0 - r.x, p.1 - r.y));
        self.push(id, UiSignalKind::Pointer { pos, buttons, wheel });
    }

    fn press(&mut self) {
        let Some(p) = self.ui.pointer else { return };
        // Scrollbar thumbs first.
        let path = self.ui.path.clone();
        for sid in path.iter().rev() {
            let Some(n) = self.model.find(*sid).filter(|n| matches!(n.kind, NodeKind::Scroll { .. })) else { continue };
            let content_h = n.content.1;
            let Some(r) = self.abs_rect(*sid) else { continue };
            let off = self.ui.scroll.get(sid).copied().unwrap_or((0.0, 0.0));
            if let Some(t) = thumb_rect(r, content_h, off.1) {
                let hot = LRect { x: t.x - 6.0, w: t.w + 12.0, ..t };
                if hot.contains(p) {
                    self.ui.drag = Some(Drag::Thumb(*sid, p.1 - t.y));
                    self.ui.pressed = Some(*sid);
                    self.dirty = true;
                    return;
                }
            }
        }
        let target = self.ui.target;
        let kind_enabled = target.and_then(|id| self.model.find(id)).map(|n| (n.enabled, matches!(n.kind, NodeKind::Button { .. }), matches!(n.kind, NodeKind::Checkbox { .. }), matches!(n.kind, NodeKind::Slider { .. }), matches!(n.kind, NodeKind::TextInput { .. })));
        match (target, kind_enabled) {
            (Some(id), Some((true, button, checkbox, slider, input))) if button || checkbox || slider || input => {
                if slider || input {
                    self.set_focus(Some(id), false);
                } else if self.ui.focus != Some(id) {
                    self.set_focus(None, false);
                }
                self.ui.pressed = Some(id);
                if slider {
                    self.ui.drag = Some(Drag::Slider(id));
                    self.slide_to(id, p.0);
                } else if input {
                    self.ui.drag = Some(Drag::Select(id));
                    self.place_caret(id, p, self.ui.mods.shift());
                }
                self.dirty = true;
            }
            (Some(_), Some((true, ..))) => self.gpu_pointer((0.0, 0.0)),
            _ => {
                if self.ui.focus.is_some() && self.ui.focus != target {
                    self.set_focus(None, false);
                }
            }
        }
    }

    fn release(&mut self) {
        let pressed = self.ui.pressed.take();
        let drag = self.ui.drag.take();
        if let Some(Drag::Select(id)) = drag
            && let Some(s) = self.inputs.get_mut(&id)
        {
            s.selecting = false;
        }
        if let Some(id) = pressed {
            self.dirty = true;
            if self.ui.target == Some(id) && self.model.find(id).is_some_and(|n| n.enabled) {
                match self.model.find(id).map(|n| &n.kind) {
                    Some(NodeKind::Button { .. }) => self.push(id, UiSignalKind::Click),
                    Some(NodeKind::Checkbox { .. }) => self.toggle(id),
                    _ => {}
                }
            }
        }
    }

    /// The user moved a scroll node: stop or resume sticking to its end, and report it.
    fn user_scrolled(&mut self, id: WidgetId) {
        let Some(n) = self.model.find(id) else { return };
        let max = (n.content.1 - n.rect.h).max(0.0);
        let at_end = max - self.ui.scroll.get(&id).map_or(0.0, |o| o.1) <= 1.0;
        if !at_end {
            self.ui.pinned.remove(&id);
        } else if self.ui.following.contains(&id) {
            self.ui.pinned.insert(id);
        }
        self.push(id, UiSignalKind::Scrolled { at_end });
    }

    fn toggle(&mut self, id: WidgetId) {
        let Some(n) = self.model.find_mut(id).filter(|n| n.enabled) else { return };
        if let NodeKind::Checkbox { checked, .. } = &mut n.kind {
            *checked = !*checked;
            let v = *checked;
            self.dirty = true;
            self.push(id, UiSignalKind::Changed(UiValue::Bool(v)));
        }
    }

    fn slide_to(&mut self, id: WidgetId, x: f32) {
        let Some(r) = self.abs_rect(id) else { return };
        let (x0, x1, _) = slider_track(r);
        let t = if x1 > x0 { ((x - x0) / (x1 - x0)).clamp(0.0, 1.0) } else { 0.0 };
        self.set_slider(id, |min, max, _| min + t * (max - min));
    }

    /// Sets the slider value from `f(min, max, value)`, snapped to the step and clamped.
    fn set_slider(&mut self, id: WidgetId, f: impl FnOnce(f32, f32, f32) -> f32) {
        let Some(n) = self.model.find_mut(id).filter(|n| n.enabled) else { return };
        if let NodeKind::Slider { min, max, step, value } = &mut n.kind {
            let mut v = f(*min, *max, *value);
            if *step > 0.0 {
                v = *min + ((v - *min) / *step).round() * *step;
            }
            let v = v.clamp(*min, *max);
            if v != *value {
                *value = v;
                self.dirty = true;
                self.push(id, UiSignalKind::Changed(UiValue::F32(v)));
            }
        }
    }

    fn drag_thumb(&mut self, id: WidgetId, y: f32, grab: f32) {
        let Some(n) = self.model.find(id) else { return };
        let (content_h, h) = (n.content.1, n.rect.h);
        let Some(r) = self.abs_rect(id) else { return };
        let off = self.ui.scroll.get(&id).copied().unwrap_or((0.0, 0.0));
        let Some(t) = thumb_rect(r, content_h, off.1) else { return };
        let travel = (r.h - t.h).max(1.0);
        let frac = ((y - grab - r.y) / travel).clamp(0.0, 1.0);
        let e = self.ui.scroll.entry(id).or_default();
        let before = e.1;
        e.1 = frac * (content_h - h).max(0.0);
        if e.1 != before {
            self.user_scrolled(id);
        }
        self.dirty = true;
        self.refresh_hover();
    }

    fn wheel(&mut self, d: (f32, f32)) {
        if self.ui.pointer.is_none() {
            return;
        }
        self.gpu_pointer(d);
        let Some(sid) = self.ui.path.iter().rev().copied().find(|id| self.model.find(*id).is_some_and(|n| matches!(n.kind, NodeKind::Scroll { .. }))) else { return };
        let Some(n) = self.model.find(sid) else { return };
        let row = n.style().direction == Some(Direction::Row);
        let (max_x, max_y) = ((n.content.0 - n.rect.w).max(0.0), (n.content.1 - n.rect.h).max(0.0));
        let e = self.ui.scroll.entry(sid).or_default();
        let before = *e;
        if row {
            let dd = if d.0 != 0.0 { d.0 } else { -d.1 };
            e.0 = (e.0 + dd).clamp(0.0, max_x);
        } else {
            // wheel up (+y) scrolls towards the start
            e.1 = (e.1 - d.1).clamp(0.0, max_y);
            if d.0 != 0.0 {
                e.0 = (e.0 + d.0).clamp(0.0, max_x);
            }
        }
        let moved = *e != before;
        if moved {
            self.user_scrolled(sid);
        }
        self.dirty = true;
        self.refresh_hover();
    }

    // ---- focus ----

    pub(super) fn set_focus(&mut self, now: Option<WidgetId>, visible: bool) {
        if now == self.ui.focus {
            if self.ui.focus_visible != visible && now.is_some() {
                self.ui.focus_visible = visible;
                self.dirty = true;
            }
            return;
        }
        if let Some(old) = self.ui.focus {
            if let Some(s) = self.inputs.get_mut(&old) {
                s.preedit.clear();
                s.selecting = false;
            }
            self.push(old, UiSignalKind::Focus(false));
        }
        if let Some(new) = now {
            self.push(new, UiSignalKind::Focus(true));
        }
        self.ui.focus = now;
        self.ui.focus_visible = visible && now.is_some();
        self.blink_epoch = std::time::Instant::now();
        self.dirty = true;
        if let Some(id) = now
            && visible
        {
            self.scroll_into_view(id);
        }
        self.update_ime_area();
    }

    /// Tells the plugin where the focused text input is (or that none is).
    pub(super) fn update_ime_area(&mut self) {
        let want = self.focused_input().and_then(|id| {
            let r = self.abs_rect(id)?;
            let pad = padding(self.model.find(id)?.style());
            Some(TextInputArea {
                x: (r.x + pad.left) as i32,
                y: (r.y + pad.top) as i32,
                w: (r.w - pad.left - pad.right).max(0.0).ceil() as i32,
                h: (r.h - pad.top - pad.bottom).max(0.0).ceil() as i32,
                cursor: 0,
            })
        });
        if want != self.text_sent {
            self.text_sent = want;
            self.text_request = Some(want);
        }
    }

    pub(super) fn after_focus_change(&mut self) {
        self.update_ime_area();
    }

    fn scroll_into_view(&mut self, id: WidgetId) {
        let mut ancestors = Vec::new();
        let found = self.model.layers.iter().any(|l| {
            ancestors.clear();
            chain(&l.root, id, &mut ancestors)
        });
        if !found {
            return;
        }
        let Some(target) = self.model.find(id) else { return };
        let (tw, th) = (target.rect.w, target.rect.h);
        // Position of the target relative to each scroll ancestor's content origin.
        let mut moves: Vec<(WidgetId, (f32, f32), (f32, f32))> = Vec::new();
        for (i, a) in ancestors.iter().enumerate() {
            if !matches!(a.kind, NodeKind::Scroll { .. }) {
                continue;
            }
            let (mut rx, mut ry) = (target.rect.x, target.rect.y);
            for b in &ancestors[i + 1..] {
                rx += b.rect.x;
                ry += b.rect.y;
            }
            moves.push((a.id, (rx, ry), (a.rect.w, a.rect.h)));
        }
        for (sid, (rx, ry), (sw, sh)) in moves {
            let e = self.ui.scroll.entry(sid).or_default();
            if ry < e.1 {
                e.1 = ry;
            } else if ry + th > e.1 + sh {
                e.1 = ry + th - sh;
            }
            if rx < e.0 {
                e.0 = rx;
            } else if rx + tw > e.0 + sw {
                e.0 = rx + tw - sw;
            }
        }
        self.ui.clamp_scroll(&self.model);
        self.dirty = true;
    }

    // ---- keyboard ----

    fn key(&mut self, physical: KeyCode, logical: &Key, state: ElementState, repeat: bool, mods: Modifiers) -> bool {
        self.ui.mods = mods;
        let modal = self.model.modal();
        if state == ElementState::Released {
            return self.ui.held.remove(&physical) || modal;
        }
        let used = self.key_pressed(logical, repeat, mods);
        if used {
            self.ui.held.insert(physical);
        }
        used || modal
    }

    fn key_pressed(&mut self, logical: &Key, repeat: bool, mods: Modifiers) -> bool {
        let focus = self.ui.focus.filter(|id| self.model.find(*id).is_some());
        if let Key::Named(NamedKey::Tab) = logical {
            let list = self.model.focusable();
            if list.is_empty() {
                return false;
            }
            let pos = focus.and_then(|f| list.iter().position(|i| *i == f));
            let next = match (pos, mods.shift()) {
                (None, false) => 0,
                (None, true) => list.len() - 1,
                (Some(i), false) => (i + 1) % list.len(),
                (Some(i), true) => (i + list.len() - 1) % list.len(),
            };
            self.set_focus(Some(list[next]), true);
            return true;
        }
        if let Key::Named(NamedKey::Escape) = logical
            && focus.is_some()
        {
            self.set_focus(None, false);
            return true;
        }
        let Some(id) = focus else { return false };
        let Some(n) = self.model.find(id) else { return false };
        if !n.enabled {
            return false;
        }
        let activate = matches!(logical, Key::Named(NamedKey::Enter | NamedKey::Space)) || matches!(logical, Key::Character(c) if c == " ");
        match &n.kind {
            NodeKind::Button { .. } => {
                if activate {
                    if !repeat {
                        self.push(id, UiSignalKind::Click);
                    }
                    return true;
                }
                false
            }
            NodeKind::Checkbox { .. } => {
                if activate {
                    if !repeat {
                        self.toggle(id);
                    }
                    return true;
                }
                false
            }
            NodeKind::Slider { min, max, step, .. } => {
                let st = if *step > 0.0 { *step } else { (max - min) / 100.0 };
                let (lo, hi) = (*min, *max);
                match logical {
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => self.set_slider(id, |_, _, v| v - st),
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => self.set_slider(id, |_, _, v| v + st),
                    Key::Named(NamedKey::Home) => self.set_slider(id, |_, _, _| lo),
                    Key::Named(NamedKey::End) => self.set_slider(id, |_, _, _| hi),
                    _ => return false,
                }
                true
            }
            NodeKind::TextInput { .. } => self.text_key(id, logical, mods),
            _ => false,
        }
    }

    // ---- text input ----

    fn input_meta(&self, id: WidgetId) -> Option<(u32, bool)> {
        match &self.model.find(id)?.kind {
            NodeKind::TextInput { max_len, password, .. } => Some((*max_len, *password)),
            _ => None,
        }
    }

    /// `(read_only, multiline)` of a text input.
    fn input_flags(&self, id: WidgetId) -> (bool, bool) {
        match self.model.find(id).map(|n| &n.kind) {
            Some(NodeKind::TextInput { read_only, multiline, .. }) => (*read_only, *read_only && *multiline),
            _ => (false, false),
        }
    }

    fn clear_preedit(&mut self, id: WidgetId) {
        if let Some(s) = self.inputs.get_mut(&id) {
            s.preedit.clear();
        }
    }

    /// Applies the editor's text to the model and reports it.
    fn commit_input(&mut self, id: WidgetId) {
        let Some(s) = self.inputs.get_mut(&id) else { return };
        let text = s.text();
        s.value.clone_from(&text);
        let Some(n) = self.model.find_mut(id) else { return };
        if let NodeKind::TextInput { value, .. } = &mut n.kind
            && *value != text
        {
            value.clone_from(&text);
            self.push(id, UiSignalKind::Changed(UiValue::Text(text)));
        }
        self.blink_epoch = std::time::Instant::now();
        self.dirty = true;
    }

    fn state_for(&mut self, id: WidgetId) -> Option<&mut super::edit::InputState> {
        if !self.inputs.contains_key(&id) {
            let n = self.model.find(id)?;
            let NodeKind::TextInput { value, .. } = &n.kind else { return None };
            let spec = super::layout::text_spec(n.style(), value, false);
            let s = super::edit::InputState::new(&mut self.text, &spec, value);
            self.inputs.insert(id, s);
        }
        self.inputs.get_mut(&id)
    }

    fn type_text(&mut self, id: WidgetId, t: &str) {
        let Some((max_len, _)) = self.input_meta(id) else { return };
        if self.input_flags(id).0 {
            return;
        }
        if self.state_for(id).is_none() {
            return;
        }
        let (text, inputs) = (&mut self.text, &mut self.inputs);
        if inputs.get_mut(&id).is_some_and(|s| s.insert(text, t, max_len)) {
            self.commit_input(id);
        }
    }

    fn text_key(&mut self, id: WidgetId, logical: &Key, mods: Modifiers) -> bool {
        let Some((max_len, _)) = self.input_meta(id) else { return false };
        if self.state_for(id).is_none() {
            return false;
        }
        let (ctrl, shift) = (mods.ctrl() || mods.meta(), mods.shift());
        let (read_only, multiline) = self.input_flags(id);
        self.clear_preedit(id);
        if read_only {
            return self.read_only_key(id, logical, ctrl, multiline);
        }
        let op = match logical {
            Key::Named(NamedKey::ArrowLeft) => Some((if ctrl { EditOp::WordLeft } else { EditOp::Left }, shift)),
            Key::Named(NamedKey::ArrowRight) => Some((if ctrl { EditOp::WordRight } else { EditOp::Right }, shift)),
            Key::Named(NamedKey::Home) => Some((EditOp::Home, shift)),
            Key::Named(NamedKey::End) => Some((EditOp::End, shift)),
            Key::Named(NamedKey::Backspace) => Some((if ctrl { EditOp::BackspaceWord } else { EditOp::Backspace }, false)),
            Key::Named(NamedKey::Delete) => Some((if ctrl { EditOp::DeleteWord } else { EditOp::Delete }, false)),
            Key::Character(c) if ctrl && c == "a" => Some((EditOp::SelectAll, false)),
            _ => None,
        };
        if let Some((op, select)) = op {
            let changed = {
                let (text, inputs) = (&mut self.text, &mut self.inputs);
                inputs.get_mut(&id).is_some_and(|s| s.apply(text, op, select))
            };
            if changed {
                self.commit_input(id);
            }
            self.blink_epoch = std::time::Instant::now();
            self.dirty = true;
            return true;
        }
        match logical {
            Key::Named(NamedKey::Enter) => {
                let v = self.inputs.get(&id).map(|s| s.text()).unwrap_or_default();
                self.push(id, UiSignalKind::Submit(v));
                true
            }
            Key::Character(c) if ctrl && (c == "c" || c == "x") => {
                let sel = self.inputs.get(&id).and_then(|s| s.selected_text());
                if let Some(t) = sel {
                    // A password field never puts its text on the clipboard.
                    if !self.input_meta(id).is_some_and(|m| m.1) {
                        let _ = efeengine_core::clipboard::set_clipboard_text(&t);
                    }
                    if c == "x" && self.inputs.get_mut(&id).is_some_and(|s| s.delete_selection()) {
                        self.commit_input(id);
                    }
                }
                true
            }
            Key::Character(c) if ctrl && c == "v" => {
                if let Some(t) = efeengine_core::clipboard::clipboard_text() {
                    self.type_text(id, &t);
                }
                let _ = max_len;
                true
            }
            // Other keys with Ctrl / Alt (shortcuts) are not text; plain characters arrive as `Text`.
            Key::Named(NamedKey::Tab) => false,
            _ => true,
        }
    }

    /// Keys in a read-only input: Ctrl+A selects all, Ctrl+C / Ctrl+X copy the selection; every
    /// edit, Enter (no submit) and, in a wrapped input, the cursor keys do nothing.
    fn read_only_key(&mut self, id: WidgetId, logical: &Key, ctrl: bool, multiline: bool) -> bool {
        match logical {
            Key::Character(c) if ctrl && c == "a" => {
                let (text, inputs) = (&mut self.text, &mut self.inputs);
                if let Some(s) = inputs.get_mut(&id) {
                    s.apply(text, EditOp::SelectAll, false);
                }
            }
            Key::Character(c) if ctrl && (c == "c" || c == "x") => {
                if let Some(t) = self.inputs.get(&id).and_then(|s| s.selected_text()) {
                    let _ = efeengine_core::clipboard::set_clipboard_text(&t);
                }
            }
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::Home | NamedKey::End) if !multiline => {
                let shift = self.ui.mods.shift();
                let op = match logical {
                    Key::Named(NamedKey::ArrowLeft) => EditOp::Left,
                    Key::Named(NamedKey::ArrowRight) => EditOp::Right,
                    Key::Named(NamedKey::Home) => EditOp::Home,
                    _ => EditOp::End,
                };
                let (text, inputs) = (&mut self.text, &mut self.inputs);
                if let Some(s) = inputs.get_mut(&id) {
                    s.apply(text, op, shift);
                }
            }
            Key::Named(NamedKey::Tab) => return false,
            _ => {}
        }
        self.blink_epoch = std::time::Instant::now();
        self.dirty = true;
        true
    }

    /// Wrapped read-only text: the caret goes to the glyph under pointer `p` (the same
    /// press / drag / double click / shift-click rules as `place_caret`).
    fn place_caret_wrapped(&mut self, id: WidgetId, p: (f32, f32), extend: bool) {
        let Some(r) = self.abs_rect(id) else { return };
        let Some(n) = self.model.find(id) else { return };
        let pad = padding(n.style());
        let (ox, oy) = (r.x + pad.left, r.y + pad.top);
        if self.state_for(id).is_none() {
            return;
        }
        let drag = self.ui.drag == Some(Drag::Select(id)) && self.inputs.get(&id).is_some_and(|s| s.selecting);
        let Some(s) = self.inputs.get_mut(&id) else { return };
        // The display buffer is laid out by the painter; before the first frame there is no hit.
        let Some(c) = s.display.buffer.hit(p.0 - ox, p.1 - oy) else { return };
        let now = std::time::Instant::now();
        if !drag && !extend {
            let double = s.last_click.is_some_and(|t| now.duration_since(t).as_millis() < 400);
            s.last_click = Some(now);
            if double {
                s.select_word_at(c);
            } else {
                s.place_cursor_at(c, false);
            }
            s.selecting = true;
        } else {
            s.place_cursor_at(c, true);
            s.selecting = true;
        }
        self.blink_epoch = now;
        self.dirty = true;
    }

    /// Places the caret (and extends the selection with `extend` / double click) at pointer `p`.
    fn place_caret(&mut self, id: WidgetId, p: (f32, f32), extend: bool) {
        let Some((_, password)) = self.input_meta(id) else { return };
        if self.input_flags(id).1 {
            return self.place_caret_wrapped(id, p, extend);
        }
        let Some(r) = self.abs_rect(id) else { return };
        let Some(n) = self.model.find(id) else { return };
        let pad = padding(n.style());
        let inner_x = r.x + pad.left;
        if self.state_for(id).is_none() {
            return;
        }
        let drag = self.ui.drag == Some(Drag::Select(id)) && self.inputs.get(&id).is_some_and(|s| s.selecting);
        let Some(s) = self.inputs.get_mut(&id) else { return };
        let x = p.0 - (inner_x - s.scroll_x);
        let d = offset_at(&s.display.buffer, x);
        let idx = s.real_offset(d, password);
        let now = std::time::Instant::now();
        if !drag && !extend {
            let double = s.last_click.is_some_and(|t| now.duration_since(t).as_millis() < 400);
            s.last_click = Some(now);
            if double {
                s.select_word(idx);
            } else {
                s.place_cursor(idx, false);
            }
            s.selecting = true;
        } else {
            s.place_cursor(idx, true);
            s.selecting = true;
        }
        self.blink_epoch = now;
        self.dirty = true;
    }
}
