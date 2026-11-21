//! Step 3 tests: the editor camera, picking, gizmos, grid, reparenting and the separate editor
//! window. Headless (`UiTest`): mouse and key events are injected into the engine.

use super::tests::{Drawn, editor, editor_with, entity, mode, pick as select_by_row, row_key, ui_state};
use super::viewport::{self, *};
use super::*;
use crate::UiViews;
use crate::capture::UiCapture;
use crate::testing::UiTest;
use efeengine_core::engine::inspect::EntityRow;
use efeengine_core::input::{ElementState, InputEvent, InputEventKind, Key, Modifiers, WheelUnit, WindowId};

const VP: (u32, u32) = (800, 600);

fn ev(t: &mut UiTest, win: u32, kind: InputEventKind) {
    t.engine.inject(InputEvent::with_time(kind, 0, WindowId(win)));
}

fn motion(t: &mut UiTest, win: u32, pos: Vec2, delta: Vec2) {
    ev(t, win, InputEventKind::MouseMotion { pos: (pos.x, pos.y), delta: (delta.x, delta.y) });
}

fn button(t: &mut UiTest, win: u32, b: MouseButton, down: bool, pos: Vec2) {
    let state = if down { ElementState::Pressed } else { ElementState::Released };
    ev(t, win, InputEventKind::MouseButton { button: b, state, pos: (pos.x, pos.y) });
}

fn key(t: &mut UiTest, code: KeyCode) {
    ev(t, 0, InputEventKind::key_pressed(code));
    t.step(1);
    ev(t, 0, InputEventKind::key_released(code));
    t.step(1);
}

fn key_in(t: &mut UiTest, win: u32, code: KeyCode) {
    ev(t, win, InputEventKind::key_pressed(code));
    t.step(1);
    ev(t, win, InputEventKind::key_released(code));
    t.step(1);
}

/// Sets the modifier state the input sees (a key event with an unknown key only carries them).
fn mods(t: &mut UiTest, m: Modifiers) {
    ev(t, 0, InputEventKind::key(KeyCode::Unknown, Key::Unidentified, ElementState::Pressed, false, m));
    t.step(1);
}

fn cam(t: &UiTest) -> (Vec2, f32) {
    let c = *t.resource::<EditorCamera>();
    let p = t.engine.world().get::<&Transform>(c.entity).unwrap().position;
    let z = t.engine.world().get::<&Camera>(c.entity).unwrap().zoom;
    (p, z)
}

fn screen(t: &UiTest, world: Vec2) -> Vec2 {
    let (p, z) = cam(t);
    world_to_screen(p, z, VP, world)
}

fn click_world(t: &mut UiTest, world: Vec2) {
    let s = screen(t, world);
    motion(t, 0, s, Vec2::ZERO);
    t.step(1);
    button(t, 0, MouseButton::Left, true, s);
    t.step(2);
    button(t, 0, MouseButton::Left, false, s);
    t.step(2);
}

/// A drag with the left button from world point `a` to `b`, in `n` motion steps.
fn drag_world(t: &mut UiTest, a: Vec2, b: Vec2, n: usize) {
    let (sa, sb) = (screen(t, a), screen(t, b));
    motion(t, 0, sa, Vec2::ZERO);
    t.step(1);
    button(t, 0, MouseButton::Left, true, sa);
    t.step(1);
    for i in 1..=n {
        let p = sa + (sb - sa) * (i as f32 / n as f32);
        let prev = sa + (sb - sa) * ((i - 1) as f32 / n as f32);
        motion(t, 0, p, p - prev);
        t.step(1);
    }
    button(t, 0, MouseButton::Left, false, sb);
    t.step(2);
}

fn sprite(layer: i32, size: Vec2) -> Sprite {
    Sprite { layer, ..Sprite::default().with_size(size) }
}

/// The step-2 world plus: `big` (id 10, layer 0, 100x100 at (300,200)), `top` (id 11, layer 1,
/// 60x60 over it), `bar` (id 12, a 100x20 sprite turned 45 degrees at (-300,-200)), `blob` (id
/// 13, a sprite of its own at (0,-250)).
fn vp() -> UiTest {
    let mut t = editor();
    let w = t.engine.world_mut();
    w.spawn((Name::new("big"), EditorId { id: 10 }, Transform::from_position(Vec2::new(300.0, 200.0)), sprite(0, Vec2::splat(100.0))));
    w.spawn((Name::new("top"), EditorId { id: 11 }, Transform::from_position(Vec2::new(300.0, 200.0)), sprite(1, Vec2::splat(60.0))));
    w.spawn((Name::new("bar"), EditorId { id: 12 }, Transform { position: Vec2::new(-300.0, -200.0), rotation: std::f32::consts::FRAC_PI_4, scale: Vec2::ONE }, sprite(0, Vec2::new(100.0, 20.0))));
    w.spawn((Name::new("blob"), EditorId { id: 13 }, Transform::from_position(Vec2::new(0.0, -250.0)), sprite(0, Vec2::splat(40.0))));
    settle(&mut t);
    t
}

/// Lets the panel read the world (it does so every quarter second).
fn settle(t: &mut UiTest) {
    t.step(20);
}

fn selected(t: &UiTest) -> Option<Entity> {
    t.resource::<EditorSelection>().entity
}

fn name_of(t: &UiTest, e: Option<Entity>) -> Option<String> {
    e.and_then(|e| t.engine.world().get::<&Name>(e).ok().map(|n| n.0.clone()))
}

fn undo_len(t: &UiTest) -> usize {
    ui_state(t).undo_len
}

fn pos(t: &UiTest, name: &str) -> Vec2 {
    t.engine.world().get::<&Transform>(entity(t, name)).unwrap().position
}

fn tf(t: &UiTest, name: &str) -> Transform {
    *t.engine.world().get::<&Transform>(entity(t, name)).unwrap()
}

fn tool(t: &mut UiTest, k: KeyCode) {
    key(t, k);
}

fn near(a: Vec2, b: Vec2) -> bool {
    (a - b).length() < 0.05
}

fn select_name(t: &mut UiTest, name: &str) {
    select_by_row(t, name);
    t.step(2);
}

fn undo(t: &mut UiTest) {
    t.click("debug_menu/editor/toolbar/undo");
    t.step(3);
}

// ----- editor camera ----------------------------------------------------------------------------

