//! Regressions from the independent review of the crate.

use efeui_core::*;

fn tree() -> (UiTree, WidgetId) {
    let mut ui = UiTree::new();
    let root = ui.layer("root", Layer::fill(0));
    ui.take_changes();
    (ui, root)
}

fn sync(be: &mut HeadlessModel, tree: &UiTree, changes: &[UiChange]) {
    be.sync(tree, changes);
}

/// Takes the journal and checks it with the headless backend (which knows the ids from `seen`).
fn check(be: &mut HeadlessModel, h: &HeadlessHandle, ui: &mut UiTree) -> Vec<UiChange> {
    let ch = ui.take_changes();
    sync(be, ui, &ch);
    assert!(h.errors().is_empty(), "{:?} for {ch:?}", h.errors());
    ch
}

fn synced(ui: &mut UiTree) -> (HeadlessModel, HeadlessHandle) {
    let (mut be, h) = HeadlessModel::new();
    // Make the backend aware of every live node.
    let created: Vec<UiChange> = ui.iter().map(|(id, _)| UiChange::Created(id)).collect();
    sync(&mut be, ui, &created);
    assert!(h.errors().is_empty());
    (be, h)
}

// ----- 1: Moved indices are final -------------------------------------------------------------

#[test]
fn moved_index_survives_a_sibling_removal() {
    let (mut ui, p) = tree();
    let a = ui.button(p, "a").id();
    let c = ui.button(p, "c").id();
    let _ = a;
    ui.take_changes();
    let (mut be, h) = synced(&mut ui);
    let a2 = ui.button(p, "a2").id();
    ui.take_changes();
    sync(&mut be, &ui, &[UiChange::Created(a2)]);
    // children [a, c, a2]; move c to the end, then remove a: c is at index 1 finally
    ui.move_to(c, p, 2).unwrap();
    ui.remove(a);
    assert_eq!(ui.children(p), &[a2, c]);
    let ch = check(&mut be, &h, &mut ui);
    assert!(ch.contains(&UiChange::Moved { id: c, parent: p, index: 1 }), "{ch:?}");
}

#[test]
fn moved_is_coalesced_with_final_index_and_parent() {
    let (mut ui, p) = tree();
    let box_ = ui.container(p).id();
    let a = ui.button(p, "a").id();
    let b = ui.button(p, "b").id();
    let c = ui.button(p, "c").id();
    ui.take_changes();
    let (mut be, h) = synced(&mut ui);
    ui.move_to(c, p, 0).unwrap();
    ui.move_to(a, box_, 0).unwrap();
    ui.move_to(a, p, 3).unwrap();
    ui.move_to(b, p, 0).unwrap();
    let ch = check(&mut be, &h, &mut ui);
    let moved: Vec<_> = ch.iter().filter(|c| matches!(c, UiChange::Moved { .. })).collect();
    // one entry per node, each naming the final parent and index
    for m in &moved {
        let UiChange::Moved { id, parent, index } = **m else { unreachable!() };
        assert_eq!(ui.children(parent)[index as usize], id);
    }
    let ids: Vec<_> = moved.iter().filter_map(|m| m.widget()).collect();
    let mut dedup = ids.clone();
    dedup.sort_by_key(|i| i.index());
    dedup.dedup();
    assert_eq!(ids.len(), dedup.len(), "{ch:?}");
}

#[test]
fn moved_with_inserts_and_removals_interleaved() {
    let (mut ui, p) = tree();
    let ids: Vec<_> = (0..5).map(|i| ui.button(p, format!("{i}")).id()).collect();
    ui.take_changes();
    let (mut be, h) = synced(&mut ui);
    ui.move_to(ids[4], p, 0).unwrap();
    let n = ui.button(p, "new").id();
    ui.remove(ids[1]);
    ui.move_to(ids[0], p, 99).unwrap();
    ui.move_to(n, p, 1).unwrap();
    ui.remove(ids[3]);
    let ch = check(&mut be, &h, &mut ui);
    for c in &ch {
        if let UiChange::Moved { id, parent, index } = *c {
            assert_eq!(ui.children(parent).get(index as usize), Some(&id), "{ch:?}");
        }
    }
}

// ----- 2: disabled always wins ----------------------------------------------------------------

