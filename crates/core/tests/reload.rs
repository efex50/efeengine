//! Mod reload and unload: content hashes and `AlreadyLoaded`, `reload_mod` (unchanged,
//! changed, cascading to dependents, failing with a restore), `reload_mods` over sources and
//! `unload_mods`. Headless; the script cache is off.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use efeengine_core::efa::asm::MemSource;
use efeengine_core::efa::mod_folder::{BuildLimits, build_mod};
use efeengine_core::efemod::{Manifest, Package};
use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::prelude::*;

type Reports = Arc<Mutex<Vec<i64>>>;

fn temp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("efe-reload-it-{}-{name}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn engine() -> (Engine, Reports) {
    let cfg = EngineConfig {
        fixed_dt: 1.0 / 60.0,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    e.define_namespace("test", 1, "testgame", "Helpers of the test game.").unwrap();
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(HostFn::new("test", "report").doc("Record `v`.").param("v", ApiType::I64), move |_c, a, _r| {
        g.lock().unwrap().push(a.i64(0)?);
        Ok(HostOk::Continue)
    })
    .unwrap();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant { namespaces: vec!["test".into()], ..Grant::default() }));
    (e, got)
}

fn take(got: &Reports) -> Vec<i64> {
    std::mem::take(&mut *got.lock().unwrap())
}

const PERM: &str = "\n[permissions]\nnamespaces = [\"test\"]\n";

fn toml(id: &str, extra: &str) -> String {
    format!("[mod]\nid = \"{id}\"\nversion = \"1.0.0\"\n{extra}")
}

/// A mod whose `init` reports `n`.
fn reporter(id: &str, n: i64) -> MemSource {
    let main = format!("import test.report\nexport fn init() {{\n    reg x\n    const.i64 x, {n}\n    call test.report(x)\n    ret\n}}\n");
    MemSource::new().with("mod.toml", &toml(id, PERM)).with("src/main.efa", &main)
}

fn no_assets() -> Vec<String> {
    Vec::new()
}

fn load_folder(e: &mut Engine, files: &MemSource) -> Result<Loaded, LoadError> {
    e.load_input(LoadInput::Folder { label: "f", files, assets: &no_assets() })
}

fn reload_folder(e: &mut Engine, files: &MemSource) -> Result<Reloaded, LoadError> {
    e.reload_mod_input(LoadInput::Folder { label: "f", files, assets: &no_assets() })
}

fn mod_err(e: LoadError) -> ModError {
    match e {
        LoadError::Mod(m) => m,
        other => panic!("expected a mod error, got: {other}"),
    }
}

fn efmod_of(e: &Engine, files: &MemSource) -> Vec<u8> {
    let index = e.api_index();
    let built = build_mod(files, Some(&index), &[], &BuildLimits::default()).unwrap_or_else(|e| panic!("{e}"));
    Package::write(&built.manifest, &built.module.encode(), &built.assets).unwrap()
}

/// `base` provides `twice` (`x + x`, or `x * 3` as version 2, or a two-argument one as 3);
/// `app` depends on it and reports `twice(21)` from `init`.
fn base(version: u32) -> MemSource {
    let body = match version {
        1 => "fn twice(x: i64) -> i64 {\n    reg r\n    i64.add r, x, x\n    ret r\n}\nprovide twice(x: i64) -> (r: i64) = twice, \"Double x.\"\n",
        2 => "fn twice(x: i64) -> i64 {\n    reg t, r\n    i64.add t, x, x\n    i64.add r, t, x\n    ret r\n}\nprovide twice(x: i64) -> (r: i64) = twice, \"Triple x.\"\n",
        _ => "fn twice(x: i64, y: i64) -> i64 {\n    reg r\n    i64.add r, x, y\n    ret r\n}\nprovide twice(x: i64, y: i64) -> (r: i64) = twice, \"Add.\"\n",
    };
    MemSource::new().with("mod.toml", &toml("base", "")).with("src/main.efa", body)
}

fn app() -> MemSource {
    let main = "import base.twice\nimport test.report\nexport fn init() {\n    reg x, r\n    const.i64 x, 21\n    call base.twice(x) -> r\n    call test.report(r)\n    ret\n}\n";
    MemSource::new().with("mod.toml", &toml("app", &format!("\n[dependencies]\nbase = \"^1\"\n{PERM}"))).with("src/main.efa", main)
}

