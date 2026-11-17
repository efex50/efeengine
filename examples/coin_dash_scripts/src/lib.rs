//! coin_dash_scripts: coin_dash with its gameplay written in efescript.
//!
//! Same game as `examples/coin_dash` (collect the coins, dodge the patrolling enemies, a new
//! round when the coins are gone), but the rules live in `scripts/*.efa` and are compiled at
//! run time by `Engine::load`, so the engine's `ScriptCache` is in use:
//!
//! | file | job |
//! |---|---|
//! | `coin_dash_scripts.efa` | the root: `use`s the others (one module, named like the file) |
//! | `common.efa` | constants and helpers shared by the others (vectors, a tiny log formatter) |
//! | `player.efa` | `player_update`: movement from the keyboard, or the autoplay bot |
//! | `enemy.efa` | `patrol_update`: walk back and forth along a lane |
//! | `coin.efa` | `coin_update`: spin (UV frames of the sheet) and bob |
//! | `rules.efa` | `init` / `frame`: spawn rounds, pickup, score, hits, status line |
//!
//! Rust keeps what scripts cannot do: the window, the renderer, textures, the ground, the
//! camera, the component *types* scripts read (`Player`, `Coin`, `Patrol`) and the game API
//! (`game.*`, see [`register_api`]). It also attaches the `Script` components of the player and
//! the enemies, as `script_demo` does; the scripts attach the ones of the coins they spawn with
//! `engine.script.attach`.
//!
//! Scripts come from `$EFE_SCRIPT_DIR`, else `scripts/` next to the executable (`cargo xtask
//! build coin_dash_scripts native` copies them there). The root is watched with
//! `Engine::watch_source`: edit a `.efa` file while the game runs and it is recompiled and swapped
//! in (the world stays). On the web the sources are embedded with `include_str!`.
//!
//! `COIN_DASH_AUTOPLAY=1` makes the script bot walk to the nearest coin.

// `#[main]` leaves `main` out of the native lib target, so its callees look unused there.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::prelude::*;
use efeengine_core::mods::{LoadInput};
use efeengine_core::scripting::{ModuleId, Script};

pub const GAME: &str = "coin_dash_scripts";
/// Name of the root script; the module (and the `[..]` prefix of its log lines) is named after it.
pub const ROOT_SCRIPT: &str = "coin_dash_scripts.efa";

const TILE: f32 = 16.0;
const MAP_W: i32 = 40;
const MAP_H: i32 = 30;
/// Half the size of the playfield in world units.
pub const HALF: Vec2 = Vec2::new(MAP_W as f32 * TILE / 2.0, MAP_H as f32 * TILE / 2.0);
const ZOOM: f32 = 2.0;
const LAYER_GROUND: i32 = -10;
const LAYER_ENEMY: i32 = 2;
const LAYER_SHADOW: i32 = 3;
const LAYER_PLAYER: i32 = 4;

// ---- components the scripts read (their layouts are in the API index) ----

/// The player. `invulnerable` is the seconds of immunity left after a hit.
#[script_component]
#[derive(Default)]
pub struct Player {
    pub invulnerable: f32,
}

/// A coin: bobs around `base_y` and spins; `phase` desynchronises the coins.
#[script_component]
#[derive(Default)]
pub struct Coin {
    pub base_y: f32,
    pub phase: f32,
}

/// An enemy moving back and forth along `axis`, within `range` of `origin`.
#[script_component]
#[derive(Default)]
pub struct Patrol {
    pub origin: Vec2,
    pub axis: Vec2,
    pub range: f32,
    pub speed: f32,
    /// +1 or -1 along the axis.
    pub dir: f32,
}

// ---- the game API ----

/// What the `game.*` functions and the tests share.
#[derive(Clone)]
pub struct Game {
    score: Arc<AtomicI64>,
    autoplay: Arc<AtomicBool>,
    textures: Arc<Mutex<HashMap<String, u32>>>,
}

impl Game {
    pub fn score(&self) -> i64 {
        self.score.load(Ordering::Relaxed)
    }

