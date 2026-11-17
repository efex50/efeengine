//! The view layer through `UiTest`: messages, reconciliation, conditions, slots.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use efeengine_core::prelude::*;
use efeengine_ui::view::*;
use efeengine_ui::{Layer, UiChange, UiTest, UiValue, column, row};

// ----- the settings menu ----------------------------------------------------------------------

#[derive(Default, Debug, Clone, PartialEq)]
struct MenuState {
    volume: f32,
    fullscreen: bool,
    name: String,
    submitted: Option<String>,
    started: bool,
    best: u32,
}

#[derive(Clone, Debug, PartialEq)]
enum MenuMsg {
    Play,
    Volume(f32),
    Name(String),
    Submit(String),
    Fullscreen(bool),
}

fn menu_view(s: &MenuState) -> El<MenuMsg> {
    column![
        text("Coin Dash").class("title").key("title"),
        button("Play").key("play").on_click(MenuMsg::Play),
        slider(0.0..=1.0, s.volume).step(0.05).key("volume").on_change(MenuMsg::Volume),
        checkbox("Fullscreen", s.fullscreen).key("fullscreen").on_toggle(MenuMsg::Fullscreen),
        text_input(&s.name)
            .placeholder("Player name")
            .max_len(8)
            .key("name")
            .on_input(MenuMsg::Name)
            .on_submit(MenuMsg::Submit),
        text(format!("Best: {}", s.best)).key("best"),
    ]
    .class("panel")
    .gap(12.0)
}

fn menu_update(cx: &mut Ctx) {
    let msgs: Vec<MenuMsg> = cx.events::<MenuMsg>().iter().cloned().collect();
    for m in msgs {
        let s = cx.resources.get_mut::<MenuState>().unwrap();
        match m {
            MenuMsg::Play => {
                s.started = true;
                s.best += 1;
            }
            MenuMsg::Volume(v) => s.volume = v,
            MenuMsg::Name(n) => s.name = n,
            MenuMsg::Submit(n) => s.submitted = Some(n),
            MenuMsg::Fullscreen(f) => s.fullscreen = f,
        }
    }
}

fn menu_test() -> UiTest {
    UiTest::new(|e| {
        e.insert_resource(MenuState { volume: 0.8, ..Default::default() });
        e.add_plugin(UiView::new("menu", menu_view).layer(Layer::centered(10))).unwrap();
        e.add_system(Stage::Update, menu_update);
    })
}

#[test]
fn menu_builds_the_tree_with_keys_and_owner() {
    let mut t = menu_test();
    t.step(1);
    assert_eq!(t.text("menu/title"), "Coin Dash");
    assert_eq!(t.text("menu/play"), "Play");
    assert_eq!(t.value("menu/volume"), Some(UiValue::F32(0.8)));
    assert_eq!(t.text("menu/best"), "Best: 0");
    let ui = t.ui();
    let layer = ui.find("menu").unwrap();
    assert_eq!(ui.layer_data(layer), Some(Layer::centered(10)));
    let owner = efeengine_ui::Owner::plugin("efeengine-ui/view/menu");
    for id in ui.subtree(layer) {
        assert_eq!(ui.owner_of(id), Some(&owner));
    }
    let col = ui.children(layer)[0];
    assert!(ui.has_class(col, "panel"));
    assert_eq!(ui.children(col).len(), 6);
}

#[test]
fn click_becomes_msg_then_state_then_next_view_run() {
    let mut t = menu_test();
    t.step(1);
    t.click("menu/play");
    t.step(1);
    // Msg delivered and handled in the same frame, the view shows it in the same frame's PostUpdate.
    assert!(t.resource::<MenuState>().started);
    assert_eq!(t.text("menu/best"), "Best: 1");
    t.click("menu/play");
    t.step(1);
    assert_eq!(t.text("menu/best"), "Best: 2");
}

