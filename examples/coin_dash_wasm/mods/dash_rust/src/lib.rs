//! dash_rust: Rust wasm mod for coin_dash_wasm (`efe-guest`, `#![no_std]`).
//!
//! Owns player movement (WASD / arrows or autoplay bot), spawning ground tiles, player,
//! shadow, enemies (choosing lanes via `dash_c::next_lane`), rounds, coins (validating
//! spots via `dash_c::check_coin_spot` and phases via `dash_c::coin_phase`), pickups, score,
//! enemy hits, lives/game-over, restart handling, and status logging.

#![no_std]

extern crate alloc;

mod api;

use alloc::format;
use api::{Coin, CoinSpawned, Entity, Parent, Patrol, Player, Sprite, Transform, Velocity, dash_c, engine, game};
use core::cell::UnsafeCell;
use efe_guest::{provide, scratch};

scratch!(256);

const TILE: f32 = 16.0;
const MAP_W: i32 = 40;
const MAP_H: i32 = 30;
const HALF_X: f32 = MAP_W as f32 * TILE / 2.0;
const HALF_Y: f32 = MAP_H as f32 * TILE / 2.0;
const PLAYER_SPEED: f32 = 110.0;
const PLAYER_R: f32 = 5.0;
const COIN_R: f32 = 6.0;
const ENEMY_R: f32 = 6.0;
const COIN_PICK2: f32 = (PLAYER_R + COIN_R) * (PLAYER_R + COIN_R);
const ENEMY_HIT2: f32 = (PLAYER_R + ENEMY_R) * (PLAYER_R + ENEMY_R);
const INVULN: f32 = 1.0;
const START_LIVES: i32 = 3;

const LAYER_GROUND: i32 = -10;
const LAYER_COIN: i32 = 1;
const LAYER_ENEMY: i32 = 2;
const LAYER_SHADOW: i32 = 3;
const LAYER_PLAYER: i32 = 4;

struct State {
    round: i32,
    coins_left: i32,
    lives: i32,
    ground_row: i32,
    next_status: f64,
    last_restart_gen: i32,
    patrol_turns: i32,
}

struct Global(UnsafeCell<State>);
unsafe impl Sync for Global {}

static STATE: Global = Global(UnsafeCell::new(State {
    round: 1,
    coins_left: 0,
    lives: START_LIVES,
    ground_row: 0,
    next_status: 0.0,
    last_restart_gen: 0,
    patrol_turns: 0,
}));

fn state() -> &'static mut State {
    unsafe { &mut *STATE.0.get() }
}

fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = f32::from_bits((x.to_bits() >> 1) + 0x1fc0_0000);
    let mut i = 0;
    while i < 4 {
        g = 0.5 * (g + x / g);
        i += 1;
    }
    g
}

fn default_transform(x: f32, y: f32) -> Transform {
    Transform {
        position: [x, y],
        rotation: 0.0,
        scale: [1.0, 1.0],
    }
}

fn frame_uv(frame: i32, frames: i32) -> ([f32; 2], [f32; 2]) {
    let w = 1.0 / frames as f32;
    ([frame as f32 * w, 0.0], [(frame + 1) as f32 * w, 1.0])
}

fn make_sprite(texture: u32, size: f32, layer: i32, frame: i32, frames: i32) -> Sprite {
    let (uv_min, uv_max) = frame_uv(frame, frames);
    Sprite {
        texture,
        color: [1.0, 1.0, 1.0, 1.0],
        size: [size, size],
        uv_min,
        uv_max,
        layer,
        flip: 0,
        auto_size: 0,
    }
}

