//! Headless engine tests: no SDL window, no GPU.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use efeengine_core::prelude::*;
use efeengine_core::engine::messages::Message;
use efeengine_core::input::{ElementState, InputEvent, InputEventKind, Modifiers};

fn engine() -> Engine {
    Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() })
        .expect("headless engine")
}

#[test]
fn fixed_steps_at_60fps() {
    let mut e = engine();
    let count = Rc::new(Cell::new(0u32));
    let c = count.clone();
    e.add_system(Stage::FixedUpdate, move |_| c.set(c.get() + 1));
    e.run_frames(100, 1.0 / 60.0);
    assert_eq!(count.get(), 99);
    assert_eq!(e.time().frame(), 99);
}

#[test]
fn fixed_steps_at_30fps_give_two_per_frame() {
    let mut e = engine();
    let per_frame = Rc::new(RefCell::new(Vec::<u32>::new()));
    let n = Rc::new(Cell::new(0u32));
    let (n1, n2, pf) = (n.clone(), n.clone(), per_frame.clone());
    e.add_system(Stage::FixedUpdate, move |_| n1.set(n1.get() + 1));
    e.add_system(Stage::PostUpdate, move |_| {
        pf.borrow_mut().push(n2.get());
        n2.set(0);
    });
    e.run_frames(10, 1.0 / 30.0);
    let v = per_frame.borrow();
    assert_eq!(v[0], 0);
    assert!(v[1..].iter().all(|&s| s == 2), "{v:?}");
}

#[test]
fn fixed_update_sees_fixed_delta() {
    let mut e = engine();
    let ok = Rc::new(Cell::new(true));
    let seen = Rc::new(Cell::new(0u32));
    let (o, s) = (ok.clone(), seen.clone());
    e.add_system(Stage::FixedUpdate, move |cx| {
        s.set(s.get() + 1);
        if cx.time.delta() != cx.time.fixed_delta() {
            o.set(false);
        }
    });
    e.run_frames(20, 1.0 / 30.0);
    assert!(seen.get() > 0);
    assert!(ok.get());
}

fn log_systems(e: &mut Engine, log: &Rc<RefCell<Vec<&'static str>>>) {
    for (stage, name) in [(Stage::Startup, "startup"), (Stage::Update, "update"), (Stage::Shutdown, "shutdown")] {
        let l = log.clone();
        e.add_system(stage, move |_| l.borrow_mut().push(name));
    }
}

#[test]
fn startup_once_before_update_and_shutdown_once() {
    let mut e = engine();
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    log_systems(&mut e, &log);
    e.run_frames(3, 0.016);
    assert_eq!(*log.borrow(), ["startup", "update", "update", "update"]);

    // Kill is drained in frame 3, before PreUpdate: no Update, Shutdown runs once.
    e.sender().send(Message::Kill).unwrap();
    e.run_frames(3, 0.016);
    assert!(!e.is_running());
    assert_eq!(*log.borrow(), ["startup", "update", "update", "update", "shutdown"]);

    // frames after Kill are no-ops
    e.frame(100.0);
    e.run_frames(5, 0.016);
    assert_eq!(*log.borrow(), ["startup", "update", "update", "update", "shutdown"]);
}

#[test]
fn kill_before_first_frame_skips_startup_but_runs_shutdown() {
    let mut e = engine();
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    log_systems(&mut e, &log);
    // Drained in frame 0, before Startup: Startup and Update never run.
    e.sender().send(Message::Kill).unwrap();
    e.run_frames(3, 0.016);
    assert!(!e.is_running());
    assert_eq!(*log.borrow(), ["shutdown"]);
}

#[test]
fn quit_command_runs_shutdown() {
    let mut e = engine();
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let l = log.clone();
    e.add_system(Stage::Update, move |cx| {
        l.borrow_mut().push("update");
        if cx.time.frame() == 2 {
            cx.commands.quit();
        }
    });
    let l = log.clone();
    e.add_system(Stage::PostUpdate, move |_| l.borrow_mut().push("post"));
    let l = log.clone();
    e.add_system(Stage::Shutdown, move |_| l.borrow_mut().push("shutdown"));
    e.run_frames(50, 0.016);
    assert!(!e.is_running());
    // PostUpdate must not run in the frame where Update called quit().
    assert_eq!(*log.borrow(), ["update", "post", "update", "post", "update", "shutdown"]);
}

#[test]
fn quit_in_first_fixed_step_stops_remaining_steps() {
    let mut e = engine();
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let l = log.clone();
    // At 30 fps against a 60 Hz step every frame after the first has two fixed steps.
    e.add_system(Stage::FixedUpdate, move |cx| {
        l.borrow_mut().push("fixed");
        if cx.time.frame() == 1 {
            cx.commands.quit();
        }
    });
    let l = log.clone();
    e.add_system(Stage::Update, move |_| l.borrow_mut().push("update"));
    let l = log.clone();
    e.add_system(Stage::Shutdown, move |_| l.borrow_mut().push("shutdown"));
    e.run_frames(10, 1.0 / 30.0);
    assert!(!e.is_running());
    // frame 0: update; frame 1: first fixed step quits, second step and Update are skipped.
    assert_eq!(*log.borrow(), ["update", "fixed", "shutdown"]);
}

