//! Read-only (selectable) text inputs and `UiTree::blur`, model and headless side.

use efeui_core::view::{El, container, reconcile, selectable_text, text_input};
use efeui_core::*;

fn tree() -> (UiTree, WidgetId) {
    let mut ui = UiTree::new();
    let root = ui.layer("root", Layer::fill(0));
    ui.take_changes();
    (ui, root)
}

#[test]
fn read_only_and_multiline_are_builder_flags() {
    let (mut ui, root) = tree();
    let a = ui.text_input(root, "log").read_only(true).multiline(true).id();
    let b = ui.text_input(root, "edit").id();
    assert!(matches!(ui.kind(a), Some(NodeKind::TextInput { read_only: true, multiline: true, .. })));
    assert!(matches!(ui.kind(b), Some(NodeKind::TextInput { read_only: false, multiline: false, .. })));
    // the other builders leave a non-input alone
    let t = ui.text(root, "hi").read_only(true).multiline(true).id();
    assert_eq!(ui.kind(t), Some(&NodeKind::Text { text: "hi".into() }));
}

#[test]
fn a_read_only_input_takes_no_value_from_the_user() {
    let (mut ui, root) = tree();
    let id = ui.text_input(root, "fixed").read_only(true).id();
    assert_eq!(ui.write_back(id, UiValue::Text("typed".into())), None);
    assert_eq!(ui.text_of(id), Some("fixed"));
    // a signal with a Changed value is dropped by the filter, Submit too
    let raw = vec![
        UiSignal::new(&ui, id, UiSignalKind::Changed(UiValue::Text("typed".into()))),
        UiSignal::new(&ui, id, UiSignalKind::Focus(true)),
    ];
    let out = filter_signals(&mut ui, raw);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].kind, UiSignalKind::Focus(true));
    assert_eq!(ui.text_of(id), Some("fixed"));
    // the program can still set it
    assert!(ui.set_text(id, "new"));
    assert_eq!(ui.text_of(id), Some("new"));
}

#[test]
fn the_view_layer_builds_selectable_text() {
    let one: El<()> = selectable_text("abc", false);
    let many: El<()> = selectable_text("a\nb", true);
    let edit: El<()> = text_input("x").read_only(true);
    let (mut ui, root) = tree();
    let mut table = Default::default();
    reconcile::reconcile_root(&mut ui, root, "v", one.key("one"), &mut table);
    assert!(matches!(ui.kind(ui.find("v/one").unwrap()), Some(NodeKind::TextInput { read_only: true, multiline: false, .. })));
    let col = container::<()>().child(many.key("many")).child(edit.key("edit"));
    reconcile::reconcile_root(&mut ui, root, "w", col, &mut table);
    assert!(matches!(ui.kind(ui.find("w/many").unwrap()), Some(NodeKind::TextInput { read_only: true, multiline: true, .. })));
    assert!(matches!(ui.kind(ui.find("w/edit").unwrap()), Some(NodeKind::TextInput { read_only: true, multiline: false, .. })));
}

#[test]
fn blur_is_a_journal_entry_without_a_node() {
    let (mut ui, _) = tree();
    ui.blur();
    ui.blur_window(3);
    let ch = ui.take_changes();
    assert_eq!(ch, vec![UiChange::Blur(None), UiChange::Blur(Some(3))]);
    assert_eq!(UiChange::Blur(None).widget(), None);
}

#[test]
fn the_multiline_flag_picks_the_dom_element() {
    assert_eq!(dom_plan::element_tag(&NodeKind::text_input("a")), "input");
    assert_eq!(dom_plan::element_tag(&NodeKind::selectable_text("a", false)), "input");
    assert_eq!(dom_plan::element_tag(&NodeKind::selectable_text("a", true)), "textarea");
}

// ----- headless: focus follows signals, blur ends it ------------------------------------------

fn sync(model: &mut HeadlessModel, ui: &mut UiTree) {
    let ch = ui.take_changes();
    model.sync(ui, &ch);
}

#[test]
fn headless_capture_follows_text_focus_and_blur() {
    let (mut model, handle) = HeadlessModel::new();
    let (mut ui, root) = tree();
    let input = ui.text_input(root, "").id();
    let button = ui.button(root, "ok").id();
    sync(&mut model, &mut ui);
    assert!(handle.errors().is_empty(), "{:?}", handle.errors());
    assert!(!model.capture().keyboard);

    // a focused button is not a text field
    handle.inject(UiSignal::new(&ui, button, UiSignalKind::Focus(true)));
    model.drain_signals(&mut Vec::new());
    assert!(!model.capture().keyboard);

    handle.inject(UiSignal::new(&ui, input, UiSignalKind::Focus(true)));
    model.drain_signals(&mut Vec::new());
    assert!(model.capture().keyboard);
    assert_eq!(handle.focused(), Some(input));

    ui.blur();
    sync(&mut model, &mut ui);
    assert!(!model.capture().keyboard, "blur drops the keyboard capture");
    // and reports the lost focus like a real backend
    let mut out = Vec::new();
    model.drain_signals(&mut out);
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].widget, &out[0].kind), (input, &UiSignalKind::Focus(false)));

    // the manual capture still ORs in
    handle.set_capture(UiCapture { pointer: false, keyboard: true });
    assert!(model.capture().keyboard);
}

