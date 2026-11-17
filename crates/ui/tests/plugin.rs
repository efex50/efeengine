//! The plugin: signals, write-back, filtering, bindings, capture, headless checks.

use std::cell::RefCell;
use std::rc::Rc;

use efeengine_core::prelude::*;
use efeengine_ui::*;

fn root(t: &mut UiTest) -> WidgetId {
    t.ui_mut().layer("root", Layer::fill(0))
}

/// Records every signal a system sees in `Update` (same frame as the input).
fn recorder(t: &mut UiTest) -> Rc<RefCell<Vec<UiSignal>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    t.engine.add_system(Stage::Update, move |cx| l.borrow_mut().extend(cx.ui_signals().cloned()));
    log
}

#[test]
fn click_is_delivered_in_the_same_frame() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().button(r, "Go").key("go");
    let log = recorder(&mut t);
    t.step(1);
    assert!(log.borrow().is_empty());

    t.click("go");
    t.step(1);
    let log = log.borrow();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].key_kind(), ("go", &UiSignalKind::Click));
    assert_eq!(log[0].owner, Owner::Game);
    // gone the next frame
    drop(log);
    t.step(1);
}

#[test]
fn signals_are_readable_once() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().button(r, "Go").key("go");
    let log = recorder(&mut t);
    t.click("go");
    t.step(3);
    assert_eq!(log.borrow().len(), 1);
}

#[test]
fn value_writeback_has_no_journal_entry() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    {
        let ui = t.ui_mut();
        ui.slider(r, 0.0..=1.0, 0.5).key("vol");
        ui.checkbox(r, "fs", false).key("fs");
        ui.text_input(r, "").max_len(4).key("name");
    }
    t.step(1);
    t.backend().clear();

    t.set_value("vol", 0.25);
    t.set_checked("fs", true);
    t.type_text("name", "abc");
    t.type_text("name", "defg"); // cut to max_len
    t.step(1);

    assert_eq!(t.value("vol"), Some(UiValue::F32(0.25)));
    assert_eq!(t.value("fs"), Some(UiValue::Bool(true)));
    assert_eq!(t.text("name"), "abcd");
    // the backend shows the edit itself: nothing was journaled
    assert!(t.changes().is_empty(), "{:?}", t.changes());

    // out of range is clamped in the tree and in the signal
    let log = recorder(&mut t);
    t.set_value("vol", 7.0);
    t.step(1);
    assert_eq!(t.value("vol"), Some(UiValue::F32(1.0)));
    assert_eq!(log.borrow()[0].kind, UiSignalKind::Changed(UiValue::F32(1.0)));
    assert!(t.changes().is_empty());
}

#[test]
fn game_changes_after_writeback_are_normal_changes() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().slider(r, 0.0..=1.0, 0.5).key("vol");
    t.step(1);
    t.backend().clear();
    // a game system overrides the value the user just set
    t.engine.add_system(Stage::Update, |cx| {
        let sigs: Vec<_> = cx.ui_signals().cloned().collect();
        let ui = cx.resources.get_mut::<UiTree>().unwrap();
        for s in sigs {
            if let UiSignalKind::Changed(UiValue::F32(_)) = s.kind {
                ui.set_value(s.widget, 0.1);
            }
        }
    });
    t.set_value("vol", 0.9);
    t.step(1);
    assert_eq!(t.value("vol"), Some(UiValue::F32(0.1)));
    let id = t.id("vol");
    assert_eq!(t.changes(), vec![UiChange::Kind(id)]);
}

#[test]
fn disabled_and_invisible_widgets_send_nothing() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    let col = t.ui_mut().column(r).key("col").id();
    {
        let ui = t.ui_mut();
        ui.button(col, "a").key("a");
        ui.button(col, "b").key("b");
        ui.button(r, "c").key("c");
        ui.slider(col, 0.0..=1.0, 0.0).key("s");
    }
    let log = recorder(&mut t);
    let (a, b) = (t.id("a"), t.id("b"));
    t.ui_mut().set_enabled(a, false);
    t.ui_mut().set_visible(b, false);
    t.click("a");
    t.click("b");
    t.click("c");
    t.set_value("s", 0.5);
    t.step(1);
    assert_eq!(log.borrow().iter().map(|s| s.key_kind().0.to_string()).collect::<Vec<_>>(), ["c", "s"]);

    // an invisible / disabled ancestor silences the children too
    log.borrow_mut().clear();
    t.ui_mut().set_enabled(col, false);
    t.set_value("s", 0.7);
    t.step(1);
    assert!(log.borrow().is_empty());
    assert_eq!(t.value("s"), Some(UiValue::F32(0.5))); // and no write-back

    t.ui_mut().set_enabled(col, true);
    t.ui_mut().set_visible(col, false);
    t.click("a");
    t.step(1);
    assert!(log.borrow().is_empty());
}

