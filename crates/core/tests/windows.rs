//! Multiple windows, headless: window ids, per-window state and events, camera targeting,
//! per-window frames, input routing, commands. No window system, no GPU.

use efeengine_core::input::{ElementState, InputEvent, InputEventKind, MouseButton, WindowEvent, WindowId};
use efeengine_core::prelude::*;
use efeengine_core::window::WindowDesc;
use std::cell::RefCell;
use std::rc::Rc;

const DT: f64 = 1.0 / 60.0;

fn engine() -> Engine {
    Engine::with_config(EngineConfig { fixed_dt: DT, ..EngineConfig::headless() }).expect("headless engine")
}

fn win_ev(window: WindowId, w: WindowEvent) -> InputEvent {
    InputEvent::with_time(InputEventKind::Window(w), 0, window)
}

#[test]
fn create_window_hands_out_ids_and_reports_events() {
    let mut e = engine();
    let a = e.create_window(WindowDesc::new("A").size(320, 240)).unwrap();
    let b = e.create_window(WindowDesc::new("B").size(100, 50)).unwrap();
    assert_eq!((a, b), (WindowId(1), WindowId(2)));
    e.run_frames(2, DT);
    let ws = &e.state.windows;
    assert_eq!(ws.len(), 3);
    assert_eq!(ws.get(a).unwrap().size, (320, 240));
    assert_eq!(ws.get(b).unwrap().size, (100, 50));
    assert_eq!(ws.main().size, e.config().size);
}

#[test]
fn windows_get_created_and_resized_events_in_their_first_frame() {
    let mut e = engine();
    let seen = Rc::new(RefCell::new(Vec::<(u32, String)>::new()));
    let s = seen.clone();
    e.add_system(Stage::PreUpdate, move |cx| {
        for ev in cx.input.events() {
            if let InputEventKind::Window(w) = &ev.kind {
                s.borrow_mut().push((ev.window.0, format!("{w:?}")));
            }
        }
    });
    let a = e.create_window(WindowDesc::new("A").size(64, 48)).unwrap();
    e.run_frames(1, DT);
    let seen = seen.borrow();
    assert!(seen.contains(&(a.0, "Created".to_string())), "{seen:?}");
    assert!(seen.iter().any(|(w, s)| *w == a.0 && s.starts_with("Resized { width: 64, height: 48 }")), "{seen:?}");
}

#[test]
fn commands_create_and_close_windows_at_runtime() {
    let mut e = engine();
    let id = Rc::new(RefCell::new(None));
    let i2 = id.clone();
    let closed = Rc::new(RefCell::new(Vec::<u32>::new()));
    let c2 = closed.clone();
    let mut n = 0;
    e.add_system(Stage::Update, move |cx| {
        n += 1;
        if n == 1 {
            let w = cx.commands.create_window(WindowDesc::new("Late").size(10, 10));
            // The id is usable at once: a camera can already target it.
            cx.commands.spawn(cx.world, (Camera::new_2d().target(w), Transform::default()));
            *i2.borrow_mut() = Some(w);
        }
        if n == 3 {
            cx.commands.close_window(i2.borrow().unwrap());
        }
        for ev in cx.input.events() {
            if matches!(ev.kind, InputEventKind::Window(WindowEvent::Closed)) {
                c2.borrow_mut().push(ev.window.0);
            }
        }
    });
    e.run_frames(2, DT);
    let w = id.borrow().unwrap();
    assert!(e.state.windows.contains(w));
    // the frame after creation: the window has its own (empty-but-present) frame entry with a camera
    {
        let f = e.render_frame();
        let (d2, _) = f.window(w.0).expect("window frame");
        assert!(d2.camera.is_some());
    }
    e.run_frames(4, DT);
    assert!(!e.state.windows.contains(w));
    assert_eq!(*closed.borrow(), vec![w.0]);
    assert!(e.render_frame().window(w.0).is_none());
}

#[test]
fn closing_a_secondary_window_keeps_the_engine_running_but_main_quits() {
    let mut e = engine();
    let a = e.create_window(WindowDesc::new("A")).unwrap();
    e.run_frames(1, DT);
    e.inject(win_ev(a, WindowEvent::CloseRequested));
    e.run_frames(2, DT);
    assert!(e.is_running());
    assert!(!e.state.windows.contains(a));
    e.inject(win_ev(WindowId::MAIN, WindowEvent::CloseRequested));
    e.run_frames(2, DT);
    assert!(!e.is_running());
}

