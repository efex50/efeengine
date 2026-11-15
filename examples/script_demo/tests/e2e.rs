//! End to end (roadmap E5.4): the game's registrations -> `.efapi` -> assemble `.efa` against
//! it -> run under the script system, headless. Also mods, and the binary's `EFE_API_EXPORT`.

use std::path::{Path, PathBuf};

use efeengine_core::efevm::ApiIndex;
use efeengine_core::prelude::*;
use efescript::asm::{AsmOptions, assemble_with};
use script_demo::{Modules, Score, setup, spawn_world};

fn engine() -> (Engine, Score) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() })
        .expect("headless engine");
    let score = setup(&mut e);
    (e, score)
}

/// A scratch file name unique to this test process.
fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("script_demo_e2e_{}_{name}", std::process::id()))
}

/// Export the engine's index through a file, as `cargo xtask api-export` and
/// `efescript build --api` do, and parse it back.
fn exported_index(e: &Engine, name: &str) -> ApiIndex {
    let path = temp(name);
    std::fs::write(&path, e.export_api_index()).expect("write the .efapi");
    let text = std::fs::read_to_string(&path).expect("read the .efapi");
    let _ = std::fs::remove_file(&path);
    ApiIndex::from_json(&text).unwrap_or_else(|err| panic!("the exported index does not parse: {err}"))
}

fn assemble(script: &str, index: &ApiIndex) -> efeengine_core::efevm::Module {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts").join(script);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assemble_with(&src, script, &AsmOptions { api: Some(index) }).unwrap_or_else(|d| panic!("{script}: {d}"))
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

#[test]
fn mover_script_assembled_against_the_exported_index_moves_entities() {
    let (mut e, score) = engine();
    let index = exported_index(&e, "mover.efapi");
    assert_eq!(index.game, "script_demo");
    assert!(index.function("game", "score_add").is_some());

    let module = assemble("mover.efa", &index);
    // section 12 records the index the module was built against
    assert_eq!(module.data().api.as_ref().expect("section 12").hash, index.hash());

    let id = e.load_module("mover", &module.encode()).expect("load mover");
    spawn_world(&mut e, &Modules { mover: Some(id) });
    let before: Vec<f32> = e.world().query::<&Transform>().iter().map(|t| t.position.x).collect();
    assert_eq!(before.len(), 3);

    // The first frame has delta 0, so 5 frames move by 4 * (60 units/s * 1/60 s).
    e.run_frames(5, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    let mut n = 0;
    for (t, s) in e.world().query::<(&Transform, &Script)>().iter() {
        assert_eq!(s.state, 5, "the script ran every frame");
        assert!(before.iter().any(|b| (t.position.x - b - 4.0).abs() < 1e-3), "x = {}", t.position.x);
        assert_eq!(t.position.y, 0.0);
        n += 1;
    }
    assert_eq!(n, 3);
    // game.score_add(1) ran once per entity per frame
    assert_eq!(score.get(), 15);
}

#[test]
fn a_mod_exports_a_function_that_another_script_calls() {
    let (mut e, score) = engine();
    let base = exported_index(&e, "base.efapi");
    assert!(base.function("adder", "add").is_none());

    // the mod
    let adder = assemble("mod_adder.efa", &base);
    let adder_id = e
        .load_mod(&adder.encode(), ModOptions::new("adder").allow("engine.mods"))
        .expect("load mod adder");

    // re-exported index: the mod's function is there, with names and docs
    let with_mod = exported_index(&e, "with_mod.efapi");
    let f = with_mod.function("adder", "add").expect("adder.add is in the re-exported index");
    assert_eq!(f.owner, "adder");
    assert_eq!(f.doc, "Add two numbers.");
    assert_eq!(f.params.len(), 2);
    assert_eq!(f.results.len(), 1);
    println!("adder.add params: {:?}", f.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>());
    assert!(f.params.iter().all(|p| !p.name.is_empty()));
    assert_ne!(with_mod.hash(), base.hash());

    // a script that imports it can only be assembled against the new index
    let user = assemble("mod_user.efa", &with_mod);
    let user_id = e.load_module("user", &user.encode()).expect("load mod_user");
    let ent = e.world_mut().spawn((Transform::default(), Script::new(user_id, "update")));
    e.run_frames(3, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(e.world().get::<&Script>(ent).unwrap().state, 30, "state = add(state, 10) per frame");
    assert_eq!(score.get(), 3);

    // unload the mod: its function leaves the index
    e.unload_module(adder_id).expect("unload adder");
    let after = exported_index(&e, "after.efapi");
    assert!(after.function("adder", "add").is_none());
    assert_eq!(after.hash(), base.hash(), "back to the original API");
    // the script that still imports it traps cleanly instead of crashing the engine
    e.run_frames(1, 1.0 / 60.0);
    assert!(e.is_running());
}

#[test]
fn the_native_binary_writes_the_index_with_efe_api_export() {
    let out = temp("binary.efapi");
    let _ = std::fs::remove_file(&out);
    // No scripts exist next to the test binary: registrations must not depend on them.
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_native"))
        .env("EFE_API_EXPORT", &out)
        .env_remove("EFE_SCRIPT_DIR")
        .status()
        .expect("run the native binary");
    assert!(status.success(), "{status}");
    let text = std::fs::read_to_string(&out).expect("the binary wrote the .efapi");
    let _ = std::fs::remove_file(&out);
    let index = ApiIndex::from_json(&text).expect("valid .efapi");
    assert_eq!(index.game, "script_demo");
    let names = index.qualified_names();
    for want in ["game.score_add", "engine.world.get", "engine.world.set", "engine.mods.export"] {
        assert!(names.iter().any(|n| n == want), "{want} missing from {names:?}");
    }
    assert!(index.struct_type("Transform").is_some());
}