#[test]
fn slider_checkbox_and_text_input_messages() {
    let mut t = menu_test();
    t.step(1);
    t.set_value("menu/volume", 0.35);
    t.set_checked("menu/fullscreen", true);
    t.type_text("menu/name", "Ada");
    t.step(1);
    {
        let s = t.resource::<MenuState>();
        assert_eq!(s.volume, 0.35);
        assert!(s.fullscreen);
        assert_eq!(s.name, "Ada");
        assert_eq!(s.submitted, None);
    }
    t.type_text("menu/name", "mo");
    t.submit("menu/name");
    t.step(1);
    let s = t.resource::<MenuState>().clone();
    assert_eq!(s.name, "Adamo");
    assert_eq!(s.submitted.as_deref(), Some("Adamo"));
    assert_eq!(t.text("menu/name"), "Adamo");
}

#[test]
fn accepted_edits_make_no_tree_change_and_unchanged_reruns_are_silent() {
    let mut t = menu_test();
    t.step(1);
    t.backend().clear();
    t.step(3);
    assert!(t.changes().is_empty(), "unchanged view wrote {:?}", t.changes());
    assert!(t.ui().changes().is_empty());
    // An accepted edit: the plugin wrote the value back, the view finds it equal.
    t.set_value("menu/volume", 0.5);
    t.type_text("menu/name", "x");
    t.step(1);
    assert!(t.changes().is_empty(), "accepted edit produced {:?}", t.changes());
}

#[test]
fn rejected_and_clamped_values_are_written_back() {
    let mut t = UiTest::new(|e| {
        e.insert_resource(MenuState { volume: 0.2, ..Default::default() });
        e.add_plugin(UiView::new("menu", menu_view)).unwrap();
        // update clamps the volume to 0.5 and refuses names with an x.
        e.add_system(Stage::Update, |cx| {
            let msgs: Vec<MenuMsg> = cx.events::<MenuMsg>().iter().cloned().collect();
            for m in msgs {
                let s = cx.resources.get_mut::<MenuState>().unwrap();
                match m {
                    MenuMsg::Volume(v) => s.volume = v.min(0.5),
                    MenuMsg::Name(n) if !n.contains('x') => s.name = n,
                    _ => {}
                }
            }
        });
    });
    t.step(1);
    t.backend().clear();
    t.set_value("menu/volume", 0.9);
    t.step(1);
    assert_eq!(t.value("menu/volume"), Some(UiValue::F32(0.5)));
    let id = t.id("menu/volume");
    assert_eq!(t.changes(), vec![UiChange::Kind(id)]);
    t.type_text("menu/name", "ab");
    t.step(1);
    t.type_text("menu/name", "x");
    t.step(1);
    assert_eq!(t.text("menu/name"), "ab");
}

#[test]
fn a_rejected_edit_is_written_back_even_under_when_changed() {
    let mut t = UiTest::new(|e| {
        e.insert_resource(MenuState::default());
        e.add_plugin(UiView::new("menu", menu_view).when(changed::<MenuState>())).unwrap();
        // update refuses every name: the state never changes.
        e.add_system(Stage::Update, |cx| {
            let _ = cx.events::<MenuMsg>().len();
        });
    });
    t.step(2);
    t.type_text("menu/name", "ab");
    t.step(1);
    assert_eq!(t.text("menu/name"), "");
}

#[test]
fn text_input_value_is_cut_to_max_len() {
    let mut t = menu_test();
    t.step(1);
    t.type_text("menu/name", "abcdefghijkl");
    t.step(1);
    assert_eq!(t.text("menu/name"), "abcdefgh");
    assert_eq!(t.resource::<MenuState>().name, "abcdefgh");
}

// ----- same frame -----------------------------------------------------------------------------