#[test]
fn cameras_render_to_their_target_window() {
    let mut e = engine();
    let map = e.create_window(WindowDesc::new("Map").size(200, 100)).unwrap();
    let main_cam = e.world_mut().spawn((Camera::new_2d().zoom(2.0), Transform::from_position(Vec2::new(1.0, 2.0))));
    let map_cam = e.world_mut().spawn((Camera::new_2d().target(map).zoom(0.5), Transform::from_position(Vec2::new(50.0, 60.0))));
    // a second camera for the same window: lowest entity id wins, the other is ignored
    let _extra = e.world_mut().spawn((Camera::new_2d().target(map), Transform::default()));
    // a camera for a window that does not exist draws nowhere
    e.world_mut().spawn((Camera::new_2d().target(WindowId(42)), Transform::default()));
    e.world_mut().spawn((Sprite::default(), Transform::default()));
    e.run_frames(2, DT);
    let f = e.render_frame().clone();
    assert_eq!(f.d2.camera.as_ref().unwrap().entity, main_cam);
    assert_eq!(f.d2.camera.as_ref().unwrap().viewport, e.config().size);
    assert_eq!(f.d2.ignored_cameras, 0);
    assert_eq!(f.windows.len(), 1);
    let m = &f.windows[0];
    assert_eq!(m.window, map.0);
    let c = m.d2.camera.as_ref().unwrap();
    assert_eq!(c.entity, map_cam);
    assert_eq!(c.viewport, (200, 100));
    assert_eq!(c.target, map.0);
    assert_eq!(c.zoom, 0.5);
    assert_eq!(m.d2.ignored_cameras, 1);
    // the sprite is drawn in both windows
    assert_eq!(f.d2.sprites.len(), 1);
    assert_eq!(m.d2.sprites.len(), 1);
}

#[test]
fn a_window_without_a_camera_gets_no_sprites() {
    let mut e = engine();
    let w = e.create_window(WindowDesc::new("Blank")).unwrap();
    e.world_mut().spawn((Camera::new_2d(), Transform::default()));
    e.world_mut().spawn((Sprite::default(), Transform::default()));
    e.run_frames(2, DT);
    let f = e.render_frame();
    let (d2, _) = f.window(w.0).unwrap();
    assert!(d2.camera.is_none());
    assert!(d2.sprites.is_empty());
}

#[test]
fn resize_and_scale_events_update_only_their_window() {
    let mut e = engine();
    let a = e.create_window(WindowDesc::new("A").size(10, 10)).unwrap();
    e.run_frames(1, DT);
    e.inject(win_ev(a, WindowEvent::Resized { width: 400, height: 300 }));
    e.inject(win_ev(a, WindowEvent::ScaleFactorChanged { scale: 2.0 }));
    e.inject(win_ev(a, WindowEvent::FullscreenChanged(true)));
    e.run_frames(1, DT);
    let main_size = e.config().size;
    assert_eq!(e.state.windows.main().size, main_size);
    assert_eq!(e.state.windows.main().scale_factor, 1.0);
    let s = e.state.windows.get(a).unwrap();
    assert_eq!((s.size, s.scale_factor, s.fullscreen), ((400, 300), 2.0, true));
    assert_eq!(s.logical_size(), (200.0, 150.0));
}

#[test]
fn pointer_state_is_per_window() {
    let mut e = engine();
    let a = e.create_window(WindowDesc::new("A")).unwrap();
    e.run_frames(1, DT);
    e.inject(InputEvent::with_time(InputEventKind::MouseMotion { pos: (5.0, 6.0), delta: (0.0, 0.0) }, 0, WindowId::MAIN));
    e.inject(InputEvent::with_time(InputEventKind::MouseMotion { pos: (70.0, 80.0), delta: (0.0, 0.0) }, 0, a));
    e.inject(InputEvent::with_time(InputEventKind::MouseButton { button: MouseButton::Left, state: ElementState::Pressed, pos: (71.0, 81.0) }, 0, a));
    e.run_frames(1, DT);
    let i = e.input();
    assert_eq!(i.mouse_pos(), Some((5.0, 6.0)), "mouse_pos is the main window's");
    assert_eq!(i.mouse_pos_in(a), Some((71.0, 81.0)));
    assert_eq!(i.pointer_window(), Some(a));
    assert!(i.mouse_pressed(MouseButton::Left));
}

