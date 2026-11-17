//! The backend's owned copy of the tree: everything the iced `Element` builder needs, with the
//! styles already resolved against the theme. Rebuilt from the `UiTree` whenever the journal is
//! not empty (the tree is the source of truth); between rebuilds the backend edits the values
//! of value widgets here, so typing and dragging never wait a frame.

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

pub struct VNode {
    pub id: WidgetId,
    pub kind: NodeKind,
    /// Resolved styles: normal, hovered, pressed (hovered + pressed), focused, disabled.
    pub styles: [Style; 5],
    /// Effectively enabled (own flag and ancestors').
    pub enabled: bool,
    /// Takes pointer events (API.md section 6: everything inside a layer except a Container or
    /// Text without background and border).
    pub blocking: bool,
    pub hover_signals: bool,
    pub children: Vec<VNode>,
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
    pub fn build(tree: &UiTree) -> Model {
        let mut layers = Vec::new();
        for root in tree.layers() {
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

    pub fn find_mut(&mut self, id: WidgetId) -> Option<&mut VNode> {
        self.layers.iter_mut().find_map(|l| l.root.find_mut(id))
    }

    pub fn find(&self, id: WidgetId) -> Option<&VNode> {
        self.layers.iter().find_map(|l| l.root.find(id))
    }

    pub fn modal(&self) -> bool {
        self.layers.iter().any(|l| l.layer.modal)
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Every text input in the model (for focus tracking).
    pub fn text_inputs(&self) -> Vec<WidgetId> {
        fn walk(n: &VNode, out: &mut Vec<WidgetId>) {
            if matches!(n.kind, NodeKind::TextInput { .. }) {
                out.push(n.id);
            }
            for c in &n.children {
                walk(c, out);
            }
        }
        let mut out = Vec::new();
        for l in &self.layers {
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

/// CSS inherits `color`, `font-*` and `text-align` from the parent; the backend does the same
/// for the properties the style subset has, per state (a hovered parent passes its hover colour on).
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
    Some(VNode { id, kind, styles, enabled: tree.is_enabled(id), blocking, hover_signals: node.hover_signals(), children })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Layer;

    #[test]
    fn model_keeps_visible_nodes_with_resolved_styles() {
        let mut ui = UiTree::new();
        let root = ui.layer("m", Layer::centered(1));
        let col = ui.column(root).class("panel").id();
        let play = ui.button(col, "Play").id();
        let gone = ui.text(col, "hidden").visible(false).id();
        let plain = ui.text(col, "plain").id();
        let off = ui.button(col, "off").enabled(false).id();
        let m = Model::build(&ui);
        assert_eq!(m.layers.len(), 1);
        assert!(m.find(gone).is_none());
        // Panel has a background: blocking. Plain text does not.
        assert!(m.find(col).unwrap().blocking);
        assert!(!m.find(plain).unwrap().blocking);
        assert!(m.find(play).unwrap().blocking);
        // Hover style differs from normal for the default button; disabled is dimmed.
        let b = m.find(play).unwrap();
        assert_ne!(b.styles[NORMAL].background, b.styles[HOVERED].background);
        let o = m.find(off).unwrap();
        assert!(!o.enabled);
        assert_eq!(o.styles[NORMAL].opacity, Some(0.5));
    }

    #[test]
    fn unset_text_properties_are_inherited_like_css() {
        use crate::style::Style;
        let mut ui = UiTree::new();
        let root = ui.layer("m", Layer::centered(1));
        // The built-in theme gives every node a colour and size, so only properties it leaves
        // unset (bold, font) reach the children; the web cascade does the same.
        let col = ui.column(root).style(Style::new().bold(true)).id();
        let t = ui.text(col, "hi").id();
        let own = ui.text(col, "own").style(Style::new().bold(false)).id();
        let m = Model::build(&ui);
        let st = m.find(t).unwrap().style();
        assert_eq!(st.bold, Some(true));
        assert_eq!(m.find(own).unwrap().style().bold, Some(false));
    }

    #[test]
    fn hidden_layers_are_dropped_and_modal_layers_block() {
        let mut ui = UiTree::new();
        let a = ui.layer("a", Layer::fill(1).modal());
        let b = ui.layer("b", Layer::fill(2));
        ui.set_visible(b, false);
        let m = Model::build(&ui);
        assert_eq!(m.layers.len(), 1);
        assert!(m.modal());
        assert!(m.find(a).unwrap().blocking);
    }
}