#[test]
fn signal_to_msg_is_readable_by_update_systems_in_the_same_step() {
    #[derive(Clone, Debug, PartialEq)]
    struct Hit;
    let seen = Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(0u8);
        e.add_plugin(UiView::new("v", |_: &u8| column![button("b").key("b").on_click(Hit)])).unwrap();
        e.add_system(Stage::Update, move |cx| {
            s.fetch_add(cx.events::<Hit>().len(), Ordering::SeqCst);
        });
    });
    t.step(1);
    assert_eq!(seen.load(Ordering::SeqCst), 0);
    t.click("v/b");
    t.step(1);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    t.step(1); // not delivered again
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[test]
fn widgets_without_handlers_send_nothing() {
    #[derive(Clone)]
    struct Never;
    let seen = Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(0u8);
        e.add_plugin(UiView::new("v", |_: &u8| -> El<()> { column![button("b").key("b"), text("x")] })).unwrap();
        e.add_system(Stage::Update, move |cx| {
            s.fetch_add(cx.events::<Never>().len(), Ordering::SeqCst);
        });
    });
    t.step(1);
    t.click("v/b");
    t.step(2);
    assert_eq!(seen.load(Ordering::SeqCst), 0);
}

#[test]
fn closure_handlers_and_map() {
    #[derive(Clone, Debug, PartialEq)]
    enum Inner {
        Pick(u32),
    }
    #[derive(Clone, Debug, PartialEq)]
    enum Outer {
        Left(Inner),
        Right(Inner),
    }
    fn component(base: u32) -> El<Inner> {
        // A closure capturing `base`: not a fn pointer.
        slider(0.0..=100.0, 0.0).key("s").on_change(move |v| Inner::Pick(base + v as u32))
    }
    let log = Arc::new(std::sync::Mutex::new(Vec::<Outer>::new()));
    let l = log.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(0u8);
        e.add_plugin(UiView::new("v", |_: &u8| {
            column![
                column![component(100).map(Outer::Left)].key("l"),
                column![component(200).map(Outer::Right)].key("r"),
            ]
        }))
        .unwrap();
        e.add_system(Stage::Update, move |cx| l.lock().unwrap().extend(cx.events::<Outer>().iter().cloned()));
    });
    t.step(1);
    t.set_value("v/l/s", 5.0);
    t.set_value("v/r/s", 7.0);
    t.step(1);
    assert_eq!(*log.lock().unwrap(), vec![Outer::Left(Inner::Pick(105)), Outer::Right(Inner::Pick(207))]);
}

// ----- reconciliation -------------------------------------------------------------------------

#[derive(Default)]
struct Items(Vec<(u32, String)>);

fn items_view(s: &Items) -> El<()> {
    column![
        text("Items").key("head"),
        s.0.iter().map(|(id, name)| row![text(name.clone()).key("name"), button("x").key("del")].key(format!("item-{id}"))),
        text("end").key("end"),
    ]
}

fn items_test(items: &[(u32, &str)]) -> UiTest {
    let v = items.iter().map(|(i, n)| (*i, n.to_string())).collect();
    UiTest::new(|e| {
        e.insert_resource(Items(v));
        e.add_plugin(UiView::new("inv", items_view)).unwrap();
    })
}

fn order_of(t: &UiTest) -> Vec<String> {
    let ui = t.ui();
    let col = ui.children(ui.find("inv").unwrap())[0];
    ui.children(col).iter().map(|c| ui.key_of(*c).unwrap().to_string()).collect()
}

#[test]
fn keyed_list_insert_remove_reorder_keep_ids() {
    let mut t = items_test(&[(1, "a"), (2, "b"), (3, "c")]);
    t.step(1);
    assert_eq!(order_of(&t), ["inv/head", "inv/item-1", "inv/item-2", "inv/item-3", "inv/end"]);
    assert_eq!(t.text("inv/item-2/name"), "b");
    let (a, b, c) = (t.id("inv/item-1"), t.id("inv/item-2"), t.id("inv/item-3"));
    let b_del = t.id("inv/item-2/del");

    // insert at the front, remove the middle one
    t.resource_mut::<Items>().0 = vec![(9, "new".into()), (1, "a".into()), (3, "c".into())];
    t.step(1);
    assert_eq!(order_of(&t), ["inv/head", "inv/item-9", "inv/item-1", "inv/item-3", "inv/end"]);
    assert_eq!(t.id("inv/item-1"), a);
    assert_eq!(t.id("inv/item-3"), c);
    assert!(t.ui().get(b).is_none() && t.ui().get(b_del).is_none());
    assert!(t.ui().find("inv/item-2").is_none());

    // reorder: ids kept, journal has Moved and no Created/Removed
    t.resource_mut::<Items>().0 = vec![(3, "c".into()), (1, "a".into()), (9, "new".into())];
    t.backend().clear();
    t.step(1);
    assert_eq!(order_of(&t), ["inv/head", "inv/item-3", "inv/item-1", "inv/item-9", "inv/end"]);
    assert_eq!(t.id("inv/item-1"), a);
    assert_eq!(t.id("inv/item-3"), c);
    let ch = t.changes();
    assert!(!ch.is_empty());
    assert!(ch.iter().all(|c| matches!(c, UiChange::Moved { .. })), "{ch:?}");

    // property change in a kept row: only that row's text
    t.resource_mut::<Items>().0 = vec![(3, "C!".into()), (1, "a".into()), (9, "new".into())];
    t.backend().clear();
    t.step(1);
    assert_eq!(t.changes(), vec![UiChange::Kind(t.id("inv/item-3/name"))]);
}