#[test]
fn focus_follows_windows_and_keys_survive_a_window_switch() {
    use efeengine_core::input::KeyCode;
    let mut e = engine();
    let a = e.create_window(WindowDesc::new("A")).unwrap();
    e.run_frames(1, DT);
    e.inject(InputEvent::new(InputEventKind::key_pressed(KeyCode::A)));
    e.inject(win_ev(a, WindowEvent::Focused(true)));
    e.inject(win_ev(WindowId::MAIN, WindowEvent::Focused(false)));
    e.run_frames(1, DT);
    let i = e.input();
    assert!(i.focused() && i.focused_in(a) && !i.focused_in(WindowId::MAIN));
    assert_eq!(i.focus_window(), a);
    assert!(i.pressed(KeyCode::A), "another window of the game still has focus");
    assert_eq!(e.state.windows.focused(), a);
    // the last window losing focus releases everything, as before
    e.inject(win_ev(a, WindowEvent::Focused(false)));
    e.run_frames(1, DT);
    assert!(!e.input().focused());
    assert!(!e.input().pressed(KeyCode::A));
}

#[test]
fn set_window_title_and_close_unknown_are_harmless() {
    let mut e = engine();
    let a = e.create_window(WindowDesc::new("A")).unwrap();
    e.run_frames(1, DT);
    e.close_window(WindowId(77));
    e.close_window(WindowId::MAIN);
    assert!(e.state.windows.contains(WindowId::MAIN));
    e.close_window(a);
    e.close_window(a);
    assert_eq!(e.state.windows.len(), 1);
}

#[test]
fn an_editor_only_camera_wins_its_window_and_debug_draw_goes_to_the_named_window() {
    let mut e = engine();
    let w = e.create_window(WindowDesc::new("Tools").size(200, 100)).unwrap();
    e.run_frames(1, DT);
    let game = e.world_mut().spawn((Camera::new_2d().zoom(1.0), Transform::from_position(Vec2::new(5.0, 0.0))));
    let tool = e.world_mut().spawn((Camera::new_2d().zoom(2.0), Transform::default(), EditorOnly));
    let tool_w = e.world_mut().spawn((Camera::new_2d().target(w), Transform::default(), EditorOnly));
    e.run_frames(2, DT);
    {
        let f = e.render_frame();
        assert_eq!(f.d2.camera.as_ref().unwrap().entity, tool, "the editor camera beats the game camera (lower id or not)");
        assert_ne!(f.d2.camera.as_ref().unwrap().entity, game);
        assert_eq!(f.window(w.0).unwrap().0.camera.as_ref().unwrap().entity, tool_w);
    }
    // debug draw: one shape per window
    e.add_system(Stage::Update, move |cx| {
        let d = cx.resources.get_mut::<DebugDraw2D>().unwrap();
        d.line(Vec2::ZERO, Vec2::new(10.0, 0.0), DebugDraw2D::RED);
        d.set_window(w).line(Vec2::ZERO, Vec2::new(0.0, 10.0), DebugDraw2D::GREEN).line(Vec2::ZERO, Vec2::new(0.0, 20.0), DebugDraw2D::GREEN);
        d.set_window(77u32).line(Vec2::ZERO, Vec2::ONE, DebugDraw2D::BLUE);
    });
    e.run_frames(1, DT);
    let f = e.render_frame();
    let main_cmds = f.d2.draw.commands().len();
    let win_cmds = f.window(w.0).unwrap().0.draw.commands().len();
    assert!(main_cmds >= 1 && win_cmds >= 1, "{main_cmds} {win_cmds}");
    assert!(f.window(w.0).unwrap().0.draw.vertices().len() > f.d2.draw.vertices().len(), "two lines in the window, one in main");
    assert!(e.resources().get::<DebugDraw2D>().unwrap().is_empty(), "nothing is left over, not even for a missing window");
}