fn game_camera(t: &UiTest) -> Entity {
    t.engine.world().query::<(Entity, &Camera)>().without::<&EditorOnly>().iter().map(|(e, _)| e).next().unwrap()
}

#[test]
fn the_editor_camera_draws_in_edit_and_paused_and_the_game_camera_in_play() {
    let mut t = vp();
    let game = game_camera(&t);
    let before = *t.engine.world().get::<&Camera>(game).unwrap();
    let editor_cam = t.resource::<EditorCamera>().entity;
    assert_eq!(t.engine.render_frame().d2.camera.as_ref().unwrap().entity, editor_cam, "Edit: the editor camera draws the main window");

    t.click("debug_menu/editor/toolbar/play");
    t.step(3);
    assert_eq!(mode(&t), EditorMode::Play);
    assert_eq!(t.engine.render_frame().d2.camera.as_ref().unwrap().entity, game, "Play: the game camera is back");
    t.click("debug_menu/editor/toolbar/pause");
    t.step(3);
    assert_eq!(t.engine.render_frame().d2.camera.as_ref().unwrap().entity, editor_cam, "Paused: the editor camera again");
    t.click("debug_menu/editor/toolbar/play");
    t.step(3);
    t.click("debug_menu/editor/toolbar/stop");
    t.step(3);
    assert_eq!(mode(&t), EditorMode::Edit);
    assert_eq!(t.engine.render_frame().d2.camera.as_ref().unwrap().entity, editor_cam);
    // The game's camera was never touched: not its fields, not its flags.
    let game_now = game_camera(&t);
    assert_eq!(*t.engine.world().get::<&Camera>(game_now).unwrap(), before, "the game camera is exactly as it was");
    assert_eq!(before.active, 1);
}

#[test]
fn the_editor_camera_is_never_saved_snapshotted_listed_or_in_the_undo_log() {
    let mut t = vp();
    let cam = t.resource::<EditorCamera>().entity;
    assert!(t.engine.editable_entities().iter().all(|r| r.entity != cam), "not in the hierarchy");
    assert!(t.resource::<EditorPanel>().rows.iter().all(|r| r.entity != cam));
    let scene = t.engine.capture_world_scene("x");
    assert_eq!(scene.entities.iter().filter(|e| e.components.contains_key("Camera")).count(), 1, "only the game camera is saved");
    assert!(t.engine.snapshot().scene.entities.iter().all(|e| e.id != Some(efeengine_core::ecs::entity_to_bits(cam))));
    let undo_before = undo_len(&t);
    // moving the camera is not an edit
    motion(&mut t, 0, Vec2::new(400.0, 300.0), Vec2::ZERO);
    t.step(1);
    button(&mut t, 0, MouseButton::Middle, true, Vec2::new(400.0, 300.0));
    t.step(1);
    motion(&mut t, 0, Vec2::new(420.0, 300.0), Vec2::new(20.0, 0.0));
    t.step(1);
    button(&mut t, 0, MouseButton::Middle, false, Vec2::new(420.0, 300.0));
    t.step(2);
    assert_eq!(undo_len(&t), undo_before);
    assert!(!ui_state(&t).dirty, "camera moves do not dirty the scene");
    // Play, then Stop restores the world; the editor camera is still the same entity at the same place
    let at = cam_pos(&t);
    t.click("debug_menu/editor/toolbar/play");
    t.step(3);
    t.click("debug_menu/editor/toolbar/stop");
    t.step(3);
    assert!(t.engine.world().contains(cam));
    assert_eq!(cam_pos(&t), at, "the camera position persists across Edit / Play / Edit");
}

fn cam_pos(t: &UiTest) -> Vec2 {
    cam(t).0
}

#[test]
fn it_starts_where_the_game_camera_is() {
    let mut t = editor();
    t.step(2);
    assert_eq!(cam(&t), (Vec2::ZERO, 1.0));
    let clear = t.engine.world().get::<&Camera>(t.resource::<EditorCamera>().entity).unwrap().clear_color;
    assert_eq!(clear, [0.05, 0.05, 0.08, 1.0], "the game camera's clear colour");
}

#[test]
fn middle_drag_and_space_drag_pan_the_wheel_zooms_at_the_cursor_home_and_f() {
    let mut t = vp();
    // middle drag: the content follows the pointer
    motion(&mut t, 0, Vec2::new(400.0, 300.0), Vec2::ZERO);
    t.step(1);
    button(&mut t, 0, MouseButton::Middle, true, Vec2::new(400.0, 300.0));
    t.step(1);
    motion(&mut t, 0, Vec2::new(430.0, 290.0), Vec2::new(30.0, -10.0));
    t.step(2);
    assert!(near(cam_pos(&t), Vec2::new(-30.0, -10.0)), "{:?}", cam_pos(&t));
    button(&mut t, 0, MouseButton::Middle, false, Vec2::new(430.0, 290.0));
    t.step(2);
    // a move without the button does nothing
    motion(&mut t, 0, Vec2::new(500.0, 290.0), Vec2::new(70.0, 0.0));
    t.step(2);
    assert!(near(cam_pos(&t), Vec2::new(-30.0, -10.0)));

    // Space + left drag
    ev(&mut t, 0, InputEventKind::key_pressed(KeyCode::Space));
    t.step(1);
    button(&mut t, 0, MouseButton::Left, true, Vec2::new(500.0, 290.0));
    t.step(1);
    motion(&mut t, 0, Vec2::new(480.0, 310.0), Vec2::new(-20.0, 20.0));
    t.step(2);
    assert!(near(cam_pos(&t), Vec2::new(-10.0, 10.0)), "{:?}", cam_pos(&t));
    button(&mut t, 0, MouseButton::Left, false, Vec2::new(480.0, 310.0));
    ev(&mut t, 0, InputEventKind::key_released(KeyCode::Space));
    t.step(2);
    assert_eq!(selected(&t), None, "a pan is not a pick");

    // wheel: the world point under the cursor stays under it
    let cursor = Vec2::new(600.0, 100.0);
    motion(&mut t, 0, cursor, Vec2::ZERO);
    t.step(1);
    let (p0, z0) = cam(&t);
    let under = screen_to_world(p0, z0, VP, cursor);
    ev(&mut t, 0, InputEventKind::MouseWheel { delta: (0.0, 2.0), unit: WheelUnit::Line });
    t.step(2);
    let (p1, z1) = cam(&t);
    assert!((z1 - z0 * WHEEL_ZOOM.powf(2.0)).abs() < 1e-3, "{z1}");
    assert!(near(screen_to_world(p1, z1, VP, cursor), under), "zoomed around the cursor");

    // Home goes back to the start, F frames the selection
    key(&mut t, KeyCode::Home);
    assert_eq!(cam(&t), (Vec2::ZERO, 1.0));
    select_name(&mut t, "big");
    key(&mut t, KeyCode::F);
    let (p, z) = cam(&t);
    assert!(near(p, Vec2::new(300.0, 200.0)), "{p:?}");
    let (_, want) = frame_box(p, Vec2::splat(100.0), VP, 0.5);
    assert!((z - want).abs() < 1e-3, "{z} vs {want}");
    key(&mut t, KeyCode::Home);
    assert_eq!(cam(&t), (Vec2::ZERO, 1.0));
}