#[test]
fn an_edited_and_focused_input_keeps_its_id_while_the_list_changes() {
    #[derive(Default)]
    struct Rows(Vec<u32>, String);
    let mut t = UiTest::new(|e| {
        e.insert_resource(Rows(vec![1, 2], String::new()));
        e.add_plugin(UiView::<()>::new("f", |s: &Rows| {
            column![s.0.iter().map(|i| text_input(if *i == 2 { s.1.as_str() } else { "" }).key(format!("in-{i}")))]
        }))
        .unwrap();
    });
    t.step(1);
    let id = t.id("f/in-2");
    t.focus("f/in-2", true);
    t.type_text("f/in-2", "hello");
    t.step(1);
    // The edit was written back by the plugin; the view still says "" (no handler), so the
    // controlled value snaps back, but the node (and so focus) is the same.
    assert_eq!(t.id("f/in-2"), id);
    t.resource_mut::<Rows>().0 = vec![0, 1, 2];
    t.resource_mut::<Rows>().1 = "kept".into();
    t.step(1);
    assert_eq!(t.id("f/in-2"), id);
    assert_eq!(t.text("f/in-2"), "kept");
}

#[test]
fn kind_change_replaces_the_node() {
    struct Mode(bool);
    let mut t = UiTest::new(|e| {
        e.insert_resource(Mode(false));
        e.add_plugin(UiView::<()>::new("k", |m: &Mode| {
            column![if m.0 { button("go").key("w") } else { text("hi").key("w") }, text("tail").key("t")]
        }))
        .unwrap();
    });
    t.step(1);
    let old = t.id("k/w");
    let tail = t.id("k/t");
    t.resource_mut::<Mode>().0 = true;
    t.step(1);
    let new = t.id("k/w");
    assert_ne!(old, new);
    assert!(t.ui().get(old).is_none());
    assert_eq!(t.id("k/t"), tail);
    assert!(matches!(t.ui().kind(new), Some(efeengine_ui::NodeKind::Button { .. })));
    // position order kept
    let ui = t.ui();
    let col = ui.children(ui.find("k").unwrap())[0];
    assert_eq!(ui.children(col), [new, tail]);
}

#[test]
fn unkeyed_children_match_by_position_and_update_in_place() {
    struct N(u32);
    let mut t = UiTest::new(|e| {
        e.insert_resource(N(1));
        e.add_plugin(UiView::<()>::new("p", |n: &N| column![text(format!("a{}", n.0)), text("b"), text("c").visible(n.0 < 2)])).unwrap();
    });
    t.step(1);
    let ui = t.ui();
    let col = ui.children(ui.find("p").unwrap())[0];
    let kids: Vec<_> = ui.children(col).to_vec();
    drop(ui);
    t.resource_mut::<N>().0 = 2;
    t.backend().clear();
    t.step(1);
    assert_eq!(t.ui().children(col), &kids[..]);
    assert_eq!(t.ui().text_of(kids[0]), Some("a2"));
    assert_eq!(t.ui().visible(kids[2]), Some(false));
    assert_eq!(t.changes().len(), 2);
}

