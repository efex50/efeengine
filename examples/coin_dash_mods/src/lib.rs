//! coin_dash_mods: a minimal base game whose content is delivered by mods.
//!
//! The base game (this file) is an empty arena: the window, the `SpriteRenderer`, a camera and
//! nothing else. Everything you see comes from `.efmod` packages built from the folders in
//! `mods/`:
//!
//! | mod | needs | gives |
//! |---|---|---|
//! | `dash_core` | | the player, score and rounds; `provide`s the functions the others call |
//! | `dash_tiles` | `dash_core` | the ground tilemap, read from the asset `map.txt` |
//! | `dash_enemies` | `dash_core` | patrolling enemies (their number and speed come from `[meta]`) |
//! | `dash_coins` | `dash_core` | the coins: spin, bob, pickup, a bigger round after the last one |
//!
//! What the base game provides to mods:
//! * the namespace `game` ([`register_api`], exported to the API index the mods are packed
//!   against): `game.texture(path)` (the texture of one of the *calling mod's own* PNG assets),
//!   `game.score_add`, `game.score`, `game.score_reset`, `game.autoplay`;
//! * a [`ModPolicy`](efeengine_core::prelude::ModPolicy) ([`mod_policy`]) that grants each mod only
//!   the namespaces it needs;
//! * an [`AssetHandler`](efeengine_core::prelude::AssetHandler) ([`PngTextures`]) that turns every
//!   `*.png` asset of a mod into a texture (`Textures::add_png`), refuses the whole mod if a PNG
//!   does not decode, and frees the textures when the mod unloads; entities that still show a
//!   freed texture are despawned by [`sweep_orphans`];
//! * the mods themselves: [`mods_dir`] is a `DirSource` over the packed `.efmod` files. R reloads
//!   (`reload_mods`: new mods load, changed ones are replaced, unchanged ones are skipped), U
//!   unloads everything (`unload_mods`). The engine logs both reports.
//!
//! Which packages are loaded: `$COIN_DASH_MODS` if set, else `target/mods/coin_dash_mods/` (where
//! `cargo xtask mods coin_dash_mods` and `cargo xtask build coin_dash_mods native` pack them, found
//! relative to the executable). The example loads *packages*, not folders, because it exists to test
//! them: `cargo xtask build` also copies the plain folders to `<profile>/mods/`, which this game
//! ignores. Rebuilding a package while the game runs hot-reloads it (`DirSource` reports the change).
//!
//! The web build would get the same bytes from `fetch`: [`BytesSource`] turns downloaded
//! `(label, bytes)` pairs into a mod source on every target.
//!
//! `COIN_DASH_AUTOPLAY=1` lets the player bot walk to the nearest coin.

// `#[main]` leaves `main` out of the native lib target, so its callees look unused there.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use efeengine_core::efemod::Manifest;
use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::prelude::*;

pub const GAME: &str = "coin_dash_mods";

/// A 40 x 30 tile map of 16 units, seen at zoom 1.25 in the default 800 x 600 window.
pub const CAMERA_ZOOM: f32 = 1.25;

// ---- state shared between the host functions, the asset handler and the tests ----

/// The textures the asset handler registered, by module and asset path.
#[derive(Default)]
struct ModTextures {
    by_module: HashMap<ModuleId, Vec<(String, TextureHandle)>>,
}

/// Texture ids freed by unloaded mods; [`sweep_orphans`] despawns the sprites that still show them.
#[derive(Default)]
pub struct Freed(pub Vec<u32>);

/// What the base game knows and the tests read.
#[derive(Clone)]
pub struct Game {
    score: Arc<AtomicI64>,
    autoplay: Arc<AtomicBool>,
    textures: Arc<Mutex<ModTextures>>,
}

impl Game {
    pub fn score(&self) -> i64 {
        self.score.load(Ordering::Relaxed)
    }

    pub fn set_autoplay(&self, on: bool) {
        self.autoplay.store(on, Ordering::Relaxed);
    }

    /// The texture a mod's PNG asset was registered as.
    pub fn texture_of(&self, module: ModuleId, path: &str) -> Option<TextureHandle> {
        self.textures.lock().unwrap().by_module.get(&module)?.iter().find(|(p, _)| p == path).map(|(_, h)| *h)
    }

    /// How many textures mods hold right now.
    pub fn mod_texture_count(&self) -> usize {
        self.textures.lock().unwrap().by_module.values().map(Vec::len).sum()
    }
}

// ---- the game API ----

