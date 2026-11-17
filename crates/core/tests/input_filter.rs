//! Input filters, captured events, TextEditing, SetTextInputArea, render overlay bookkeeping.
//! Headless: no window, no GPU.

use std::cell::RefCell;
use std::rc::Rc;

use efeengine_core::input::*;
use efeengine_core::prelude::*;
use efeengine_core::render::{Gpu, RenderOverlay};

const DT: f64 = 1.0 / 60.0;

fn engine() -> Engine {
    Engine::with_config(EngineConfig { fixed_dt: DT, ..EngineConfig::headless() }).expect("headless engine")
}

/// Captures every key event of `KeyCode::A`-and-friends while the shared flag is on, and all
/// mouse / wheel / text events too.
struct Flag(Rc<RefCell<bool>>);
impl InputFilter for Flag {
    fn filter(&mut self, ev: &InputEvent) -> Filter {
        match ev.kind {
            InputEventKind::Window(_) => Filter::Pass,
            _ if *self.0.borrow() => Filter::Captured,
            _ => Filter::Pass,
        }
    }
}

fn step(e: &mut Engine, kinds: Vec<InputEventKind>) {
    for k in kinds {
        e.inject(InputEvent::new(k));
    }
    e.run_frames(1, DT);
}

fn flagged() -> (Engine, Rc<RefCell<bool>>) {
    let mut e = engine();
    let flag = Rc::new(RefCell::new(false));
    e.add_input_filter(Flag(flag.clone()));
    (e, flag)
}

fn btn(b: MouseButton, s: ElementState) -> InputEventKind {
    InputEventKind::MouseButton { button: b, state: s, pos: (1.0, 1.0) }
}

#[test]
fn events_are_marked_and_filters_all_run_in_order() {
    let mut e = engine();
    let log = Rc::new(RefCell::new(Vec::<(u8, bool)>::new()));
    struct F(u8, Filter, Rc<RefCell<Vec<(u8, bool)>>>);
    impl InputFilter for F {
        fn filter(&mut self, ev: &InputEvent) -> Filter {
            assert!(!ev.captured, "the flag is set after all filters ran");
            if matches!(ev.kind, InputEventKind::Text(_)) {
                self.2.borrow_mut().push((self.0, self.1 == Filter::Captured));
            }
            if matches!(ev.kind, InputEventKind::Text(_)) { self.1 } else { Filter::Pass }
        }
    }
    e.add_input_filter(F(1, Filter::Pass, log.clone())).add_input_filter(F(2, Filter::Captured, log.clone()));
    step(&mut e, vec![InputEventKind::Text("x".into())]);
    assert_eq!(*log.borrow(), vec![(1, false), (2, true)]);
    let evs = e.input().events();
    let text = evs.iter().find(|ev| matches!(ev.kind, InputEventKind::Text(_))).unwrap();
    assert!(text.captured);
    // Raw text still there, game text empty.
    assert_eq!(e.input().text(), "x");
    assert_eq!(e.input().game_input().text(), "");
}

#[test]
fn no_filter_means_nothing_captured() {
    let mut e = engine();
    step(&mut e, vec![InputEventKind::key_pressed(KeyCode::A)]);
    assert!(e.input().events().iter().all(|ev| !ev.captured));
    assert!(e.input().game_input().just_pressed(KeyCode::A));
}

#[test]
fn captured_press_is_invisible_to_game_but_raw_sees_it_and_release_never_sticks() {
    let (mut e, flag) = flagged();
    *flag.borrow_mut() = true;
    step(&mut e, vec![InputEventKind::key_pressed(KeyCode::A)]);
    assert!(e.input().pressed(KeyCode::A) && e.input().just_pressed(KeyCode::A));
    let g = e.input().game_input();
    assert!(!g.pressed(KeyCode::A) && !g.just_pressed(KeyCode::A) && !g.any_pressed());
    // Filter turns off while held: the release is not a game release, and nothing sticks.
    *flag.borrow_mut() = false;
    step(&mut e, vec![]);
    assert!(!e.input().game_input().pressed(KeyCode::A));
    step(&mut e, vec![InputEventKind::key_released(KeyCode::A)]);
    assert!(e.input().just_released(KeyCode::A));
    let g = e.input().game_input();
    assert!(!g.just_released(KeyCode::A) && !g.pressed(KeyCode::A));
    // A new, uncaptured press works normally.
    step(&mut e, vec![InputEventKind::key_pressed(KeyCode::A)]);
    assert!(e.input().game_input().pressed(KeyCode::A) && e.input().game_input().just_pressed(KeyCode::A));
}

