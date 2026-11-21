//! coin_dash: a small 2D game on the built-in `SpriteRenderer`.
//!
//! Collect the coins, dodge the patrolling enemies. Move with WASD or the arrow keys.
//! Touching an enemy resets the score and puts you back at the start; collecting every coin
//! starts a new round with more of them.
//!
//! How it uses the engine:
//! * Textures: PNG files embedded with `include_bytes!` and decoded by `Textures::add_png`
//!   (so it works on the web as well). `coin_sheet.png` and `tiles.png` are atlases: a sprite
//!   shows one 16x16 frame through `Sprite::uv_min` / `uv_max`.
//! * The ground is a grid of tile sprites on layer -10, the coins spin by switching UV frames
//!   over time and bob, the enemies patrol with `Velocity`, the player is moved by `Velocity` too.
//! * The player's shadow is a child sprite (`Parent`) that follows without any code.
//! * Collisions are circle checks in a plain Rust system. Game state is the `Game` resource.
//! * Enemies get a world-space "Enemy" label; collecting a coin shows a rising "Scored!" popup.
//! * The camera follows the player smoothly. There is no text rendering yet, so events go
//!   to the log (`[coin_dash] score 3`), plus a status line once a second.
//!
//! Run: `cargo xtask run coin_dash native`. Set `COIN_DASH_AUTOPLAY=1` to let a tiny bot steer
//! toward the nearest coin (handy for a demo run without a keyboard).

// `#[main]` leaves `main` out of the native lib target, so its callees look unused there.
#![allow(dead_code)]

use efeengine_core::prelude::*;
use efeengine_world_ui::{Anchor2D, Floating2D, Text2D};

pub mod menu;

const TILE: f32 = 16.0;
const MAP_W: i32 = 40;
const MAP_H: i32 = 30;
/// Half the size of the playfield in world units.
const HALF: Vec2 = Vec2::new(MAP_W as f32 * TILE / 2.0, MAP_H as f32 * TILE / 2.0);
const ZOOM: f32 = 2.0;
const PLAYER_SPEED: f32 = 110.0;
const START: Vec2 = Vec2::ZERO;
/// Circle radii for the collision checks.
const PLAYER_R: f32 = 5.0;
const COIN_R: f32 = 6.0;
const ENEMY_R: f32 = 6.0;

const LAYER_GROUND: i32 = -10;
const LAYER_COIN: i32 = 1;
const LAYER_ENEMY: i32 = 2;
const LAYER_SHADOW: i32 = 3;
const LAYER_PLAYER: i32 = 4;

/// The four texture handles.
#[derive(Clone, Copy)]
struct Assets {
    player: TextureHandle,
    enemy: TextureHandle,
    coins: TextureHandle,
    tiles: TextureHandle,
}

/// Game state (a resource).
struct Game {
    assets: Assets,
    player: Entity,
    camera: Entity,
    /// The smoothed camera position (the camera `Transform` gets it rounded to whole pixels).
    camera_pos: Vec2,
    score: u32,
    round: u32,
    coins_left: u32,
    /// Seconds of immunity after a hit, so a reset next to an enemy cannot chain hits.
    invulnerable: f32,
    next_status: f64,
    autoplay: bool,
    rng: u32,
}