#[test]
fn signals_that_do_not_fit_the_node_are_dropped() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    {
        let ui = t.ui_mut();
        ui.text(r, "label").key("label");
        ui.button(r, "b").key("b");
        ui.text_input(r, "").key("in");
        ui.slider(r, 0.0..=1.0, 0.0).key("s");
    }
    let log = recorder(&mut t);
    t.click("label");
    t.set_checked("s", true);
    t.signal("b", UiSignalKind::Submit("x".into()));
    t.signal("b", UiSignalKind::Hover(true)); // no opt in
    t.signal("in", UiSignalKind::Focus(true));
    t.step(1);
    let log = log.borrow();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, UiSignalKind::Focus(true));
}

#[test]
fn hover_only_for_opted_in_nodes() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().button(r, "b").hover_signals().key("b");
    let log = recorder(&mut t);
    t.hover("b", true);
    t.step(1);
    assert_eq!(log.borrow().len(), 1);
}

#[test]
fn signals_for_removed_widgets_are_dropped() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().button(r, "b").key("b");
    let log = recorder(&mut t);
    t.click("b");
    let id = t.id("b");
    t.ui_mut().remove(id);
    t.step(1);
    assert!(log.borrow().is_empty());
}

#[test]
fn mod_owned_signal_carries_owner() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().with_owner(Owner::module("m"), |ui| ui.button(r, "b").key("m/b").id());
    let log = recorder(&mut t);
    t.click("m/b");
    t.step(1);
    assert_eq!(log.borrow()[0].owner, Owner::module("m"));
}

// ----- bindings -------------------------------------------------------------------------------

#[derive(Default)]
struct Score {
    value: u32,
}

struct Health {
    hp: f32,
    max: f32,
}

#[test]
fn bindings_write_only_on_change() {
    let mut t = UiTest::new(|e| {
        e.insert_resource(Score::default());
    });
    let r = root(&mut t);
    {
        let ui = t.ui_mut();
        ui.text(r, Bind::res(|s: &Score| format!("Score: {}", s.value))).key("score");
    }
    t.step(1);
    assert_eq!(t.text("score"), "Score: 0");
    let id = t.id("score");
    t.backend().clear();

    t.step(3); // nothing changed: no entries
    assert!(t.changes().is_empty());

    t.resource_mut::<Score>().value = 5;
    t.step(1);
    assert_eq!(t.text("score"), "Score: 5");
    assert_eq!(t.changes(), vec![UiChange::Kind(id)]);

    // Same produced text: no entry even though the resource was written.
    t.backend().clear();
    t.resource_mut::<Score>().value = 5;
    t.step(2);
    assert!(t.changes().is_empty());
}

#[test]
fn bindings_for_visible_enabled_value_class_and_components() {
    let mut t = UiTest::new(|e| {
        e.insert_resource(Score::default());
    });
    let player = t.engine.world_mut().spawn((Health { hp: 50.0, max: 100.0 },));
    let r = root(&mut t);
    let (panel, bar, btn) = {
        let ui = t.ui_mut();
        let panel = ui.container(r).id();
        let bar = ui.slider(r, 0.0..=1.0, 0.0).id();
        let btn = ui.button(r, "b").id();
        ui.bind_visible(panel, Bind::res(|s: &Score| s.value > 0));
        ui.bind_value(bar, Bind::query::<Health>(player, |h| h.hp / h.max));
        ui.bind_enabled(btn, Bind::res(|s: &Score| s.value < 10));
        ui.bind_class(btn, Bind::res(|s: &Score| if s.value > 5 { "danger".to_string() } else { String::new() }));
        (panel, bar, btn)
    };
    t.step(1);
    assert_eq!(t.ui().visible(panel), Some(false));
    assert_eq!(t.ui().value(bar), Some(UiValue::F32(0.5)));
    assert_eq!(t.ui().enabled(btn), Some(true));

    t.resource_mut::<Score>().value = 7;
    t.engine.world_mut().query_one_mut::<&mut Health>(player).unwrap().hp = 100.0;
    t.step(1);
    assert_eq!(t.ui().visible(panel), Some(true));
    assert_eq!(t.ui().value(bar), Some(UiValue::F32(1.0)));
    assert!(t.ui().has_class(btn, "danger"));
    t.resource_mut::<Score>().value = 12;
    t.step(1);
    assert_eq!(t.ui().enabled(btn), Some(false));
}