/// Register the `game` namespace (needs no mod: `EFE_API_EXPORT` writes the index before any frame).
pub fn register_api(engine: &mut Engine) -> Game {
    engine.set_game_name(GAME);
    engine.define_namespace("game", 1, GAME, "Functions of the coin_dash_mods base game.").expect("define namespace game");
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
                .doc("The texture id of the PNG asset `path` (relative to `assets/`, e.g. \"player.png\") of the calling mod's own package; 0 if it has no such PNG.")
                .param("path", ApiType::Str)
                .result("id", ApiType::I32),
            move |c, a, r| {
                let path = c.read_str(a.str(0)?)?;
                let id = c.host().current_module().and_then(|m| {
                    t.lock().unwrap().by_module.get(&m)?.iter().find(|(p, _)| *p == path).map(|(_, h)| h.id as i32)
                });
                r.set_i32(0, id.unwrap_or(0))?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.texture");
    game
}

// ---- policy ----

fn grant(ns: &[&str]) -> Grant {
    Grant { namespaces: ns.iter().map(|s| s.to_string()).collect(), ..Grant::default() }
}

/// What each mod may import. A mod only *requests* namespaces in its `mod.toml`; the grant is the
/// intersection of the request and this table, and covers the whole package. `dash_tiles` gets
/// no `engine.world`: it can only draw through `dash_core`'s functions. A mod this game has
/// never heard of may log and read the clock, nothing else.
pub fn mod_policy(r: &ModRequest<'_>) -> Result<Grant, String> {
    Ok(match r.manifest.id.as_str() {
        "dash_core" => grant(&["engine.world", "engine.input", "engine.log", "engine.time", "engine.events", "game"]),
        "dash_tiles" => grant(&["engine.log", "engine.mods", "game"]),
        "dash_enemies" => grant(&["engine.world", "engine.log", "engine.mods", "game"]),
        "dash_coins" => grant(&["engine.world", "engine.random", "engine.math", "engine.time", "engine.events", "engine.log", "game"]),
        _ => grant(&["engine.log", "engine.time"]),
    })
}

// ---- assets: PNG -> texture ----

/// Takes over every `*.png` asset of a mod: it must decode (else the whole mod is refused), it is
/// registered as a texture, and it is freed again when the mod unloads (or is replaced).
pub struct PngTextures {
    shared: Arc<Mutex<ModTextures>>,
}

impl PngTextures {
    pub fn new(game: &Game) -> Self {
        PngTextures { shared: game.textures.clone() }
    }
}

impl AssetHandler for PngTextures {
    fn matches(&self, path: &str) -> bool {
        path.to_ascii_lowercase().ends_with(".png")
    }

    fn check(&mut self, _m: &Manifest, path: &str, bytes: &[u8]) -> Result<(), String> {
        Image::from_png(bytes).map(|_| ()).map_err(|e| format!("{path} is not a usable PNG: {e}"))
    }

    fn commit(&mut self, id: ModuleId, m: &Manifest, path: &str, bytes: &[u8], res: &mut Resources) {
        let Some(textures) = res.get_mut::<Textures>() else { return };
        match textures.add_png(bytes) {
            Ok(h) => {
                let (w, hgt) = textures.size(h).unwrap_or((0, 0));
                global_info(&format!("[{GAME}] mod {}: texture {path} registered as id {} ({w}x{hgt})", m.id, h.id));
                self.shared.lock().unwrap().by_module.entry(id).or_default().push((path.to_string(), h));
            }
            Err(e) => global_warn(&format!("[{GAME}] mod {}: {path} was checked but failed to decode: {e}", m.id)),
        }
    }

    fn unload(&mut self, id: ModuleId, res: &mut Resources) {
        let Some(list) = self.shared.lock().unwrap().by_module.remove(&id) else { return };
        for (path, h) in list {
            if let Some(t) = res.get_mut::<Textures>() {
                t.remove(h);
            }
            if let Some(f) = res.get_mut::<Freed>() {
                f.0.push(h.id);
            }
            global_info(&format!("[{GAME}] texture {path} (id {}) of module {id:?} freed", h.id));
        }
    }
}

/// The engine does not know which entities a mod spawned, so entities that show a texture freed
/// by an unloaded mod are despawned here.
pub fn sweep_orphans(ctx: &mut Ctx<'_>) {
    let Some(freed) = ctx.resources.get_mut::<Freed>() else { return };
    if freed.0.is_empty() {
        return;
    }
    let ids: Vec<u32> = std::mem::take(&mut freed.0);
    let orphans: Vec<Entity> =
        ctx.world.query::<(Entity, &Sprite)>().iter().filter(|(_, s)| ids.contains(&s.texture.id)).map(|(e, _)| e).collect();
    for e in &orphans {
        ctx.commands.despawn(*e);
    }
    global_info(&format!("[{GAME}] despawned {} entities that showed freed textures", orphans.len()));
}

// ---- mod sources ----

/// A mod source over packages already in memory (a download on the web, or a test).
#[derive(Default)]
pub struct BytesSource {
    pub items: Vec<(String, Vec<u8>)>,
}

impl ModSource for BytesSource {
    fn list(&mut self) -> Vec<ModItem> {
        self.items.iter().map(|(l, b)| ModItem { label: l.clone(), body: ModBody::Package(b.clone()) }).collect()
    }
}

/// Where the packages are: `$COIN_DASH_MODS`, else `target/mods/coin_dash_mods/` next to the
/// executable's `target/<profile>/`.
#[cfg(not(target_family = "wasm"))]
pub fn mods_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("COIN_DASH_MODS").filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.parent()?.join("mods").join(GAME))
}