#[test]
fn duplicate_keys_panic_before_anything_is_applied() {
    struct Bad(bool);
    let mut t = UiTest::new(|e| {
        e.insert_resource(Bad(false));
        e.add_plugin(UiView::<()>::new("d", |b: &Bad| {
            if b.0 { column![text("a").key("x"), text("b").key("x")] } else { column![text("fine").key("x")] }
        }))
        .unwrap();
    });
    t.step(1);
    t.resource_mut::<Bad>().0 = true;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.engine.run_frames(1, 1.0 / 60.0)));
    assert!(r.is_err());
    assert_eq!(t.text("d/x"), "fine");
    assert!(t.ui().changes().is_empty());
}

// ----- conditions, enabling, slots ------------------------------------------------------------

struct A(u32);
struct B(u32);

fn counting_view(runs: Arc<AtomicUsize>) -> UiView<()> {
    UiView::with("c", move |cx| {
        runs.fetch_add(1, Ordering::SeqCst);
        let a = cx.res::<A>().unwrap().0;
        let b = cx.res::<B>().unwrap().0;
        text(format!("{a}/{b}")).key("t")
    })
}

#[test]
fn when_changed_skips_unchanged_frames_and_runs_on_change() {
    let runs = Arc::new(AtomicUsize::new(0));
    let r = runs.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(A(0));
        e.insert_resource(B(0));
        e.add_plugin(counting_view(r).when(changed::<A>())).unwrap();
    });
    t.step(1);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    t.step(5);
    assert_eq!(runs.load(Ordering::SeqCst), 1, "unchanged state must not rebuild");
    t.resource_mut::<B>().0 = 9; // not watched
    t.step(2);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    t.resource_mut::<A>().0 = 4;
    t.step(1);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(t.text("c/t"), "4/9");
    t.step(3);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn several_conditions_are_ored() {
    let runs = Arc::new(AtomicUsize::new(0));
    let r = runs.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(A(0));
        e.insert_resource(B(0));
        e.add_plugin(counting_view(r).when(changed::<A>()).when(changed::<B>())).unwrap();
    });
    t.step(3);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    t.resource_mut::<B>().0 = 1;
    t.step(1);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    t.resource_mut::<A>().0 = 1;
    t.step(1);
    assert_eq!(runs.load(Ordering::SeqCst), 3);
    // the explicit `or` form
    let c = changed::<A>().or(changed::<B>());
    let _ = counting_view(Arc::new(AtomicUsize::new(0))).when(c);
}

#[test]
fn disabled_view_hides_its_layer_and_does_not_run() {
    let runs = Arc::new(AtomicUsize::new(0));
    let r = runs.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(A(0));
        e.insert_resource(B(0));
        e.add_plugin(counting_view(r)).unwrap();
    });
    t.step(2);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    let layer = t.id("c");
    assert_eq!(t.ui().visible(layer), Some(true));
    t.resource_mut::<UiViews>().set_enabled("c", false);
    t.step(1);
    assert_eq!(t.ui().visible(layer), Some(false));
    t.step(4);
    assert_eq!(runs.load(Ordering::SeqCst), 2, "a disabled view must not run");
    // state changes while disabled show up as soon as it is enabled again
    t.resource_mut::<A>().0 = 7;
    t.resource_mut::<UiViews>().set_enabled("c", true);
    t.step(1);
    assert_eq!(t.ui().visible(layer), Some(true));
    assert_eq!(runs.load(Ordering::SeqCst), 3);
    assert_eq!(t.text("c/t"), "7/0");
}

#[test]
fn view_enabled_false_from_the_start_creates_nothing_until_enabled() {
    let runs = Arc::new(AtomicUsize::new(0));
    let r = runs.clone();
    let mut t = UiTest::new(|e| {
        e.insert_resource(A(0));
        e.insert_resource(B(0));
        e.add_plugin(counting_view(r).enabled(false)).unwrap();
    });
    t.step(3);
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert!(t.ui().find("c").is_none());
    t.resource_mut::<UiViews>().set_enabled("c", true);
    t.step(1);
    assert_eq!(t.text("c/t"), "0/0");
}