#[test]
fn bindings_are_removed_with_the_widget() {
    let mut t = UiTest::new(|e| {
        e.insert_resource(Score::default());
    });
    let r = root(&mut t);
    let a = t.ui_mut().text(r, Bind::res(|s: &Score| s.value.to_string())).id();
    t.step(1);
    assert_eq!(t.resource::<UiBindings>().len(), 1);
    assert!(t.resource::<UiBindings>().is_bound(a));
    t.ui_mut().remove(a);
    t.step(1);
    assert_eq!(t.resource::<UiBindings>().len(), 0);
    t.resource_mut::<Score>().value = 1;
    t.step(1); // nothing to write to, no panic, consistent journal

    // created and removed before any frame: never registered
    let b = t.ui_mut().text(r, Bind::res(|s: &Score| s.value.to_string())).id();
    t.ui_mut().remove(b);
    t.step(1);
    assert_eq!(t.resource::<UiBindings>().len(), 0);
}

#[test]
fn bind_with_missing_resource_leaves_text() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().text(r, Bind::res(|s: &Score| s.value.to_string())).key("t");
    t.step(2);
    assert_eq!(t.text("t"), "");
    t.engine.insert_resource(Score { value: 3 });
    t.step(1);
    assert_eq!(t.text("t"), "3");
}

#[test]
fn bindings_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<UiBindings>();
    check::<Bind<String>>();
    check::<UiTree>();
    check::<HeadlessHandle>();
}

// ----- capture --------------------------------------------------------------------------------

#[test]
fn capture_follows_backend_and_modal_layers() {
    let mut t = UiTest::new(|_| {});
    t.step(1);
    assert_eq!(t.capture(), UiCapture::default());
    t.backend().set_capture(UiCapture { pointer: true, keyboard: false });
    t.step(1);
    assert_eq!(t.capture(), UiCapture { pointer: true, keyboard: false });
    t.backend().set_capture(UiCapture::default());
    let pause = t.ui_mut().layer("pause", Layer::centered(100).modal());
    t.step(1);
    assert_eq!(t.capture(), UiCapture { pointer: true, keyboard: true });
    t.ui_mut().set_visible(pause, false);
    t.step(1);
    assert_eq!(t.capture(), UiCapture::default());
}

// ----- headless consistency -------------------------------------------------------------------

#[test]
fn headless_records_the_journal_per_frame() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    let b = t.ui_mut().button(r, "x").id();
    t.step(1);
    assert_eq!(t.changes(), vec![UiChange::Created(r), UiChange::Created(b)]);
    t.ui_mut().set_text(b, "y");
    t.step(2);
    assert_eq!(t.backend().batches(), vec![2, 1]);
    assert_eq!(t.changes().len(), 3);
}

fn sync(be: &mut HeadlessBackend, tree: &UiTree, changes: &[UiChange]) {
    let mut cx = UiBackendCx::new((800.0, 600.0), 1.0);
    be.sync(tree, changes, tree.theme(), &mut cx);
}