#[test]
fn camera_controls_do_nothing_over_ui_or_while_typing_or_in_play() {
    let mut t = vp();
    t.backend().set_capture(UiCapture { pointer: true, keyboard: false });
    t.step(2);
    ev(&mut t, 0, InputEventKind::MouseWheel { delta: (0.0, 3.0), unit: WheelUnit::Line });
    motion(&mut t, 0, Vec2::new(400.0, 300.0), Vec2::ZERO);
    t.step(1);
    button(&mut t, 0, MouseButton::Middle, true, Vec2::new(400.0, 300.0));
    t.step(1);
    motion(&mut t, 0, Vec2::new(450.0, 300.0), Vec2::new(50.0, 0.0));
    t.step(2);
    assert_eq!(cam(&t), (Vec2::ZERO, 1.0), "the pointer is over UI");
    button(&mut t, 0, MouseButton::Middle, false, Vec2::new(450.0, 300.0));
    t.backend().set_capture(UiCapture::default());
    t.step(2);

    // keyboard captured (a text input has focus): F and Home are the input's
    select_name(&mut t, "big");
    t.backend().set_capture(UiCapture { pointer: false, keyboard: true });
    t.step(1);
    key(&mut t, KeyCode::F);
    assert_eq!(cam(&t), (Vec2::ZERO, 1.0));
    t.backend().set_capture(UiCapture::default());
    t.step(1);
    key(&mut t, KeyCode::W);
    assert_eq!(t.resource::<EditorView>().tool, GizmoTool::Move);

    // Play: the editor camera is off, nothing reacts
    t.click("debug_menu/editor/toolbar/play");
    t.step(3);
    key(&mut t, KeyCode::E);
    assert_eq!(t.resource::<EditorView>().tool, GizmoTool::Move, "tool keys belong to the game in Play");
}

// ----- picking ----------------------------------------------------------------------------------

#[test]
fn a_click_selects_the_topmost_sprite_by_layer() {
    let mut t = vp();
    click_world(&mut t, Vec2::new(300.0, 200.0));
    assert_eq!(name_of(&t, selected(&t)).as_deref(), Some("top"), "layer 1 is over layer 0");
    // outside `top` but inside `big`
    click_world(&mut t, Vec2::new(300.0 + 40.0, 200.0 + 40.0));
    assert_eq!(name_of(&t, selected(&t)).as_deref(), Some("big"));
    // the hierarchy row follows
    let row = t.id(&row_key(&t, "big"));
    assert!(t.ui().has_class(row, "tab-active"), "the hierarchy highlights the picked entity");
    // the same layer: the higher entity id (drawn later) wins
    let a = t.engine.world_mut().spawn((Name::new("a"), EditorId { id: 20 }, Transform::from_position(Vec2::new(-100.0, 100.0)), sprite(2, Vec2::splat(50.0))));
    let b = t.engine.world_mut().spawn((Name::new("b"), EditorId { id: 21 }, Transform::from_position(Vec2::new(-100.0, 100.0)), sprite(2, Vec2::splat(50.0))));
    settle(&mut t);
    click_world(&mut t, Vec2::new(-100.0, 100.0));
    let want = if a.id() > b.id() { "a" } else { "b" };
    assert_eq!(name_of(&t, selected(&t)).as_deref(), Some(want));
}

#[test]
fn rotated_sprites_are_hit_by_their_real_quad() {
    let mut t = vp();
    // `bar` is 100x20 turned 45 degrees at (-300,-200): its long axis runs along (1,1)
    let along = Vec2::new(-300.0, -200.0) + Vec2::new(1.0, 1.0).normalize() * 40.0;
    click_world(&mut t, along);
    assert_eq!(name_of(&t, selected(&t)).as_deref(), Some("bar"), "on the rotated quad");
    // the same distance across the short axis is outside (and outside the axis-aligned bounds of the unrotated quad too)
    let across = Vec2::new(-300.0, -200.0) + Vec2::new(1.0, -1.0).normalize() * 40.0;
    click_world(&mut t, across);
    assert_eq!(selected(&t), None, "empty space clears the selection");
    // the unrotated 100x20 box would have contained this point
    click_world(&mut t, Vec2::new(-300.0 + 40.0, -200.0));
    assert_eq!(selected(&t), None, "the rotated quad does not cover it");
}

#[test]
fn entities_without_a_sprite_are_picked_by_their_handle_and_empty_space_clears() {
    let mut t = vp();
    // `rock` and the game camera sit at the origin; move rock away to have a handle of its own
    let rock = entity(&t, "rock");
    t.engine.world_mut().get::<&mut Transform>(rock).unwrap().position = Vec2::new(-150.0, 150.0);
    t.step(2);
    click_world(&mut t, Vec2::new(-150.0 + 5.0, 150.0 - 4.0));
    assert_eq!(name_of(&t, selected(&t)).as_deref(), Some("rock"), "the handle is a few pixels wide");
    click_world(&mut t, Vec2::new(150.0, -120.0));
    assert_eq!(selected(&t), None, "nothing there: the selection is cleared");
    // handles win over a sprite under them
    t.engine.world_mut().get::<&mut Transform>(rock).unwrap().position = Vec2::new(300.0, 200.0);
    t.step(2);
    click_world(&mut t, Vec2::new(300.0, 200.0));
    assert_eq!(name_of(&t, selected(&t)).as_deref(), Some("rock"));
}