#[test]
fn class_hover_does_not_beat_kind_disabled() {
    let mut ui = UiTree::new();
    let root = ui.layer("r", Layer::fill(0));
    let mut t = Theme::empty();
    t.set_kind(KindTag::Button, StateStyles { disabled: Style::new().opacity(0.4), ..Default::default() });
    t.set_class("c", StateStyles { hovered: Style::new().opacity(1.0), pressed: Style::new().opacity(0.9), ..Default::default() });
    ui.set_theme(t);
    let b = ui.button(root, "x").class("c").id();
    ui.set_enabled(b, false);
    let st = WidgetState { hovered: true, pressed: true, ..Default::default() };
    assert_eq!(ui.resolve(b, st).unwrap().opacity, Some(0.4));
}

// ----- 6 / 7: kind edits re-sanitize ----------------------------------------------------------

#[test]
fn max_len_truncates_existing_value() {
    let (mut ui, p) = tree();
    let t = ui.text_input(p, "abcdef").id();
    ui.node(t).unwrap().max_len(3);
    assert_eq!(ui.text_of(t), Some("abc"));
    ui.set_kind(t, NodeKind::TextInput { value: "xyzxyz".into(), placeholder: String::new(), max_len: 2, password: false }).unwrap();
    assert_eq!(ui.text_of(t), Some("xy"));
}

#[test]
fn slider_edits_clamp_value_and_never_panic() {
    let (mut ui, p) = tree();
    let s = ui.slider(p, 0.0..=10.0, 8.0).id();
    ui.edit_kind(s, |k| {
        if let NodeKind::Slider { max, .. } = k {
            *max = 5.0;
        }
    });
    assert_eq!(ui.value(s), Some(UiValue::F32(5.0)));
    // inverted and NaN limits
    ui.edit_kind(s, |k| {
        if let NodeKind::Slider { min, max, .. } = k {
            *min = 9.0;
            *max = 1.0;
        }
    });
    assert!(ui.set_value(s, 100.0));
    assert_eq!(ui.value(s), Some(UiValue::F32(9.0)));
    ui.set_kind(s, NodeKind::Slider { min: f32::NAN, max: 2.0, step: f32::NAN, value: f32::NAN }).unwrap();
    assert!(ui.set_value(s, 1.0));
    let Some(UiValue::F32(v)) = ui.value(s) else { panic!() };
    assert!(!v.is_nan());
    // creation with inverted / NaN limits
    let s2 = ui.slider(p, 5.0..=1.0, 3.0).id();
    assert_eq!(ui.value(s2), Some(UiValue::F32(3.0)));
    let _ = ui.slider(p, f32::NAN..=f32::NAN, f32::NAN);
}

// ----- 8: layer order -------------------------------------------------------------------------

#[test]
fn equal_z_layers_keep_creation_order_after_slot_reuse() {
    let mut ui = UiTree::new();
    let a = ui.layer("a", Layer::fill(0));
    let b = ui.layer("b", Layer::fill(0));
    ui.remove(a); // frees slot 0
    let c = ui.layer("c", Layer::fill(0)); // reuses slot 0, created after b
    assert_eq!(ui.layers(), vec![b, c]);
}

// ----- 9: keys --------------------------------------------------------------------------------

#[test]
fn find_never_returns_a_dead_id() {
    let (mut ui, p) = tree();
    let a = ui.button(p, "a").key("k").id();
    ui.remove(a);
    assert_eq!(ui.find("k"), None);
    let b = ui.button(p, "b").key("k").id();
    assert_eq!(ui.find("k"), Some(b));
    let sub = ui.container(p).key("s").id();
    let kid = ui.button(sub, "x").key("s/x").id();
    let _ = kid;
    ui.remove(sub);
    assert_eq!(ui.find("s/x"), None);
    assert_eq!(ui.find("s"), None);
}

// ----- 5: hover signals are journaled ---------------------------------------------------------

#[test]
fn hover_signals_change_is_journaled() {
    let (mut ui, p) = tree();
    let b = ui.button(p, "b").id();
    ui.take_changes();
    ui.set_hover_signals(b, true);
    assert_eq!(ui.take_changes(), vec![UiChange::HoverSignals(b)]);
    ui.set_hover_signals(b, true); // no change, no entry
    assert!(ui.take_changes().is_empty());
    ui.set_hover_signals(b, false);
    assert_eq!(ui.take_changes(), vec![UiChange::HoverSignals(b)]);
    // a node created this frame is covered by Created
    let c = ui.button(p, "c").hover_signals().id();
    assert_eq!(ui.take_changes(), vec![UiChange::Created(c)]);
}