#[test]
fn headless_detects_inconsistent_journals() {
    let mut ui = UiTree::new();
    let r = ui.layer("r", Layer::fill(0));
    let a = ui.button(r, "a").id();
    let changes = ui.take_changes();

    // a consistent journal is fine
    let (mut be, h) = HeadlessBackend::new();
    sync(&mut be, &ui, &changes);
    assert!(h.errors().is_empty(), "{:?}", h.errors());

    // a change for a removed id
    ui.remove(a);
    sync(&mut be, &ui, &[UiChange::Style(a)]);
    assert_eq!(h.errors().len(), 1);
    h.clear();

    // a change for an id the backend never saw
    let b = ui.button(r, "b").id();
    sync(&mut be, &ui, &[UiChange::Kind(b)]);
    assert_eq!(h.errors().len(), 1);
    h.clear();

    // created twice
    let (mut be2, h2) = HeadlessBackend::new();
    sync(&mut be2, &ui, &[UiChange::Created(r), UiChange::Created(r)]);
    assert_eq!(h2.errors().len(), 1);
    h2.clear();

    // Removed while the node is alive, and Removed of an unknown id
    sync(&mut be2, &ui, &[UiChange::Removed(r)]);
    assert_eq!(h2.errors().len(), 1);
    h2.clear();
    sync(&mut be2, &ui, &[UiChange::Removed(a)]);
    assert_eq!(h2.errors().len(), 1);
    h2.clear();

    // moved into a parent that does not exist
    let (mut be3, h3) = HeadlessBackend::new();
    sync(&mut be3, &ui, &[UiChange::Created(r), UiChange::Created(b), UiChange::Moved { id: b, parent: a, index: 0 }]);
    assert_eq!(h3.errors().len(), 1);
}

#[test]
#[should_panic(expected = "journal errors")]
fn ui_test_step_panics_on_inconsistent_journal() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.step(1);
    // A node whose Created entry was taken away: the backend never saw it.
    let ghost = {
        let ui = t.ui_mut();
        let g = ui.button(r, "ghost").id();
        ui.take_changes();
        g
    };
    t.ui_mut().set_text(ghost, "later"); // journal has Kind(ghost) but the backend never saw Created
    t.step(1);
}

#[test]
fn custom_backend_receives_calls() {
    #[derive(Default)]
    struct Log {
        init: u32,
        syncs: Vec<usize>,
        drains: u32,
    }
    struct Spy(Rc<RefCell<Log>>);
    impl UiBackend for Spy {
        fn init(&mut self, _: &mut UiBackendCx) {
            self.0.borrow_mut().init += 1;
        }
        fn sync(&mut self, _: &UiTree, changes: &[UiChange], _: &Theme, _: &mut UiBackendCx) {
            self.0.borrow_mut().syncs.push(changes.len());
        }
        fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
            self.0.borrow_mut().drains += 1;
            let _ = out;
        }
        fn capture(&self) -> UiCapture {
            UiCapture { pointer: true, keyboard: false }
        }
    }
    let log = Rc::new(RefCell::new(Log::default()));
    let mut e = Engine::with_config(EngineConfig::headless()).unwrap();
    e.add_plugin(UiPlugin::with_backend(Spy(log.clone()))).unwrap();
    let r = e.resources_mut().get_mut::<UiTree>().unwrap().layer("r", Layer::fill(0));
    e.resources_mut().get_mut::<UiTree>().unwrap().button(r, "b");
    e.run_frames(3, 1.0 / 60.0);
    let l = log.borrow();
    assert_eq!(l.init, 1);
    assert_eq!(l.syncs, vec![2]); // empty frames are not synced
    assert_eq!(l.drains, 3);
    drop(l);
    assert!(e.input().capture().pointer);
}

// ----- capture wiring and binding ticks -------------------------------------------------------

