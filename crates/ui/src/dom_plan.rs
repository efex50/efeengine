//! Target-independent part of the web backend: which DOM element a node becomes, its class
//! list and inline declarations, and the property diff. Everything here is pure (tree in,
//! strings out) so it is unit tested natively; `web.rs` only applies the results to the DOM.

use crate::css;
use crate::id::WidgetId;
use crate::node::{Layer, NodeKind};
use crate::style::{Direction, Style};
use crate::theme::WidgetState;
use crate::tree::UiTree;

/// A CSS property and its value, as produced by [`css::declarations`].
pub type Decl = (&'static str, String);

/// HTML tag name of a node's outer element.
pub fn element_tag(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Container | NodeKind::Scroll => "div",
        NodeKind::Text { .. } => "span",
        NodeKind::Image { .. } | NodeKind::GpuView { .. } => "canvas",
        NodeKind::Button { .. } => "button",
        NodeKind::Checkbox { .. } => "label",
        NodeKind::Slider { .. } | NodeKind::TextInput { .. } => "input",
    }
}

/// The `class` attribute of a node: `efui`, `efui-layer` for a layer root, the kind class and
/// the node's theme classes in order.
pub fn class_attr(tree: &UiTree, id: WidgetId) -> String {
    let Some(node) = tree.get(id) else { return String::new() };
    let mut s = String::from("efui");
    if node.layer().is_some() {
        s.push_str(" efui-layer");
    }
    s.push(' ');
    s.push_str(&css::kind_class(node.tag()));
    for name in tree.class_names(id) {
        s.push(' ');
        s.push_str(&css::class_class(name));
    }
    s
}

/// The direction the parent lays its children out in (`Column` when unset); `None` for a layer
/// root.
pub fn parent_direction(tree: &UiTree, id: WidgetId) -> Option<Direction> {
    let parent = tree.parent(id)?;
    let style = tree.resolve(parent, WidgetState::default()).unwrap_or_default();
    Some(style.direction.unwrap_or(Direction::Column))
}

/// `true` for nodes that take pointer events: everything except plain layout boxes and text
/// without a background or border (those let clicks through to the game).
pub fn takes_pointer(kind: &NodeKind, style: &Style) -> bool {
    match kind {
        NodeKind::Container | NodeKind::Text { .. } => style.background.is_some() || style.border.is_some(),
        _ => true,
    }
}

/// Later entries win over earlier ones with the same property; the order of first appearance is
/// kept.
pub fn merge_decls(mut base: Vec<Decl>, over: Vec<Decl>) -> Vec<Decl> {
    for (k, v) in over {
        match base.iter_mut().find(|(bk, _)| *bk == k) {
            Some(slot) => slot.1 = v,
            None => base.push((k, v)),
        }
    }
    base
}

/// The full inline declarations of a node: its resolved normal style (disabled included, as
/// `UiTree::resolve` does) mapped to CSS with the parent's direction, plus `pointer-events`.
/// A layer root also gets [`css::layer_declarations`], and always `pointer-events: none` (its
/// children and the modal backdrop take the events).
pub fn node_decls(tree: &UiTree, id: WidgetId) -> Vec<Decl> {
    let Some(node) = tree.get(id) else { return Vec::new() };
    let style = tree.resolve(id, WidgetState::default()).unwrap_or_default();
    let mut decls = css::declarations(&style, parent_direction(tree, id));
    if matches!(node.kind(), NodeKind::GpuView { .. }) {
        decls = merge_decls(decls, gpu_view_auto_decls(&style));
    }
    if let Some(layer) = node.layer() {
        decls = merge_decls(decls, layer_decls(layer));
        decls = merge_decls(decls, vec![("pointer-events", "none".to_string())]);
    } else {
        let pe = if takes_pointer(node.kind(), &style) { "auto" } else { "none" };
        decls = merge_decls(decls, vec![("pointer-events", pe.to_string())]);
    }
    decls
}

/// A GPU view with an `Auto` (or unset) width / height is empty and never painted (API.md 17.1,
/// as on native). A `<canvas>` would otherwise take its backing size as client size and, with a
/// `devicePixelRatio` other than 1, grow every frame. So that axis is pinned to `0px`.
pub fn gpu_view_auto_decls(style: &Style) -> Vec<Decl> {
    let auto = |s: Option<crate::style::Size>| matches!(s, None | Some(crate::style::Size::Auto));
    let mut d = Vec::new();
    if auto(style.width) {
        d.push(("width", "0px".to_string()));
    }
    if auto(style.height) {
        d.push(("height", "0px".to_string()));
    }
    d
}

/// [`css::layer_declarations`] for a layer root (a modal layer's root is placed by its anchor
/// like any other; the backdrop element covers the window).
pub fn layer_decls(layer: &Layer) -> Vec<Decl> {
    css::layer_declarations(layer)
}