#[test]
fn loading_a_loaded_mod_is_already_loaded() {
    let (mut e, got) = engine();
    let pkg = efmod_of(&e, &reporter("pk", 1));
    let id = e.load_input(LoadInput::Package { label: "pk.efmod", bytes: &pkg }).unwrap().id();
    assert_eq!(take(&got), [1]);
    let info = e.package(id).unwrap();
    assert_eq!(info.hash.len(), 64);
    assert_eq!(info.source, "pk.efmod");

    let err = mod_err(e.load_input(LoadInput::Package { label: "pk.efmod", bytes: &pkg }).unwrap_err());
    assert!(matches!(err, ModError::AlreadyLoaded { ref id, module, same_contents: true } if id == "pk" && module == info_id(&e, "pk")), "{err:?}");
    assert_eq!(err.to_string(), "mod `pk` is already loaded");

    let other = efmod_of(&e, &reporter("pk", 2));
    let err = mod_err(e.load_input(LoadInput::Package { label: "pk.efmod", bytes: &other }).unwrap_err());
    assert!(matches!(err, ModError::AlreadyLoaded { same_contents: false, .. }), "{err:?}");
    assert_eq!(err.to_string(), "mod `pk` is already loaded (contents changed; use reload_mod)");
    assert_eq!(take(&got), Vec::<i64>::new(), "nothing ran");

    // folders: same files hash the same, any changed file differs
    let f = reporter("fo", 5);
    load_folder(&mut e, &f).unwrap();
    let err = mod_err(load_folder(&mut e, &f).unwrap_err());
    assert!(matches!(err, ModError::AlreadyLoaded { same_contents: true, .. }));
    let err = mod_err(load_folder(&mut e, &reporter("fo", 6)).unwrap_err());
    assert!(matches!(err, ModError::AlreadyLoaded { same_contents: false, .. }));
    let f2 = f.clone().with("README.txt", "docs count as content");
    let err = mod_err(load_folder(&mut e, &f2).unwrap_err());
    assert!(matches!(err, ModError::AlreadyLoaded { same_contents: false, .. }));
}

fn info_id(e: &Engine, name: &str) -> ModuleId {
    e.scripting().module_id(name).unwrap()
}

/// A source whose folders the test edits between calls.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<BTreeMap<String, MemSource>>>);

impl Shared {
    fn set(&self, label: &str, files: MemSource) {
        self.0.lock().unwrap().insert(label.to_string(), files);
    }
    fn remove(&self, label: &str) {
        self.0.lock().unwrap().remove(label);
    }
}

impl ModSource for Shared {
    fn list(&mut self) -> Vec<ModItem> {
        let map = self.0.lock().unwrap();
        map.iter().map(|(l, f)| ModItem { label: l.clone(), body: ModBody::Folder { files: Box::new(f.clone()), assets: Vec::new() } }).collect()
    }
}

#[test]
fn a_second_load_mods_skips_what_is_loaded() {
    let (mut e, got) = engine();
    let src = Shared::default();
    src.set("a", reporter("a", 1));
    src.set("b", reporter("b", 2));
    e.add_mod_source(src.clone());
    let r = e.load_mods();
    assert_eq!((r.loaded.len(), r.failed.len(), r.skipped.len()), (2, 0, 0), "{:?}", r.failed);
    assert_eq!(take(&got), [1, 2]);

    let r = e.load_mods();
    assert!(r.failed.is_empty() && r.loaded.is_empty(), "{:?}", r.failed);
    let skipped: Vec<&str> = r.skipped.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(skipped, ["a", "b"]);
    assert_eq!(take(&got), Vec::<i64>::new(), "nothing ran again");

    // changed contents are an error for load_mods (reload_mods replaces them)
    src.set("a", reporter("a", 10));
    let r = e.load_mods();
    assert!(matches!(&r.failed[..], [(l, ModError::AlreadyLoaded { same_contents: false, .. })] if l == "a"), "{:?}", r.failed);
    assert_eq!(r.skipped.len(), 1);
}