fn spawn_ground_row(y: i32, tiles_tex: u32) {
    let mut x = 0;
    while x < MAP_W {
        let hash = x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663);
        let frame = if x == MAP_W / 2 || y == MAP_H / 2 {
            3
        } else if hash.rem_euclid(11) == 0 {
            2
        } else {
            hash.rem_euclid(2)
        };
        let px = (x as f32 + 0.5) * TILE - HALF_X;
        let py = (y as f32 + 0.5) * TILE - HALF_Y;
        let ent = engine::world::spawn();
        if ent.0 != 0 {
            let tr = default_transform(px, py);
            let sp = make_sprite(tiles_tex, TILE, LAYER_GROUND, frame, 5);
            unsafe {
                engine::world::insert(ent, Transform::COMPONENT as i32, (&raw const tr).cast::<u8>().cast_mut());
                engine::world::insert(ent, Sprite::COMPONENT as i32, (&raw const sp).cast::<u8>().cast_mut());
            }
        }
        x += 1;
    }
}

fn lane_spec(lane: i32) -> ([f32; 2], [f32; 2], f32, f32) {
    match lane {
        0 => ([0.0, 70.0], [1.0, 0.0], 140.0, 55.0),
        1 => ([-60.0, -80.0], [1.0, 0.0], 100.0, 75.0),
        _ => ([110.0, 0.0], [0.0, 1.0], 100.0, 45.0),
    }
}

fn spawn_player_and_shadow() {
    let player_tex = game::texture("player") as u32;
    let tiles_tex = game::texture("tiles") as u32;
    let player = engine::world::spawn();
    if player.0 != 0 {
        let p = Player { invulnerable: INVULN };
        let tr = default_transform(0.0, 0.0);
        let vel = Velocity::default();
        let sp = make_sprite(player_tex, TILE, LAYER_PLAYER, 0, 1);
        unsafe {
            engine::world::insert(player, Player::COMPONENT as i32, (&raw const p).cast::<u8>().cast_mut());
            engine::world::insert(player, Transform::COMPONENT as i32, (&raw const tr).cast::<u8>().cast_mut());
            engine::world::insert(player, Velocity::COMPONENT as i32, (&raw const vel).cast::<u8>().cast_mut());
            engine::world::insert(player, Sprite::COMPONENT as i32, (&raw const sp).cast::<u8>().cast_mut());
        }
        let shadow = engine::world::spawn();
        if shadow.0 != 0 {
            let strans = default_transform(0.0, -6.0);
            let parent = Parent { entity: player };
            let ssp = make_sprite(tiles_tex, TILE, LAYER_SHADOW, 4, 5);
            unsafe {
                engine::world::insert(shadow, Transform::COMPONENT as i32, (&raw const strans).cast::<u8>().cast_mut());
                engine::world::insert(shadow, Parent::COMPONENT as i32, (&raw const parent).cast::<u8>().cast_mut());
                engine::world::insert(shadow, Sprite::COMPONENT as i32, (&raw const ssp).cast::<u8>().cast_mut());
            }
        }
    }
}

fn spawn_enemies() {
    let enemy_tex = game::texture("enemy") as u32;
    let mut lane = -1;
    let mut i = 0;
    while i < 3 {
        lane = dash_c::next_lane(lane);
        let (origin, axis, range, speed) = lane_spec(lane);
        let e = engine::world::spawn();
        if e.0 != 0 {
            let tr = default_transform(origin[0], origin[1]);
            let vel = Velocity::default();
            let patrol = Patrol {
                origin,
                axis,
                range,
                speed,
                dir: 1.0,
            };
            let sp = make_sprite(enemy_tex, TILE, LAYER_ENEMY, 0, 1);
            unsafe {
                engine::world::insert(e, Transform::COMPONENT as i32, (&raw const tr).cast::<u8>().cast_mut());
                engine::world::insert(e, Velocity::COMPONENT as i32, (&raw const vel).cast::<u8>().cast_mut());
                engine::world::insert(e, Patrol::COMPONENT as i32, (&raw const patrol).cast::<u8>().cast_mut());
                engine::world::insert(e, Sprite::COMPONENT as i32, (&raw const sp).cast::<u8>().cast_mut());
            }
            game::label_enemy(e);
        }
        i += 1;
    }
}