/// Declarations of the full-window backdrop of a modal layer: same z as the layer, below its
/// root in DOM order, transparent, takes all pointer events.
pub fn backdrop_decls(layer: &Layer) -> Vec<Decl> {
    vec![
        ("position", "absolute".to_string()),
        ("inset", "0".to_string()),
        ("z-index", layer.z.to_string()),
        ("pointer-events", "auto".to_string()),
    ]
}

/// What to do to bring an element from `old` declarations to `new`: properties to set (new or
/// changed) and properties to remove.
pub fn style_diff(old: &[Decl], new: &[Decl]) -> (Vec<Decl>, Vec<&'static str>) {
    let set = new.iter().filter(|(k, v)| old.iter().find(|(ok, _)| ok == k).is_none_or(|(_, ov)| ov != v)).cloned().collect();
    let remove = old.iter().filter(|(k, _)| !new.iter().any(|(nk, _)| nk == k)).map(|(k, _)| *k).collect();
    (set, remove)
}

/// The resolved layout direction of a node (`Column` when unset).
pub fn direction_of(tree: &UiTree, id: WidgetId) -> Direction {
    tree.resolve(id, WidgetState::default()).and_then(|s| s.direction).unwrap_or(Direction::Column)
}

/// Whether a node is effectively disabled (own flag or an ancestor's).
pub fn effectively_disabled(tree: &UiTree, id: WidgetId) -> bool {
    !tree.is_enabled(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_sets_changed_and_removes_gone() {
        let old = vec![("a", "1".to_string()), ("b", "2".to_string()), ("c", "3".to_string())];
        let new = vec![("a", "1".to_string()), ("b", "9".to_string()), ("d", "4".to_string())];
        let (set, rm) = style_diff(&old, &new);
        assert_eq!(set, vec![("b", "9".to_string()), ("d", "4".to_string())]);
        assert_eq!(rm, vec!["c"]);
        let (set, rm) = style_diff(&new, &new);
        assert!(set.is_empty() && rm.is_empty());
    }

    #[test]
    fn auto_sized_gpu_view_is_pinned_empty() {
        let mut ui = UiTree::new();
        let layer = ui.layer("main", Layer::fill(0));
        let auto = ui.gpu_view(layer, "p").id();
        let sized = ui.gpu_view(layer, "p").width(crate::Size::Px(160.0)).height(crate::Size::Fill).id();
        let get = |id, k: &str| node_decls(&ui, id).into_iter().find(|(p, _)| *p == k).map(|(_, v)| v);
        assert_eq!(get(auto, "width").as_deref(), Some("0px"));
        assert_eq!(get(auto, "height").as_deref(), Some("0px"));
        assert_eq!(get(sized, "width").as_deref(), Some("160px"));
        assert_ne!(get(sized, "height").as_deref(), Some("0px"));
    }

    #[test]
    fn merge_later_wins() {
        let m = merge_decls(vec![("a", "1".into()), ("b", "2".into())], vec![("b", "3".into()), ("c", "4".into())]);
        assert_eq!(m, vec![("a", "1".to_string()), ("b", "3".to_string()), ("c", "4".to_string())]);
    }

    #[test]
    fn tags_and_classes() {
        let mut ui = UiTree::new();
        let layer = ui.layer("main", Layer::fill(0));
        let b = ui.button(layer, "ok").class("primary").id();
        let t = ui.text(layer, "hi").id();
        assert_eq!(element_tag(ui.kind(b).unwrap()), "button");
        assert_eq!(element_tag(ui.kind(t).unwrap()), "span");
        let c = class_attr(&ui, b);
        assert!(c.starts_with("efui "), "{c}");
        assert!(c.contains("efui-k-button") && c.contains("efui-c-primary"), "{c}");
        assert!(class_attr(&ui, layer).contains("efui-layer"));
    }

    #[test]
    fn pointer_rules_and_direction() {
        let mut ui = UiTree::new();
        let layer = ui.layer("main", Layer::fill(0));
        let row = ui.row(layer).id();
        let plain = ui.container(row).id();
        let panel = ui.container(row).background(crate::Color::BLACK).id();
        let btn = ui.button(row, "x").id();
        let get = |id| node_decls(&ui, id).into_iter().find(|(k, _)| *k == "pointer-events").map(|(_, v)| v);
        assert_eq!(get(layer).as_deref(), Some("none"));
        assert_eq!(get(plain).as_deref(), Some("none"));
        assert_eq!(get(panel).as_deref(), Some("auto"));
        assert_eq!(get(btn).as_deref(), Some("auto"));
        assert_eq!(parent_direction(&ui, layer), None);
        assert_eq!(parent_direction(&ui, btn), Some(Direction::Row));
        assert_eq!(parent_direction(&ui, row), Some(Direction::Column));
    }
}