    pub fn set_autoplay(&self, on: bool) {
        self.autoplay.store(on, Ordering::Relaxed);
    }

    /// The texture registered under `name` (`player`, `enemy`, `coin_sheet`, `tiles`).
    pub fn texture(&self, name: &str) -> Option<TextureHandle> {
        self.textures.lock().unwrap().get(name).map(|&id| TextureHandle { id })
    }
}

/// Register the components and the `game` namespace. Needs no script file: `EFE_API_EXPORT` makes
/// `game_loop` write the index and return before any frame.
pub fn register_api(engine: &mut Engine) -> Game {
    engine.set_game_name(GAME);
    engine.register::<Player>().expect("register Player");
    engine.register::<Coin>().expect("register Coin");
    engine.register::<Patrol>().expect("register Patrol");
    engine.define_namespace("game", 1, GAME, "Functions of the coin_dash_scripts game.").expect("define namespace game");
    let game = Game {
        score: Arc::default(),
        autoplay: Arc::new(AtomicBool::new(std::env::var_os("COIN_DASH_AUTOPLAY").is_some_and(|v| !v.is_empty()))),
        textures: Arc::default(),
    };

    let s = game.score.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "score_add")
                .doc("Add `points` to the score and return the new total.")
                .param("points", ApiType::I64)
                .result("total", ApiType::I64),
            move |_c, a, r| {
                let points = a.i64(0)?;
                r.set_i64(0, s.fetch_add(points, Ordering::Relaxed) + points)?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.score_add");
    let s = game.score.clone();
    engine
        .register_host_fn(HostFn::new("game", "score").doc("The current score.").result("total", ApiType::I64), move |_c, _a, r| {
            r.set_i64(0, s.load(Ordering::Relaxed))?;
            Ok(HostOk::Continue)
        })
        .expect("register game.score");
    let s = game.score.clone();
    engine
        .register_host_fn(HostFn::new("game", "score_reset").doc("Set the score to 0."), move |_c, _a, _r| {
            s.store(0, Ordering::Relaxed);
            Ok(HostOk::Continue)
        })
        .expect("register game.score_reset");
    let f = game.autoplay.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "autoplay").doc("1 when the bot should play (`COIN_DASH_AUTOPLAY` is set).").result("on", ApiType::Bool),
            move |_c, _a, r| {
                r.set_bool(0, f.load(Ordering::Relaxed))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.autoplay");
    let t = game.textures.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "texture")
                .doc("The texture id of the asset `name` (`player`, `enemy`, `coin_sheet`, `tiles`), 0 if unknown.")
                .param("name", ApiType::Str)
                .result("id", ApiType::I32),
            move |c, a, r| {
                let name = c.read_str(a.str(0)?)?;
                r.set_i32(0, t.lock().unwrap().get(&name).map_or(0, |&id| id as i32))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.texture");
    game
}

// ---- the world Rust builds ----

/// The four PNGs, decoded by `Textures::add_png` (all targets, so it works on the web).
fn load_textures(engine: &mut Engine, game: &Game) {
    let t = engine.textures_mut();
    let mut load = |name: &str, bytes: &[u8]| {
        let h = t.add_png(bytes).unwrap_or_else(|e| panic!("{name}.png: {e}"));
        game.textures.lock().unwrap().insert(name.to_string(), h.id);
    };
    load("player", include_bytes!("../assets/player.png"));
    load("enemy", include_bytes!("../assets/enemy.png"));
    load("coin_sheet", include_bytes!("../assets/coin_sheet.png"));
    load("tiles", include_bytes!("../assets/tiles.png"));
}

fn frame_uv(frame: usize, frames: usize) -> (Vec2, Vec2) {
    let w = 1.0 / frames as f32;
    (Vec2::new(frame as f32 * w, 0.0), Vec2::new((frame + 1) as f32 * w, 1.0))
}

