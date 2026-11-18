//! The coin_dash menus through `UiTest` (headless backend): main menu, settings, pause with Esc,
//! the game freezing behind a menu, and game input not leaking while a menu captures it.

use coin_dash::menu::{self, Menu, Screen};
use coin_dash::setup;
use efeengine_core::input::{InputEvent, InputEventKind};
use efeengine_core::prelude::*;
use efeengine_ui::UiTest;

const PLAYER_TEX: u32 = 1;

fn game(start_playing: bool) -> UiTest {
    UiTest::new(|e| {
        setup(e);
        menu::setup(e, start_playing);
    })
}

fn player_x(t: &UiTest) -> f32 {
    let f = t.engine.render_frame();
    let s = f.d2.sprites.iter().find(|s| s.texture.id == PLAYER_TEX).expect("player sprite").entity;
    t.engine.world().get::<&Transform>(s).unwrap().position.x
}

fn enemy_positions(t: &UiTest) -> Vec<Vec2> {
    let mut v: Vec<Vec2> = t.engine.world().query::<(&Transform, &Velocity)>().iter().map(|(t, _)| t.position).collect();
    v.sort_by(|a, b| a.x.total_cmp(&b.x));
    v
}

fn key(t: &mut UiTest, k: KeyCode, down: bool) {
    let kind = if down { InputEventKind::key_pressed(k) } else { InputEventKind::key_released(k) };
    t.engine.inject(InputEvent::new(kind));
}

fn tap(t: &mut UiTest, k: KeyCode) {
    key(t, k, true);
    t.step(1);
    key(t, k, false);
    t.step(1);
}

fn visible(t: &UiTest, layer: &str) -> bool {
    // a view that was never enabled has no layer yet
    t.ui().find(layer).is_some_and(|id| t.ui().visible(id).unwrap())
}

#[test]
fn starts_on_the_main_menu_and_the_game_is_frozen_behind_it() {
    let mut t = game(false);
    t.step(2);
    assert_eq!(t.resource::<Menu>().screen, Screen::Main);
    assert!(visible(&t, "main_menu"));
    assert!(!visible(&t, "pause") && !visible(&t, "settings"));
    // the modal layer captures both devices
    assert!(t.capture().pointer && t.capture().keyboard);
    let enemies = enemy_positions(&t);
    // movement keys do not reach the game, and the enemies do not patrol
    key(&mut t, KeyCode::D, true);
    t.step(60);
    assert_eq!(player_x(&t), 0.0);
    assert_eq!(enemy_positions(&t), enemies);
}

#[test]
fn play_unfreezes_the_game_and_releases_input() {
    let mut t = game(false);
    t.step(2);
    t.click("main_menu/play");
    t.step(2);
    assert_eq!(t.resource::<Menu>().screen, Screen::Playing);
    assert!(!visible(&t, "main_menu"));
    assert!(!t.capture().keyboard, "no modal layer, no capture");
    let enemies = enemy_positions(&t);
    key(&mut t, KeyCode::D, true);
    t.step(30);
    assert!(player_x(&t) > 20.0, "the player moves with D: {}", player_x(&t));
    assert_ne!(enemy_positions(&t), enemies, "the enemies patrol again");
}

#[test]
fn escape_pauses_and_resumes() {
    let mut t = game(true);
    t.step(2);
    assert!(!visible(&t, "main_menu"));
    key(&mut t, KeyCode::D, true);
    t.step(10);
    let x = player_x(&t);
    assert!(x > 0.0);
    tap(&mut t, KeyCode::Escape);
    key(&mut t, KeyCode::D, true);
    t.step(2);
    assert_eq!(t.resource::<Menu>().screen, Screen::Paused);
    assert!(visible(&t, "pause"));
    assert!(t.capture().keyboard);
    let (frozen_x, enemies) = (player_x(&t), enemy_positions(&t));
    t.step(30);
    assert_eq!(player_x(&t), frozen_x, "the game is frozen while paused");
    assert_eq!(enemy_positions(&t), enemies);
    tap(&mut t, KeyCode::Escape);
    key(&mut t, KeyCode::D, true);
    t.step(10);
    assert_eq!(t.resource::<Menu>().screen, Screen::Playing);
    assert!(!visible(&t, "pause"));
    assert!(player_x(&t) > frozen_x);
}

#[test]
fn settings_change_volume_and_fullscreen_and_go_back() {
    let mut t = game(true);
    t.step(2);
    tap(&mut t, KeyCode::Escape);
    t.click("pause/settings");
    t.step(2);
    assert!(t.resource::<Menu>().settings_open);
    assert!(visible(&t, "settings") && !visible(&t, "pause"));
    assert_eq!(t.text("settings/volume_text"), "Volume: 80%");
    t.set_value("settings/volume", 0.35);
    t.set_checked("settings/fullscreen", true);
    t.step(2);
    assert_eq!(t.resource::<Menu>().volume, 0.35);
    assert!(t.resource::<Menu>().fullscreen);
    assert_eq!(t.text("settings/volume_text"), "Volume: 35%");
    // Esc goes back to the pause menu, not to the game
    tap(&mut t, KeyCode::Escape);
    assert!(!t.resource::<Menu>().settings_open);
    assert_eq!(t.resource::<Menu>().screen, Screen::Paused);
    assert!(visible(&t, "pause") && !visible(&t, "settings"));
    // the Back button does the same from the main menu
    t.click("pause/main");
    t.step(2);
    assert_eq!(t.resource::<Menu>().screen, Screen::Main);
    t.click("main_menu/settings");
    t.step(2);
    t.click("settings/back");
    t.step(2);
    assert!(visible(&t, "main_menu"));
}

#[test]
fn quit_shuts_the_engine_down() {
    let mut t = game(false);
    t.step(2);
    assert!(t.engine.is_running());
    t.click("main_menu/quit");
    t.step(2);
    assert!(!t.engine.is_running());
}

#[test]
fn the_hud_shows_the_score_and_round() {
    let mut t = game(true);
    t.step(2);
    assert!(t.text("hud/stats").starts_with("Score 0   Round 1"), "{}", t.text("hud/stats"));
}
