//! coin_dash_wasm: coin_dash whose gameplay runs entirely in two wasm mods (Rust + C),
//! translated in-process to efevm modules with `efe-wasm` and wrapped as `.efmod` packages.
//!
//! The Rust host keeps only what mods cannot do:
//! - the window, `SpriteRenderer`, textures/assets, and camera;
//! - the registered component types (`Player`, `Coin`, `Patrol`) and `CoinSpawned` struct;
//! - the `game.*` host API (score, round, coins_left, lives, game_over, restart_gen, autoplay,
//!   texture lookup, world-UI enemy labels / "Scored!" popups) and `game.*` events;
//! - the UI (`menu.rs`: main menu, settings, pause, game-over, HUD, plus `DebugMenuPlugin`);
//! - loading and translating the `.wasm` mods from `target/coin_dash_wasm/wasm/` (or `$COIN_DASH_WASM`).

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use efeengine_core::ecs::entity_from_bits;
use efeengine_core::efevm::{ApiIndex, ApiType, EventDesc, FieldDesc, HostFn, HostOk, Interface, Prim, Provide, StructDesc};
use efeengine_core::mods::{Grant, LoadInput, ModRequest};
use efeengine_core::prelude::*;
use efeengine_core::scripting::ModuleId;
use efeengine_world_ui::{Anchor2D, Floating2D, Text2D};

pub mod menu;

pub const GAME: &str = "coin_dash_wasm";
pub const START_LIVES: i32 = 3;

const TILE: f32 = 16.0;
const MAP_W: i32 = 40;
const MAP_H: i32 = 30;
/// Half the size of the playfield in world units.
pub const HALF: Vec2 = Vec2::new(MAP_W as f32 * TILE / 2.0, MAP_H as f32 * TILE / 2.0);
const ZOOM: f32 = 2.0;

// ---- components and structs visible to the wasm mods (layouts live in the API index) ----

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

/// Layout of the `game.coin_spawned` event payload.
fn coin_spawned_desc() -> StructDesc {
    StructDesc::new("CoinSpawned", 12, 4)
        .doc("Payload of the `game.coin_spawned` event.")
        .field(FieldDesc::new("x", 0, Prim::F32, 1))
        .field(FieldDesc::new("y", 4, Prim::F32, 1))
        .field(FieldDesc::new("round", 8, Prim::I32, 1))
}

// ---- shared game state ----

#[derive(Default)]
struct WorldUiQueue {
    enemy_labels: Vec<Entity>,
    scored_popups: Vec<Vec2>,
}

/// Shared state between the `game.*` host functions, the UI, and headless tests.
#[derive(Clone)]
pub struct Game {
    score: Arc<AtomicI64>,
    round: Arc<AtomicI32>,
    coins_left: Arc<AtomicI32>,
    lives: Arc<AtomicI32>,
    game_over: Arc<AtomicBool>,
    restart_gen: Arc<AtomicI32>,
    autoplay: Arc<AtomicBool>,
    textures: Arc<Mutex<HashMap<String, u32>>>,
    mod_status: Arc<Mutex<Option<String>>>,
    loaded_mods: Arc<Mutex<Vec<(String, ModuleId)>>>,
    ui_queue: Arc<Mutex<WorldUiQueue>>,
}

impl Game {
    pub fn score(&self) -> i64 {
        self.score.load(Ordering::Relaxed)
    }

    pub fn round(&self) -> i32 {
        self.round.load(Ordering::Relaxed)
    }

    pub fn coins_left(&self) -> i32 {
        self.coins_left.load(Ordering::Relaxed)
    }

    pub fn lives(&self) -> i32 {
        self.lives.load(Ordering::Relaxed)
    }

    pub fn is_game_over(&self) -> bool {
        self.game_over.load(Ordering::Relaxed)
    }

    pub fn autoplay(&self) -> bool {
        self.autoplay.load(Ordering::Relaxed)
    }