#[test]
fn a_click_over_ui_or_in_play_is_not_a_pick() {
    let mut t = vp();
    t.backend().set_capture(UiCapture { pointer: true, keyboard: false });
    t.step(2);
    click_world(&mut t, Vec2::new(300.0, 200.0));
    assert_eq!(selected(&t), None, "the pointer is over a panel");
    t.backend().set_capture(UiCapture::default());
    t.step(2);
    t.click("debug_menu/editor/toolbar/play");
    t.step(3);
    click_world(&mut t, Vec2::new(300.0, 200.0));
    assert_eq!(selected(&t), None, "Play: the game gets the click");
}

#[test]
fn a_pick_is_not_an_edit() {
    let mut t = vp();
    click_world(&mut t, Vec2::new(300.0, 200.0));
    assert_eq!(undo_len(&t), 0);
    assert!(!ui_state(&t).dirty);
}

// ----- gizmos -----------------------------------------------------------------------------------

#[test]
fn the_move_gizmo_drags_freely_or_along_an_axis_with_one_undo_entry() {
    let mut t = vp();
    select_name(&mut t, "blob");
    let start = pos(&t, "blob");
    assert_eq!(undo_len(&t), 0);
    // free: the centre square
    drag_world(&mut t, start, start + Vec2::new(30.0, 20.0), 6);
    assert!(near(pos(&t, "blob"), start + Vec2::new(30.0, 20.0)), "{:?}", pos(&t, "blob"));
    assert_eq!(undo_len(&t), 1, "one entry for the whole drag");
    assert!(ui_state(&t).dirty);
    // X arrow: 40 px along +x from the centre
    let c = pos(&t, "blob");
    drag_world(&mut t, c + Vec2::new(40.0, 0.0), c + Vec2::new(70.0, 33.0), 4);
    assert!(near(pos(&t, "blob"), c + Vec2::new(30.0, 0.0)), "only x moves: {:?}", pos(&t, "blob"));
    assert_eq!(undo_len(&t), 2);
    // Y arrow
    let c = pos(&t, "blob");
    drag_world(&mut t, c + Vec2::new(0.0, 40.0), c + Vec2::new(25.0, 10.0), 4);
    assert!(near(pos(&t, "blob"), c + Vec2::new(0.0, -30.0)), "only y moves: {:?}", pos(&t, "blob"));
    assert_eq!(undo_len(&t), 3);
    // undo goes back drag by drag
    undo(&mut t);
    assert!(near(pos(&t, "blob"), c));
    undo(&mut t);
    undo(&mut t);
    assert!(near(pos(&t, "blob"), start));
    assert_eq!(undo_len(&t), 0);
}

#[test]
fn a_drag_that_ends_where_it_began_records_nothing_and_escape_cancels() {
    let mut t = vp();
    select_name(&mut t, "blob");
    let start = pos(&t, "blob");
    drag_world(&mut t, start, start, 2);
    assert_eq!(undo_len(&t), 0);
    // Escape in the middle of a drag puts the start back
    let s = screen(&t, start);
    motion(&mut t, 0, s, Vec2::ZERO);
    t.step(1);
    button(&mut t, 0, MouseButton::Left, true, s);
    t.step(1);
    motion(&mut t, 0, s + Vec2::new(40.0, 0.0), Vec2::new(40.0, 0.0));
    t.step(2);
    assert!(near(pos(&t, "blob"), start + Vec2::new(40.0, 0.0)));
    key(&mut t, KeyCode::Escape);
    assert!(near(pos(&t, "blob"), start));
    button(&mut t, 0, MouseButton::Left, false, s);
    t.step(2);
    assert!(near(pos(&t, "blob"), start));
    assert_eq!(undo_len(&t), 0);
}

#[test]
fn the_rotate_gizmo_turns_the_entity() {
    let mut t = vp();
    select_name(&mut t, "blob");
    tool(&mut t, KeyCode::E);
    assert_eq!(t.resource::<EditorView>().tool, GizmoTool::Rotate);
    let c = pos(&t, "blob");
    // grab the ring (64 px) at the right, drag a quarter turn counter-clockwise (to the top)
    drag_world(&mut t, c + Vec2::new(64.0, 0.0), c + Vec2::new(0.0, 64.0), 8);
    let r = tf(&t, "blob").rotation;
    assert!((r - std::f32::consts::FRAC_PI_2).abs() < 0.02, "{r}");
    assert!(near(pos(&t, "blob"), c), "rotating does not move it");
    assert_eq!(undo_len(&t), 1);
    undo(&mut t);
    assert_eq!(tf(&t, "blob").rotation, 0.0);
    // Ctrl snaps to 15 degrees
    mods(&mut t, Modifiers::LCTRL);
    drag_world(&mut t, c + Vec2::new(64.0, 0.0), c + Vec2::new(64.0, 0.0) + Vec2::new(-10.0, 30.0), 5);
    mods(&mut t, Modifiers::NONE);
    let r = tf(&t, "blob").rotation;
    let steps = r / ROTATE_SNAP;
    assert!((steps - steps.round()).abs() < 1e-3 && steps.round() != 0.0, "snapped: {r}");
}

#[test]
fn the_scale_gizmo_uses_the_corners_and_shift_keeps_proportions() {
    let mut t = vp();
    select_name(&mut t, "blob");
    tool(&mut t, KeyCode::R);
    assert_eq!(t.resource::<EditorView>().tool, GizmoTool::Scale);
    let c = pos(&t, "blob");
    // blob is 40x40: its top-right corner is (20, 20) away
    let corner = c + Vec2::new(20.0, 20.0);
    drag_world(&mut t, corner, c + Vec2::new(40.0, 20.0), 5);
    let s = tf(&t, "blob").scale;
    assert!((s.x - 2.0).abs() < 0.02 && (s.y - 1.0).abs() < 0.02, "x doubled, y unchanged: {s:?}");
    assert_eq!(undo_len(&t), 1);
    undo(&mut t);
    assert_eq!(tf(&t, "blob").scale, Vec2::ONE);
    // Shift: both axes by the same factor
    mods(&mut t, Modifiers::LSHIFT);
    drag_world(&mut t, corner, c + Vec2::new(40.0, 40.0), 5);
    mods(&mut t, Modifiers::NONE);
    let s = tf(&t, "blob").scale;
    assert!((s.x - 2.0).abs() < 0.02 && (s.x - s.y).abs() < 1e-3, "uniform: {s:?}");
    // Ctrl snaps the scale to quarters
    undo(&mut t);
    mods(&mut t, Modifiers::LCTRL);
    drag_world(&mut t, corner, c + Vec2::new(31.0, 20.0), 3);
    mods(&mut t, Modifiers::NONE);
    let s = tf(&t, "blob").scale;
    let q = s.x / SCALE_SNAP;
    assert!((q - q.round()).abs() < 1e-3, "snapped: {s:?}");
}

