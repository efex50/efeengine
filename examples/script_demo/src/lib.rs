//! script_demo: a game with its own script API and `.efa` scripts.
//!
//! * [`setup`] registers the game namespace `game` (v1, owner `script_demo`) with
//!   `game.score_add(points) -> total`, and names the game for the API index. It must not
//!   depend on any script file: `EFE_API_EXPORT=<path>` makes `game_loop` write the index and
//!   return before any frame (`cargo xtask api-export script_demo`).
//! * [`load_scripts`] loads `mover.efvm` from the script directory (native only):
//!   `$EFE_SCRIPT_DIR` if set, else `scripts/` next to the executable (where
//!   `cargo xtask build script_demo native` puts the assembled scripts). A missing or broken
//!   script is a warning and the game runs without it.
//! * [`spawn_world`] spawns a few entities; those get a `Script` when the module is loaded.
//!
//! `mods/` holds two real mod folders (`inventory`, `shop`: `mod.toml`, `src/*.efa`, assets). They
//! are not part of the game binary: `cargo xtask mods script_demo` packs them into `.efmod`
//! files against the game's API index (and each other's), and `tests/packages_e2e.rs` packs,
//! loads and runs them headless under a game policy.
//!
//! The end-to-end test (`tests/e2e.rs`) calls the same `setup` and `spawn_world`.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};

use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::prelude::*;

pub const GAME: &str = "script_demo";

/// The game's score; `game.score_add` adds to it.
#[derive(Clone, Default)]
pub struct Score(Arc<AtomicI64>, Arc<Mutex<Vec<i64>>>);

impl Score {
    /// The values scripts passed to `game.report`, in call order.
    pub fn reports(&self) -> Vec<i64> {
        self.1.lock().unwrap().clone()
    }

    pub fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Modules the game loaded.
#[derive(Default)]
pub struct Modules {
    /// `mover.efvm`, if it was found and loaded.
    pub mover: Option<ModuleId>,
}

/// Register the game's script API. Returns the score the functions write to.
pub fn setup(engine: &mut Engine) -> Score {
    engine.set_game_name(GAME);
    engine
        .define_namespace("game", 1, GAME, "Functions of the script_demo game.")
        .expect("define namespace game");
    let score = Score::default();
    let s = score.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "score_add")
                .doc("Add `points` to the score and return the new total.")
                .param("points", ApiType::I64)
                .result("total", ApiType::I64),
            move |_c, a, r| {
                let points = a.i64(0)?;
                let total = s.0.fetch_add(points, Ordering::Relaxed) + points;
                r.set_i64(0, total)?;
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.score_add");
    let r = score.1.clone();
    engine
        .register_host_fn(
            HostFn::new("game", "report").doc("Record `v` (read back with `Score::reports`).").param("v", ApiType::I64),
            move |_c, a, _r| {
                r.lock().unwrap().push(a.i64(0)?);
                Ok(HostOk::Continue)
            },
        )
        .expect("register game.report");
    score
}

/// Spawn the demo entities; with a loaded `mover` module each gets a `Script` running `update`.
pub fn spawn_world(engine: &mut Engine, modules: &Modules) {
    for i in 0..3 {
        let e = engine
            .world_mut()
            .spawn((Transform::from_position(Vec2::new(i as f32 * 2.0, 0.0)), MeshHandle { id: 0 }));
        if let Some(m) = modules.mover {
            engine.world_mut().insert_one(e, Script::new(m, "update")).expect("entity just spawned");
        }
    }
}

/// Directory the assembled scripts are loaded from: `$EFE_SCRIPT_DIR`, else `scripts/` next to
/// the executable.
#[cfg(not(target_family = "wasm"))]
pub fn script_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("EFE_SCRIPT_DIR").filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    Some(std::env::current_exe().ok()?.parent()?.join("scripts"))
}

/// Load the game's scripts. Never fails: a missing file is a warning.
pub fn load_scripts(engine: &mut Engine) -> Modules {
    #[cfg(not(target_family = "wasm"))]
    {
        let Some(dir) = script_dir() else { return Modules::default() };
        let path = dir.join("mover.efvm");
        if !path.is_file() {
            global_warn(&format!(
                "{} not found: running without scripts (cargo xtask build script_demo native assembles them)",
                path.display()
            ));
            return Modules::default();
        }
        match engine.load_module_file(&path) {
            Ok(id) => {
                global_info(&format!("[script_demo] loaded {}", path.display()));
                return Modules { mover: Some(id) };
            }
            Err(e) => global_warn(&format!("cannot load {}: {e}", path.display())),
        }
    }
    #[cfg(target_family = "wasm")]
    let _ = engine;
    Modules::default()
}

/// Once a second, log the score and where the entities are (only the game binary adds this;
/// the tests check the numbers directly).
#[allow(dead_code)] // called from `main`, which `#[main]` leaves out of the native lib target
fn log_progress(engine: &mut Engine, score: Score) {
    let mut next = 1.0;
    engine.add_system(Stage::PostUpdate, move |ctx| {
        if ctx.time.elapsed() < next {
            return;
        }
        next += 1.0;
        let xs: Vec<String> =
            ctx.world.query_mut::<&Transform>().into_iter().map(|t| format!("{:.0}", t.position.x)).collect();
        global_info(&format!(
            "[script_demo] t={:.0}s frame {} score {} x=[{}]",
            ctx.time.elapsed(),
            ctx.time.frame(),
            score.get(),
            xs.join(", ")
        ));
    });
}

#[main]
fn main() {
    match Engine::new(GAME) {
        Ok(mut engine) => {
            let score = setup(&mut engine);
            global_info("[script_demo] registered game.score_add and game.report");
            let modules = load_scripts(&mut engine);
            spawn_world(&mut engine, &modules);
            let scripted = if modules.mover.is_some() { 3 } else { 0 };
            global_info(&format!("[script_demo] spawned 3 entities, {scripted} with the mover script"));
            log_progress(&mut engine, score);
            engine.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