#[test]
fn headless_blur_is_scoped_to_a_window_and_removal_clears_focus() {
    let (mut model, handle) = HeadlessModel::new();
    let mut ui = UiTree::new();
    let r0 = ui.layer("main", Layer::fill(0));
    let r2 = ui.layer_in(2u32, "tool", Layer::fill(0));
    let in_main = ui.text_input(r0, "").id();
    let in_tool = ui.text_input(r2, "").id();
    sync(&mut model, &mut ui);
    for w in [in_main, in_tool] {
        handle.inject(UiSignal::new(&ui, w, UiSignalKind::Focus(true)));
        model.drain_signals(&mut Vec::new());
        assert!(model.capture().keyboard);
        // another window's blur does nothing
        ui.blur_window(if w == in_main { 2 } else { 0 });
        sync(&mut model, &mut ui);
        assert!(model.capture().keyboard);
        ui.blur_window(if w == in_main { 0 } else { 2 });
        sync(&mut model, &mut ui);
        assert!(!model.capture().keyboard);
        model.drain_signals(&mut Vec::new());
    }
    // removing the focused node ends the focus too
    handle.inject(UiSignal::new(&ui, in_tool, UiSignalKind::Focus(true)));
    model.drain_signals(&mut Vec::new());
    assert!(model.capture().keyboard);
    ui.remove(in_tool);
    sync(&mut model, &mut ui);
    assert!(!model.capture().keyboard);
    assert!(handle.errors().is_empty(), "{:?}", handle.errors());
}

// ----- follow_end scrolling and ellipsized text -------------------------------------------------

#[test]
fn scroll_and_ellipsize_are_builder_flags() {
    let (mut ui, root) = tree();
    let sc = ui.scroll(root).follow_end(true).id();
    let plain = ui.scroll(root).id();
    let t = ui.text_input(root, "x").read_only(true).ellipsize(true).id();
    assert_eq!(ui.kind(sc), Some(&NodeKind::Scroll { follow_end: true }));
    assert_eq!(ui.kind(plain), Some(&NodeKind::Scroll { follow_end: false }));
    assert!(matches!(ui.kind(t), Some(NodeKind::TextInput { ellipsize: true, .. })));
    ui.take_changes();
    ui.node(sc).unwrap().follow_end(false);
    assert_eq!(ui.take_changes(), vec![UiChange::Kind(sc)]);
    // other kinds ignore the setters
    let b = ui.button(root, "b").follow_end(true).ellipsize(true).id();
    assert_eq!(ui.kind(b), Some(&NodeKind::Button { label: "b".into() }));
}

#[test]
fn scroll_and_truncation_signals_are_filtered_by_kind() {
    let (mut ui, root) = tree();
    let sc = ui.scroll(root).id();
    let cut = ui.text_input(root, "x").read_only(true).ellipsize(true).id();
    let plain = ui.text_input(root, "x").id();
    let btn = ui.button(root, "b").id();
    let raw = vec![
        UiSignal::new(&ui, sc, UiSignalKind::Scrolled { at_end: true }),
        UiSignal::new(&ui, btn, UiSignalKind::Scrolled { at_end: true }),
        UiSignal::new(&ui, cut, UiSignalKind::Truncated(true)),
        UiSignal::new(&ui, plain, UiSignalKind::Truncated(true)),
        UiSignal::new(&ui, sc, UiSignalKind::Truncated(true)),
    ];
    let out = filter_signals(&mut ui, raw);
    let got: Vec<_> = out.iter().map(|s| (s.widget, s.kind.clone())).collect();
    assert_eq!(got, vec![(sc, UiSignalKind::Scrolled { at_end: true }), (cut, UiSignalKind::Truncated(true))]);
}

#[test]
fn the_view_layer_dispatches_scroll_and_truncation_messages() {
    #[derive(Clone, Debug, PartialEq)]
    enum M {
        Scrolled(bool),
        Cut(bool),
    }
    let body = container::<M>()
        .child(selectable_text("long", false).ellipsize(true).on_truncated(M::Cut).key("t"))
        .child(efeui_core::view::scroll::<M>().follow_end(true).on_scroll(M::Scrolled).key("s"));
    let (mut ui, root) = tree();
    let mut table = Default::default();
    reconcile::reconcile_root(&mut ui, root, "v", body, &mut table);
    let (t, s) = (ui.find("v/t").unwrap(), ui.find("v/s").unwrap());
    assert!(matches!(ui.kind(t), Some(NodeKind::TextInput { ellipsize: true, read_only: true, .. })));
    assert_eq!(ui.kind(s), Some(&NodeKind::Scroll { follow_end: true }));
    assert_eq!(table[&s].dispatch(&UiSignalKind::Scrolled { at_end: false }), Some(M::Scrolled(false)));
    assert_eq!(table[&t].dispatch(&UiSignalKind::Truncated(true)), Some(M::Cut(true)));
    assert_eq!(table[&s].dispatch(&UiSignalKind::Truncated(true)), None);
}