impl Game {
    /// A pseudo-random number in `0.0..1.0` (xorshift).
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

// ---- components ----

struct Player;

/// A coin: bobs around `base_y` and spins from the sprite sheet; `phase` desynchronises them.
struct Coin {
    base_y: f32,
    phase: f32,
}

/// An enemy moving back and forth along `axis` within `range` of `origin`.
struct Patrol {
    origin: Vec2,
    axis: Vec2,
    range: f32,
    speed: f32,
    /// +1 or -1 along the axis.
    dir: f32,
}

// ---- spawning ----

/// One 16x16 frame of an atlas that is `frames` frames wide.
fn frame_uv(frame: usize, frames: usize) -> (Vec2, Vec2) {
    let w = 1.0 / frames as f32;
    (Vec2::new(frame as f32 * w, 0.0), Vec2::new((frame + 1) as f32 * w, 1.0))
}

fn tile_sprite(tiles: TextureHandle, frame: usize) -> Sprite {
    let (uv_min, uv_max) = frame_uv(frame, 5);
    Sprite { uv_min, uv_max, layer: LAYER_GROUND, ..Sprite::new(tiles) }.with_size(Vec2::splat(TILE))
}

/// The ground: a MAP_W x MAP_H grid of tile sprites (grass, a few flowers, a stone cross).
fn spawn_ground(world: &mut World, tiles: TextureHandle) {
    for y in 0..MAP_H {
        for x in 0..MAP_W {
            let hash = x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663);
            let frame = if x == MAP_W / 2 || y == MAP_H / 2 {
                3
            } else if hash.rem_euclid(11) == 0 {
                2
            } else {
                (hash.rem_euclid(2)) as usize
            };
            let pos = Vec2::new((x as f32 + 0.5) * TILE, (y as f32 + 0.5) * TILE) - HALF;
            world.spawn((Transform::from_position(pos), tile_sprite(tiles, frame)));
        }
    }
}

fn coin_bundle(coins: TextureHandle, pos: Vec2, phase: f32) -> (Transform, Sprite, Coin) {
    (
        Transform::from_position(pos),
        Sprite { layer: LAYER_COIN, ..Sprite::new(coins) }.with_size(Vec2::splat(TILE)),
        Coin { base_y: pos.y, phase },
    )
}

/// Coins in a round: 5, then 3 more each round, at most 30.
fn coins_in_round(round: u32) -> u32 {
    (5 + 3 * (round - 1)).min(30)
}

/// A random spot inside the playfield, away from the start.
fn random_spot(game: &mut Game) -> Vec2 {
    loop {
        let p = Vec2::new((game.random() - 0.5) * (HALF.x * 2.0 - 40.0), (game.random() - 0.5) * (HALF.y * 2.0 - 40.0));
        if p.distance(START) > 40.0 {
            return p;
        }
    }
}

fn spawn_player(world: &mut World, assets: Assets) -> Entity {
    let player = world.spawn((
        Player,
        Transform::from_position(START),
        Velocity::default(),
        Sprite { layer: LAYER_PLAYER, ..Sprite::new(assets.player) }.with_size(Vec2::splat(TILE)),
    ));
    // The shadow: a child sprite (tile 4 of the atlas), offset below the feet. It moves with
    // the player because its Transform is relative to the parent's.
    let (uv_min, uv_max) = frame_uv(4, 5);
    world.spawn((
        Transform::from_position(Vec2::new(0.0, -6.0)),
        Parent::new(player),
        Sprite { uv_min, uv_max, layer: LAYER_SHADOW, ..Sprite::new(assets.tiles) }.with_size(Vec2::splat(TILE)),
    ));
    player
}

fn spawn_enemies(world: &mut World, enemy: TextureHandle) {
    // (origin, axis, range, speed)
    let lanes = [
        (Vec2::new(0.0, 70.0), Vec2::X, 140.0, 55.0),
        (Vec2::new(-60.0, -80.0), Vec2::X, 100.0, 75.0),
        (Vec2::new(110.0, 0.0), Vec2::Y, 100.0, 45.0),
    ];
    for (origin, axis, range, speed) in lanes {
        let e = world.spawn((
            Transform::from_position(origin),
            Velocity::default(),
            Patrol { origin, axis, range, speed, dir: 1.0 },
            Sprite { layer: LAYER_ENEMY, ..Sprite::new(enemy) }.with_size(Vec2::splat(TILE)),
        ));
        // world-space label that follows the enemy
        world.spawn((
            Anchor2D::entity(e).with_offset(Vec2::new(0.0, -14.0)),
            Text2D { text: "Enemy".into(), size: 11.0, color: [1.0, 0.45, 0.45, 1.0], ..Default::default() },
        ));
    }
}

