//! coin_dash_scripts without a window: the scripts compile through `Engine::load` (and the
//! script cache), entities move, the autoplay bot scores, and an edit of a `.efa` file is
//! hot-reloaded. Run on its own: `cargo test -p coin_dash_scripts`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use coin_dash_scripts::{Coin, Game, Patrol, Player, load_embedded, load_scripts, setup, spawn_actors};
use efeengine_core::prelude::*;
use efeengine_core::cache::{CacheConfig, CacheDir};
use efeengine_core::scripting::{LogLevel, ModuleId, Script};

const DT: f64 = 1.0 / 60.0;

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts")
}

fn engine() -> (Engine, Game) {
    // never the platform cache directory
    let cfg = EngineConfig {
        fixed_dt: DT,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    let game = setup(&mut e);
    game.set_autoplay(false); // whatever COIN_DASH_AUTOPLAY says
    (e, game)
}

/// An engine with the scripts of `dir` loaded and the actors spawned.
fn running(dir: &Path) -> (Engine, Game, ModuleId) {
    let (mut e, game) = engine();
    let id = load_scripts(&mut e, dir).expect("the scripts compile");
    spawn_actors(&mut e, &game, Some(id));
    (e, game, id)
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn coins(e: &Engine) -> usize {
    e.world().query::<&Coin>().iter().count()
}

fn enemies(e: &Engine) -> usize {
    e.world().query::<&Patrol>().iter().count()
}

fn player_pos(e: &Engine) -> Vec2 {
    let (_, t) = e.world().query::<(&Player, &Transform)>().iter().next().map(|(p, t)| (*p, *t)).expect("player");
    t.position
}

fn enemy_positions(e: &Engine) -> Vec<Vec2> {
    let mut v: Vec<Vec2> = e.world().query::<(&Patrol, &Transform)>().iter().map(|(_, t)| t.position).collect();
    v.sort_by(|a, b| a.x.total_cmp(&b.x));
    v
}

fn temp_copy_of_scripts() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("coin_dash_scripts_{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for f in std::fs::read_dir(scripts_dir()).unwrap() {
        let p = f.unwrap().path();
        std::fs::copy(&p, d.join(p.file_name().unwrap())).unwrap();
    }
    d
}

#[test]
fn the_scripts_compile_and_spawn_the_first_round_through_the_cache() {
    let (mut e, _game, id) = running(&scripts_dir());
    assert_eq!(e.scripting().module_id("coin_dash_scripts"), Some(id));
    // `init` ran at load: 5 coins are queued and alive after the load's commands
    assert_eq!(coins(&e), 5);
    // the script cache compiled the root once (memory only here: the disk cache is off)
    assert_eq!(e.cache().stats().compiles, 1, "{:?}", e.cache().stats());
    e.run_frames(120, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    // 1200 ground tiles + 5 coins + 3 enemies + player + shadow
    assert_eq!(e.render_frame().sprites.len(), 1200 + 5 + 3 + 2);
    // coins spawned by the script have the 16 x 16 size it wrote at the field offset
    assert!(e.world().query::<(&Coin, &Sprite)>().iter().all(|(_, s)| s.size == Vec2::splat(16.0)));
    // the coins got their Script from `engine.script.attach` inside the script
    let scripted_coins = e.world().query::<(&Coin, &Script)>().iter().count();
    assert_eq!(scripted_coins, 5);
    // the status line is written by the script once a second
    e.run_frames(60, DT);
    let lines: Vec<String> = e.scripting().log_lines().map(|l| l.text.clone()).collect();
    assert!(lines.iter().any(|l| l.contains("round 1")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("t=1s pos=(0, 0) score=0 coins left=5 round=1")), "{lines:?}");
}

#[test]
fn enemies_patrol_and_coins_spin_and_bob_from_script() {
    let (mut e, _game, _id) = running(&scripts_dir());
    e.run_frames(2, DT);
    let before = enemy_positions(&e);
    let coin_frames_before: Vec<Vec2> = e.world().query::<(&Coin, &Sprite)>().iter().map(|(_, s)| s.uv_min).collect();
    let coin_y_before: Vec<f32> = e.world().query::<(&Coin, &Transform)>().iter().map(|(_, t)| t.position.y).collect();
    // 85 frames: not a whole number of spin cycles (8 frames per second, 4 frames per cycle)
    e.run_frames(85, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    let after = enemy_positions(&e);
    assert_eq!(before.len(), 3);
    for (b, a) in before.iter().zip(&after) {
        assert!(b.distance(*a) > 10.0, "an enemy did not move: {b} -> {a}");
    }
    // spin: the UV frame (x of uv_min, a multiple of 0.25) changed for some coin
    let coin_frames_after: Vec<Vec2> = e.world().query::<(&Coin, &Sprite)>().iter().map(|(_, s)| s.uv_min).collect();
    assert!(coin_frames_before.iter().zip(&coin_frames_after).any(|(a, b)| a.x != b.x), "{coin_frames_before:?} -> {coin_frames_after:?}");
    assert!(coin_frames_after.iter().all(|u| (u.x * 4.0).fract() == 0.0 && u.x < 1.0));
    // bob: y stays within 2 units of base_y and differs from before
    let mut moved = 0;
    for ((c, t), before) in e.world().query::<(&Coin, &Transform)>().iter().zip(&coin_y_before) {
        assert!((t.position.y - c.base_y).abs() <= 2.0 + 1e-3);
        moved += ((before - t.position.y).abs() > 1e-4) as usize;
    }
    assert!(moved >= 3, "only {moved} coins bobbed");
    // the player stands still without input
    assert_eq!(player_pos(&e), Vec2::ZERO);
}

#[test]
fn the_autoplay_bot_collects_coins_and_the_score_changes() {
    let (mut e, game, _id) = running(&scripts_dir());
    game.set_autoplay(true);
    let mut best = 0;
    let mut moved = false;
    for _ in 0..30 * 60 {
        e.run_frames(1, DT);
        best = best.max(game.score());
        moved |= player_pos(&e).length() > 20.0;
    }
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert!(moved, "the player never left the start");
    assert!(best >= 3, "best score {best}");
    // at least one round was completed: more than the first 5 coins were needed
    assert!(coins(&e) > 0);
}

#[test]
fn an_enemy_hit_resets_the_score_and_the_player() {
    let (mut e, game, _id) = running(&scripts_dir());
    e.run_frames(5, DT);
    // the player cheats a coin: score 1
    let coin = e.world().query::<(Entity, &Coin)>().iter().next().map(|(e, _)| e).unwrap();
    let coin_pos = e.world().get::<&Transform>(coin).unwrap().position;
    let player = e.world().query::<(Entity, &Player)>().iter().next().map(|(e, _)| e).unwrap();
    e.world_mut().get::<&mut Transform>(player).unwrap().position = coin_pos;
    e.run_frames(3, DT);
    assert_eq!(game.score(), 1);
    // the player is put on an enemy; the next frames reset score and position
    let enemy = enemy_positions(&e)[0];
    e.world_mut().get::<&mut Transform>(player).unwrap().position = enemy;
    // the 1 second of immunity at the start is over after 60 frames
    e.run_frames(70, DT);
    e.world_mut().get::<&mut Transform>(player).unwrap().position = enemy_positions(&e)[0];
    e.run_frames(2, DT);
    assert_eq!(game.score(), 0, "hit resets the score");
    assert!(player_pos(&e).length() < 5.0, "back at the start: {}", player_pos(&e));
    assert!(e.world().get::<&Player>(player).unwrap().invulnerable > 0.5);
}

#[test]
fn editing_a_script_file_hot_reloads_it_in_place() {
    let dir = temp_copy_of_scripts();
    let (mut e, game, id) = running(&dir);
    game.set_autoplay(true);
    e.run_frames(20, DT);
    let speed = |e: &Engine| e.world().query::<(&Player, &Velocity)>().iter().next().map(|(_, v)| v.linear.length()).unwrap();
    assert!((speed(&e) - 110.0).abs() < 0.5, "{}", speed(&e));
    assert_eq!(e.cache().stats().compiles, 1);

    let coins_before = coins(&e);
    // double the player's speed in common.efa (a file the root only `use`s)
    let common = dir.join("common.efa");
    let text = std::fs::read_to_string(&common).unwrap().replace("PLAYER_SPEED = 110.0", "PLAYER_SPEED = 220.0");
    std::fs::write(&common, text).unwrap();
    let f = std::fs::File::options().write(true).open(&common).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
    e.run_frames(30, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert!((speed(&e) - 220.0).abs() < 0.5, "{}", speed(&e));
    assert_eq!(e.cache().stats().compiles, 2);
    assert_eq!(e.scripting().module_id("coin_dash_scripts"), Some(id), "same module id");
    // the world was kept: the 5 coins of round 1 (minus pickups) and the same entities
    assert_eq!(enemies(&e), 3);
    // `init` ran again but adopted the coins on the field instead of spawning a second round
    assert!(coins(&e) <= coins_before, "{} coins after, {coins_before} before", coins(&e));
    assert!(e.scripting().log_lines().any(|l| l.text.contains("keeping the coins on the field")));

    // a syntax error keeps the old version running and is logged
    std::fs::write(dir.join("player.efa"), "nonsense\n").unwrap();
    let f = std::fs::File::options().write(true).open(dir.join("player.efa")).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10)).unwrap();
    e.run_frames(30, DT);
    assert!((speed(&e) - 220.0).abs() < 0.5);
    assert!(e.scripting().log_lines().any(|l| l.level == LogLevel::Error && l.text.contains("keeping the old version")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_embedded_sources_compile_too() {
    let (mut e, game) = engine();
    let id = load_embedded(&mut e).expect("embedded scripts compile");
    spawn_actors(&mut e, &game, Some(id));
    game.set_autoplay(true);
    e.run_frames(300, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert!(player_pos(&e).length() > 5.0);
}
