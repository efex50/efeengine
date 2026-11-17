//! Tree operations, ids, keys, owners, layers and the journal.

use efeengine_ui::*;

fn tree() -> (UiTree, WidgetId) {
    let mut ui = UiTree::new();
    let root = ui.layer("root", Layer::fill(0));
    ui.take_changes();
    (ui, root)
}

#[test]
fn builder_chain_creates_nodes() {
    let (mut ui, root) = tree();
    let col = ui.column(root).class("panel").gap(12.0).padding(24.0).id();
    ui.text(col, "Coin Dash").class("title");
    ui.button(col, "Play").key("menu/play");
    ui.slider(col, 0.0..=1.0, 0.8).key("menu/volume");
    ui.checkbox(col, "Fullscreen", false).key("menu/fullscreen");
    ui.text_input(col, "").placeholder("Player name").max_len(16).key("menu/name");

    assert_eq!(ui.children(root), &[col]);
    assert_eq!(ui.children(col).len(), 5);
    let n = ui.get(col).unwrap();
    assert_eq!(n.style().direction, Some(Direction::Column));
    assert_eq!(n.style().gap, Some(12.0));
    assert_eq!(n.style().padding, Some(Edges::all(24.0)));
    assert_eq!(ui.class_names(col), vec!["panel"]);
    let name = ui.find("menu/name").unwrap();
    match ui.kind(name).unwrap() {
        NodeKind::TextInput { placeholder, max_len, .. } => {
            assert_eq!(placeholder, "Player name");
            assert_eq!(*max_len, 16);
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(ui.parent(name), Some(col));
}

#[test]
fn stale_ids_are_rejected() {
    let (mut ui, root) = tree();
    let a = ui.button(root, "a").id();
    assert!(ui.remove(a));
    assert!(!ui.contains(a));
    assert!(!ui.remove(a));
    assert!(!ui.set_text(a, "x"));
    assert!(!ui.set_visible(a, false));
    assert!(ui.get(a).is_none());
    assert_eq!(ui.try_create(a, NodeKind::Container), Err(TreeError::Stale));
    assert_eq!(ui.move_to(a, root, 0), Err(TreeError::Stale));
    // The slot is reused with a new generation: the old id stays dead.
    let b = ui.button(root, "b").id();
    assert_eq!(a.index(), b.index());
    assert_ne!(a.generation(), b.generation());
    assert!(!ui.contains(a));
    assert!(ui.contains(b));
    assert_eq!(WidgetId::from_bits(b.to_bits()), b);
}

#[test]
fn keys_are_unique_and_found() {
    let (mut ui, root) = tree();
    let a = ui.button(root, "a").key("k/a").id();
    let b = ui.button(root, "b").id();
    assert_eq!(ui.find("k/a"), Some(a));
    assert_eq!(ui.key_of(a), Some("k/a"));
    assert_eq!(ui.set_key(b, "k/a"), Err(TreeError::KeyTaken("k/a".into())));
    assert_eq!(ui.set_key(a, "k/a"), Ok(())); // same node, same key
    ui.set_key(b, "k/b").unwrap();
    // Renaming frees the old key.
    ui.set_key(a, "k/c").unwrap();
    assert_eq!(ui.find("k/a"), None);
    ui.set_key(b, "k/a").unwrap();
    assert_eq!(ui.find("k/a"), Some(b));
    ui.clear_key(b);
    assert_eq!(ui.find("k/a"), None);
    // Removal frees the key.
    ui.remove(a);
    assert_eq!(ui.find("k/c"), None);
}

#[test]
#[should_panic(expected = "already used")]
fn builder_key_duplicate_panics() {
    let (mut ui, root) = tree();
    ui.button(root, "a").key("dup");
    ui.button(root, "b").key("dup");
}

#[test]
fn subtree_removal() {
    let (mut ui, root) = tree();
    let col = ui.column(root).key("col").id();
    let row = ui.row(col).id();
    let b = ui.button(row, "x").key("deep").id();
    let keep = ui.button(root, "keep").id();
    assert_eq!(ui.len(), 5);
    ui.take_changes();
    assert!(ui.remove(col));
    assert_eq!(ui.len(), 2);
    for id in [col, row, b] {
        assert!(!ui.contains(id));
    }
    assert!(ui.contains(keep));
    assert_eq!(ui.find("deep"), None);
    assert_eq!(ui.find("col"), None);
    assert_eq!(ui.children(root), &[keep]);
    // Children before parents.
    assert_eq!(ui.changes(), &[UiChange::Removed(b), UiChange::Removed(row), UiChange::Removed(col)]);
}

#[test]
fn owner_removal() {
    let (mut ui, root) = tree();
    let game = ui.button(root, "g").id();
    let modl = Owner::module("coins");
    let (m_root, m_child) = ui.with_owner(modl.clone(), |ui| {
        let r = ui.layer("mod:coins/hud", Layer::fill(5));
        let c = ui.button(r, "m").id();
        (r, c)
    });
    let plugin_node = ui.with_owner(Owner::plugin("p"), |ui| ui.button(root, "p").id());
    assert_eq!(ui.owner_of(game), Some(&Owner::Game));
    assert_eq!(ui.owner_of(m_child), Some(&modl));
    assert_eq!(ui.owner_of(plugin_node), Some(&Owner::plugin("p")));
    assert_eq!(ui.owner(), &Owner::Game);
    assert_eq!(ui.nodes_of(&modl).len(), 2);

    assert_eq!(ui.remove_owner(&modl), 2);
    assert!(!ui.contains(m_root) && !ui.contains(m_child));
    assert!(ui.contains(game) && ui.contains(plugin_node));
    assert_eq!(ui.find("mod:coins/hud"), None);
    assert_eq!(ui.remove_owner(&modl), 0);
}

#[test]
fn move_to_reorders_and_reparents() {
    let (mut ui, root) = tree();
    let a = ui.button(root, "a").id();
    let b = ui.button(root, "b").id();
    let c = ui.button(root, "c").id();
    let box_ = ui.container(root).id();
    ui.take_changes();

    ui.move_to(c, root, 0).unwrap();
    assert_eq!(ui.children(root), &[c, a, b, box_]);
    assert_eq!(ui.changes(), &[UiChange::Moved { id: c, parent: root, index: 0 }]);
    ui.take_changes();

    // Same place: no entry.
    ui.move_to(c, root, 0).unwrap();
    assert!(ui.changes().is_empty());

    ui.move_to(a, box_, 99).unwrap(); // clamped
    assert_eq!(ui.children(box_), &[a]);
    assert_eq!(ui.parent(a), Some(box_));
    assert_eq!(ui.changes(), &[UiChange::Moved { id: a, parent: box_, index: 0 }]);

    assert_eq!(ui.move_to(box_, a, 0), Err(TreeError::Cycle));
    assert_eq!(ui.move_to(box_, box_, 0), Err(TreeError::Cycle));
    assert_eq!(ui.move_to(root, box_, 0), Err(TreeError::LayerRoot));
}

#[test]
fn layers_sorted_by_z_and_modal() {
    let mut ui = UiTree::new();
    let pause = ui.layer("pause", Layer::centered(100).modal());
    let hud = ui.layer("hud", Layer::fill(0));
    let mid = ui.layer("mid", Layer::top_right(50).offset(-8.0, 8.0));
    assert_eq!(ui.layers(), vec![hud, mid, pause]);
    assert_eq!(ui.layer_data(mid).unwrap().anchor, Anchor::TopRight);
    assert_eq!(ui.layer_data(mid).unwrap().offset, (-8.0, 8.0));
    assert!(ui.modal_active());
    ui.set_visible(pause, false);
    assert!(!ui.modal_active());
    // Same name again keeps the id and updates the data.
    ui.take_changes();
    assert_eq!(ui.layer("hud", Layer::fill(7)), hud);
    assert_eq!(ui.layer_data(hud).unwrap().z, 7);
    assert_eq!(ui.changes(), &[UiChange::Layer(hud)]);
    assert_eq!(ui.layer_of(hud), Some(hud));
}

#[test]
fn journal_entry_per_op() {
    let (mut ui, root) = tree();
    let b = ui.button(root, "a").id();
    let s = ui.slider(root, 0.0..=10.0, 1.0).id();
    ui.take_changes();

    ui.set_text(b, "b");
    ui.set_value(s, 3.0);
    ui.set_visible(b, false);
    ui.set_enabled(b, false);
    ui.set_class(b, "primary");
    ui.style_mut(b).unwrap().gap = Some(2.0);
    ui.edit_style(b, |st| st.bold = Some(true));
    ui.set_style(s, Style::new().width(Size::Fill));
    assert_eq!(
        ui.take_changes(),
        vec![
            UiChange::Kind(b),
            UiChange::Kind(s),
            UiChange::Visible(b),
            UiChange::Enabled(b),
            UiChange::Classes(b),
            UiChange::Style(b),
            UiChange::Style(b),
            UiChange::Style(s),
        ]
    );
    assert!(ui.changes().is_empty());

    // Setting what is already there writes nothing.
    ui.set_text(b, "b");
    ui.set_value(s, 3.0);
    ui.set_visible(b, false);
    ui.set_class(b, "primary");
    ui.style_mut(b).unwrap().gap = Some(2.0);
    assert!(ui.changes().is_empty());

    let id = ui.create(root, NodeKind::Container).id();
    assert_eq!(ui.take_changes(), vec![UiChange::Created(id)]);
    assert_eq!(ui.text_of(b), Some("b"));
    assert_eq!(ui.visible(b), Some(false));
    assert_eq!(ui.enabled(b), Some(false));
    assert!(!ui.is_enabled(b) && !ui.is_shown(b));
}

#[test]
fn created_node_gets_no_extra_entries_and_create_remove_cancels() {
    let (mut ui, root) = tree();
    let a = ui.button(root, "a").class("x").gap(4.0).key("a").id();
    ui.set_text(a, "changed");
    assert_eq!(ui.changes(), &[UiChange::Created(a)]);

    let keep = ui.button(root, "keep").id();
    let col = ui.column(root).id();
    let child = ui.button(col, "c").id();
    ui.remove(col);
    // created and removed in the same frame: no trace
    assert_eq!(ui.changes(), &[UiChange::Created(a), UiChange::Created(keep)]);
    assert!(!ui.contains(child));

    // An old node removed after changes: only Removed remains.
    ui.take_changes();
    ui.set_text(keep, "k2");
    ui.set_visible(keep, false);
    ui.remove(keep);
    assert_eq!(ui.take_changes(), vec![UiChange::Removed(keep)]);
}

#[test]
fn moved_entries_under_removed_parent_are_dropped() {
    let (mut ui, root) = tree();
    let a = ui.container(root).id();
    let b = ui.button(root, "b").id();
    ui.take_changes();
    ui.move_to(b, a, 0).unwrap();
    ui.remove(a); // removes b too
    assert_eq!(ui.take_changes(), vec![UiChange::Removed(b), UiChange::Removed(a)]);
}

#[test]
fn kinds_values_and_clamping() {
    let (mut ui, root) = tree();
    let s = ui.slider(root, 0.0..=1.0, 5.0).id();
    assert_eq!(ui.value(s), Some(UiValue::F32(1.0)));
    ui.set_value(s, 2.0);
    assert_eq!(ui.value(s), Some(UiValue::F32(1.0)));
    assert!(!ui.set_value(s, true)); // wrong type
    assert!(!ui.set_value(s, f32::NAN));
    let t = ui.text_input(root, "").max_len(3).id();
    ui.set_value(t, "abcdef");
    assert_eq!(ui.text_of(t), Some("abc"));
    let c = ui.checkbox(root, "c", false).id();
    ui.set_value(c, true);
    assert_eq!(ui.value(c), Some(UiValue::Bool(true)));
    // set_kind keeps the tag
    assert_eq!(ui.set_kind(c, NodeKind::Container), Err(TreeError::KindMismatch));
    ui.set_kind(c, NodeKind::Checkbox { label: "z".into(), checked: false }).unwrap();
    assert_eq!(ui.text_of(c), Some("z"));
}

#[test]
fn fonts_are_journaled() {
    let (mut ui, _) = tree();
    let f = ui.load_font("Inter", vec![1u8, 2, 3]);
    assert_eq!(ui.take_changes(), vec![UiChange::Font(f)]);
    assert_eq!(ui.font(f).unwrap().bytes.len(), 3);
    assert_eq!(ui.font_by_name("Inter"), Some(f));
    assert_eq!(ui.font_by_name("nope"), None);
}

#[test]
fn iteration_and_subtree() {
    let (mut ui, root) = tree();
    let a = ui.column(root).id();
    let b = ui.button(a, "b").id();
    let c = ui.button(a, "c").id();
    assert_eq!(ui.subtree(root), vec![root, a, b, c]);
    assert_eq!(ui.iter().count(), 4);
    assert_eq!(ui.layer_of(c), Some(root));
}
