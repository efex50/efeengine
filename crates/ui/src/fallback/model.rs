//! The backend's owned copy of the tree: styles resolved against the theme for every
//! interaction state, text properties inherited like CSS, layout results filled in later.
//! Rebuilt from the `UiTree` whenever the journal is not empty (the tree stays the source of
//! truth); interaction state (hover, focus, scroll offsets, editors) lives outside, keyed by
//! `WidgetId`, so it survives rebuilds.

use crate::id::WidgetId;
use crate::node::{Layer, NodeKind};
use crate::style::{Border, Style};
use crate::theme::WidgetState;
use crate::tree::UiTree;

/// Index into [`VNode::styles`].
pub const NORMAL: usize = 0;
pub const HOVERED: usize = 1;
pub const PRESSED: usize = 2;
pub const FOCUSED: usize = 3;
pub const DISABLED: usize = 4;

/// A rectangle in logical px.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl LRect {
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }

    pub fn intersect(&self, o: &LRect) -> LRect {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        LRect { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
    }

    pub fn offset(&self, dx: f32, dy: f32) -> LRect {
        LRect { x: self.x + dx, y: self.y + dy, ..*self }
    }
}

pub struct VNode {
    pub id: WidgetId,
    pub kind: NodeKind,
    /// Resolved styles: normal, hovered (hovered), pressed (hovered + pressed), focused, disabled.
    pub styles: [Style; 5],
    /// Effectively enabled (own flag and ancestors').
    pub enabled: bool,
    /// Takes pointer events (everything inside a layer except a Container or Text without
    /// background and border).
    pub blocking: bool,
    pub hover_signals: bool,
    pub children: Vec<VNode>,
    /// Layout result: position relative to the parent's top-left corner, size; logical px.
    pub rect: LRect,
    /// Size of the children's extent (scrollable content), logical px, relative to the node.
    pub content: (f32, f32),
    pub(crate) tn: Option<taffy::NodeId>,
}

pub struct VLayer {
    pub layer: Layer,
    pub root: VNode,
}

#[derive(Default)]
pub struct Model {
    /// Ordered by z (lowest first, drawn first).
    pub layers: Vec<VLayer>,
}

impl Model {
    /// The layers shown in `window`.
    pub fn build(tree: &UiTree, window: u32) -> Model {
        let mut layers = Vec::new();
        for root in tree.layers_in(window) {
            if !tree.is_shown(root) {
                continue;
            }
            let Some(layer) = tree.get(root).and_then(|n| n.layer().copied()) else { continue };
            if let Some(mut node) = build_node(tree, root, None) {
                if layer.modal {
                    node.blocking = true;
                }
                layers.push(VLayer { layer, root: node });
            }
        }
        Model { layers }
    }

    pub fn find(&self, id: WidgetId) -> Option<&VNode> {
        self.layers.iter().find_map(|l| l.root.find(id))
    }

    pub fn find_mut(&mut self, id: WidgetId) -> Option<&mut VNode> {
        self.layers.iter_mut().find_map(|l| l.root.find_mut(id))
    }

    pub fn modal(&self) -> bool {
        self.layers.iter().any(|l| l.layer.modal)
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Focusable nodes in tree order (layers by z, then depth first): enabled buttons,
    /// checkboxes, sliders and text inputs. Inside a modal layer only that layer counts.
    pub fn focusable(&self) -> Vec<WidgetId> {
        fn walk(n: &VNode, out: &mut Vec<WidgetId>) {
            if n.enabled && matches!(n.kind, NodeKind::Button { .. } | NodeKind::Checkbox { .. } | NodeKind::Slider { .. } | NodeKind::TextInput { .. }) {
                out.push(n.id);
            }
            for c in &n.children {
                walk(c, out);
            }
        }
        let top_modal = self.layers.iter().rposition(|l| l.layer.modal).unwrap_or(0);
        let mut out = Vec::new();
        for l in &self.layers[top_modal.min(self.layers.len())..] {
            walk(&l.root, &mut out);
        }
        out
    }
}

impl VNode {
    pub fn find(&self, id: WidgetId) -> Option<&VNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    pub fn find_mut(&mut self, id: WidgetId) -> Option<&mut VNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    pub fn style(&self) -> &Style {
        &self.styles[NORMAL]
    }
}

fn has_look(s: &Style) -> bool {
    s.background.is_some_and(|c| c.a > 0.0) || s.border.is_some_and(|Border { width, .. }| width > 0.0)
}

/// CSS inherits `color`, `font-*` and `text-align` from the parent; so does the backend, per
/// state (a hovered parent passes its hover colour on).
fn inherit(styles: &mut [Style; 5], parent: &[Style; 5]) {
    for (s, p) in styles.iter_mut().zip(parent) {
        s.text_color = s.text_color.or(p.text_color);
        s.font_size = s.font_size.or(p.font_size);
        s.font = s.font.or(p.font);
        s.bold = s.bold.or(p.bold);
        s.text_align = s.text_align.or(p.text_align);
    }
}

fn build_node(tree: &UiTree, id: WidgetId, parent: Option<&[Style; 5]>) -> Option<VNode> {
    let node = tree.get(id)?;
    let st = |hovered, pressed, focused| WidgetState { hovered, pressed, focused, disabled: false };
    let mut styles = [
        tree.resolve(id, st(false, false, false))?,
        tree.resolve(id, st(true, false, false))?,
        tree.resolve(id, st(true, true, false))?,
        tree.resolve(id, st(false, false, true))?,
        tree.resolve(id, WidgetState { disabled: true, ..Default::default() })?,
    ];
    if let Some(p) = parent {
        inherit(&mut styles, p);
    }
    let kind = node.kind().clone();
    let blocking = !matches!(kind, NodeKind::Container | NodeKind::Text { .. }) || has_look(&styles[NORMAL]);
    let children = tree
        .children(id)
        .iter()
        .filter(|c| tree.visible(**c).unwrap_or(false))
        .filter_map(|c| build_node(tree, *c, Some(&styles)))
        .collect();
    Some(VNode {
        id,
        kind,
        styles,
        enabled: tree.is_enabled(id),
        blocking,
        hover_signals: node.hover_signals(),
        children,
        rect: LRect::default(),
        content: (0.0, 0.0),
        tn: None,
    })
}