#[test]
fn input_just_pressed_only_in_injected_frame() {
    let mut e = engine();
    let rec = Rc::new(RefCell::new(Vec::<(bool, bool)>::new()));
    let r = rec.clone();
    e.add_system(Stage::Update, move |cx| {
        r.borrow_mut().push((cx.input.just_pressed(KeyCode::A), cx.input.pressed(KeyCode::A)));
    });
    e.run_frames(2, 0.016); // frames 0, 1
    e.inject(InputEvent::new(InputEventKind::key_pressed(KeyCode::A)));
    e.run_frames(3, 0.016); // frames 2, 3, 4
    e.inject(InputEvent::new(InputEventKind::key_released(KeyCode::A)));
    e.run_frames(1, 0.016); // frame 5
    let v = rec.borrow();
    assert_eq!(
        *v,
        [(false, false), (false, false), (true, true), (false, true), (false, true), (false, false)]
    );
}

#[test]
fn repeat_press_never_just_pressed() {
    let mut e = engine();
    let any = Rc::new(Cell::new(false));
    let a = any.clone();
    e.add_system(Stage::Update, move |cx| {
        if cx.input.just_pressed(KeyCode::B) {
            a.set(true);
        }
    });
    e.run_frames(1, 0.016);
    e.inject(InputEvent::new(InputEventKind::key(
        KeyCode::B,
        Key::Character("b".into()),
        ElementState::Pressed,
        true,
        Modifiers::NONE,
    )));
    e.run_frames(2, 0.016);
    assert!(!any.get());
}

struct Ping(u32);

#[test]
fn typed_event_visible_next_frame_only() {
    let mut e = engine();
    let seen = Rc::new(RefCell::new(Vec::<(u64, usize)>::new()));
    let s = seen.clone();
    let payload = Rc::new(Cell::new(0u32));
    let p = payload.clone();
    e.add_system(Stage::PreUpdate, move |cx| {
        // reader runs before the sender in the same frame
        s.borrow_mut().push((cx.time.frame(), cx.events::<Ping>().len()));
        for ping in cx.events::<Ping>().iter() {
            p.set(ping.0);
        }
    });
    e.add_system(Stage::Update, |cx| {
        if cx.time.frame() == 3 {
            cx.send(Ping(7));
        }
    });
    e.run_frames(7, 0.016);
    let v = seen.borrow();
    for (frame, n) in v.iter() {
        assert_eq!(*n, if *frame == 4 { 1 } else { 0 }, "frame {frame}");
    }
    assert_eq!(payload.get(), 7);
}

#[test]
fn headless_has_no_gamepads() {
    let mut e = engine();
    let seen = Rc::new(Cell::new(None::<bool>));
    let s = seen.clone();
    e.add_system(Stage::Update, move |cx| s.set(Some(cx.gamepads.is_empty())));
    e.run_frames(5, 0.016);
    assert!(e.gamepads().is_empty());
    assert_eq!(e.gamepads().len(), 0);
    assert_eq!(seen.get(), Some(true));
}

#[test]
fn message_readable_same_frame_and_isolated() {
    let mut a = engine();
    let mut b = engine();
    let got_a = Rc::new(RefCell::new(Vec::<String>::new()));
    let got_b = Rc::new(RefCell::new(Vec::<String>::new()));
    for (e, got) in [(&mut a, &got_a), (&mut b, &got_b)] {
        let g = got.clone();
        e.add_system(Stage::Update, move |cx| {
            for m in cx.events::<Message>().iter() {
                if let Message::Say(s) = m {
                    g.borrow_mut().push(format!("{}@{}", s, cx.time.frame()));
                }
            }
        });
    }
    a.run_frames(1, 0.016);
    b.run_frames(1, 0.016);
    a.sender().send(Message::Say("hi".into())).unwrap();
    a.run_frames(1, 0.016);
    b.run_frames(1, 0.016);
    a.run_frames(1, 0.016);
    assert_eq!(*got_a.borrow(), ["hi@1"]);
    assert!(got_b.borrow().is_empty());
}

#[test]
fn set_frame_rate_message_and_command_do_not_panic() {
    let mut e = engine();
    e.sender().send(Message::SetFrameRate(30.0)).unwrap();
    e.sender().send(Message::SetFrameRate(-1.0)).unwrap();
    e.add_system(Stage::Update, |cx| cx.commands.set_frame_rate(f32::NAN));
    e.run_frames(2, 0.016);
    assert!(e.is_running());
}