#[test]
fn game_press_stays_game_owned_and_its_captured_release_still_arrives() {
    let (mut e, flag) = flagged();
    step(&mut e, vec![InputEventKind::key_pressed(KeyCode::W)]);
    assert!(e.input().game_input().just_pressed(KeyCode::W));
    // UI grabs focus; repeat and release come in captured.
    *flag.borrow_mut() = true;
    step(&mut e, vec![InputEventKind::key(KeyCode::W, Key::Unidentified, ElementState::Pressed, true, Modifiers::NONE)]);
    assert!(e.input().game_input().pressed(KeyCode::W), "ownership is decided by the press");
    step(&mut e, vec![InputEventKind::key_released(KeyCode::W)]);
    let g = e.input().game_input();
    assert!(g.just_released(KeyCode::W) && !g.pressed(KeyCode::W));
}

#[test]
fn captured_mouse_click_wheel_delta_and_focus_loss() {
    let (mut e, flag) = flagged();
    *flag.borrow_mut() = true;
    step(&mut e, vec![
        btn(MouseButton::Left, ElementState::Pressed),
        InputEventKind::MouseWheel { delta: (0.0, 3.0), unit: WheelUnit::Line },
        InputEventKind::MouseMotion { pos: (5.0, 5.0), delta: (2.0, 2.0) },
    ]);
    assert!(e.input().mouse_pressed(MouseButton::Left) && e.input().mouse_just_pressed(MouseButton::Left));
    assert_eq!(e.input().wheel(), (0.0, 3.0));
    assert_eq!(e.input().mouse_delta(), (2.0, 2.0));
    let g = e.input().game_input();
    assert!(!g.mouse_pressed(MouseButton::Left) && !g.mouse_just_pressed(MouseButton::Left));
    assert_eq!((g.wheel(), g.mouse_delta()), ((0.0, 0.0), (0.0, 0.0)));
    *flag.borrow_mut() = false;
    step(&mut e, vec![btn(MouseButton::Left, ElementState::Released)]);
    let g = e.input().game_input();
    assert!(!g.mouse_just_released(MouseButton::Left) && !g.mouse_pressed(MouseButton::Left));
    assert!(e.input().mouse_just_released(MouseButton::Left));

    // Game-owned button released by focus loss reaches the game.
    step(&mut e, vec![btn(MouseButton::Right, ElementState::Pressed), InputEventKind::key_pressed(KeyCode::D)]);
    step(&mut e, vec![InputEventKind::Window(WindowEvent::Focused(false))]);
    let g = e.input().game_input();
    assert!(g.mouse_just_released(MouseButton::Right) && g.just_released(KeyCode::D));
    assert!(!g.mouse_pressed(MouseButton::Right) && !g.pressed(KeyCode::D));
}

#[test]
fn text_editing_passes_through_input_state_raw() {
    let mut e = engine();
    let kind = InputEventKind::TextEditing { text: "かな".into(), cursor: 1, len: 1 };
    step(&mut e, vec![kind.clone()]);
    assert!(e.input().events().iter().any(|ev| ev.kind == kind));
    assert_eq!(e.input().text(), "", "preedit is not typed text");
    assert_eq!(e.input().game_input().text(), "");
}

#[test]
fn set_text_input_area_reaches_the_window() {
    let mut e = engine();
    assert_eq!(e.windows.text_input_area(), None);
    e.add_system(Stage::Update, |cx| cx.commands.set_text_input_area(10, 20, 30, 40, 2));
    e.run_frames(1, DT);
    assert_eq!(e.windows.text_input_area(), Some((10, 20, 30, 40, 2)));
}

#[test]
fn overlays_are_never_called_without_a_gpu() {
    struct O(Rc<RefCell<Vec<&'static str>>>);
    impl RenderOverlay for O {
        fn init(&mut self, _: &Gpu) { self.0.borrow_mut().push("init"); }
        fn resize(&mut self, _: &Gpu) { self.0.borrow_mut().push("resize"); }
        fn render(&mut self, _: &Gpu, _: &wgpu::TextureView) { self.0.borrow_mut().push("render"); }
    }
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut e = engine();
    e.add_render_overlay(O(log.clone())).add_render_overlay(O(log.clone()));
    e.run_frames(3, DT);
    assert!(log.borrow().is_empty());
}
