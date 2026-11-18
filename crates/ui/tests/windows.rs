//! UI layers per window: `Layer::window`, `UiTree::layer_in`, one backend instance per window,
//! input routing and modal scoping. Headless (no window system).

use std::cell::RefCell;
use std::rc::Rc;

use efeengine_core::input::{InputEvent, InputEventKind, MouseButton, ElementState, WindowId};
use efeengine_core::prelude::*;
use efeengine_core::window::WindowDesc;
use efeengine_ui::*;

const DT: f64 = 1.0 / 60.0;

#[derive(Default)]
struct Log {
    /// (window, what)
    entries: Vec<(u32, String)>,
}

/// A backend that records what it is told, per window.
struct Rec {
    window: u32,
    log: Rc<RefCell<Log>>,
    signals: Vec<UiSignal>,
}

impl Rec {
    fn new(log: &Rc<RefCell<Log>>) -> Self {
        Self { window: 0, log: log.clone(), signals: Vec::new() }
    }
    fn note(&self, s: String) {
        self.log.borrow_mut().entries.push((self.window, s));
    }
}

impl UiBackend for Rec {
    fn init(&mut self, cx: &mut UiBackendCx) {
        self.window = cx.window;
        self.note(format!("init {:?}", cx.logical_size));
    }
    fn input(&mut self, ev: &InputEvent) -> Filter {
        self.note(format!("input window {}", ev.window.0));
        if let InputEventKind::MouseButton { state: ElementState::Pressed, .. } = ev.kind {
            return Filter::Captured;
        }
        Filter::Pass
    }
    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], _theme: &Theme, _cx: &mut UiBackendCx) {
        // what this instance would draw
        let mine = tree.layers_in(self.window).len();
        self.note(format!("sync {} changes, {mine} layers of mine", changes.len()));
    }
    fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
        out.append(&mut self.signals);
    }
    fn capture(&self) -> UiCapture {
        UiCapture::default()
    }
    fn resize(&mut self, logical: (f32, f32), scale: f32) {
        self.note(format!("resize {logical:?} @{scale}"));
    }
}

fn setup() -> (Engine, Rc<RefCell<Log>>) {
    let log = Rc::new(RefCell::new(Log::default()));
    let mut engine = Engine::with_config(EngineConfig { fixed_dt: DT, ..EngineConfig::headless() }).unwrap();
    let l = log.clone();
    let l2 = log.clone();
    engine
        .add_plugin(UiPlugin::with_backend(Rec::new(&l)).with_window_backends(move |_| Some(Box::new(Rec::new(&l2)))))
        .unwrap();
    (engine, log)
}

fn notes(log: &Rc<RefCell<Log>>, window: u32) -> Vec<String> {
    log.borrow().entries.iter().filter(|(w, _)| *w == window).map(|(_, s)| s.clone()).collect()
}

#[test]
fn layer_in_sets_the_window_and_scopes_queries() {
    let mut ui = UiTree::new();
    let a = ui.layer("a", Layer::top_left(0));
    let b = ui.layer_in(WindowId(2), "b", Layer::top_left(5));
    let c = ui.layer("c", Layer::fill(3).window(2u32));
    let child = ui.text(b, "hello").id();
    assert_eq!(ui.layer_data(a).unwrap().window, 0);
    assert_eq!(ui.layer_data(b).unwrap().window, 2);
    assert_eq!(ui.window_of(child), Some(2));
    assert_eq!(ui.layers_in(0), vec![a]);
    assert_eq!(ui.layers_in(2), vec![c, b], "by z within the window");
    assert_eq!(ui.layer_windows(), vec![0, 2]);
    // moving a layer to another window is a journal entry
    ui.take_changes();
    ui.set_layer(a, Layer::top_left(0).window(2u32));
    assert_eq!(ui.take_changes(), vec![UiChange::Layer(a)]);
    assert_eq!(ui.layers_in(0), vec![]);
}

