//! E10 loading: every `LoadInput` kind, `Engine::load`, the script cache (memory and disk),
//! folder mods with dependencies, asset handlers, `[meta]`, the default policy and the build
//! limits. Headless; the disk cache is always `Off` or a unique temp directory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use efeengine_core::efa::asm::MemSource;
use efeengine_core::efa::mod_folder::{BuildLimits, build_mod};
use efeengine_core::efemod::{Manifest, Package};
use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::efevm::api::json::Value;
use efeengine_core::prelude::*;
use efeengine_core::cache::{CacheConfig};
use efeengine_core::mods::{DirSource};
use efeengine_core::ecs::{Resources};
use efeengine_core::mods::{AssetHandler, Grant, LoadError, LoadInput, Loaded, ModError, ModRequest};
use efeengine_core::cache::{CacheDir};
use efeengine_core::scripting::{ModuleId};

type Reports = Arc<Mutex<Vec<i64>>>;

fn temp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("efe-load-it-{}-{name}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn engine_with(disk: CacheDir, extra_ns: bool) -> (Engine, Reports) {
    let cfg = EngineConfig {
        fixed_dt: 1.0 / 60.0,
        script_cache: CacheConfig { disk, ..Default::default() },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    e.define_namespace("test", 1, "testgame", "Helpers of the test game.").unwrap();
    if extra_ns {
        e.define_namespace("extra", 1, "testgame", "Extra namespace.").unwrap();
    }
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(HostFn::new("test", "report").doc("Record `v`.").param("v", ApiType::I64), move |_c, a, _r| {
        g.lock().unwrap().push(a.i64(0)?);
        Ok(HostOk::Continue)
    })
    .unwrap();
    (e, got)
}

fn engine() -> (Engine, Reports) {
    engine_with(CacheDir::Off, false)
}

fn grant_test(e: &mut Engine) {
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant { namespaces: vec!["test".into()], ..Grant::default() }));
}

fn mod_err(e: LoadError) -> ModError {
    match e {
        LoadError::Mod(m) => m,
        other => panic!("expected a mod error, got: {other}"),
    }
}

const REPORT42: &str = "import test.report\nexport fn init() {\n    reg x\n    const.i64 x, 42\n    call test.report(x)\n    ret\n}\n";

fn toml(id: &str, extra: &str) -> String {
    format!("[mod]\nid = \"{id}\"\nversion = \"1.0.0\"\n{extra}")
}

fn folder(id: &str, extra_toml: &str, main: &str) -> MemSource {
    MemSource::new().with("mod.toml", &toml(id, extra_toml)).with("src/main.efa", main)
}

const TEST_PERM: &str = "\n[permissions]\nnamespaces = [\"test\"]\n";

#[test]
fn efa_input_loads_and_runs_init() {
    let (mut e, got) = engine();
    let files = MemSource::new().with("main.efa", REPORT42);
    let l = e.load_input(LoadInput::Efa { name: "script", files: &files, root: "main.efa" }).expect("load efa");
    assert!(matches!(l, Loaded::Module(_)));
    assert_eq!(*got.lock().unwrap(), [42]);
}

/// A compiled module of `src`, assembled against the engine's index.
fn efvm_of(e: &Engine, src: &str) -> Vec<u8> {
    let index = e.api_index();
    let opts = efeengine_core::efa::asm::AsmOptions { api: Some(&index), ..Default::default() };
    efeengine_core::efa::asm::assemble_with(src, "a.efa", &opts).unwrap_or_else(|d| panic!("{d}")).encode()
}

#[test]
fn efvm_input_loads_and_runs_init() {
    let (mut e, got) = engine();
    let bytes = efvm_of(&e, REPORT42);
    let l = e.load_input(LoadInput::Efvm { name: "bin", bytes: &bytes }).expect("load efvm");
    assert!(matches!(l, Loaded::Module(_)));
    assert_eq!(*got.lock().unwrap(), [42]);
}

/// A `.efmod` built from a folder in memory (no engine involved).
fn efmod_of(files: &MemSource, assets: &[&str]) -> Vec<u8> {
    let assets: Vec<String> = assets.iter().map(|s| s.to_string()).collect();
    let built = build_mod(files, None, &assets, &BuildLimits::default()).unwrap_or_else(|e| panic!("{e}"));
    Package::write(&built.manifest, &built.module.encode(), &built.assets).unwrap()
}

#[test]
fn package_input_loads() {
    let (mut e, _) = engine();
    let bytes = efmod_of(&folder("pk", "", "export fn init() {\n    ret\n}\n"), &[]);
    let l = e.load_input(LoadInput::Package { label: "pk.efmod", bytes: &bytes }).expect("load package");
    assert!(matches!(l, Loaded::Package(_)));
    assert_eq!(e.package(l.id()).unwrap().manifest.id, "pk");
}

