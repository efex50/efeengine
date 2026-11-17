//! Resource change ticks (`Ctx::res_changed`, `ParCtx::res_changed`, `Engine::res_changed`)
//! and the UI input capture (`Ctx::game_input`).

use std::sync::{Arc, Mutex};

use efeengine_core::input::*;
use efeengine_core::prelude::*;

struct Score(u32);

fn engine() -> Engine {
    Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine")
}

type Log = Arc<Mutex<Vec<bool>>>;

#[test]
fn exclusive_systems_see_get_mut_and_insert() {
    let mut e = engine();
    e.insert_resource(Score(0));
    let log: Log = Default::default();
    let l = log.clone();
    e.add_system(Stage::Update, move |cx| l.lock().unwrap().push(cx.res_changed::<Score>()));
    let mut frame = 0;
    e.add_system(Stage::PostUpdate, move |cx| {
        frame += 1;
        match frame {
            2 => cx.resources.get_mut::<Score>().unwrap().0 += 1,
            4 => {
                cx.resources.insert(Score(9));
            }
            _ => {}
        }
    });
    e.run_frames(5, 1.0 / 60.0);
    // Frame 1 sees the insert (never ran before); the write in frame 2 is seen in frame 3, the insert in frame 4 in frame 5.
    assert_eq!(*log.lock().unwrap(), vec![true, false, true, false, true]);
    assert!(!e.res_changed::<u8>(e.change_tick()));
}

#[test]
fn par_res_mut_marks_on_deref_mut_only() {
    let mut e = engine();
    e.insert_resource(Score(0));
    let log: Log = Default::default();
    let l = log.clone();
    e.add_par_system(Stage::Update, Access::new().res::<Score>(), move |cx| l.lock().unwrap().push(cx.res_changed::<Score>()));
    let mut frame = 0;
    e.add_par_system(Stage::PostUpdate, Access::new().res_mut::<Score>(), move |cx| {
        frame += 1;
        let mut r = cx.res_mut::<Score>();
        if frame == 2 {
            r.0 += 1; // mutable deref
        } else {
            let _ = r.0; // read-only
        }
    });
    e.run_frames(4, 1.0 / 60.0);
    assert_eq!(*log.lock().unwrap(), vec![true, false, true, false]);
}

#[test]
fn engine_level_helpers() {
    let mut e = engine();
    e.insert_resource(Score(0));
    e.run_frames(2, 1.0 / 60.0);
    let t = e.change_tick();
    assert!(!e.res_changed::<Score>(t));
    e.resources_mut().get_mut::<Score>().unwrap().0 = 5;
    assert!(e.resources().changed_tick::<Score>().is_some());
    assert!(e.res_changed::<Score>(t));
}

#[test]
fn game_input_in_ctx() {
    let mut e = engine();
    let log: Log = Default::default();
    let l = log.clone();
    e.add_system(Stage::Update, move |cx| {
        let before = cx.game_input().mouse_pressed(MouseButton::Left);
        cx.input.set_capture(InputCapture { pointer: true, keyboard: false });
        let after = cx.game_input().mouse_pressed(MouseButton::Left);
        l.lock().unwrap().push(before);
        l.lock().unwrap().push(after);
    });
    e.inject(InputEvent::new(InputEventKind::MouseButton { button: MouseButton::Left, state: ElementState::Pressed, pos: (0.0, 0.0) }));
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(*log.lock().unwrap(), vec![true, false]);
    assert!(e.input().mouse_pressed(MouseButton::Left));
    assert!(e.input().capture().pointer);
}