// ---- systems ----

/// WASD / arrows set the player's `Velocity` (the engine's velocity system moves it).
/// A par system: reads `Game`, writes the player's `Transform`, `Velocity` and `Sprite`.
fn player_input(ctx: &ParCtx<'_>) {
    let Some(game) = ctx.try_res::<Game>() else { return };
    let (player, autoplay) = (game.player, game.autoplay);
    drop(game);
    // the game's view of the input: empty while the UI captures the keyboard
    let input = ctx.game_input();
    let held = |a: KeyCode, b: KeyCode| input.pressed(a) || input.pressed(b);
    let mut dir = Vec2::new(
        held(KeyCode::D, KeyCode::Right) as i32 as f32 - held(KeyCode::A, KeyCode::Left) as i32 as f32,
        held(KeyCode::W, KeyCode::Up) as i32 as f32 - held(KeyCode::S, KeyCode::Down) as i32 as f32,
    );
    if autoplay {
        // a bot: walk to the nearest coin
        let from = ctx.get::<&Transform>(player).map(|t| t.position).unwrap_or(START);
        let nearest = ctx
            .query::<(&Transform, &Coin)>()
            .iter()
            .map(|(t, _)| t.position)
            .min_by(|a, b| a.distance_squared(from).total_cmp(&b.distance_squared(from)));
        dir = nearest.map_or(Vec2::ZERO, |c| c - from);
    }
    let mut q = ctx.query_one::<(&mut Transform, &mut Velocity, &mut Sprite)>(player);
    let Ok((t, v, s)) = q.get() else { return };
    v.linear = dir.normalize_or_zero() * PLAYER_SPEED;
    t.position = t.position.clamp(-HALF + Vec2::splat(8.0), HALF - Vec2::splat(8.0));
    if dir.x != 0.0 {
        s.flip = if dir.x < 0.0 { Sprite::FLIP_X } else { 0 };
    }
}

/// Enemies turn around at the ends of their lane. Reads `Transform`; writes `Velocity`,
/// `Patrol` and `Sprite`. Each enemy is independent, so it runs with `par_for_each`.
fn patrol(ctx: &ParCtx<'_>) {
    ctx.par_for_each::<(&Transform, &mut Velocity, &mut Patrol, &mut Sprite), _>(64, |(t, v, p, s)| {
        let along = (t.position - p.origin).dot(p.axis);
        if along > p.range {
            p.dir = -1.0;
        } else if along < -p.range {
            p.dir = 1.0;
        }
        v.linear = p.axis * p.dir * p.speed;
        if p.axis.x != 0.0 {
            s.flip = if p.dir < 0.0 { Sprite::FLIP_X } else { 0 };
        }
    });
}

/// Coins spin (UV frames of the sheet, 8 frames per second) and bob. Writes `Transform` and
/// `Sprite`, reads `Coin`.
fn animate_coins(ctx: &ParCtx<'_>) {
    let t = ctx.time.elapsed() as f32;
    ctx.par_for_each::<(&mut Transform, &mut Sprite, &Coin), _>(64, move |(tr, s, c)| {
        let frame = ((t * 8.0 + c.phase * 4.0) as usize) % 4;
        (s.uv_min, s.uv_max) = frame_uv(frame, 4);
        tr.position.y = c.base_y + (t * 3.0 + c.phase * 6.0).sin() * 2.0;
    });
}