fn spawn_ground(world: &mut World, tiles: TextureHandle) {
    for y in 0..MAP_H {
        for x in 0..MAP_W {
            let hash = x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663);
            let frame = if x == MAP_W / 2 || y == MAP_H / 2 {
                3
            } else if hash.rem_euclid(11) == 0 {
                2
            } else {
                hash.rem_euclid(2) as usize
            };
            let (uv_min, uv_max) = frame_uv(frame, 5);
            let pos = Vec2::new((x as f32 + 0.5) * TILE, (y as f32 + 0.5) * TILE) - HALF;
            world.spawn((
                Transform::from_position(pos),
                Sprite { uv_min, uv_max, layer: LAYER_GROUND, ..Sprite::new(tiles) }.with_size(Vec2::splat(TILE)),
            ));
        }
    }
}

/// The camera follows the player; filled by [`spawn_actors`].
struct CameraRig {
    camera: Entity,
    player: Entity,
    pos: Vec2,
}

fn follow_camera(ctx: &mut Ctx<'_>) {
    let Some(rig) = ctx.resources.get_mut::<CameraRig>() else { return };
    let Ok(target) = ctx.world.get::<&Transform>(rig.player).map(|t| t.position) else { return };
    let k = 1.0 - (-8.0 * ctx.time.delta()).exp();
    rig.pos += (target - rig.pos) * k;
    let view = Vec2::new(ctx.window.size.0 as f32, ctx.window.size.1 as f32) / (2.0 * ZOOM);
    let limit = (HALF - view).max(Vec2::ZERO);
    rig.pos = rig.pos.clamp(-limit, limit);
    let snapped = (rig.pos * ZOOM).round() / ZOOM;
    let camera = rig.camera;
    if let Ok(mut t) = ctx.world.get::<&mut Transform>(camera) {
        t.position = snapped;
    }
}

/// Camera and ground: the part of the world that has no script.
pub fn spawn_scenery(engine: &mut Engine, game: &Game) {
    let tiles = game.texture("tiles").expect("tiles texture");
    let world = engine.world_mut();
    spawn_ground(world, tiles);
    let camera = world.spawn((
        Camera { zoom: ZOOM, clear_color: [0.02, 0.03, 0.05, 1.0], ..Camera::orthographic_2d() },
        Transform::default(),
    ));
    // the camera rig is completed by `spawn_actors`
    engine.insert_resource(CameraRig { camera, player: Entity::DANGLING, pos: Vec2::ZERO });
    engine.add_system(Stage::Update, follow_camera);
}

/// The player, its shadow and the three enemies. With a loaded script module they get their
/// `Script`s (`player_update`, `patrol_update`); without one they just stand there.
pub fn spawn_actors(engine: &mut Engine, game: &Game, module: Option<ModuleId>) {
    let (player_tex, enemy_tex, tiles) =
        (game.texture("player").unwrap(), game.texture("enemy").unwrap(), game.texture("tiles").unwrap());
    let world = engine.world_mut();
    let player = world.spawn((
        Player { invulnerable: 1.0 }, // a second of immunity at the start, as after a hit
        Transform::default(),
        Velocity::default(),
        Sprite { layer: LAYER_PLAYER, ..Sprite::new(player_tex) }.with_size(Vec2::splat(TILE)),
    ));
    // the shadow: a child sprite that follows without any code
    let (uv_min, uv_max) = frame_uv(4, 5);
    world.spawn((
        Transform::from_position(Vec2::new(0.0, -6.0)),
        Parent::new(player),
        Sprite { uv_min, uv_max, layer: LAYER_SHADOW, ..Sprite::new(tiles) }.with_size(Vec2::splat(TILE)),
    ));
    if let Some(m) = module {
        world.insert_one(player, Script::new(m, "player_update")).expect("player alive");
    }
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
            Sprite { layer: LAYER_ENEMY, ..Sprite::new(enemy_tex) }.with_size(Vec2::splat(TILE)),
        ));
        if let Some(m) = module {
            world.insert_one(e, Script::new(m, "patrol_update")).expect("enemy alive");
        }
    }
    if let Some(rig) = engine.resources_mut().get_mut::<CameraRig>() {
        rig.player = player;
    }
}

