//! E8 end to end: two real mod folders (`mods/inventory`, `mods/shop`) packed with
//! `efescript::pack::pack_dir` against the game's index (and, for the shop, the inventory
//! package's index too), loaded as `.efmod` packages and run headless.
//!
//! Grants: besides `engine.log` (the import in the NON-root file `items.efa`), the engine
//! needs `engine.events` for any package that uses `engine.events.*`, and the shop reports
//! its results through `game.report`, so the policy grants `engine.events` to both and `game`
//! to the shop.

use std::path::{Path, PathBuf};

use efeengine_core::efemod::{Package, ReadLimits};
use efeengine_core::efevm::ApiIndex;
use efeengine_core::prelude::*;
use efescript::pack::{Packed, pack_dir, read_api_input};
use script_demo::{Score, setup};

fn mods() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("mods")
}

fn engine() -> (Engine, Score) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() })
        .expect("headless engine");
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

/// Pack both mods: inventory against the game index, shop against the game index and the
/// written inventory `.efmod` (two `--api` inputs).
fn build(game: &ApiIndex) -> (Packed, Packed) {
    let inv = pack_dir(&mods().join("inventory"), &[game]).unwrap_or_else(|e| panic!("inventory: {e}"));
    let dir = temp_dir("inv");
    let file = dir.join(&inv.file_name);
    std::fs::write(&file, &inv.bytes).unwrap();
    let inv_index = read_api_input(&file).expect("inventory index from the .efmod");
    let _ = std::fs::remove_dir_all(&dir);
    let shop = pack_dir(&mods().join("shop"), &[game, &inv_index]).unwrap_or_else(|e| panic!("shop: {e}"));
    (inv, shop)
}

fn loaded() -> (Engine, Score, ModuleId, ModuleId, Packed, Packed) {
    let (mut e, score) = engine();
    let game = e.api_index();
    let (inv, shop) = build(&game);
    e.set_mod_policy(policy);
    let i = e.load_package(&inv.bytes).expect("load inventory");
    let s = e.load_package(&shop.bytes).expect("load shop");
    (e, score, i, s, inv, shop)
}

#[test]
fn a_use_makes_a_multi_file_module_with_qualified_names() {
    let (e, _) = engine();
    let (inv, shop) = build(&e.api_index());
    let data = inv.module.data();
    assert!(data.lines.files.len() > 1, "{:?}", data.lines.files);
    assert!(data.lines.files.iter().any(|f| f.ends_with("items.efa")), "{:?}", data.lines.files);
    assert!(data.names.funcs.iter().any(|(_, n)| n == "items.give"), "{:?}", data.names.funcs);
    assert_eq!(inv.manifest.id, "inventory");
    assert_eq!(inv.file_name, "inventory-1.0.0.efmod");
    // the asset travels in the package
    let pkg = Package::read(&inv.bytes, &ReadLimits::default()).unwrap();
    assert!(pkg.asset("items.json").is_some_and(|a| a.starts_with(b"{")));
    // the shop depends on inventory
    assert_eq!(shop.manifest.dependencies.len(), 1);
}

#[test]
fn the_grant_covers_the_whole_package_and_the_mods_run() {
    let (mut e, score, i, s, _, _) = loaded();
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
    let (mut e, _, i, s, _, _) = loaded();
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
    let (e, _) = engine();
    let (inv, _) = build(&e.api_index());
    // f: a fresh engine, no set_mod_policy
    let (mut fresh, _) = engine();
    match fresh.load_package(&inv.bytes).unwrap_err() {
        ModError::NotGranted { id, namespaces } => {
            assert_eq!(id, "inventory");
            assert!(namespaces.iter().any(|n| n == "engine.log"), "{namespaces:?}");
        }
        other => panic!("{other:?}"),
    }
    // a policy that grants engine.log only still refuses: engine.events is imported too
    let (mut half, _) = engine();
    half.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine.log"])));
    match half.load_package(&inv.bytes).unwrap_err() {
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
    let p = pack_dir(&dir, &[&e.api_index()]).unwrap_or_else(|err| panic!("{err}"));
    let _ = std::fs::remove_dir_all(&dir);
    e.set_mod_policy(policy);
    match e.load_package(&p.bytes).unwrap_err() {
        ModError::IdClash { id, namespace, owner } => {
            assert_eq!((id.as_str(), namespace.as_str(), owner.as_str()), ("game", "game", script_demo::GAME))
        }
        other => panic!("{other:?}"),
    }
}