#[test]
fn snapping_to_the_grid_needs_ctrl_and_follows_the_step() {
    let mut t = vp();
    select_name(&mut t, "blob");
    let start = pos(&t, "blob");
    // Ctrl held: the position lands on multiples of the step (16 by default)
    mods(&mut t, Modifiers::LCTRL);
    drag_world(&mut t, start, start + Vec2::new(21.0, 13.0), 4);
    mods(&mut t, Modifiers::NONE);
    let p = pos(&t, "blob");
    assert_eq!((p.x % 16.0, p.y % 16.0), (0.0, 0.0), "{p:?}");
    assert_eq!(p.x, 16.0);
    undo(&mut t);
    // without Ctrl nothing snaps
    drag_world(&mut t, start, start + Vec2::new(21.0, 13.0), 4);
    assert!(near(pos(&t, "blob"), start + Vec2::new(21.0, 13.0)));
    undo(&mut t);
    // the step is a toolbar setting
    t.set_text("debug_menu/editor/viewport/grid_step", "10");
    t.step(2);
    t.submit("debug_menu/editor/viewport/grid_step");
    t.step(3);
    assert_eq!(t.resource::<EditorView>().grid_step, 10.0);
    mods(&mut t, Modifiers::LCTRL);
    drag_world(&mut t, start, start + Vec2::new(21.0, 13.0), 4);
    mods(&mut t, Modifiers::NONE);
    let p = pos(&t, "blob");
    assert_eq!((p.x % 10.0, p.y % 10.0), (0.0, 0.0), "{p:?}");
    // a bad step is refused and keeps the old one
    t.set_text("debug_menu/editor/viewport/grid_step", "-3");
    t.step(2);
    t.submit("debug_menu/editor/viewport/grid_step");
    t.step(3);
    assert_eq!(t.resource::<EditorView>().grid_step, 10.0);
}

#[test]
fn g_toggles_the_grid_and_it_is_drawn() {
    let mut t = vp();
    select_name(&mut t, "big");
    let base = t.resource::<Drawn>().0;
    key(&mut t, KeyCode::G);
    assert!(t.resource::<EditorView>().grid);
    t.step(2);
    assert!(t.resource::<Drawn>().0 > base + 20, "grid lines: {} vs {base}", t.resource::<Drawn>().0);
    key(&mut t, KeyCode::G);
    t.step(1);
    assert_eq!(t.resource::<Drawn>().0, base);
    // the toolbar checkbox says the same
    t.set_checked("debug_menu/editor/viewport/grid", true);
    t.step(3);
    assert!(t.resource::<EditorView>().grid);
}

#[test]
fn a_child_is_moved_in_world_space_and_its_local_transform_is_written() {
    let mut t = vp();
    // a rotated, scaled parent with a child at its local (10, 0)
    let parent = t.engine.world_mut().spawn((
        Name::new("holder"),
        EditorId { id: 30 },
        Transform { position: Vec2::new(-100.0, -100.0), rotation: std::f32::consts::FRAC_PI_2, scale: Vec2::splat(2.0) },
    ));
    let child = t.engine.world_mut().spawn((
        Name::new("kid"),
        EditorId { id: 31 },
        Transform::from_position(Vec2::new(10.0, 0.0)),
        Parent::new(parent),
        sprite(3, Vec2::splat(20.0)),
    ));
    settle(&mut t);
    select_name(&mut t, "kid");
    let w0 = world_affine_of(t.engine.world(), child).translation;
    assert!(near(w0, Vec2::new(-100.0, -80.0)), "rotated 90 degrees and scaled 2: {w0:?}");
    // drag by (+30, 0) in world space
    drag_world(&mut t, w0, w0 + Vec2::new(30.0, 0.0), 5);
    let w1 = world_affine_of(t.engine.world(), child).translation;
    assert!(near(w1, w0 + Vec2::new(30.0, 0.0)), "the child follows the pointer in world space: {w1:?}");
    let local = tf(&t, "kid").position;
    // world +x is the parent's -y direction scaled by 2: local = (10, -15)
    assert!(near(local, Vec2::new(10.0, -15.0)), "the local transform is what was written: {local:?}");
    assert_eq!(undo_len(&t), 1);
    undo(&mut t);
    assert!(near(tf(&t, "kid").position, Vec2::new(10.0, 0.0)));
    // rotating a child: the world rotation changes by the dragged angle
    tool(&mut t, KeyCode::E);
    let c = world_affine_of(t.engine.world(), child).translation;
    drag_world(&mut t, c + Vec2::new(64.0, 0.0), c + Vec2::new(0.0, 64.0), 6);
    let aff = world_affine_of(t.engine.world(), child);
    let world_rot = aff.matrix2.x_axis.to_angle();
    assert!((world_rot - std::f32::consts::PI).abs() < 0.03 || (world_rot + std::f32::consts::PI).abs() < 0.03, "{world_rot}");
}

#[test]
fn children_follow_a_dragged_parent() {
    let mut t = vp();
    select_name(&mut t, "hero");
    let pet_before = world_affine_of(t.engine.world(), entity(&t, "pet")).translation;
    let hero = pos(&t, "hero");
    drag_world(&mut t, hero, hero + Vec2::new(50.0, 0.0), 4);
    t.step(2);
    let pet_after = t.engine.world().get::<&GlobalTransform>(entity(&t, "pet")).unwrap().translation();
    assert!(near(pet_after, pet_before + Vec2::new(50.0, 0.0)), "{pet_after:?}");
}

// ----- reparenting ------------------------------------------------------------------------------

fn parent_of(t: &UiTest, name: &str) -> Option<String> {
    let p = t.engine.world().get::<&Parent>(entity(t, name)).ok().and_then(|p| p.entity())?;
    name_of(t, Some(p))
}