#[test]
fn reload_mod_is_unchanged_then_replaces_by_path() {
    let (mut e, got) = engine();
    let d = temp("path");
    let w = |n: i64| {
        std::fs::create_dir_all(d.join("m/src")).unwrap();
        std::fs::write(d.join("m/mod.toml"), toml("m", PERM)).unwrap();
        std::fs::write(
            d.join("m/src/main.efa"),
            format!("import test.report\nexport fn init() {{\n    reg x\n    const.i64 x, {n}\n    call test.report(x)\n    ret\n}}\n"),
        )
        .unwrap();
    };
    w(1);
    // not loaded yet: reload_mod loads
    let r = e.reload_mod(d.join("m")).unwrap();
    assert!(matches!(r, Reloaded::Loaded(_)));
    let old = r.id();
    assert_eq!(take(&got), [1]);
    // the same files: unchanged, nothing runs
    assert_eq!(e.reload_mod(d.join("m")).unwrap(), Reloaded::Unchanged(old));
    assert!(matches!(mod_err(e.load(d.join("m")).unwrap_err()), ModError::AlreadyLoaded { same_contents: true, .. }));
    assert_eq!(take(&got), Vec::<i64>::new());
    // changed: the new code runs
    w(2);
    let r = e.reload_mod(d.join("m")).unwrap();
    let Reloaded::Replaced { id, dependents } = r else { panic!("{r:?}") };
    assert!(dependents.is_empty());
    assert_ne!(id, old);
    assert!(!e.scripting().is_loaded(old) && e.scripting().is_loaded(id));
    assert_eq!(take(&got), [2]);
    assert_eq!(e.reload_mod(d.join("m")).unwrap(), Reloaded::Unchanged(id));
    // a game module is not a mod
    std::fs::write(d.join("g.efa"), "export fn init() {\n    ret\n}\n").unwrap();
    assert!(matches!(e.reload_mod(d.join("g.efa")), Err(LoadError::NotAMod(_))));
    let _ = std::fs::remove_dir_all(&d);

    // a package, through reload_mod_input
    let p1 = efmod_of(&e, &reporter("pk", 1));
    let p2 = efmod_of(&e, &reporter("pk", 2));
    assert!(matches!(e.reload_mod_input(LoadInput::Package { label: "pk.efmod", bytes: &p1 }).unwrap(), Reloaded::Loaded(_)));
    assert!(matches!(e.reload_mod_input(LoadInput::Package { label: "pk.efmod", bytes: &p1 }).unwrap(), Reloaded::Unchanged(_)));
    assert!(matches!(e.reload_mod_input(LoadInput::Package { label: "pk.efmod", bytes: &p2 }).unwrap(), Reloaded::Replaced { .. }));
    assert_eq!(take(&got), [1, 2]);
}

/// Handler that records which module ids were committed and unloaded.
#[derive(Clone, Default)]
struct Log {
    commits: Arc<Mutex<Vec<ModuleId>>>,
    unloads: Arc<Mutex<Vec<ModuleId>>>,
}

struct BinHandler(Log);

