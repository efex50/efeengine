//! coin_dash without a window: the world builds, coins are collected, a new round spawns.

use coin_dash::setup;
use efeengine_core::prelude::*;
use efeengine_core::render::{SpriteInstance};

const DT: f64 = 1.0 / 60.0;
// load order in `setup`: player, enemy, coin sheet, tiles
const PLAYER_TEX: u32 = 1;
const COIN_TEX: u32 = 3;

fn engine() -> Engine {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: DT, ..EngineConfig::headless() }).expect("headless engine");
    setup(&mut e);
    e
}

fn sprites_with(e: &Engine, tex: u32) -> Vec<SpriteInstance> {
    e.render_frame().d2.sprites.iter().filter(|s| s.texture.id == tex).cloned().collect()
}

#[test]
fn world_builds_and_runs() {
    let mut e = engine();
    e.run_frames(120, DT);
    let f = e.render_frame();
    assert!(f.d2.camera.is_some());
    // 1200 ground tiles + 5 coins + 3 enemies + player + shadow
    assert_eq!(f.d2.sprites.len(), 1200 + 5 + 3 + 2);
    assert_eq!(sprites_with(&e, COIN_TEX).len(), 5);
    // the ground is drawn first
    assert!(f.d2.sprites[0].layer < f.d2.sprites.last().unwrap().layer);
}

#[test]
fn collecting_every_coin_starts_a_bigger_round() {
    let mut e = engine();
    e.run_frames(2, DT);
    let player = sprites_with(&e, PLAYER_TEX)[0].entity;
    for round_size in [5usize, 8] {
        let coins = sprites_with(&e, COIN_TEX);
        assert_eq!(coins.len(), round_size);
        for c in &coins {
            let pos = e.world().get::<&Transform>(c.entity).unwrap().position;
            e.world_mut().get::<&mut Transform>(player).unwrap().position = pos;
            e.run_frames(2, DT);
        }
        e.run_frames(2, DT);
    }
    assert_eq!(sprites_with(&e, COIN_TEX).len(), 11);
}