fn send_msg(t: &mut UiTest, m: EditorMsg) {
    struct Out(Option<EditorMsg>);
    fn go(cx: &mut Ctx) {
        if let Some(m) = cx.resources.get_mut::<Out>().and_then(|s| s.0.take()) {
            cx.send(m);
        }
    }
    if t.engine.resources().get::<Out>().is_none() {
        t.engine.insert_resource(Out(None));
        t.engine.add_system(Stage::Update, go).name("test_send_msg").order(order::EARLY - 10);
    }
    t.resource_mut::<Out>().0 = Some(m);
    t.step(4);
}

fn world_of(t: &UiTest, name: &str) -> Affine2Like {
    let a = world_affine_of(t.engine.world(), entity(t, name));
    Affine2Like { t: a.translation, x: a.matrix2.x_axis, y: a.matrix2.y_axis }
}

#[derive(Debug, PartialEq, Clone, Copy)]
struct Affine2Like {
    t: Vec2,
    x: Vec2,
    y: Vec2,
}

fn same_world(a: Affine2Like, b: Affine2Like) -> bool {
    near(a.t, b.t) && near(a.x, b.x) && near(a.y, b.y)
}

#[test]
fn reparenting_keeps_the_world_transform_and_undo_restores_everything() {
    let mut t = vp();
    // `big` is turned and scaled so the conversion has something to do
    let big = entity(&t, "big");
    *t.engine.world_mut().get::<&mut Transform>(big).unwrap() = Transform { position: Vec2::new(300.0, 200.0), rotation: 0.5, scale: Vec2::splat(2.0) };
    t.step(2);
    select_name(&mut t, "blob");
    let before = world_of(&t, "blob");
    let local_before = tf(&t, "blob");
    send_msg(&mut t, EditorMsg::Reparent(Some(big)));
    assert_eq!(parent_of(&t, "blob").as_deref(), Some("big"));
    assert!(same_world(world_of(&t, "blob"), before), "same place in the world: {:?} vs {before:?}", world_of(&t, "blob"));
    assert_ne!(tf(&t, "blob"), local_before, "the local transform changed");
    assert_eq!(undo_len(&t), 1);
    assert!(ui_state(&t).dirty);
    // the hierarchy shows it as a child
    let rows = t.resource::<EditorPanel>().rows.clone();
    let blob_row: &EntityRow = rows.iter().find(|r| r.name == "blob").unwrap();
    assert_eq!(blob_row.depth, 1);
    // undo: root again with the old local transform; redo: child again
    undo(&mut t);
    assert_eq!(parent_of(&t, "blob"), None);
    assert_eq!(tf(&t, "blob"), local_before);
    t.click("debug_menu/editor/toolbar/redo");
    t.step(3);
    assert_eq!(parent_of(&t, "blob").as_deref(), Some("big"));
    assert!(same_world(world_of(&t, "blob"), before));
    // Unparent keeps the world place too
    t.click("debug_menu/editor/hierarchy/actions/unparent");
    t.step(3);
    assert_eq!(parent_of(&t, "blob"), None);
    assert!(same_world(world_of(&t, "blob"), before));
    assert_eq!(undo_len(&t), 2);
    undo(&mut t);
    assert_eq!(parent_of(&t, "blob").as_deref(), Some("big"));
}

#[test]
fn the_inspector_picker_reparents() {
    let mut t = vp();
    select_name(&mut t, "blob");
    let before = world_of(&t, "blob");
    t.click("debug_menu/editor/inspector/parent/row/toggle");
    t.step(3);
    let rows = t.resource::<EditorPanel>().rows.clone();
    // candidates: every row but blob itself, in list order
    let list: Vec<&str> = rows.iter().filter(|r| r.name != "blob").map(|r| r.name.as_str()).collect();
    let i = list.iter().position(|n| *n == "top").unwrap();
    t.click(&format!("debug_menu/editor/inspector/parent/picker/p{i}"));
    t.step(4);
    assert_eq!(parent_of(&t, "blob").as_deref(), Some("top"));
    assert!(same_world(world_of(&t, "blob"), before));
    assert_eq!(t.text("debug_menu/editor/inspector/parent/row/value"), "top");
    // the picker closed itself; "none" unparents
    assert!(!ui_state(&t).parent_picker);
    t.click("debug_menu/editor/inspector/parent/row/toggle");
    t.step(3);
    t.click("debug_menu/editor/inspector/parent/picker/none");
    t.step(4);
    assert_eq!(parent_of(&t, "blob"), None);
}

#[test]
fn a_cycle_is_refused() {
    let mut t = vp();
    // pet is a child of hero: hero under pet (or under itself) would be a cycle
    select_name(&mut t, "hero");
    let (pet, hero) = (entity(&t, "pet"), entity(&t, "hero"));
    send_msg(&mut t, EditorMsg::Reparent(Some(pet)));
    assert_eq!(parent_of(&t, "hero"), None, "unchanged");
    assert_eq!(parent_of(&t, "pet").as_deref(), Some("hero"));
    assert!(ui_state(&t).status_error && ui_state(&t).status.contains("parent"), "{}", ui_state(&t).status);
    assert_eq!(undo_len(&t), 0);
    send_msg(&mut t, EditorMsg::Reparent(Some(hero)));
    assert_eq!(parent_of(&t, "hero"), None);
    assert_eq!(undo_len(&t), 0);
    // the picker does not even offer the entity or its descendants
    t.click("debug_menu/editor/inspector/parent/row/toggle");
    t.step(3);
    let n = t.resource::<EditorPanel>().rows.len();
    assert!(t.ui().find(&format!("debug_menu/editor/inspector/parent/picker/p{}", n - 3)).is_some());
    assert!(t.ui().find(&format!("debug_menu/editor/inspector/parent/picker/p{}", n - 2)).is_none(), "hero and pet are left out");
}

// ----- step-2 leftovers: stale references ---------------------------------------------------------