    pub fn set_autoplay(&self, on: bool) {
        self.autoplay.store(on, Ordering::Relaxed);
    }

    pub fn request_restart(&self) {
        self.score.store(0, Ordering::Relaxed);
        self.round.store(1, Ordering::Relaxed);
        self.coins_left.store(0, Ordering::Relaxed);
        self.lives.store(START_LIVES, Ordering::Relaxed);
        self.game_over.store(false, Ordering::Relaxed);
        self.restart_gen.fetch_add(1, Ordering::Relaxed);
    }

    pub fn texture(&self, name: &str) -> Option<TextureHandle> {
        self.textures.lock().unwrap().get(name).map(|&id| TextureHandle { id })
    }

    pub fn mod_status(&self) -> Option<String> {
        self.mod_status.lock().unwrap().clone()
    }

    pub fn set_mod_status(&self, msg: Option<String>) {
        *self.mod_status.lock().unwrap() = msg;
    }

    pub fn loaded_mods(&self) -> Vec<(String, ModuleId)> {
        self.loaded_mods.lock().unwrap().clone()
    }
}

/// Register the components, event structs, events, and the `game` namespace.
pub fn register_api(engine: &mut Engine) -> Game {
    engine.set_game_name(GAME);
    engine.register::<Player>().expect("register Player");
    engine.register::<Coin>().expect("register Coin");
    engine.register::<Patrol>().expect("register Patrol");
    engine.register_struct(coin_spawned_desc()).expect("register CoinSpawned");
    engine
        .define_namespace("game", 1, GAME, "Functions and events of the coin_dash_wasm host.")
        .expect("define namespace game");
    engine
        .register_event(
            EventDesc::new("game", "coin_spawned")
                .payload("CoinSpawned")
                .doc("Sent when dash_rust spawns a coin; read by dash_c."),
        )
        .expect("register game.coin_spawned");
    engine
        .register_event(
            EventDesc::new("game", "patrol_turn")
                .doc("Sent when an enemy reverses direction in dash_c; read by dash_rust."),
        )
        .expect("register game.patrol_turn");

    let game = Game {
        score: Arc::default(),
        round: Arc::new(AtomicI32::new(1)),
        coins_left: Arc::default(),
        lives: Arc::new(AtomicI32::new(START_LIVES)),
        game_over: Arc::default(),
        restart_gen: Arc::default(),
        autoplay: Arc::new(AtomicBool::new(
            std::env::var_os("COIN_DASH_AUTOPLAY").is_some_and(|v| !v.is_empty()),
        )),
        textures: Arc::default(),
        mod_status: Arc::default(),
        loaded_mods: Arc::default(),
        ui_queue: Arc::default(),
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
        .register_host_fn(
            HostFn::new("game", "score")
                .doc("The current score.")
                .result("total", ApiType::I64),
            move |_c, _a, r| {
                r.set_i64(0, s.load(Ordering::Relaxed))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.score");

    let s = game.score.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "score_reset").doc("Set the score to 0."),
            move |_c, _a, _r| {
                s.store(0, Ordering::Relaxed);
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.score_reset");

    let rd = game.round.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "round")
                .doc("The current round number.")
                .result("round", ApiType::I32),
            move |_c, _a, r| {
                r.set_i32(0, rd.load(Ordering::Relaxed))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.round");

    let rd = game.round.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "set_round")
                .doc("Set the current round number displayed by the HUD.")
                .param("round", ApiType::I32),
            move |_c, a, _r| {
                rd.store(a.i32(0)?, Ordering::Relaxed);
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.set_round");

    let cl = game.coins_left.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "coins_left")
                .doc("Coins left in the current round.")
                .result("count", ApiType::I32),
            move |_c, _a, r| {
                r.set_i32(0, cl.load(Ordering::Relaxed))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.coins_left");

    let cl = game.coins_left.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "set_coins_left")
                .doc("Set the number of coins left displayed by the HUD.")
                .param("count", ApiType::I32),
            move |_c, a, _r| {
                cl.store(a.i32(0)?, Ordering::Relaxed);
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.set_coins_left");

    let lv = game.lives.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "lives")
                .doc("Remaining player lives.")
                .result("lives", ApiType::I32),
            move |_c, _a, r| {
                r.set_i32(0, lv.load(Ordering::Relaxed))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.lives");

    let lv = game.lives.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "set_lives")
                .doc("Set remaining player lives displayed by the HUD.")
                .param("lives", ApiType::I32),
            move |_c, a, _r| {
                lv.store(a.i32(0)?, Ordering::Relaxed);
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.set_lives");

    let go = game.game_over.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "is_game_over")
                .doc("True when the player has lost all lives.")
                .result("over", ApiType::Bool),
            move |_c, _a, r| {
                r.set_bool(0, go.load(Ordering::Relaxed))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.is_game_over");

    let go = game.game_over.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "set_game_over")
                .doc("Mark whether the game-over screen should be shown.")
                .param("over", ApiType::Bool),
            move |_c, a, _r| {
                go.store(a.bool(0)?, Ordering::Relaxed);
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.set_game_over");

    let rg = game.restart_gen.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "restart_gen")
                .doc("Monotonically increasing counter incremented when the player triggers Restart / Play again.")
                .result("gen", ApiType::I32),
            move |_c, _a, r| {
                r.set_i32(0, rg.load(Ordering::Relaxed))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.restart_gen");

    let f = game.autoplay.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "autoplay")
                .doc("True when the autoplay bot should steer toward the nearest coin.")
                .result("on", ApiType::Bool),
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

    let q = game.ui_queue.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "popup_scored")
                .doc("Spawn a rising world-space \"Scored!\" text popup at `(x, y)`.")
                .param("x", ApiType::F32)
                .param("y", ApiType::F32),
            move |_c, a, _r| {
                let x = a.f32(0)?;
                let y = a.f32(1)?;
                q.lock().unwrap().scored_popups.push(Vec2::new(x, y));
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.popup_scored");

    let q = game.ui_queue.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "label_enemy")
                .doc("Attach a world-space \"Enemy\" label to `enemy`.")
                .param("enemy", ApiType::Entity),
            move |_c, a, _r| {
                if let Some(e) = entity_from_bits(a.entity(0)?) {
                    q.lock().unwrap().enemy_labels.push(e);
                }
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.label_enemy");

    game
}

// ---- host scenery: textures, camera, world-UI popups/labels ----

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

struct CameraRig {
    camera: Entity,
    pos: Vec2,
}

fn follow_camera(ctx: &mut Ctx<'_>) {
    let target = ctx.world.query::<(&Player, &Transform)>().iter().next().map(|(_, t)| t.position);
    let Some(target) = target else { return };
    let Some(rig) = ctx.resources.get_mut::<CameraRig>() else { return };
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

fn drain_world_ui(ctx: &mut Ctx<'_>) {
    let Some(game) = ctx.resources.get::<Game>().map(|g| (*g).clone()) else { return };
    let (labels, popups) = {
        let mut q = game.ui_queue.lock().unwrap();
        (std::mem::take(&mut q.enemy_labels), std::mem::take(&mut q.scored_popups))
    };
    for e in labels {
        if ctx.world.contains(e) {
            ctx.commands.spawn(
                ctx.world,
                (
                    Anchor2D::entity(e).with_offset(Vec2::new(0.0, -14.0)),
                    Text2D {
                        text: "Enemy".into(),
                        size: 11.0,
                        color: [1.0, 0.45, 0.45, 1.0],
                        ..Default::default()
                    },
                ),
            );
        }
    }
    for at in popups {
        ctx.commands.spawn(
            ctx.world,
            (
                Anchor2D::point(at).with_offset(Vec2::new(0.0, -10.0)),
                Text2D {
                    text: "Scored!".into(),
                    size: 14.0,
                    color: [1.0, 0.84, 0.2, 1.0],
                    bold: true,
                },
                Floating2D::new(40.0, 0.8),
            ),
        );
    }
}

// ---- mod policy & wasm translation / loading (native) ----

fn grant(ns: &[&str]) -> Grant {
    Grant {
        namespaces: ns.iter().map(|s| s.to_string()).collect(),
        // These mods are bundled with the game, so they may run on the JIT when the `jit`
        // feature is on (`./build.py run coin_dash_wasm native --jit`); otherwise ignored.
        jit: true,
        ..Grant::default()
    }
}

pub fn mod_policy(r: &ModRequest<'_>) -> Result<Grant, String> {
    match r.manifest.id.as_str() {
        "dash_c" => Ok(grant(&["engine.world", "engine.time", "engine.math", "engine.events", "engine.log", "game"])),
        "dash_rust" => Ok(grant(&[
            "engine.world",
            "engine.input",
            "engine.random",
            "engine.time",
            "engine.events",
            "engine.log",
            "game",
        ])),
        other => Err(format!("unknown mod `{other}`")),
    }
}

#[cfg(not(target_family = "wasm"))]
const DASH_C_TOML_FALLBACK: &str = include_str!("../mods/dash_c/mod.toml");
#[cfg(not(target_family = "wasm"))]
const DASH_RUST_TOML_FALLBACK: &str = include_str!("../mods/dash_rust/mod.toml");

#[cfg(not(target_family = "wasm"))]
pub fn parse_mod_manifest(toml_text: &str) -> Result<efeengine_core::efemod::Manifest, String> {
    let m = efeengine_core::efa::mod_folder::read_mod_toml(toml_text)?.manifest;
    m.validate().map_err(|e| format!("invalid manifest: {e}"))?;
    Ok(m)
}

#[cfg(not(target_family = "wasm"))]
pub fn parse_c_fallback(toml_text: &str) -> Result<efe_wasm::Fallback, String> {
    let mt = efeengine_core::efa::mod_folder::read_mod_toml(toml_text)?;
    Ok(efe_wasm::Fallback {
        provides: mt
            .provides
            .into_iter()
            .map(|p| efe_wasm::FallbackProvide {
                name: p.name,
                export: p.export,
                doc: p.doc,
                params: p.params,
                results: p.results,
            })
            .collect(),
        events: mt
            .events
            .into_iter()
            .map(|e| efe_wasm::FallbackEvent {
                name: e.name,
                payload: e.payload,
                doc: e.doc,
            })
            .collect(),
        scratch: mt.build.scratch,
    })
}

/// Build `dash_c`'s own exported `ApiIndex` from its `mod.toml` and `game_index` (no clang needed).
#[cfg(not(target_family = "wasm"))]
pub fn dash_c_own_index(game_index: &ApiIndex, c_toml_text: &str) -> Result<ApiIndex, String> {
    let manifest = parse_mod_manifest(c_toml_text)?;
    let fb = parse_c_fallback(c_toml_text)?;
    let mut iface = Interface::default();
    for fp in &fb.provides {
        for p in &fp.params {
            if let ApiType::Struct(sn) = &p.ty
                && !iface.structs.iter().any(|s| s.name == *sn)
            {
                let mut sd = game_index
                    .struct_type(sn)
                    .ok_or_else(|| format!("struct `{sn}` not in game index"))?
                    .clone();
                if sd.owner.is_empty() {
                    sd.owner = game_index.game.clone();
                }
                iface.structs.push(sd);
            }
        }
        iface.provides.push(Provide {
            name: fp.name.clone(),
            func: 0,
            doc: fp.doc.clone(),
            params: fp.params.clone(),
            results: fp.results.clone(),
        });
    }
    Ok(iface.index(&manifest.id, manifest.api_version, &manifest.namespace_doc()))
}

/// Locate the built `.wasm` directory: `$COIN_DASH_WASM`, else relative to the executable / target dir.
#[cfg(not(target_family = "wasm"))]
pub fn default_wasm_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("COIN_DASH_WASM").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().skip(1).take(4) {
            let direct = anc.join("coin_dash_wasm/wasm");
            if direct.is_dir() {
                return direct;
            }
            let nested = anc.join("target/coin_dash_wasm/wasm");
            if nested.is_dir() {
                return nested;
            }
        }
    }
    if let Some(td) = std::env::var_os("CARGO_TARGET_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(td).join("coin_dash_wasm/wasm");
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/coin_dash_wasm/wasm")
}

#[cfg(not(target_family = "wasm"))]
fn read_mod_toml_text(mods_dir: Option<&Path>, mod_id: &str, fallback: &'static str) -> String {
    if let Some(dir) = mods_dir {
        let p = dir.join(mod_id).join("mod.toml");
        if let Ok(text) = std::fs::read_to_string(&p) {
            return text;
        }
    }
    let repo_toml = Path::new(env!("CARGO_MANIFEST_DIR")).join("mods").join(mod_id).join("mod.toml");
    std::fs::read_to_string(repo_toml).unwrap_or_else(|_| fallback.to_string())
}

#[cfg(not(target_family = "wasm"))]
pub fn load_wasm_mods(engine: &mut Engine, game: &Game, wasm_dir: &Path, mods_dir: Option<&Path>) -> Result<(), String> {
    let c_wasm_path = wasm_dir.join("dash_c.wasm");
    let rust_wasm_path = wasm_dir.join("dash_rust.wasm");
    if !c_wasm_path.is_file() || !rust_wasm_path.is_file() {
        let msg = "mods not built: run build_mods.py".to_string();
        game.set_mod_status(Some(msg.clone()));
        global_warn(&format!(
            "[{GAME}] {msg} (looked in {}; run `python3 examples/coin_dash_wasm/build_mods.py`)",
            wasm_dir.display()
        ));
        return Err(msg);
    }

    let c_wasm = std::fs::read(&c_wasm_path).map_err(|e| format!("read {}: {e}", c_wasm_path.display()))?;
    let rust_wasm = std::fs::read(&rust_wasm_path).map_err(|e| format!("read {}: {e}", rust_wasm_path.display()))?;

    let c_toml = read_mod_toml_text(mods_dir, "dash_c", DASH_C_TOML_FALLBACK);
    let rust_toml = read_mod_toml_text(mods_dir, "dash_rust", DASH_RUST_TOML_FALLBACK);

    let c_manifest = parse_mod_manifest(&c_toml)?;
    let c_fallback = parse_c_fallback(&c_toml)?;
    let rust_manifest = parse_mod_manifest(&rust_toml)?;

    let game_index = engine.api_index();

    let c_opts = efe_wasm::TranslateOptions {
        fallback: c_fallback,
        require_efe_api: true,
        ..Default::default()
    };
    let c_module = efe_wasm::translate(&c_wasm, &game_index, &c_opts).map_err(|e| format!("translate dash_c: {e}"))?;
    efemod::check_module(&c_manifest, &c_module).map_err(|e| format!("check dash_c: {e}"))?;
    let c_pkg_bytes =
        efemod::Package::write(&c_manifest, &c_module.encode(), &BTreeMap::new()).map_err(|e| format!("pack dash_c: {e}"))?;
    let c_pkg_index = efemod::Package::read(&c_pkg_bytes, &efemod::ReadLimits::default())
        .map_err(|e| format!("read dash_c.efmod: {e}"))?
        .index();

    let rust_index = ApiIndex::merge(&[&game_index, &c_pkg_index]).map_err(|e| format!("merge index: {e}"))?;
    let rust_opts = efe_wasm::TranslateOptions {
        require_efe_api: true,
        ..Default::default()
    };
    let rust_module =
        efe_wasm::translate(&rust_wasm, &rust_index, &rust_opts).map_err(|e| format!("translate dash_rust: {e}"))?;
    efemod::check_module(&rust_manifest, &rust_module).map_err(|e| format!("check dash_rust: {e}"))?;
    let rust_pkg_bytes =
        efemod::Package::write(&rust_manifest, &rust_module.encode(), &BTreeMap::new()).map_err(|e| format!("pack dash_rust: {e}"))?;

    let c_id = engine
        .load_input(LoadInput::Package { label: "dash_c.efmod", bytes: &c_pkg_bytes })
        .map_err(|e| format!("load dash_c: {e}"))?
        .id();
    let r_id = engine
        .load_input(LoadInput::Package { label: "dash_rust.efmod", bytes: &rust_pkg_bytes })
        .map_err(|e| format!("load dash_rust: {e}"))?
        .id();

    *game.loaded_mods.lock().unwrap() = vec![("dash_c".to_string(), c_id), ("dash_rust".to_string(), r_id)];
    game.set_mod_status(None);
    global_info(&format!("[{GAME}] loaded wasm mods: dash_c ({c_id:?}), dash_rust ({r_id:?})"));
    Ok(())
}

/// Set up the host API, textures, camera, policy, and optionally load wasm mods from `wasm_dir`.
/// Passing `None` uses [`default_wasm_dir`]. Missing `.wasm` files leave a status message in
/// `Game::mod_status()` so the UI opens and displays `"mods not built: run build_mods.py"`.
pub fn setup_with_wasm_dir(engine: &mut Engine, wasm_dir: Option<&Path>) -> Game {
    let game = register_api(engine);
    load_textures(engine, &game);
    let camera = engine.world_mut().spawn((
        Camera {
            zoom: ZOOM,
            clear_color: [0.02, 0.03, 0.05, 1.0],
            ..Camera::orthographic_2d()
        },
        Transform::default(),
    ));
    engine.insert_resource(CameraRig { camera, pos: Vec2::ZERO });
    engine.insert_resource(game.clone());
    engine.set_mod_policy(mod_policy);
    engine.add_system(Stage::Update, drain_world_ui).order(order::SCRIPTS + 1).name("coin_dash_world_ui");
    engine.add_system(Stage::Update, follow_camera).order(order::LATE).name("coin_dash_camera");

    #[cfg(not(target_family = "wasm"))]
    if std::env::var_os("EFE_API_EXPORT").is_none() {
        let dir = wasm_dir.map(Path::to_path_buf).unwrap_or_else(default_wasm_dir);
        if let Err(e) = load_wasm_mods(engine, &game, &dir, None) {
            if game.mod_status().is_none() {
                game.set_mod_status(Some(format!("mod load error: {e}")));
            }
            global_warn(&format!("[{GAME}] {e}"));
        }
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = wasm_dir;
        game.set_mod_status(Some("Web build not supported: run native".to_string()));
        global_warn(&format!("[{GAME}] web build is not supported for wasm mods"));
    }

    game
}

/// Build the world, host API, and load the wasm mods. Also used by tests.
pub fn setup(engine: &mut Engine) -> Game {
    setup_with_wasm_dir(engine, None)
}

#[main]
fn main() {
    match Engine::new("coin dash wasm") {
        Ok(mut engine) => {
            if let Err(e) = engine.add_plugin(menu::plugin()) {
                eprintln!("ui plugin failed: {e}");
            }
            if let Err(e) = engine.add_plugin(efeengine_world_ui::WorldUiPlugin) {
                eprintln!("world ui plugin failed: {e}");
            }
            if let Err(e) = engine.add_plugin(efeengine_ui::DebugMenuPlugin) {
                eprintln!("debug menu failed: {e}");
            }
            let game = setup(&mut engine);
            let autoplay = game.autoplay();
            menu::setup(&mut engine, autoplay);
            engine.set_renderer_2d(SpriteRenderer::new());
            global_info(&format!("[{GAME}] WASD / arrows to move: collect the coins, avoid the red enemies"));
            engine.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