impl AssetHandler for BinHandler {
    fn matches(&self, path: &str) -> bool {
        path.ends_with(".bin")
    }
    fn check(&mut self, _m: &Manifest, _path: &str, _bytes: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn commit(&mut self, id: ModuleId, _m: &Manifest, _path: &str, _bytes: &[u8], _res: &mut Resources) {
        self.0.commits.lock().unwrap().push(id);
    }
    fn unload(&mut self, id: ModuleId, _res: &mut Resources) {
        self.0.unloads.lock().unwrap().push(id);
    }
}

fn with_asset(f: MemSource) -> (MemSource, Vec<String>) {
    (f.with_bytes("assets/d.bin", b"data"), vec!["d.bin".to_string()])
}

fn load_with_assets(e: &mut Engine, f: MemSource) -> ModuleId {
    let (f, assets) = with_asset(f);
    e.load_input(LoadInput::Folder { label: "f", files: &f, assets: &assets }).unwrap().id()
}

#[test]
fn a_reload_cascades_to_dependents() {
    let (mut e, got) = engine();
    let log = Log::default();
    e.add_asset_handler(BinHandler(log.clone()));
    let base_id = load_with_assets(&mut e, base(1));
    let app_id = load_with_assets(&mut e, app());
    assert_eq!(take(&got), [42]);

    let (b2, assets) = with_asset(base(2));
    let r = e.reload_mod_input(LoadInput::Folder { label: "f", files: &b2, assets: &assets }).unwrap();
    let Reloaded::Replaced { id, dependents } = r else { panic!("{r:?}") };
    assert_eq!(dependents, ["app"]);
    assert_eq!(take(&got), [63], "the dependent runs again against the new base");
    assert!(!e.scripting().is_loaded(base_id) && !e.scripting().is_loaded(app_id));
    assert_eq!(info_id(&e, "base"), id);
    assert_ne!(info_id(&e, "app"), app_id);
    // dependents unloaded first (asset unload), then the new versions committed
    assert_eq!(*log.unloads.lock().unwrap(), [app_id, base_id]);
    assert_eq!(log.commits.lock().unwrap().len(), 4);
    assert!(e.package(id).unwrap().assets.is_empty(), "handled assets are not kept in the package");
}

#[test]
fn a_failed_reload_restores_the_previous_version() {
    let (mut e, got) = engine();
    let log = Log::default();
    e.add_asset_handler(BinHandler(log.clone()));
    load_with_assets(&mut e, base(1));
    load_with_assets(&mut e, app());
    let hash = e.package(info_id(&e, "base")).unwrap().hash.clone();
    assert_eq!(take(&got), [42]);

    // 1. a new version that does not compile: nothing is touched
    let broken = base(1).with("src/main.efa", "oops oops\n");
    let (broken, assets) = with_asset(broken);
    let err = mod_err(e.reload_mod_input(LoadInput::Folder { label: "f", files: &broken, assets: &assets }).unwrap_err());
    assert!(matches!(err, ModError::ReloadFailed { ref id, restored: true, .. } if id == "base"), "{err:?}");
    assert!(err.to_string().contains("the previous version is loaded again"));
    assert_eq!(take(&got), Vec::<i64>::new(), "nothing was unloaded or re-run");
    assert!(log.unloads.lock().unwrap().is_empty());

    // 2. a new version the dependent cannot link against: swapped, then rolled back
    let (b3, assets) = with_asset(base(3));
    let err = mod_err(e.reload_mod_input(LoadInput::Folder { label: "f", files: &b3, assets: &assets }).unwrap_err());
    assert!(matches!(err, ModError::ReloadFailed { ref id, restored: true, .. } if id == "base"), "{err:?}");
    assert!(err.to_string().contains("app"), "{err}");
    assert_eq!(take(&got), [42], "the old versions run again");
    assert_eq!(e.package(info_id(&e, "base")).unwrap().hash, hash, "the old base is back");
    assert!(e.scripting().module_id("app").is_some());
    assert!(e.scripting().log_lines().any(|l| l.text.contains("reload failed")));
    // the handlers saw balanced commits and unloads: everything committed is loaded now
    let commits = log.commits.lock().unwrap().len();
    let unloads = log.unloads.lock().unwrap().len();
    assert_eq!(commits - unloads, 2);

    // and the engine still reloads fine afterwards
    let (b2, assets) = with_asset(base(2));
    assert!(matches!(e.reload_mod_input(LoadInput::Folder { label: "f", files: &b2, assets: &assets }).unwrap(), Reloaded::Replaced { .. }));
    assert_eq!(take(&got), [63]);
}

#[test]
fn a_game_module_that_uses_a_mod_blocks_its_reload() {
    let (mut e, _) = engine();
    load_folder(&mut e, &base(1)).unwrap();
    let files = MemSource::new().with(
        "main.efa",
        "import base.twice\nexport fn init() {\n    reg x, r\n    const.i64 x, 1\n    call base.twice(x) -> r\n    ret\n}\n",
    );
    e.load_input(LoadInput::Efa { name: "game", files: &files, root: "main.efa" }).unwrap();
    let err = mod_err(reload_folder(&mut e, &base(2)).unwrap_err());
    assert!(matches!(err, ModError::ReloadFailed { restored: true, .. }), "{err:?}");
    assert!(err.to_string().contains("game"), "{err}");
    assert!(e.scripting().module_id("base").is_some() && e.scripting().module_id("game").is_some());
    // unload_mods cannot take `base` either
    let r = e.unload_mods();
    assert!(r.unloaded.is_empty() && matches!(&r.failed[..], [(n, _)] if n == "base"), "{r:?}");
}

#[test]
fn reload_mods_picks_up_added_changed_and_removed_mods() {
    let (mut e, got) = engine();
    let src = Shared::default();
    src.set("a", reporter("a", 1));
    src.set("b", reporter("b", 2));
    src.set("same", reporter("same", 3));
    e.add_mod_source(src.clone());
    let r = e.reload_mods();
    assert_eq!(r.loaded.len(), 3, "{:?}", r.failed);
    assert_eq!(take(&got), [1, 2, 3]);
    let (a_old, b_old) = (info_id(&e, "a"), info_id(&e, "b"));

    src.set("a", reporter("a", 10)); // changed
    src.remove("b"); // removed
    src.set("c", reporter("c", 4)); // added
    let r = e.reload_mods();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.loaded.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["c"]);
    assert_eq!(r.reloaded.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["a"]);
    assert_eq!(r.skipped.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["same"]);
    assert_eq!(r.unloaded, ["b"]);
    let mut ran = take(&got);
    ran.sort();
    assert_eq!(ran, [4, 10], "only the new and the changed mods ran");
    assert!(!e.scripting().is_loaded(a_old) && !e.scripting().is_loaded(b_old));
    assert!(e.scripting().module_id("b").is_none());
    assert!(e.scripting().module_id("a").is_some() && e.scripting().module_id("c").is_some());