#[test]
fn slot_children_survive_reruns() {
    struct Tick(u32);
    let mut t = UiTest::new(|e| {
        e.insert_resource(Tick(0));
        e.add_plugin(UiView::<()>::new("s", |n: &Tick| column![text(format!("t{}", n.0)).key("t"), slot("mods"), text("tail").key("tail")])).unwrap();
    });
    t.step(1);
    let slot_id = t.id("s/mods");
    let child = {
        let ui = t.ui_mut();
        ui.text(slot_id, "from a mod").key("mod/label").id()
    };
    t.resource_mut::<Tick>().0 = 1;
    t.step(2);
    assert_eq!(t.id("s/mods"), slot_id);
    assert_eq!(t.ui().children(slot_id), [child]);
    assert_eq!(t.text("s/t"), "t1");
    t.backend().clear();
    t.step(2);
    assert!(t.changes().is_empty());
}

#[test]
fn view_owns_its_layer_and_overwrites_foreign_edits() {
    let mut t = UiTest::new(|e| {
        e.insert_resource(A(1));
        e.add_plugin(UiView::new("o", |a: &A| -> El<()> { column![text(format!("{}", a.0)).key("t")] })).unwrap();
    });
    t.step(1);
    let id = t.id("o/t");
    t.ui_mut().set_text(id, "hacked");
    let stray = {
        let ui = t.ui_mut();
        let col = ui.children(ui.find("o").unwrap())[0];
        ui.text(col, "stray").id()
    };
    t.step(1);
    assert_eq!(t.text("o/t"), "1");
    assert!(t.ui().get(stray).is_none());
}

// ----- API.md 16.1 ----------------------------------------------------------------------------

mod api_example {
    use super::*;
    use efeengine_ui::column;

    #[derive(Default)]
    struct MenuState {
        volume: f32,
        fullscreen: bool,
        name: String,
        best: u32,
        start: bool,
    }

    #[derive(Clone, Debug)]
    enum MenuMsg {
        Play,
        Volume(f32),
        Name(String),
        Fullscreen(bool),
    }

    fn menu_view(s: &MenuState) -> El<MenuMsg> {
        column![
            text("Coin Dash").class("title"),
            button("Play").key("play").on_click(MenuMsg::Play),
            slider(0.0..=1.0, s.volume).step(0.05).key("volume").on_change(MenuMsg::Volume),
            checkbox("Fullscreen", s.fullscreen).key("fullscreen").on_toggle(MenuMsg::Fullscreen),
            text_input(&s.name).placeholder("Player name").max_len(16).key("name").on_input(MenuMsg::Name),
            text(format!("Best: {}", s.best)).key("best"),
        ]
        .class("panel")
        .gap(12.0)
    }

    #[test]
    fn the_16_1_example_runs() {
        let mut t = UiTest::new(|engine| {
            engine.insert_resource(MenuState::default());
            engine.add_plugin(UiView::new("menu", menu_view).layer(Layer::centered(10))).unwrap();
            engine.add_system(Stage::Update, |cx| {
                let msgs: Vec<MenuMsg> = cx.events::<MenuMsg>().iter().cloned().collect();
                for m in msgs {
                    let s = cx.resources.get_mut::<MenuState>().unwrap();
                    match m {
                        MenuMsg::Play => s.start = true,
                        MenuMsg::Volume(v) => s.volume = v,
                        MenuMsg::Name(n) => s.name = n,
                        MenuMsg::Fullscreen(f) => s.fullscreen = f,
                    }
                }
            });
        });
        t.step(1);
        t.click("menu/play");
        t.set_value("menu/volume", 0.5);
        t.set_checked("menu/fullscreen", true);
        t.type_text("menu/name", "Ada");
        t.step(1);
        let s = t.resource::<MenuState>();
        assert!(s.start && s.fullscreen);
        assert_eq!((s.volume, s.name.as_str()), (0.5, "Ada"));
    }
}
