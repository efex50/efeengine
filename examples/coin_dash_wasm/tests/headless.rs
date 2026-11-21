//! Headless tests for `coin_dash_wasm`:
//! - `bindings_are_up_to_date` (checks committed `efe_api.h` and `src/api.rs`; `EFE_BLESS=1` rewrites)
//! - missing `.wasm` opens cleanly and shows `"mods not built: run build_mods.py"` in UI
//! - main menu, pause, settings, HUD, game-over and restart flow through `UiTest`
//! - building both wasm mods via `build_mods.py` (skipped cleanly if toolchains are missing),
//!   verifying both mods load, C provides are called, events flow, enemies patrol, coins spin/bob,
//!   autoplay collects coins, score rises, and a round restarts.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use coin_dash_wasm::menu::{self, Menu, Screen};
use coin_dash_wasm::{
    Coin, Game, Patrol, Player, dash_c_own_index, register_api, setup_with_wasm_dir,
};
use efeengine_core::cache::{CacheConfig, CacheDir};
use efeengine_core::efevm::ApiIndex;
use efeengine_core::input::{InputEvent, InputEventKind};
use efeengine_core::prelude::*;
use efeengine_core::scripting::LogLevel;
use efeengine_ui::{UiPlugin, UiTest};

const DT: f64 = 1.0 / 60.0;

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn c_mod_dir() -> PathBuf {
    example_dir().join("mods/dash_c")
}

fn rust_mod_dir() -> PathBuf {
    example_dir().join("mods/dash_rust")
}

// ---------------------------------------------------------------- generated bindings check

fn check_generated(path: &Path, fresh: &str) {
    if std::env::var_os("EFE_BLESS").is_some() {
        std::fs::write(path, fresh).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let have = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        have == fresh,
        "{} is out of date: regenerate it with `EFE_BLESS=1 cargo test -p coin_dash_wasm --test headless bindings`",
        path.display()
    );
}

#[test]
fn bindings_are_up_to_date() {
    let mut e = Engine::with_config(EngineConfig {
        fixed_dt: DT,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    })
    .expect("headless engine");
    e.add_plugin(UiPlugin::headless()).expect("UiPlugin");
    register_api(&mut e);
    let game_index = e.api_index();

    let c_header = efescript::cgen::generate(&game_index).expect("c bindings");
    check_generated(&c_mod_dir().join("efe_api.h"), &c_header);

    let c_toml = std::fs::read_to_string(c_mod_dir().join("mod.toml")).expect("dash_c/mod.toml");
    let c_own = dash_c_own_index(&game_index, &c_toml).expect("dash_c own index");
    let merged = ApiIndex::merge(&[&game_index, &c_own]).expect("merge game + dash_c index");
    let rust_api = efescript::rustgen::generate(&merged).expect("rust bindings");
    check_generated(&rust_mod_dir().join("src/api.rs"), &rust_api);
}

// ---------------------------------------------------------------- building the wasm mods

fn built_wasm_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("wasm_mods_out");
        let _ = std::fs::remove_dir_all(&out);
        let script = example_dir().join("build_mods.py");
        let o = match Command::new("python3").arg(&script).arg("--out").arg(&out).output() {
            Ok(o) => o,
            Err(err) => {
                eprintln!("SKIPPED: cannot run python3 ({err})");
                return None;
            }
        };
        assert!(
            o.status.success(),
            "build_mods.py failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr),
        );
        let c_wasm = out.join("dash_c.wasm");
        let r_wasm = out.join("dash_rust.wasm");
        if !c_wasm.is_file() || !r_wasm.is_file() {
            eprintln!("SKIPPED: {}", String::from_utf8_lossy(&o.stdout).trim());
            return None;
        }
        Some(out)
    })
    .as_deref()
}