// ---- the base world ----

/// Textures bookkeeping, the policy and the handler, the camera, the sweep and the R / U keys.
pub fn setup(engine: &mut Engine) -> Game {
    let game = register_api(engine);
    engine.insert_resource(Freed::default());
    engine.set_mod_policy(mod_policy);
    engine.add_asset_handler(PngTextures::new(&game));
    engine.world_mut().spawn((
        Camera { zoom: CAMERA_ZOOM, clear_color: [0.02, 0.03, 0.05, 1.0], ..Camera::orthographic_2d() },
        Transform::default(),
    ));
    engine.add_system(Stage::Update, sweep_orphans);
    game
}

/// R: `reload_mods`, U: `unload_mods` (the engine logs the reports).
pub fn add_mod_keys(engine: &mut Engine) {
    engine.add_system(Stage::Update, |ctx| {
        if ctx.input.just_pressed(KeyCode::R) {
            global_info(&format!("[{GAME}] R pressed: reload_mods()"));
            ctx.commands.reload_mods();
        }
        if ctx.input.just_pressed(KeyCode::U) {
            global_info(&format!("[{GAME}] U pressed: unload_mods()"));
            ctx.commands.unload_mods();
        }
    });
}

/// `[meta]` as compact JSON for the log.
pub fn meta_text(v: &efeengine_core::efevm::api::json::Value) -> String {
    use efeengine_core::efevm::api::json::Value;
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Num(n) => n.clone(),
        Value::Str(t) => format!("{t:?}"),
        Value::Arr(a) => format!("[{}]", a.iter().map(meta_text).collect::<Vec<_>>().join(", ")),
        Value::Obj(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{k}: {}", meta_text(v))).collect::<Vec<_>>().join(", ")),
    }
}

/// Log the outcome of `load_mods`: loaded, skipped and failed, and each mod's `[meta]`.
pub fn log_report(engine: &Engine, report: &LoadReport) {
    global_info(&format!(
        "[{GAME}] load report: {} loaded, {} skipped, {} failed",
        report.loaded.len(),
        report.skipped.len(),
        report.failed.len()
    ));
    for (label, id) in &report.loaded {
        global_info(&format!("[{GAME}]   loaded {label} as {id:?}"));
        if let Some(pkg) = engine.package(*id) {
            let meta = pkg.manifest.meta.as_ref().map_or("(none)".to_string(), meta_text);
            global_info(&format!("[{GAME}]   {} {} meta: {meta}", pkg.manifest.id, pkg.manifest.version));
        }
    }
    for (label, id) in &report.skipped {
        global_info(&format!("[{GAME}]   skipped {label} (unchanged, {id:?})"));
    }
    for (label, e) in &report.failed {
        global_warn(&format!("[{GAME}]   FAILED {label}: {e}"));
    }
}

/// Native: the packages of [`mods_dir`] as the mod source, then `load_mods`.
#[cfg(not(target_family = "wasm"))]
pub fn load_mods(engine: &mut Engine) -> Option<LoadReport> {
    let dir = mods_dir()?;
    if !dir.is_dir() {
        global_warn(&format!(
            "[{GAME}] no mods at {} (run `cargo xtask mods {GAME}` or set COIN_DASH_MODS)",
            dir.display()
        ));
        return None;
    }
    global_info(&format!("[{GAME}] loading mods from {}", dir.display()));
    engine.add_mod_source(DirSource::new(&dir));
    let report = engine.load_mods();
    log_report(engine, &report);
    Some(report)
}

#[main]
fn main() {
    match Engine::new(GAME) {
        Ok(mut engine) => {
            let _game = setup(&mut engine);
            // Not while exporting the API index: the index is the game's alone, the mods are
            // packed against it.
            #[cfg(not(target_family = "wasm"))]
            if std::env::var_os("EFE_API_EXPORT").is_none() {
                load_mods(&mut engine);
            }
            #[cfg(target_family = "wasm")]
            global_warn(&format!("[{GAME}] the web build has no package download yet: feed `BytesSource` with fetched .efmod files"));
            add_mod_keys(&mut engine);
            engine.set_renderer(SpriteRenderer::new());
            global_info(&format!("[{GAME}] WASD / arrows to move, R reloads the mods, U unloads them"));
            engine.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