#[test]
fn modal_layers_only_block_their_own_window() {
    let mut ui = UiTree::new();
    let main = ui.layer("main", Layer::top_left(0));
    let btn_main = ui.button(main, "main").id();
    let map = ui.layer_in(WindowId(1), "map", Layer::top_left(0));
    let btn_map = ui.button(map, "map").id();
    ui.layer_in(WindowId(1), "dialog", Layer::fill(10).modal());
    assert!(ui.modal_active());
    assert!(ui.is_below_modal(btn_map));
    assert!(!ui.is_below_modal(btn_main), "the modal dialog is in another window");
}

#[test]
fn a_window_gets_its_own_backend_once_it_exists_and_has_a_layer() {
    let (mut engine, log) = setup();
    engine.run_frames(2, DT);
    assert_eq!(notes(&log, 0)[0], "init (0.0, 0.0)");
    let map = engine.create_window(WindowDesc::new("Map").size(200, 100)).unwrap();
    engine.run_frames(2, DT);
    assert!(notes(&log, map.0).is_empty(), "no layer in the window yet, no instance");
    {
        let ui = engine.resources_mut().get_mut::<UiTree>().unwrap();
        let l = ui.layer_in(map, "hud", Layer::top_left(0));
        ui.text(l, "coins");
        ui.layer("main-hud", Layer::top_left(0));
    }
    engine.run_frames(2, DT);
    let n = notes(&log, map.0);
    assert_eq!(n[0], "init (200.0, 100.0)", "{n:?}");
    // the new instance was synced with the whole tree and sees its own layer only
    assert!(n.iter().any(|s| s == "sync 6 changes, 1 layers of mine") || n.iter().any(|s| s.starts_with("sync") && s.ends_with("1 layers of mine")), "{n:?}");
    // later changes reach every instance
    let main_notes = notes(&log, 0);
    assert!(main_notes.iter().any(|s| s.starts_with("sync") && s.ends_with("1 layers of mine")), "{main_notes:?}");
}

#[test]
fn input_goes_to_the_backend_of_its_window_and_capture_is_merged() {
    let (mut engine, log) = setup();
    let map = engine.create_window(WindowDesc::new("Map").size(200, 100)).unwrap();
    engine.resources_mut().get_mut::<UiTree>().unwrap().layer_in(map, "hud", Layer::top_left(0));
    engine.run_frames(2, DT);
    log.borrow_mut().entries.clear();
    let press = |w: WindowId| InputEvent::with_time(InputEventKind::MouseButton { button: MouseButton::Left, state: ElementState::Pressed, pos: (1.0, 1.0) }, 0, w);
    engine.inject(press(map));
    engine.run_frames(1, DT);
    assert_eq!(notes(&log, map.0), vec!["input window 1"]);
    assert!(notes(&log, 0).is_empty());
    // the UI of that window consumed it: the game does not see the press
    assert!(engine.input().capture().pointer);
    engine.inject(press(WindowId::MAIN));
    engine.run_frames(1, DT);
    assert!(notes(&log, 0).contains(&"input window 0".to_string()));
}

#[test]
fn a_closed_window_drops_its_backend_and_a_reopened_layer_makes_a_new_one() {
    let (mut engine, log) = setup();
    let map = engine.create_window(WindowDesc::new("Map")).unwrap();
    engine.resources_mut().get_mut::<UiTree>().unwrap().layer_in(map, "hud", Layer::top_left(0));
    engine.run_frames(2, DT);
    assert!(!notes(&log, map.0).is_empty());
    engine.close_window(map);
    engine.run_frames(2, DT);
    let before = notes(&log, map.0).len();
    // the layer stays in the tree (it may come back with a new window): nothing is sent anywhere
    engine.resources_mut().get_mut::<UiTree>().unwrap().layer_in(map, "hud2", Layer::top_left(0));
    engine.run_frames(2, DT);
    assert_eq!(notes(&log, map.0).len(), before);
}

#[test]
fn headless_backend_records_every_window_in_one_instance() {
    let mut t = UiTest::new(|e| {
        e.create_window(WindowDesc::new("Map")).unwrap();
    });
    t.ui_mut().layer("main", Layer::fill(0));
    t.ui_mut().layer_in(WindowId(1), "map", Layer::fill(0));
    t.step(2);
    let created = t.changes().iter().filter(|c| matches!(c, UiChange::Created(_))).count();
    assert_eq!(created, 2);
}
