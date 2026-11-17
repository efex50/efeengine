//! E8/E10 end to end: two real mod folders (`mods/inventory`, `mods/shop`) loaded as folders
//! with `Engine::load` (compiled against the engine's registry index, which holds the
//! inventory's provides once it is loaded) and run headless.
//!
//! Grants: besides `engine.log` (the import in the NON-root file `items.efa`), the engine
//! needs `engine.events` for any package that uses `engine.events.*`, and the shop reports
//! its results through `game.report`, so the policy grants `engine.events` to both and `game`
//! to the shop.

use std::path::{Path, PathBuf};

use efeengine_core::efa::asm::FsSource;
use efeengine_core::efa::mod_folder::{BuildLimits, build_mod, list_assets, read_mod_toml};
use efeengine_core::prelude::*;
use efeengine_core::cache::{CacheConfig, CacheDir};
use efeengine_core::mods::{Grant, ModError, ModRequest};
use efeengine_core::scripting::{LogLevel, ModuleId, ScriptError};
use script_demo::{Score, setup};

fn mod_err(e: efeengine_core::mods::LoadError) -> ModError {
    match e {
        efeengine_core::mods::LoadError::Mod(m) => m,
        other => panic!("expected a mod error, got: {other}"),
    }
}

fn mods() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("mods")
}