#[test]
fn captured_events_and_ui_capture_filter_game_input() {
    use efeengine_core::input::{ElementState, InputEvent, InputEventKind, KeyCode, MouseButton};

    // A backend that consumes mouse events and nothing else.
    struct Eats;
    impl UiBackend for Eats {
        fn init(&mut self, _: &mut UiBackendCx) {}
        fn input(&mut self, ev: &InputEvent) -> Filter {
            if matches!(ev.kind, InputEventKind::MouseButton { .. }) { Filter::Captured } else { Filter::Pass }
        }
        fn sync(&mut self, _: &UiTree, _: &[UiChange], _: &Theme, _: &mut UiBackendCx) {}
        fn drain_signals(&mut self, _: &mut Vec<UiSignal>) {}
        fn capture(&self) -> UiCapture {
            UiCapture::default()
        }
    }
    let seen = Rc::new(RefCell::new(Vec::<(bool, bool, bool)>::new()));
    let mut e = Engine::with_config(EngineConfig::headless()).unwrap();
    e.add_plugin(UiPlugin::with_backend(Eats)).unwrap();
    let s = seen.clone();
    e.add_system(Stage::Update, move |cx| {
        let g = cx.game_input();
        s.borrow_mut().push((g.mouse_just_pressed(MouseButton::Left), cx.input.mouse_just_pressed(MouseButton::Left), g.just_pressed(KeyCode::Space)));
    });
    e.inject(InputEvent::new(InputEventKind::MouseButton { button: MouseButton::Left, state: ElementState::Pressed, pos: (1.0, 1.0) }));
    e.inject(InputEvent::new(InputEventKind::key_pressed(KeyCode::Space)));
    e.run_frames(2, 1.0 / 60.0);
    // Frame 1: the pointer was consumed (game view false, raw view true), the key passed.
    assert_eq!(seen.borrow()[0], (false, true, true));
    // The capture is per frame: frame 2 has no consumed event.
    assert!(!e.input().capture().pointer);
}

#[test]
fn modal_layer_captures_through_input_state() {
    let mut t = UiTest::new(|_| {});
    t.ui_mut().layer("m", Layer::fill(5).modal());
    t.step(1);
    assert_eq!(t.engine.input().capture(), UiCapture { pointer: true, keyboard: true });
    assert_eq!(t.capture(), t.engine.input().capture());
}

#[test]
fn bindings_resource_tick_does_not_move_every_frame() {
    struct Score(u32);
    let ticks = Rc::new(RefCell::new(Vec::<bool>::new()));
    let tk = ticks.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(Score(1));
        e.add_system(Stage::Update, move |cx| tk.borrow_mut().push(cx.res_changed::<UiBindings>()));
    });
    let r = root(&mut t);
    t.ui_mut().text(r, Bind::res(|s: &Score| s.0.to_string())).key("score");
    t.step(4);
    assert_eq!(t.text("score"), "1");
    t.resource_mut::<Score>().0 = 2;
    t.step(2);
    assert_eq!(t.text("score"), "2");
    // Registered in frame 1 (absorbed in its PostUpdate), seen as changed in frame 2 at most; later frames quiet.
    let v = ticks.borrow().clone();
    assert!(v[2..].iter().all(|c| !c), "UiBindings tick moved every frame: {v:?}");
}

// ----- review regressions ---------------------------------------------------------------------

#[test]
fn signals_below_a_visible_modal_layer_are_dropped() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().button(r, "under").key("under");
    let modal = t.ui_mut().layer("modal", Layer::centered(10).modal());
    t.ui_mut().button(modal, "over").key("over");
    let above = t.ui_mut().layer("above", Layer::centered(20));
    t.ui_mut().button(above, "top").key("top");
    let log = recorder(&mut t);
    t.click("under");
    t.click("over");
    t.click("top");
    t.step(1);
    let keys: Vec<String> = log.borrow().iter().map(|s| s.key_kind().0.to_string()).collect();
    assert_eq!(keys, ["over", "top"]);
    // hidden modal: nothing blocked
    let m = t.id("modal");
    t.ui_mut().set_visible(m, false);
    t.click("under");
    t.step(1);
    assert_eq!(log.borrow().last().unwrap().key_kind().0, "under");
}

#[test]
fn blur_passes_for_disabled_or_hidden_widgets() {
    let mut t = UiTest::new(|_| {});
    let r = root(&mut t);
    t.ui_mut().text_input(r, "").key("in");
    let log = recorder(&mut t);
    let id = t.id("in");
    t.ui_mut().set_enabled(id, false);
    t.focus("in", true);
    t.focus("in", false);
    t.step(1);
    assert_eq!(log.borrow().iter().map(|s| s.kind.clone()).collect::<Vec<_>>(), vec![UiSignalKind::Focus(false)]);
    t.ui_mut().set_enabled(id, true);
    t.ui_mut().set_visible(id, false);
    t.focus("in", false);
    t.step(1);
    assert_eq!(log.borrow().len(), 2);
}