fn headless_engine(wasm_dir: &Path) -> (Engine, Game) {
    let cfg = EngineConfig {
        fixed_dt: DT,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    e.add_plugin(UiPlugin::headless()).expect("UiPlugin");
    let game = setup_with_wasm_dir(&mut e, Some(wasm_dir));
    game.set_autoplay(false);
    (e, game)
}

fn ui_test(wasm_dir: &Path, start_playing: bool) -> UiTest {
    let dir = wasm_dir.to_path_buf();
    UiTest::new(move |e| {
        let game = setup_with_wasm_dir(e, Some(&dir));
        game.set_autoplay(false);
        menu::setup(e, start_playing);
    })
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting()
        .log_lines()
        .filter(|l| l.level == LogLevel::Error)
        .map(|l| l.text.clone())
        .collect()
}

fn logs(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().map(|l| l.text.clone()).collect()
}

fn enemy_positions(e: &Engine) -> Vec<Vec2> {
    let mut v: Vec<Vec2> = e
        .world()
        .query::<(&Patrol, &Transform)>()
        .iter()
        .map(|(_, t)| t.position)
        .collect();
    v.sort_by(|a, b| a.x.total_cmp(&b.x).then_with(|| a.y.total_cmp(&b.y)));
    v
}

fn player_pos(e: &Engine) -> Vec2 {
    e.world()
        .query::<(&Player, &Transform)>()
        .iter()
        .next()
        .map(|(_, t)| t.position)
        .expect("player entity")
}

fn coins_count(e: &Engine) -> usize {
    e.world().query::<&Coin>().iter().count()
}

fn key(t: &mut UiTest, k: KeyCode, down: bool) {
    let kind = if down {
        InputEventKind::key_pressed(k)
    } else {
        InputEventKind::key_released(k)
    };
    t.engine.inject(InputEvent::new(kind));
}

fn tap(t: &mut UiTest, k: KeyCode) {
    key(t, k, true);
    t.step(1);
    key(t, k, false);
    t.step(1);
}

fn visible(t: &UiTest, layer: &str) -> bool {
    t.ui().find(layer).is_some_and(|id| t.ui().visible(id).unwrap())
}

// ---------------------------------------------------------------- tests

#[test]
fn missing_wasm_opens_and_shows_build_message_in_ui() {
    let missing = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("definitely_missing_wasm_dir");
    let _ = std::fs::remove_dir_all(&missing);
    let mut t = UiTest::new(move |e| {
        setup_with_wasm_dir(e, Some(&missing));
        menu::setup(e, false);
    });
    t.step(2);
    assert!(t.engine.is_running());
    assert_eq!(
        t.resource::<Game>().mod_status().as_deref(),
        Some("mods not built: run build_mods.py")
    );
    assert_eq!(t.text("main_menu/mod_status"), "mods not built: run build_mods.py");
    assert_eq!(t.text("hud/mod_status"), "mods not built: run build_mods.py");
}

#[test]
fn both_mods_load_c_provide_called_and_autoplay_collects_and_restarts_round() {
    let Some(dir) = built_wasm_dir() else { return };
    let (mut e, game) = headless_engine(dir);

    assert!(e.scripting().module_id("dash_c").is_some(), "dash_c loaded");
    assert!(e.scripting().module_id("dash_rust").is_some(), "dash_rust loaded");
    assert!(game.mod_status().is_none(), "{:?}", game.mod_status());

    // C provides are registered in the engine API index.
    let idx = e.api_index();
    for name in ["next_lane", "coin_phase", "check_coin_spot", "provide_calls"] {
        assert!(idx.function("dash_c", name).is_some(), "dash_c.{name} missing");
    }
    assert!(idx.function("dash_rust", "current_round").is_some(), "dash_rust.current_round missing");

    // 40 frames lets all 30 ground rows finish spawning (1 row/frame after row 0 in init).
    e.run_frames(40, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    // 1200 ground tiles + 5 coins + 3 enemies + player + shadow = 1210 sprites.
    assert_eq!(e.render_frame().d2.sprites.len(), 1200 + 5 + 3 + 2);
    assert_eq!(coins_count(&e), 5);

    // Check C provide was called at startup (init logs `dash_c provide calls=...`).
    let l = logs(&e);
    let call_line = l
        .iter()
        .find(|line| line.contains("dash_c provide calls="))
        .unwrap_or_else(|| panic!("expected dash_c provide calls log in {l:?}"));
    let count_str = call_line.split("dash_c provide calls=").nth(1).unwrap().trim_end_matches(')');
    let calls: i32 = count_str.parse().expect("numeric provide call count");
    assert!(calls >= 13, "expected at least 13 C provide calls (3 lanes + 5 spots + 5 phases), got {calls}");

    // Check event flowed from dash_rust -> dash_c.
    assert!(
        l.iter().any(|line| line.contains("[dash_c]") && line.contains("coin_spawned")),
        "expected dash_c to read game.coin_spawned event: {l:?}"
    );

    // Enemies patrol and coins spin & bob from dash_c.
    let enemies_before = enemy_positions(&e);
    let coin_uv_before: Vec<Vec2> = e.world().query::<(&Coin, &Sprite)>().iter().map(|(_, s)| s.uv_min).collect();
    let coin_y_before: Vec<f32> = e.world().query::<(&Coin, &Transform)>().iter().map(|(_, t)| t.position.y).collect();

    e.run_frames(20, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    let enemies_after = enemy_positions(&e);
    assert_eq!(enemies_before.len(), 3);
    for (a, b) in enemies_before.iter().zip(&enemies_after) {
        assert!(a.distance(*b) > 10.0, "enemy did not move: {a} -> {b}");
    }
    e.run_frames(65, DT);
    let coin_uv_after: Vec<Vec2> = e.world().query::<(&Coin, &Sprite)>().iter().map(|(_, s)| s.uv_min).collect();
    assert!(
        coin_uv_before.iter().zip(&coin_uv_after).any(|(a, b)| a.x != b.x),
        "coins did not spin: {coin_uv_before:?} -> {coin_uv_after:?}"
    );
    let moved_y = e
        .world()
        .query::<(&Coin, &Transform)>()
        .iter()
        .zip(&coin_y_before)
        .filter(|((_, t), y0)| (t.position.y - **y0).abs() > 1e-4)
        .count();
    assert!(moved_y >= 3, "only {moved_y} coins bobbed");

    // Autoplay bot collects coins, score rises, and a new round starts!
    game.set_autoplay(true);
    let mut best_score = 0;
    let mut max_round = 1;
    for _ in 0..45 * 60 {
        e.run_frames(1, DT);
        best_score = best_score.max(game.score());
        max_round = max_round.max(game.round());
        if max_round >= 2 && best_score >= 6 {
            break;
        }
    }
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert!(best_score >= 5, "expected score >= 5, got {best_score}");
    assert!(max_round >= 2, "expected round to advance >= 2, got {max_round}");
    assert!(coins_count(&e) > 0);
}

#[test]
fn menu_pause_settings_and_freeze_work_like_coin_dash() {
    let Some(dir) = built_wasm_dir() else { return };
    let mut t = ui_test(dir, false);
    t.step(2);

    assert_eq!(t.resource::<Menu>().screen, Screen::Main);
    assert!(visible(&t, "main_menu"));
    assert!(!visible(&t, "pause") && !visible(&t, "settings") && !visible(&t, "game_over"));
    assert!(t.capture().pointer && t.capture().keyboard);

    // Frozen behind main menu.
    let enemies0 = enemy_positions(&t.engine);
    key(&mut t, KeyCode::D, true);
    t.step(30);
    assert_eq!(player_pos(&t.engine), Vec2::ZERO);
    assert_eq!(enemy_positions(&t.engine), enemies0);
    key(&mut t, KeyCode::D, false);

    // Click Play -> unfreezes game, HUD appears.
    t.click("main_menu/play");
    t.step(2);
    assert_eq!(t.resource::<Menu>().screen, Screen::Playing);
    assert!(!visible(&t, "main_menu"));
    assert!(!t.capture().keyboard);
    assert!(
        t.text("hud/stats").starts_with("Score 0   Round 1   Coins left 5   Lives 3"),
        "{}",
        t.text("hud/stats")
    );

    key(&mut t, KeyCode::D, true);
    t.step(20);
    assert!(player_pos(&t.engine).x > 15.0);
    assert_ne!(enemy_positions(&t.engine), enemies0);
    key(&mut t, KeyCode::D, false);

    // Esc pauses and freezes mods.
    tap(&mut t, KeyCode::Escape);
    assert_eq!(t.resource::<Menu>().screen, Screen::Paused);
    assert!(visible(&t, "pause"));
    let frozen_pos = player_pos(&t.engine);
    let frozen_enemies = enemy_positions(&t.engine);
    key(&mut t, KeyCode::D, true);
    t.step(20);
    assert_eq!(player_pos(&t.engine), frozen_pos);
    assert_eq!(enemy_positions(&t.engine), frozen_enemies);
    key(&mut t, KeyCode::D, false);

    // Settings slider & checkbox, Esc back to pause.
    t.click("pause/settings");
    t.step(2);
    assert!(visible(&t, "settings"));
    t.set_value("settings/volume", 0.4);
    t.set_checked("settings/fullscreen", true);
    t.step(2);
    assert_eq!(t.text("settings/volume_text"), "Volume: 40%");
    tap(&mut t, KeyCode::Escape);
    assert!(visible(&t, "pause") && !visible(&t, "settings"));

    // Quit shuts engine down.
    t.click("pause/quit");
    t.step(2);
    assert!(!t.engine.is_running());
}

#[test]
fn enemy_hits_cost_lives_trigger_game_over_and_restart_resets() {
    let Some(dir) = built_wasm_dir() else { return };
    let mut t = ui_test(dir, true);
    t.step(5);

    // Pick up one coin so score = 1.
    let coin_pos = {
        let w = t.engine.world();
        let mut q = w.query::<(&Coin, &Transform)>();
        q.iter().next().unwrap().1.position
    };
    let player = t.engine.world().query::<(Entity, &Player)>().iter().next().unwrap().0;
    t.engine.world_mut().get::<&mut Transform>(player).unwrap().position = coin_pos;
    t.step(3);
    assert_eq!(t.resource::<Game>().score(), 1);
    assert_eq!(t.resource::<Game>().lives(), 3);

    // Three enemy hits: 3 -> 2 -> 1 -> 0 (GameOver).
    for expected_lives in [2, 1, 0] {
        // Wait out 1.0s invulnerability.
        t.step(65);
        let epos = enemy_positions(&t.engine)[0];
        t.engine.world_mut().get::<&mut Transform>(player).unwrap().position = epos;
        t.step(3);
        assert_eq!(t.resource::<Game>().score(), 0);
        assert_eq!(t.resource::<Game>().lives(), expected_lives);
    }

    assert!(t.resource::<Game>().is_game_over());
    t.step(2);
    assert_eq!(t.resource::<Menu>().screen, Screen::GameOver);
    assert!(visible(&t, "game_over"));

    // Click Play again -> resets back to Playing with 3 lives, round 1, 5 coins.
    t.click("game_over/restart");
    t.step(5);
    assert_eq!(t.resource::<Menu>().screen, Screen::Playing);
    assert!(!visible(&t, "game_over"));
    assert_eq!(t.resource::<Game>().lives(), 3);
    assert_eq!(t.resource::<Game>().score(), 0);
    assert_eq!(t.resource::<Game>().round(), 1);
    assert_eq!(coins_count(&t.engine), 5);
}