#[test]
fn removing_parent_deleting_the_parent_and_undoing_both_restores_the_link() {
    let mut t = vp();
    select_name(&mut t, "pet");
    let comps = t.resource::<EditorPanel>().inspected.clone().unwrap();
    let i = comps.iter().position(|c| c.name == "Parent").expect("pet has a Parent");
    t.click(&format!("debug_menu/editor/inspector/c{i}/bar/remove"));
    t.step(3);
    assert_eq!(parent_of(&t, "pet"), None);
    let old_hero = entity(&t, "hero");
    select_name(&mut t, "hero");
    t.click("debug_menu/editor/hierarchy/actions/delete");
    t.step(3);
    assert!(!t.engine.world().contains(old_hero));
    undo(&mut t); // the hero is back, with other entity bits
    let new_hero = entity(&t, "hero");
    assert_ne!(new_hero, old_hero, "the respawned hero has new bits");
    undo(&mut t); // the Parent component of pet is back and points at the new hero
    assert_eq!(parent_of(&t, "pet").as_deref(), Some("hero"));
    let p = t.engine.world().get::<&Parent>(entity(&t, "pet")).unwrap().entity();
    assert_eq!(p, Some(new_hero), "resolved by EditorId at undo time, not the stale bits");
}

#[test]
fn undoing_a_delete_resolves_references_outside_the_deleted_group_by_editor_id() {
    let mut t = vp();
    // pet (child of hero) is deleted, then hero is deleted and brought back, then pet is undone: its Parent must point at the new hero
    select_name(&mut t, "pet");
    t.click("debug_menu/editor/hierarchy/actions/delete");
    t.step(3);
    let old = entity(&t, "hero");
    select_name(&mut t, "hero");
    t.click("debug_menu/editor/hierarchy/actions/delete");
    t.step(3);
    undo(&mut t);
    let new = entity(&t, "hero");
    assert_ne!(new, old);
    undo(&mut t);
    assert_eq!(t.engine.world().get::<&Parent>(entity(&t, "pet")).unwrap().entity(), Some(new));
}

// ----- layout B ---------------------------------------------------------------------------------

fn detached() -> UiTest {
    let mut t = editor_with(EditorPlugin::new().separate_window(true));
    t.step(4);
    t
}

#[test]
fn separate_window_opens_a_window_with_the_editor_camera_and_the_panels() {
    let mut t = detached();
    let view = (*t.resource::<EditorView>()).clone();
    let w = view.window.expect("the editor window is open");
    assert!(view.detached && t.engine.state.windows.contains(WindowId(w)));
    assert_eq!(t.engine.state.windows.len(), 2);
    let editor_cam = t.resource::<EditorCamera>().entity;
    let game = game_camera(&t);
    {
        let f = t.engine.render_frame();
        assert_eq!(f.d2.camera.as_ref().unwrap().entity, game, "the main window keeps the game camera in Edit");
        assert_eq!(f.window(w).unwrap().0.camera.as_ref().unwrap().entity, editor_cam, "the editor window shows the editor camera");
    }
    // the panels are in the editor window's layer; the Debug menu tab only points there
    assert_eq!(t.ui().layer_window(t.id("editor_panel")), Some(w));
    assert!(t.ui().find("editor_panel/editor/toolbar/play").is_some());
    assert!(t.ui().find("editor_panel/editor/hierarchy/list").is_some());
    t.resource_mut::<UiViews>().set_enabled("debug_menu", true);
    t.step(2);
    t.click("debug_menu/tab_editor");
    t.step(3);
    assert!(t.ui().find("debug_menu/editor/detached/row/attach").is_some());
    assert!(t.ui().find("debug_menu/editor/toolbar/play").is_none(), "the panels are not shown twice");
    // the game camera still draws the main window in Play and Pause
    t.click("editor_panel/editor/toolbar/play");
    t.step(3);
    assert_eq!(mode(&t), EditorMode::Play);
    assert_eq!(t.engine.render_frame().d2.camera.as_ref().unwrap().entity, game);
    assert_eq!(t.engine.render_frame().window(w).unwrap().0.camera.as_ref().unwrap().entity, editor_cam, "the editor window stays live in Play");
    t.click("editor_panel/editor/toolbar/stop");
    t.step(3);
    assert_eq!(mode(&t), EditorMode::Edit);
}

#[test]
fn the_viewport_works_in_the_editor_window_only() {
    let mut t = detached();
    let w = t.resource::<EditorView>().window.unwrap();
    let vp_size = t.engine.state.windows.get(WindowId(w)).unwrap().size;
    let c = *t.resource::<EditorCamera>();
    let cam_world = |t: &UiTest| (t.engine.world().get::<&Transform>(c.entity).unwrap().position, t.engine.world().get::<&Camera>(c.entity).unwrap().zoom);
    ev(&mut t, w, InputEventKind::Window(efeengine_core::input::WindowEvent::Focused(true)));
    t.step(1);
    // a click in the main window picks nothing in the editor's world view
    let at = Vec2::new(vp_size.0 as f32 / 2.0 + 300.0, vp_size.1 as f32 / 2.0 - 200.0);
    motion(&mut t, 0, at, Vec2::ZERO);
    t.step(1);
    button(&mut t, 0, MouseButton::Left, true, at);
    t.step(2);
    button(&mut t, 0, MouseButton::Left, false, at);
    t.step(2);
    assert_eq!(selected(&t), None, "the main window is the game's");
    // a click in the editor window does; the world has no sprite there but `rock` handles: add one
    t.engine.world_mut().spawn((Name::new("big"), EditorId { id: 10 }, Transform::from_position(Vec2::new(300.0, 200.0)), sprite(0, Vec2::splat(100.0))));
    t.step(2);
    motion(&mut t, w, at, Vec2::ZERO);
    t.step(1);
    button(&mut t, w, MouseButton::Left, true, at);
    t.step(2);
    button(&mut t, w, MouseButton::Left, false, at);
    t.step(2);
    assert_eq!(name_of(&t, selected(&t)).as_deref(), Some("big"));
    // wheel in the editor window zooms the editor camera
    ev(&mut t, w, InputEventKind::MouseWheel { delta: (0.0, 1.0), unit: WheelUnit::Line });
    t.step(2);
    assert!(cam_world(&t).1 > 1.05);
    // the shortcut keys follow the keyboard focus window
    key_in(&mut t, w, KeyCode::R);
    assert_eq!(t.resource::<EditorView>().tool, GizmoTool::Scale);
    ev(&mut t, 0, InputEventKind::Window(efeengine_core::input::WindowEvent::Focused(true)));
    t.step(1);
    key_in(&mut t, 0, KeyCode::E);
    assert_eq!(t.resource::<EditorView>().tool, GizmoTool::Scale, "keys typed into the game window are the game's");
    // gizmo shapes land in the editor window's frame
    assert!(!t.engine.render_frame().window(w).unwrap().0.draw.is_empty(), "outline and gizmo are drawn in the editor window");
}