    // nothing changed: everything is skipped
    let r = e.reload_mods();
    assert!(r.loaded.is_empty() && r.reloaded.is_empty() && r.unloaded.is_empty() && r.failed.is_empty());
    assert_eq!(r.skipped.len(), 3);
}

#[test]
fn reload_mods_changes_a_mod_and_its_dependent_together() {
    let (mut e, got) = engine();
    let src = Shared::default();
    src.set("base", base(1));
    src.set("app", app());
    e.add_mod_source(src.clone());
    let r = e.reload_mods();
    assert_eq!(r.loaded.len(), 2, "{:?}", r.failed);
    assert_eq!(take(&got), [42]);

    // an API break in `base` together with the matching change in `app`
    src.set("base", base(3));
    let app3 = "import base.twice\nimport test.report\nexport fn init() {\n    reg x, y, r\n    const.i64 x, 40\n    const.i64 y, 2\n    call base.twice(x, y) -> r\n    call test.report(r)\n    ret\n}\n";
    src.set("app", app().with("src/main.efa", app3));
    let r = e.reload_mods();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.reloaded.len(), 2, "{:?}", r.reloaded);
    assert_eq!(take(&got), [42]);

    // a bad update rolls back, and its failure is in the report
    src.set("base", base(1).with("src/main.efa", "oops oops\n"));
    let r = e.reload_mods();
    assert!(matches!(&r.failed[..], [(l, ModError::ReloadFailed { restored: true, .. })] if l == "base"), "{:?}", r.failed);
    assert!(e.scripting().module_id("base").is_some() && e.scripting().module_id("app").is_some());
}

#[test]
fn unload_mods_runs_dependents_first_and_keeps_game_modules() {
    let (mut e, _) = engine();
    let log = Log::default();
    e.add_asset_handler(BinHandler(log.clone()));
    let game = MemSource::new().with("main.efa", "export fn init() {\n    ret\n}\n");
    let game_id = e.load_input(LoadInput::Efa { name: "game", files: &game, root: "main.efa" }).unwrap().id();
    let base_id = load_with_assets(&mut e, base(1));
    let app_id = load_with_assets(&mut e, app());
    let solo = efmod_of(&e, &reporter("solo", 7));
    let solo_id = e.load_input(LoadInput::Package { label: "solo.efmod", bytes: &solo }).unwrap().id();

    let r = e.unload_mods();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.unloaded, ["solo", "app", "base"], "latest loaded first, dependents before dependencies");
    assert_eq!(*log.unloads.lock().unwrap(), [app_id, base_id], "asset handlers saw the unloads in that order");
    assert!(e.scripting().is_loaded(game_id));
    for id in [base_id, app_id, solo_id] {
        assert!(!e.scripting().is_loaded(id));
    }
    assert!(e.unload_mods().unloaded.is_empty());
    // they can be loaded again
    load_folder(&mut e, &base(1)).unwrap();
}