#[test]
fn folder_input_loads_with_assets_and_runs_init() {
    let (mut e, got) = engine();
    grant_test(&mut e);
    let files = folder("fo", TEST_PERM, REPORT42).with_bytes("assets/data/a.bin", b"AAA");
    let assets = vec!["data/a.bin".to_string()];
    let l = e.load_input(LoadInput::Folder { label: "fo", files: &files, assets: &assets }).expect("load folder");
    assert!(matches!(l, Loaded::Package(_)));
    assert_eq!(e.package_asset(l.id(), "data/a.bin"), Some(&b"AAA"[..]));
    assert_eq!(*got.lock().unwrap(), [42]);
}

#[test]
fn load_by_path_dispatches_on_extension_and_folder() {
    let (mut e, got) = engine();
    grant_test(&mut e);
    let d = temp("bypath");
    std::fs::write(d.join("s1.efa"), REPORT42).unwrap();
    std::fs::write(d.join("s2.efvm"), efvm_of(&e, REPORT42)).unwrap();
    std::fs::write(d.join("p.efmod"), efmod_of(&folder("pk", "", "export fn init() {\n    ret\n}\n"), &[])).unwrap();
    let fo = d.join("fo");
    std::fs::create_dir_all(fo.join("src")).unwrap();
    std::fs::create_dir_all(fo.join("assets")).unwrap();
    std::fs::write(fo.join("mod.toml"), toml("fo", TEST_PERM)).unwrap();
    std::fs::write(fo.join("src/main.efa"), REPORT42).unwrap();
    std::fs::write(fo.join("assets/x.txt"), "hello").unwrap();

    assert!(matches!(e.load(d.join("s1.efa")).unwrap(), Loaded::Module(_)));
    assert!(matches!(e.load(d.join("s2.efvm")).unwrap(), Loaded::Module(_)));
    assert!(matches!(e.load(d.join("p.efmod")).unwrap(), Loaded::Package(_)));
    let f = e.load(&fo).unwrap();
    assert!(matches!(f, Loaded::Package(_)));
    assert_eq!(e.package_asset(f.id(), "x.txt"), Some(&b"hello"[..]));
    assert_eq!(*got.lock().unwrap(), [42, 42, 42]);
    assert!(e.scripting().module_id("s1").is_some() && e.scripting().module_id("s2").is_some());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unknown_kind_is_refused() {
    let (mut e, _) = engine();
    let d = temp("unknown");
    std::fs::write(d.join("x.txt"), "x").unwrap();
    std::fs::create_dir_all(d.join("plain")).unwrap();
    assert!(matches!(e.load(d.join("x.txt")), Err(LoadError::UnknownKind(_))));
    assert!(matches!(e.load(d.join("plain")), Err(LoadError::UnknownKind(_))));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_memory_cache_hit_skips_decoding() {
    let (mut e, _) = engine();
    let bytes = efvm_of(&e, "export fn init() {\n    ret\n}\n");
    let a = e.load_input(LoadInput::Efvm { name: "a", bytes: &bytes }).unwrap().id();
    assert_eq!(e.cache().stats().decodes, 1);
    e.unload_module(a).unwrap();
    e.load_input(LoadInput::Efvm { name: "a", bytes: &bytes }).unwrap();
    e.load_input(LoadInput::Efvm { name: "b", bytes: &bytes }).unwrap();
    let s = e.cache().stats();
    assert_eq!(s.decodes, 1, "{s:?}");
    assert!(s.mem_hits >= 2, "{s:?}");
}

const LIB_MAIN: &str = "use \"lib.efa\" as l\nexport fn init() {\n    reg x\n    i64.add x, x, l.LIMIT\n    ret\n}\n";

fn lib_files(limit: u32) -> MemSource {
    MemSource::new().with("main.efa", LIB_MAIN).with("lib.efa", &format!("pub const LIMIT = {limit}\n"))
}

fn load_efa(e: &mut Engine, name: &str, files: &MemSource) {
    e.load_input(LoadInput::Efa { name, files, root: "main.efa" }).unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn disk_cache_miss_then_hit_then_miss_after_editing_a_used_file() {
    let dir = temp("disk");
    let (mut a, _) = engine_with(CacheDir::Path(dir.clone()), false);
    load_efa(&mut a, "s", &lib_files(100));
    let s = a.cache().stats();
    assert_eq!((s.compiles, s.disk_hits), (1, 0), "{s:?}");

    let (mut b, _) = engine_with(CacheDir::Path(dir.clone()), false);
    load_efa(&mut b, "s", &lib_files(100));
    let s = b.cache().stats();
    assert_eq!((s.compiles, s.disk_hits), (0, 1), "{s:?}");

    let (mut c, _) = engine_with(CacheDir::Path(dir.clone()), false);
    load_efa(&mut c, "s", &lib_files(200));
    let s = c.cache().stats();
    assert_eq!((s.compiles, s.disk_hits), (1, 0), "{s:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_api_index_invalidates_the_disk_cache() {
    let dir = temp("api");
    let (mut a, _) = engine_with(CacheDir::Path(dir.clone()), false);
    load_efa(&mut a, "s", &lib_files(100));
    let (mut b, _) = engine_with(CacheDir::Path(dir.clone()), true);
    load_efa(&mut b, "s", &lib_files(100));
    let s = b.cache().stats();
    assert_eq!((s.compiles, s.disk_hits), (1, 0), "{s:?}");
    let (mut c, _) = engine_with(CacheDir::Path(dir.clone()), true);
    load_efa(&mut c, "s", &lib_files(100));
    assert_eq!(c.cache().stats().disk_hits, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_mod_imports_a_function_of_a_packaged_mod() {
    let base = folder(
        "base",
        "",
        "fn twice(x: i64) -> i64 {\n    reg r\n    i64.add r, x, x\n    ret r\n}\nprovide twice(x: i64) -> (r: i64) = twice, \"Double x.\"\n",
    );
    let app_main = "import base.twice\nimport test.report\nexport fn init() {\n    reg x, r\n    const.i64 x, 21\n    call base.twice(x) -> r\n    call test.report(r)\n    ret\n}\n";
    let d = temp("deps");
    std::fs::write(d.join("base-1.0.0.efmod"), efmod_of(&base, &[])).unwrap();
    let app = d.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(app.join("mod.toml"), toml("app", &format!("\n[dependencies]\nbase = \"^1\"\n{TEST_PERM}"))).unwrap();
    std::fs::write(app.join("src/main.efa"), app_main).unwrap();

    let (mut e, got) = engine();
    grant_test(&mut e);
    e.add_mod_source(DirSource::new(&d));
    let report = e.load_mods();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.loaded.len(), 2);
    assert_eq!(report.loaded[0].0, "base-1.0.0.efmod", "the dependency loads first");
    assert_eq!(*got.lock().unwrap(), [42]);
    let _ = std::fs::remove_dir_all(&d);
}

/// Counts what an asset handler was asked; refuses `bad.bin`.
#[derive(Clone, Default)]
struct Counts {
    commits: Arc<AtomicUsize>,
    unloads: Arc<AtomicUsize>,
}

struct BinHandler(Counts);

impl AssetHandler for BinHandler {
    fn matches(&self, path: &str) -> bool {
        path.ends_with(".bin")
    }
    fn check(&mut self, _m: &Manifest, _path: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes == b"bad" { Err("refused".into()) } else { Ok(()) }
    }
    fn commit(&mut self, _id: ModuleId, _m: &Manifest, _path: &str, _bytes: &[u8], _res: &mut Resources) {
        self.0.commits.fetch_add(1, Ordering::SeqCst);
    }
    fn unload(&mut self, _id: ModuleId, _res: &mut Resources) {
        self.0.unloads.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_rejected_asset_rejects_the_whole_package() {
    let (mut e, _) = engine();
    let counts = Counts::default();
    e.add_asset_handler(BinHandler(counts.clone()));
    let main = "export fn init() {\n    ret\n}\n";
    let files = folder("am", "", main).with_bytes("assets/a_ok.bin", b"ok").with_bytes("assets/z_bad.bin", b"bad");
    let assets = vec!["a_ok.bin".to_string(), "z_bad.bin".to_string()];
    let err = e.load_input(LoadInput::Folder { label: "am", files: &files, assets: &assets }).unwrap_err();
    match mod_err(err) {
        ModError::Asset { path, reason } => assert!(path.ends_with("z_bad.bin") && reason.contains("refused"), "{path} {reason}"),
        other => panic!("{other:?}"),
    }
    assert!(e.scripting().module_id("am").is_none());
    assert_eq!(counts.commits.load(Ordering::SeqCst), 0, "no handler commit happened");

    // the same mod without the bad asset loads; owned assets are not kept in the package
    let files = folder("am", "", main).with_bytes("assets/a_ok.bin", b"ok").with_bytes("assets/n.txt", b"n");
    let assets = vec!["a_ok.bin".to_string(), "n.txt".to_string()];
    let id = e.load_input(LoadInput::Folder { label: "am", files: &files, assets: &assets }).unwrap().id();
    assert_eq!(counts.commits.load(Ordering::SeqCst), 1);
    let pkg = e.package(id).unwrap();
    assert!(pkg.assets.contains_key("n.txt") && !pkg.assets.contains_key("a_ok.bin"));
    e.unload_module(id).unwrap();
    assert_eq!(counts.unloads.load(Ordering::SeqCst), 1);
}

#[test]
fn meta_survives_mod_toml_to_package_to_engine() {
    let meta = "\n[meta]\nflavor = \"mint\"\nlevel = 3\ntags = [\"a\", \"b\"]\n[meta.nested]\nok = true\n";
    let files = folder("mm", meta, "export fn init() {\n    ret\n}\n");
    let built = build_mod(&files, None, &[], &BuildLimits::default()).unwrap();
    let want = built.manifest.meta.clone().expect("meta built from mod.toml");
    let bytes = Package::write(&built.manifest, &built.module.encode(), &built.assets).unwrap();
    let (mut e, _) = engine();
    let id = e.load_input(LoadInput::Package { label: "mm.efmod", bytes: &bytes }).unwrap().id();
    let got = e.package(id).unwrap().manifest.meta.clone().expect("meta after load");
    assert_eq!(got, want);
    let Value::Obj(fields) = &got else { panic!("{got:?}") };
    let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(get("flavor"), Some(Value::Str("mint".into())));
    assert_eq!(get("level"), Some(Value::Num("3".into())));
    assert_eq!(get("tags"), Some(Value::Arr(vec![Value::Str("a".into()), Value::Str("b".into())])));
    assert!(matches!(get("nested"), Some(Value::Obj(_))));
}

#[test]
fn nothing_loads_without_a_mod_source() {
    let (mut e, _) = engine();
    let r = e.load_mods();
    assert!(r.loaded.is_empty() && r.failed.is_empty());
}

#[test]
fn the_default_policy_grants_nothing() {
    let (mut e, _) = engine();
    let main = "import engine.log.info\nname db \"hi\"\nexport fn init() {\n    call engine.log.info(name, name.len)\n    ret\n}\n";
    let files = folder("lg", "\n[permissions]\nnamespaces = [\"engine.log\"]\n", main);
    let err = e.load_input(LoadInput::Folder { label: "lg", files: &files, assets: &[] }).unwrap_err();
    match mod_err(err) {
        ModError::NotGranted { id, namespaces } => {
            assert_eq!(id, "lg");
            assert!(namespaces.iter().any(|n| n == "engine.log"), "{namespaces:?}");
        }
        other => panic!("{other:?}"),
    }
}

fn load_folder(files: &MemSource, assets: &[String]) -> Result<Loaded, LoadError> {
    let (mut e, _) = engine();
    e.load_input(LoadInput::Folder { label: "x", files, assets })
}

const NOOP: &str = "export fn init() {\n    ret\n}\n";

#[test]
fn too_many_assets_is_refused() {
    let files = folder("big", "", NOOP);
    let assets: Vec<String> = (0..BuildLimits::default().max_files + 1).map(|i| format!("a{i}.txt")).collect();
    let err = mod_err(load_folder(&files, &assets).unwrap_err());
    assert!(matches!(err, ModError::Build(_)), "{err:?}");
}

#[test]
fn a_deep_use_chain_is_refused() {
    let depth = BuildLimits::default().max_use_depth + 6;
    let mut files = folder("deep", "", "use \"f0.efa\"\nexport fn init() {\n    ret\n}\n");
    for i in 0..depth {
        let body = if i + 1 < depth { format!("use \"f{}.efa\"\n", i + 1) } else { "pub const X = 1\n".to_string() };
        files = files.with(&format!("f{i}.efa"), &body);
    }
    let err = mod_err(load_folder(&files, &[]).unwrap_err());
    assert!(matches!(err, ModError::Build(_)), "{err:?}");
}

#[test]
fn reserved_asset_names_are_refused() {
    for name in ["manifest.json", "module.efvm", "engine/x.bin", "engine"] {
        let files = folder("rs", "", NOOP).with_bytes(&format!("assets/{name}"), b"x");
        let err = mod_err(load_folder(&files, &[name.to_string()]).unwrap_err());
        assert!(matches!(&err, ModError::Build(m) if m.contains("reserved")), "{name}: {err:?}");
    }
}

#[test]
fn an_api_path_outside_the_folder_is_refused() {
    let files = folder("ap", "\n[build]\napi = [\"../x\"]\n", NOOP);
    let err = mod_err(load_folder(&files, &[]).unwrap_err());
    assert!(matches!(&err, ModError::Build(m) if m.contains("api")), "{err:?}");
}