#[test]
fn closing_the_editor_window_attaches_the_panels_again() {
    let mut t = detached();
    let w = t.resource::<EditorView>().window.unwrap();
    t.engine.close_window(WindowId(w));
    t.step(4);
    let v = (*t.resource::<EditorView>()).clone();
    assert!(!v.detached && v.window.is_none());
    assert_eq!(t.engine.state.windows.len(), 1);
    let cam = t.resource::<EditorCamera>().entity;
    assert_eq!(t.engine.render_frame().d2.camera.as_ref().unwrap().entity, cam, "layout A: the editor camera draws the main window in Edit");
    t.resource_mut::<UiViews>().set_enabled("debug_menu", true);
    t.step(2);
    t.click("debug_menu/tab_editor");
    t.step(3);
    assert!(t.ui().find("debug_menu/editor/toolbar/play").is_some(), "the tab has the panels back");
    assert!(!t.engine.resources().get::<UiViews>().unwrap().is_enabled(PANEL_VIEW));
}

#[test]
fn the_detach_and_attach_buttons_switch_layouts() {
    let mut t = editor();
    assert!(!t.resource::<EditorView>().detached);
    assert_eq!(t.text("debug_menu/editor/viewport/detach"), "Detach");
    t.click("debug_menu/editor/viewport/detach");
    t.step(4);
    let w = t.resource::<EditorView>().window.expect("opened");
    assert_eq!(t.engine.state.windows.len(), 2);
    assert_eq!(t.engine.world().get::<&Camera>(t.resource::<EditorCamera>().entity).unwrap().target, w);
    assert_eq!(t.text("editor_panel/editor/viewport/detach"), "Attach");
    t.click("editor_panel/editor/viewport/detach");
    t.step(4);
    assert!(t.resource::<EditorView>().window.is_none());
    assert_eq!(t.engine.state.windows.len(), 1, "the window closed");
    assert_eq!(t.engine.world().get::<&Camera>(t.resource::<EditorCamera>().entity).unwrap().target, 0);
    // and back through the tab's note
    t.click("debug_menu/editor/viewport/detach");
    t.step(4);
    t.click("debug_menu/editor/detached/row/attach");
    t.step(4);
    assert!(!t.resource::<EditorView>().detached);
}

#[test]
fn without_a_second_window_the_editor_stays_in_layout_a() {
    struct Nothing;
    impl efeengine_core::render::CustomRenderer for Nothing {
        fn render(&mut self, _: &efeengine_core::render::Gpu, _: &efeengine_core::render::wgpu::TextureView, _: &efeengine_core::render::RenderFrame) {}
    }
    crate::debug_menu::serial();
    let mut t = UiTest::new(|e| {
        e.add_plugin(crate::debug_menu::DebugMenuPlugin).unwrap();
        e.add_plugin(EditorPlugin::new().separate_window(true)).unwrap();
        e.set_renderer(Nothing);
    });
    t.resource_mut::<UiViews>().set_enabled("debug_menu", true);
    t.step(5);
    let v = (*t.resource::<EditorView>()).clone();
    assert!(!v.detached && v.window.is_none(), "{v:?}");
    assert_eq!(t.engine.state.windows.len(), 1, "no window was opened");
    assert!(ui_state(&t).status.contains("custom renderer"), "{}", ui_state(&t).status);
    let cam = t.resource::<EditorCamera>().entity;
    assert_eq!(t.engine.world().get::<&Camera>(cam).unwrap().target, 0);
    let _ = viewport::MIN_ZOOM;
}

// ----- screenshots: the world through the real sprite renderer (`--nocapture` prints the PNGs base64 encoded) ---

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Renders the main window's frame offscreen; `None` without a graphics adapter.
fn shoot(t: &UiTest, name: &str) -> Option<Vec<u8>> {
    use efeengine_core::render::{OffscreenGpu, Renderer2D, SpriteRenderer, encode_png};
    let Ok(mut off) = OffscreenGpu::new(VP) else {
        println!("SHOT-SKIPPED no graphics adapter");
        return None;
    };
    let frame = t.engine.render_frame().clone();
    let mut r = SpriteRenderer::new();
    off.render_2d(&mut r as &mut dyn Renderer2D, &frame.d2, &frame.textures);
    let rgba = off.read_rgba8();
    println!("SHOT {name} {}", b64(&encode_png(VP.0, VP.1, &rgba)));
    Some(rgba)
}

fn pixel(rgba: &[u8], p: Vec2) -> [u8; 4] {
    let i = ((p.y as usize) * VP.0 as usize + p.x as usize) * 4;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn screenshots_of_the_viewport() {
    let mut t = vp();
    for (name, color) in [("big", [0.25, 0.45, 0.8, 1.0]), ("top", [0.9, 0.7, 0.2, 1.0]), ("bar", [0.4, 0.8, 0.5, 1.0]), ("blob", [0.85, 0.35, 0.4, 1.0])] {
        let e = entity(&t, name);
        t.engine.world_mut().get::<&mut Sprite>(e).unwrap().color = color;
    }
    key(&mut t, KeyCode::G);
    click_world(&mut t, Vec2::new(300.0, 200.0));
    t.step(2);
    if let Some(rgba) = shoot(&t, "step3_move_grid") {
        let blob = pixel(&rgba, screen(&t, Vec2::new(0.0, -250.0)));
        assert!(blob[0] > blob[1] + 40, "the blob sprite is drawn red-ish: {blob:?}");
        let top = pixel(&rgba, screen(&t, Vec2::new(290.0, 190.0)));
        assert!(top[0] > top[2], "top is drawn over big: {top:?}");
        // the gizmo's red X arrow, 40 px right of the selected sprite's centre
        let arrow = pixel(&rgba, screen(&t, Vec2::new(340.0, 200.0)));
        assert!(arrow[0] > arrow[2] + 60, "the X arrow is drawn: {arrow:?}");
    }
    tool(&mut t, KeyCode::E);
    select_name(&mut t, "bar");
    t.step(2);
    shoot(&t, "step3_rotate");
    tool(&mut t, KeyCode::R);
    select_name(&mut t, "big");
    t.step(2);
    shoot(&t, "step3_scale");
}