fn coins_in_round(round: i32) -> i32 {
    (5 + 3 * (round - 1)).min(30)
}

fn spawn_round(round: i32) {
    let coin_tex = game::texture("coin_sheet") as u32;
    let count = coins_in_round(round);
    state().coins_left = count;
    game::set_coins_left(count);
    game::set_round(round);

    let mut i = 0;
    while i < count {
        let mut tr = default_transform(0.0, 0.0);
        loop {
            let x = (engine::random::r#f32() - 0.5) * 600.0;
            let y = (engine::random::r#f32() - 0.5) * 440.0;
            tr.position = [x, y];
            if dash_c::check_coin_spot(&tr) {
                break;
            }
        }
        let phase = dash_c::coin_phase(i, count);
        let e = engine::world::spawn();
        if e.0 != 0 {
            let sp = make_sprite(coin_tex, TILE, LAYER_COIN, 0, 4);
            let coin = Coin {
                base_y: tr.position[1],
                phase,
            };
            unsafe {
                engine::world::insert(e, Transform::COMPONENT as i32, (&raw const tr).cast::<u8>().cast_mut());
                engine::world::insert(e, Sprite::COMPONENT as i32, (&raw const sp).cast::<u8>().cast_mut());
                engine::world::insert(e, Coin::COMPONENT as i32, (&raw const coin).cast::<u8>().cast_mut());
            }
            game::coin_spawned::send(&CoinSpawned {
                x: tr.position[0],
                y: tr.position[1],
                round,
            });
        }
        i += 1;
    }
}

fn reset_world_for_restart() {
    let mut ids = [Entity(0); 32];
    let n = unsafe { engine::world::query(Coin::COMPONENT as i32, ids.as_mut_ptr().cast::<u8>(), 32) };
    let mut i = 0;
    while i < n as usize {
        engine::world::despawn(ids[i]);
        i += 1;
    }

    let mut p_ids = [Entity(0); 1];
    if unsafe { engine::world::query(Player::COMPONENT as i32, p_ids.as_mut_ptr().cast::<u8>(), 1) } > 0 {
        let player = p_ids[0];
        let tr = default_transform(0.0, 0.0);
        let vel = Velocity::default();
        let pl = Player { invulnerable: INVULN };
        unsafe {
            engine::world::set(player, Transform::COMPONENT as i32, (&raw const tr).cast::<u8>().cast_mut());
            engine::world::set(player, Velocity::COMPONENT as i32, (&raw const vel).cast::<u8>().cast_mut());
            engine::world::set(player, Player::COMPONENT as i32, (&raw const pl).cast::<u8>().cast_mut());
        }
    }

    let mut e_ids = [Entity(0); 8];
    let en = unsafe { engine::world::query(Patrol::COMPONENT as i32, e_ids.as_mut_ptr().cast::<u8>(), 8) };
    let mut k = 0;
    while k < en as usize {
        let mut p = Patrol::default();
        if unsafe { engine::world::get(e_ids[k], Patrol::COMPONENT as i32, (&raw mut p).cast::<u8>()) } == 0 {
            p.dir = 1.0;
            let tr = default_transform(p.origin[0], p.origin[1]);
            let vel = Velocity::default();
            unsafe {
                engine::world::set(e_ids[k], Patrol::COMPONENT as i32, (&raw const p).cast::<u8>().cast_mut());
                engine::world::set(e_ids[k], Transform::COMPONENT as i32, (&raw const tr).cast::<u8>().cast_mut());
                engine::world::set(e_ids[k], Velocity::COMPONENT as i32, (&raw const vel).cast::<u8>().cast_mut());
            }
        }
        k += 1;
    }

    let st = state();
    st.round = 1;
    st.lives = START_LIVES;
    game::set_lives(START_LIVES);
    game::score_reset();
    game::set_game_over(false);
    spawn_round(1);
    engine::log::info("round 1 (restarted)");
}

fn nearest_coin(px: f32, py: f32) -> (f32, f32, bool) {
    let mut ids = [Entity(0); 32];
    let n = unsafe { engine::world::query(Coin::COMPONENT as i32, ids.as_mut_ptr().cast::<u8>(), 32) };
    let mut best = 1e30_f32;
    let mut bx = 0.0_f32;
    let mut by = 0.0_f32;
    let mut found = false;
    let mut i = 0;
    while i < n as usize {
        let mut tr = Transform::default();
        if unsafe { engine::world::get(ids[i], Transform::COMPONENT as i32, (&raw mut tr).cast::<u8>()) } == 0 {
            let dx = tr.position[0] - px;
            let dy = tr.position[1] - py;
            let d2 = dx * dx + dy * dy;
            if d2 < best {
                best = d2;
                bx = dx;
                by = dy;
                found = true;
            }
        }
        i += 1;
    }
    (bx, by, found)
}

fn held(a: &str, b: &str) -> bool {
    engine::input::key_down(a) || engine::input::key_down(b)
}

provide! {
    /// Current round number in `dash_rust`.
    fn current_round() -> i32 {
        state().round
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    engine::random::seed(2545);
    let st = state();
    st.round = 1;
    st.lives = START_LIVES;
    st.ground_row = 1;
    game::set_round(1);
    game::set_lives(START_LIVES);
    game::score_reset();
    game::set_game_over(false);

    let tiles_tex = game::texture("tiles") as u32;
    spawn_ground_row(0, tiles_tex);
    spawn_player_and_shadow();
    spawn_enemies();
    spawn_round(1);
    engine::log::info(&format!("round 1 (dash_c provide calls={})", dash_c::provide_calls()));
}

#[unsafe(no_mangle)]
pub extern "C" fn frame(dt: f32) {
    let st = state();

    let rg = game::restart_gen();
    if rg != st.last_restart_gen {
        st.last_restart_gen = rg;
        reset_world_for_restart();
        return;
    }
    if game::is_game_over() {
        return;
    }

    if st.ground_row < MAP_H {
        let tiles_tex = game::texture("tiles") as u32;
        spawn_ground_row(st.ground_row, tiles_tex);
        st.ground_row += 1;
    }

    let turns = game::patrol_turn::count();
    if turns > 0 {
        st.patrol_turns += turns as i32;
    }

    let mut p_ids = [Entity(0); 1];
    if unsafe { engine::world::query(Player::COMPONENT as i32, p_ids.as_mut_ptr().cast::<u8>(), 1) } <= 0 {
        return;
    }
    let player = p_ids[0];
    let mut ptr = Transform::default();
    let mut pl = Player::default();
    if unsafe { engine::world::get(player, Transform::COMPONENT as i32, (&raw mut ptr).cast::<u8>()) } != 0 {
        return;
    }
    if unsafe { engine::world::get(player, Player::COMPONENT as i32, (&raw mut pl).cast::<u8>()) } != 0 {
        return;
    }

    // Player movement (WASD / arrows, or autoplay bot).
    let mut dx = held("D", "Right") as i32 as f32 - held("A", "Left") as i32 as f32;
    let mut dy = held("W", "Up") as i32 as f32 - held("S", "Down") as i32 as f32;
    if game::autoplay() {
        let (tx, ty, found) = nearest_coin(ptr.position[0], ptr.position[1]);
        if found {
            dx = tx;
            dy = ty;
        } else {
            dx = 0.0;
            dy = 0.0;
        }
    }
    let len = sqrt_f32(dx * dx + dy * dy);
    if len > 0.0 {
        dx /= len;
        dy /= len;
    }
    let vel = Velocity {
        linear: [dx * PLAYER_SPEED, dy * PLAYER_SPEED],
        angular: 0.0,
    };
    ptr.position[0] = ptr.position[0].clamp(-HALF_X + 8.0, HALF_X - 8.0);
    ptr.position[1] = ptr.position[1].clamp(-HALF_Y + 8.0, HALF_Y - 8.0);
    unsafe {
        engine::world::set(player, Velocity::COMPONENT as i32, (&raw const vel).cast::<u8>().cast_mut());
        engine::world::set(player, Transform::COMPONENT as i32, (&raw const ptr).cast::<u8>().cast_mut());
    }
    if dx != 0.0 {
        let mut sp = Sprite::default();
        if unsafe { engine::world::get(player, Sprite::COMPONENT as i32, (&raw mut sp).cast::<u8>()) } == 0 {
            sp.flip = if dx < 0.0 { 1 } else { 0 };
            unsafe {
                engine::world::set(player, Sprite::COMPONENT as i32, (&raw const sp).cast::<u8>().cast_mut());
            }
        }
    }

    // Coins pickup.
    let px = ptr.position[0];
    let py = ptr.position[1];
    let mut coin_ids = [Entity(0); 32];
    let cn = unsafe { engine::world::query(Coin::COMPONENT as i32, coin_ids.as_mut_ptr().cast::<u8>(), 32) };
    let mut ci = 0;
    while ci < cn as usize {
        let ce = coin_ids[ci];
        let mut ctr = Transform::default();
        if unsafe { engine::world::get(ce, Transform::COMPONENT as i32, (&raw mut ctr).cast::<u8>()) } == 0 {
            let cx = ctr.position[0] - px;
            let cy = ctr.position[1] - py;
            if cx * cx + cy * cy < COIN_PICK2 {
                engine::world::despawn(ce);
                game::popup_scored(ctr.position[0], ctr.position[1]);
                let total = game::score_add(1);
                st.coins_left -= 1;
                game::set_coins_left(st.coins_left);
                engine::log::info(&format!("score {total}"));
            }
        }
        ci += 1;
    }
    if st.coins_left <= 0 {
        st.round += 1;
        engine::log::info(&format!("round {}", st.round));
        spawn_round(st.round);
    }

    // Enemy hit check.
    pl.invulnerable = (pl.invulnerable - dt).max(0.0);
    if pl.invulnerable <= 0.0 {
        let mut e_ids = [Entity(0); 16];
        let en = unsafe { engine::world::query(Patrol::COMPONENT as i32, e_ids.as_mut_ptr().cast::<u8>(), 16) };
        let mut ei = 0;
        while ei < en as usize {
            let mut etr = Transform::default();
            if unsafe { engine::world::get(e_ids[ei], Transform::COMPONENT as i32, (&raw mut etr).cast::<u8>()) } == 0 {
                let ex = etr.position[0] - px;
                let ey = etr.position[1] - py;
                if ex * ex + ey * ey < ENEMY_HIT2 {
                    game::score_reset();
                    pl.invulnerable = INVULN;
                    ptr.position = [0.0, 0.0];
                    let zero_vel = Velocity::default();
                    unsafe {
                        engine::world::set(player, Transform::COMPONENT as i32, (&raw const ptr).cast::<u8>().cast_mut());
                        engine::world::set(player, Velocity::COMPONENT as i32, (&raw const zero_vel).cast::<u8>().cast_mut());
                    }
                    st.lives -= 1;
                    game::set_lives(st.lives);
                    if st.lives <= 0 {
                        game::set_game_over(true);
                        engine::log::info("hit by enemy, game over");
                    } else {
                        engine::log::info("hit by enemy, score reset");
                    }
                    break;
                }
            }
            ei += 1;
        }
    }
    unsafe {
        engine::world::set(player, Player::COMPONENT as i32, (&raw const pl).cast::<u8>().cast_mut());
    }

    // Status line once a second.
    let now = engine::time::elapsed();
    if now >= st.next_status {
        st.next_status = (now as i64 + 1) as f64;
        engine::log::info(&format!(
            "t={}s pos=({}, {}) score={} coins left={} round={} lives={} turns={}",
            now as i64,
            px as i32,
            py as i32,
            game::score(),
            st.coins_left,
            st.round,
            st.lives,
            st.patrol_turns,
        ));
    }
}