/// Circle checks: player vs coins and vs enemies. Despawns and spawns go through
/// `ctx.commands()`; writes `Game`, the player's `Transform` and `Velocity`.
fn collisions(ctx: &ParCtx<'_>) {
    let Some(mut game) = ctx.try_res_mut::<Game>() else { return };
    game.invulnerable = (game.invulnerable - ctx.time.delta()).max(0.0);
    let player = game.player;
    let Ok(pos) = ctx.get::<&Transform>(player).map(|t| t.position) else { return };

    // coins
    let taken: Vec<(Entity, Vec2)> = ctx
        .query::<(Entity, &Transform, &Coin)>()
        .iter()
        .filter(|(_, t, _)| t.position.distance(pos) < PLAYER_R + COIN_R)
        .map(|(e, t, _)| (e, t.position))
        .collect();
    let mut new_coins = Vec::new();
    for (e, at) in taken {
        ctx.commands().despawn(e);
        ctx.commands().spawn((
            Anchor2D::point(at).with_offset(Vec2::new(0.0, -10.0)),
            Text2D { text: "Scored!".into(), size: 14.0, color: [1.0, 0.84, 0.2, 1.0], bold: true },
            Floating2D::new(40.0, 0.8),
        ));
        game.score += 1;
        game.coins_left -= 1;
        global_info(&format!("[coin_dash] score {}", game.score));
    }
    if game.coins_left == 0 {
        game.round += 1;
        global_info(&format!("[coin_dash] round {}", game.round));
        for _ in 0..coins_in_round(game.round) {
            let p = random_spot(&mut game);
            new_coins.push((p, game.random()));
        }
        game.coins_left = new_coins.len() as u32;
    }
    let coins_tex = game.assets.coins;
    for (p, phase) in new_coins {
        ctx.commands().spawn(coin_bundle(coins_tex, p, phase));
    }

    // enemies
    if game.invulnerable > 0.0 {
        return;
    }
    let hit = ctx
        .query::<(&Transform, &Patrol)>()
        .iter()
        .any(|(t, _)| t.position.distance(pos) < PLAYER_R + ENEMY_R);
    if hit {
        game.score = 0;
        game.invulnerable = 1.0;
        global_info("[coin_dash] hit by enemy, score reset");
        if let Ok((t, v)) = ctx.query_one::<(&mut Transform, &mut Velocity)>(player).get() {
            t.position = START;
            v.linear = Vec2::ZERO;
        }
    }
}

/// The camera eases toward the player and stays inside the playfield.
fn follow_camera(ctx: &mut Ctx<'_>) {
    let Some(game) = ctx.resources.get_mut::<Game>() else { return };
    let (player, camera) = (game.player, game.camera);
    let Ok(target) = ctx.world.get::<&Transform>(player).map(|t| t.position) else { return };
    let k = 1.0 - (-8.0 * ctx.time.delta()).exp();
    game.camera_pos += (target - game.camera_pos) * k;
    // half of what the window shows, in world units; no black borders if the map is bigger
    let view = Vec2::new(ctx.window.size.0 as f32, ctx.window.size.1 as f32) / (2.0 * ZOOM);
    let limit = (HALF - view).max(Vec2::ZERO);
    game.camera_pos = game.camera_pos.clamp(-limit, limit);
    // whole screen pixels keep the pixel art from shimmering
    let snapped = (game.camera_pos * ZOOM).round() / ZOOM;
    if let Ok(mut t) = ctx.world.get::<&mut Transform>(camera) {
        t.position = snapped;
    }
}

/// Once a second, log a status line (there is no text rendering yet).
fn status(ctx: &mut Ctx<'_>) {
    let now = ctx.time.elapsed();
    let Some(game) = ctx.resources.get_mut::<Game>() else { return };
    if now < game.next_status {
        return;
    }
    game.next_status = now.floor() + 1.0;
    let (score, round, left, player) = (game.score, game.round, game.coins_left, game.player);
    let pos = ctx.world.get::<&Transform>(player).map(|t| t.position).unwrap_or(START);
    global_info(&format!(
        "[coin_dash] t={:.0}s pos=({:.0}, {:.0}) score={score} coins left={left} round={round}",
        now, pos.x, pos.y
    ));
}