// ---- loading the scripts ----

/// The script sources embedded in the binary (the web build has no files to read, and a native
/// build without a script directory falls back to them).
const EMBEDDED: [(&str, &str); 6] = [
    ("coin_dash_scripts.efa", include_str!("../scripts/coin_dash_scripts.efa")),
    ("common.efa", include_str!("../scripts/common.efa")),
    ("player.efa", include_str!("../scripts/player.efa")),
    ("enemy.efa", include_str!("../scripts/enemy.efa")),
    ("coin.efa", include_str!("../scripts/coin.efa")),
    ("rules.efa", include_str!("../scripts/rules.efa")),
];

/// Compile the embedded sources (every target). Not hot reloaded.
pub fn load_embedded(engine: &mut Engine) -> Option<ModuleId> {
    let mut files = efeengine_core::efa::asm::MemSource::new();
    for (name, text) in EMBEDDED {
        files = files.with(name, text);
    }
    match engine.load_input(LoadInput::Efa { name: "coin_dash_scripts", files: &files, root: ROOT_SCRIPT }) {
        Ok(loaded) => {
            global_info(&format!("[{GAME}] scripts compiled from the embedded sources as module {:?}", loaded.id()));
            Some(loaded.id())
        }
        Err(e) => {
            global_warn(&format!("[{GAME}] cannot compile the embedded scripts: {e}"));
            None
        }
    }
}

/// Directory the scripts are loaded from: `$EFE_SCRIPT_DIR`, else `scripts/` next to the executable.
#[cfg(not(target_family = "wasm"))]
pub fn script_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("EFE_SCRIPT_DIR").filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    Some(std::env::current_exe().ok()?.parent()?.join("scripts"))
}

/// Native: compile `coin_dash_scripts.efa` from `dir` with `Engine::load` (through the script
/// cache) and watch it for edits. A missing or broken script is a warning: the game runs
/// without scripts (nothing moves).
#[cfg(not(target_family = "wasm"))]
pub fn load_scripts(engine: &mut Engine, dir: &std::path::Path) -> Option<ModuleId> {
    let path = dir.join(ROOT_SCRIPT);
    if !path.is_file() {
        global_warn(&format!(
            "[{GAME}] {} not found (cargo xtask build {GAME} native copies the scripts, or set EFE_SCRIPT_DIR)",
            path.display()
        ));
        return None;
    }
    match engine.load(&path) {
        Ok(loaded) => {
            let id = loaded.id();
            global_info(&format!("[{GAME}] {} compiled as module {id:?}; script cache {:?}", path.display(), engine.cache().stats()));
            match engine.watch_source(id, &path) {
                Ok(()) => global_info(&format!("[{GAME}] watching {} for edits (hot reload)", dir.display())),
                Err(e) => global_warn(&format!("[{GAME}] cannot watch {}: {e}", path.display())),
            }
            Some(id)
        }
        Err(e) => {
            global_warn(&format!("[{GAME}] cannot load {}: {e}", path.display()));
            None
        }
    }
}

/// Everything except the scripts: components, API, textures, camera, ground. Returns the game
/// handle; call [`load_scripts`] / [`load_embedded`] next, then [`spawn_actors`].
pub fn setup(engine: &mut Engine) -> Game {
    let game = register_api(engine);
    load_textures(engine, &game);
    spawn_scenery(engine, &game);
    game
}

#[main]
fn main() {
    match Engine::new(GAME) {
        Ok(mut engine) => {
            let game = setup(&mut engine);
            #[cfg(not(target_family = "wasm"))]
            let module = script_dir().and_then(|d| load_scripts(&mut engine, &d)).or_else(|| load_embedded(&mut engine));
            #[cfg(target_family = "wasm")]
            let module = load_embedded(&mut engine);
            spawn_actors(&mut engine, &game, module);
            engine.set_renderer(SpriteRenderer::new());
            global_info(&format!("[{GAME}] WASD / arrows to move: collect the coins, avoid the red enemies"));
            engine.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