fn engine() -> (Engine, Score) {
    // never the platform cache directory
    let cfg = EngineConfig {
        fixed_dt: 1.0 / 60.0,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    let score = setup(&mut e);
    (e, score)
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn grant(ns: &[&str]) -> Grant {
    Grant { namespaces: ns.iter().map(|s| s.to_string()).collect(), ..Grant::default() }
}

/// The game's policy: what each mod may import.
fn policy(r: &ModRequest<'_>) -> Result<Grant, String> {
    match r.manifest.id.as_str() {
        "inventory" => Ok(grant(&["engine.log", "engine.events"])),
        "shop" => Ok(grant(&["engine.events", "game"])),
        other => Err(format!("{other} is not welcome")),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("script_demo_pkg_{}_{name}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn loaded() -> (Engine, Score, ModuleId, ModuleId) {
    let (mut e, score) = engine();
    e.set_mod_policy(policy);
    let i = e.load(mods().join("inventory")).unwrap_or_else(|err| panic!("inventory: {err}")).id();
    let s = e.load(mods().join("shop")).unwrap_or_else(|err| panic!("shop: {err}")).id();
    (e, score, i, s)
}

#[test]
fn a_use_makes_a_multi_file_module_with_qualified_names() {
    let (e, _) = engine();
    let dir = mods().join("inventory");
    let assets = list_assets(&dir).expect("assets");
    let built = build_mod(&FsSource::new(&dir), Some(&e.api_index()), &assets, &BuildLimits::default()).unwrap_or_else(|err| panic!("inventory: {err}"));
    let data = built.module.data();
    assert!(data.lines.files.len() > 1, "{:?}", data.lines.files);
    assert!(data.lines.files.iter().any(|f| f.ends_with("items.efa")), "{:?}", data.lines.files);
    assert!(data.names.funcs.iter().any(|(_, n)| n == "items.give"), "{:?}", data.names.funcs);
    assert_eq!(built.manifest.id, "inventory");
    // the asset travels in the package
    assert!(built.assets.get("items.json").is_some_and(|a| a.starts_with(b"{")));
    // the shop depends on inventory
    let shop = read_mod_toml(&std::fs::read_to_string(mods().join("shop/mod.toml")).unwrap()).unwrap();
    assert_eq!(shop.manifest.dependencies.len(), 1);
    // the loaded folder keeps its assets in the package info
    let (mut e2, _) = engine();
    e2.set_mod_policy(policy);
    let id = e2.load(&dir).unwrap().id();
    assert!(e2.package_asset(id, "items.json").is_some_and(|a| a.starts_with(b"{")));
}

#[test]
fn the_grant_covers_the_whole_package_and_the_mods_run() {
    let (mut e, score, i, s) = loaded();
    // b: the grant for inventory holds engine.log (import lives in items.efa) and not `game`
    let g = &e.package(i).unwrap().grant;
    assert!(g.namespaces.iter().any(|n| n == "engine.log"), "{g:?}");
    assert!(!g.namespaces.iter().any(|n| n == "game"));
    assert!(!e.package(s).unwrap().grant.namespaces.iter().any(|n| n == "engine.log"));

    // c: frame 1: give("sword", {id 7, count 5}) = len 5 + count 5 = 10, count copied back as 6;
    // d: frame 2: shop reads the event: 1 event, id 7, count 6
    e.run_frames(3, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(score.reports(), [10, 6, 1, 7, 6]);
    // engine.log.info of inventory ran (the message is the string it was given)
    assert!(e.scripting().log_lines().any(|l| l.text.contains("sword")));

    // d: the index
    let idx = e.api_index();
    idx.validate().unwrap();
    let f = idx.function("inventory", "give").expect("inventory.give");
    assert_eq!(f.owner, "inventory");
    assert_eq!(idx.function("shop", "last_status").expect("shop.last_status").owner, "shop");
    assert_eq!(idx.struct_type("Item").expect("struct Item").owner, "inventory");
    let ev = idx.event("inventory", "given").expect("event inventory.given");
    assert_eq!((ev.owner.as_str(), ev.payload.as_deref()), ("inventory", Some("Item")));
    assert_eq!(ev.doc, "A stack was given; the payload is the stack after the gift.");
}

#[test]
fn unloading_a_needed_mod_is_refused_and_the_cascade_removes_both() {
    let (mut e, _, i, s) = loaded();
    let base = {
        let (fresh, _) = engine();
        fresh.api_index()
    };
    match e.unload_module(i) {
        Err(ScriptError::Needed { module, by }) => {
            assert_eq!((module.as_str(), by.as_slice()), ("inventory", ["shop".to_string()].as_slice()))
        }
        other => panic!("{other:?}"),
    }
    let removed = e.unload_module_and_dependents(i).expect("cascade");
    assert_eq!(removed, [s, i]);
    let idx = e.api_index();
    for owner in ["inventory", "shop"] {
        assert!(idx.namespaces.iter().all(|n| n.name != owner && n.owner != owner), "{owner}");
        assert!(idx.functions.iter().all(|f| f.namespace != owner && f.owner != owner), "{owner}");
        assert!(idx.types.iter().all(|t| t.owner != owner), "{owner}");
        assert!(idx.events.iter().all(|v| v.namespace != owner && v.owner != owner), "{owner}");
    }
    assert_eq!(idx.hash(), base.hash(), "back to the game's own API");
    e.run_frames(1, 1.0 / 60.0);
    assert!(e.is_running());
}

#[test]
fn the_default_policy_refuses_the_whole_package_over_the_import_in_items_efa() {
    let inv = mods().join("inventory");
    // f: a fresh engine, no set_mod_policy
    let (mut fresh, _) = engine();
    match mod_err(fresh.load(&inv).unwrap_err()) {
        ModError::NotGranted { id, namespaces } => {
            assert_eq!(id, "inventory");
            assert!(namespaces.iter().any(|n| n == "engine.log"), "{namespaces:?}");
        }
        other => panic!("{other:?}"),
    }
    // a policy that grants engine.log only still refuses: engine.events is imported too
    let (mut half, _) = engine();
    half.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine.log"])));
    match mod_err(half.load(&inv).unwrap_err()) {
        ModError::NotGranted { namespaces, .. } => assert_eq!(namespaces, ["engine.events"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_mod_id_equal_to_the_game_namespace_is_refused() {
    let dir = temp_dir("clash");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("mod.toml"), "[mod]\nid = \"game\"\nversion = \"1.0.0\"\n").unwrap();
    std::fs::write(dir.join("src/main.efa"), "export fn frame(dt: f32) {\n    ret\n}\n").unwrap();
    let (mut e, _) = engine();
    e.set_mod_policy(policy);
    let r = e.load(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    match mod_err(r.unwrap_err()) {
        ModError::IdClash { id, namespace, owner } => {
            assert_eq!((id.as_str(), namespace.as_str(), owner.as_str()), ("game", "game", script_demo::GAME))
        }
        other => panic!("{other:?}"),
    }
}