fn load_assets(engine: &mut Engine) -> Assets {
    let t = engine.textures_mut();
    let mut load = |name: &str, bytes: &[u8]| t.add_png(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    Assets {
        player: load("player.png", include_bytes!("../assets/player.png")),
        enemy: load("enemy.png", include_bytes!("../assets/enemy.png")),
        coins: load("coin_sheet.png", include_bytes!("../assets/coin_sheet.png")),
        tiles: load("tiles.png", include_bytes!("../assets/tiles.png")),
    }
}

/// Build the world and the systems. Also used by tests.
pub fn setup(engine: &mut Engine) {
    let assets = load_assets(engine);
    let world = engine.world_mut();
    let camera = world.spawn((
        Camera { zoom: ZOOM, clear_color: [0.02, 0.03, 0.05, 1.0], ..Camera::orthographic_2d() },
        Transform::default(),
    ));
    spawn_ground(world, assets.tiles);
    let player = spawn_player(world, assets);
    spawn_enemies(world, assets.enemy);

    let mut game = Game {
        assets,
        player,
        camera,
        camera_pos: START,
        score: 0,
        round: 1,
        coins_left: 0,
        invulnerable: 1.0,
        next_status: 0.0,
        autoplay: std::env::var_os("COIN_DASH_AUTOPLAY").is_some_and(|v| !v.is_empty()),
        rng: 0x2545F491,
    };
    for _ in 0..coins_in_round(1) {
        let p = random_spot(&mut game);
        let phase = game.random();
        engine.world_mut().spawn(coin_bundle(assets.coins, p, phase));
        game.coins_left += 1;
    }
    engine.insert_resource(game);

    // Systems of a stage run in the order they are added.
    // The movement and collision systems are par systems with exact `Access`; the camera and
    // the status line need the window / the log, so they stay exclusive.
    engine.add_par_system(
        Stage::Update,
        Access::new()
            .named("player_input")
            .res::<Game>()
            .read::<Coin>()
            .write::<Transform>()
            .write::<Velocity>()
            .write::<Sprite>(),
        player_input,
    )
    .run_if(menu::running);
    engine.add_par_system(
        Stage::Update,
        Access::new().named("patrol").read::<Transform>().write::<Velocity>().write::<Patrol>().write::<Sprite>(),
        patrol,
    )
    .run_if(menu::running);
    engine.add_par_system(
        Stage::Update,
        Access::new().named("animate_coins").write::<Transform>().write::<Sprite>().read::<Coin>(),
        animate_coins,
    )
    .run_if(menu::running);
    engine.add_par_system(
        Stage::Update,
        Access::new()
            .named("collisions")
            .res_mut::<Game>()
            .write::<Transform>()
            .write::<Velocity>()
            .read::<Coin>()
            .read::<Patrol>(),
        collisions,
    )
    .run_if(menu::running);
    engine.add_system(Stage::Update, follow_camera);
    engine.add_system(Stage::Update, status);
}

#[main]
fn main() {
    match Engine::new("coin dash") {
        Ok(mut engine) => {
            setup(&mut engine);
            // menus: main menu, settings, pause (Esc); see menu.rs
            let autoplay = engine.resources().get::<Game>().is_some_and(|g| g.autoplay);
            if let Err(e) = engine.add_plugin(menu::plugin()) {
                eprintln!("ui plugin failed: {e}");
            }
            if let Err(e) = engine.add_plugin(efeengine_world_ui::WorldUiPlugin) {
                eprintln!("world ui plugin failed: {e}");
            }
            if let Err(e) = engine.add_plugin(efeengine_ui::DebugMenuPlugin) {
                eprintln!("debug menu failed: {e}");
            }
            menu::setup(&mut engine, autoplay);
            engine.set_renderer_2d(SpriteRenderer::new());
            global_info("[coin_dash] WASD / arrows to move: collect the coins, avoid the red enemies");
            engine.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
