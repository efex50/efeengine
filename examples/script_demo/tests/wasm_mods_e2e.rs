//! E7.4b example mods: `wasm_mods/rust_mod` (Rust) and `wasm_mods/c_mod` (C), two twins that
//! use the script_demo API for real. Built to wasm here, translated with `efe-wasm`, wrapped in
//! an `.efmod`, loaded under a policy and run headless; a small `.efa` driver mod calls their
//! provides (E7.4c, `efescript pack` of wasm, is not built yet: everything is done in this test).
//!
//! What each mod does: `frame` 1 adds the sum of the squares 1..=5 (heap) to `game.score_add`,
//! calls `inventory.give("sword", &Item {7, 5})` and reports the results with `game.report`;
//! `frame` 2 reads the `inventory.given` event. It provides `sum_squares(n) -> i64`,
//! `checksum(name: str) -> i64` and `boom(code) -> i32` (traps on 13).
//!
//! Needs a Rust toolchain with the wasm32-unknown-unknown target and, for the C mod, clang with the
//! wasm32 target and a wasm-ld; a missing one prints `SKIPPED` and the test returns.
//! Regenerate the bindings with `EFE_BLESS=1 cargo test -p script_demo --test wasm_mods_e2e bindings`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use efeengine_core::cache::{CacheConfig, CacheDir};
use efeengine_core::efa::asm::FsSource;
use efeengine_core::efa::mod_folder::{BuildLimits, prepare_mod, read_mod_toml, to_fallback};
use efeengine_core::efemod::{Manifest, Package, ReadLimits, check_module};
use efeengine_core::efevm::{ApiIndex, ApiType, Module, Param};
use efeengine_core::mods::{Grant, LoadInput, ModRequest};
use efeengine_core::prelude::*;
use efeengine_core::scripting::{LogLevel, ModuleId};
use script_demo::{Score, setup};

const DT: f64 = 1.0 / 60.0;

// ---------------------------------------------------------------- the two mods

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Rust,
    C,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Kind::Rust => "wasm_rust",
            Kind::C => "wasm_c",
        }
    }

    fn dir(self) -> PathBuf {
        wasm_mods().join(match self {
            Kind::Rust => "rust_mod",
            Kind::C => "c_mod",
        })
    }
}

fn wasm_mods() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("wasm_mods")
}

fn game_mods() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("mods")
}

// ---------------------------------------------------------------- the API indices

fn engine(jit: bool) -> (Engine, Score) {
    let cfg = EngineConfig {
        fixed_dt: DT,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    let score = setup(&mut e);
    #[cfg(feature = "jit")]
    if jit {
        use efeengine_core::scripting::{JitConfig, ScriptBackend};
        e.scripting_mut().set_backend(ScriptBackend::Jit, JitConfig::default());
    }
    #[cfg(not(feature = "jit"))]
    assert!(!jit, "the jit feature is off");
    (e, score)
}

struct Indices {
    /// The packed `inventory` mod (a dependency of both wasm mods).
    inventory: Vec<u8>,
    /// What `efescript api-export --rust|--c script_demo.efapi inventory.efmod` merges.
    merged: ApiIndex,
}

fn indices() -> &'static Indices {
    static I: OnceLock<Indices> = OnceLock::new();
    I.get_or_init(|| {
        let (e, _) = engine(false);
        let game = ApiIndex::from_json(&e.export_api_index()).expect("the game's .efapi");
        let packed = efescript::pack::pack_dir(&game_mods().join("inventory"), &[&game]).unwrap_or_else(|err| panic!("pack inventory: {err}"));
        let pkg = Package::read(&packed.bytes, &ReadLimits::default()).expect("read inventory.efmod");
        let merged = ApiIndex::merge(&[&game, &pkg.index()]).expect("merge");
        Indices { inventory: packed.bytes, merged }
    })
}

// ---------------------------------------------------------------- bindings are current

fn check_generated(path: &Path, fresh: &str) {
    if std::env::var_os("EFE_BLESS").is_some() {
        std::fs::write(path, fresh).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let have = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(have == fresh, "{} is out of date: regenerate it with `efescript api-export` (see wasm_mods/README.md) or EFE_BLESS=1", path.display());
}

#[test]
fn bindings_are_up_to_date() {
    let ix = &indices().merged;
    check_generated(&Kind::Rust.dir().join("src/api.rs"), &efescript::rustgen::generate(ix).expect("rust bindings"));
    check_generated(&Kind::C.dir().join("efe_api.h"), &efescript::cgen::generate(ix).expect("c bindings"));
}

// ---------------------------------------------------------------- building the wasm

/// Sysroots of the toolchains to look in: the active one first, then every installed one.
fn sysroots() -> Vec<PathBuf> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let mut v = Vec::new();
    if let Ok(o) = Command::new(rustc).args(["--print", "sysroot"]).output() {
        v.push(PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    }
    let home = std::env::var_os("RUSTUP_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup")));
    if let Some(dir) = home.map(|h| h.join("toolchains")).and_then(|d| std::fs::read_dir(d).ok()) {
        let mut found: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
        found.sort();
        v.extend(found);
    }
    v
}

fn wasm32_toolchain() -> Option<PathBuf> {
    sysroots().into_iter().find(|r| r.join("lib/rustlib/wasm32-unknown-unknown").is_dir() && r.join("bin/cargo").is_file())
}

/// `Some(None)`: `wasm-ld` is on PATH. `Some(Some(path))`: rustup's lld. `None`: neither.
fn wasm_ld() -> Option<Option<PathBuf>> {
    if Command::new("wasm-ld").arg("--version").output().is_ok_and(|o| o.status.success()) {
        return Some(None);
    }
    sysroots().into_iter().map(|r| r.join("lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld/wasm-ld")).find(|p| p.is_file()).map(Some)
}

fn build_rust() -> Option<Vec<u8>> {
    let Some(toolchain) = wasm32_toolchain() else {
        eprintln!("SKIPPED: no installed toolchain has the wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)");
        return None;
    };
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("wasm_rust_mod");
    let o = Command::new(toolchain.join("bin/cargo"))
        .args(["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(Kind::Rust.dir().join("Cargo.toml"))
        .env("RUSTC", toolchain.join("bin/rustc"))
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_BUILD_JOBS", "2")
        // RUSTFLAGS wins over the repo's `[target.wasm32-unknown-unknown] rustflags` (atomics,
        // shared and imported memory for the web build), which the translator rejects.
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env("RUSTFLAGS", "-Cdebuginfo=0")
        .output()
        .expect("run cargo");
    assert!(o.status.success(), "building rust_mod failed:\n{}", String::from_utf8_lossy(&o.stderr));
    Some(std::fs::read(target.join("wasm32-unknown-unknown/release/wasm_rust_mod.wasm")).expect("rust_mod wasm"))
}

fn build_c() -> Option<Vec<u8>> {
    if !Command::new("clang").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("SKIPPED: clang is not installed");
        return None;
    }
    let Some(ld) = wasm_ld() else {
        eprintln!("SKIPPED: no wasm-ld (neither on PATH nor in a rustup toolchain)");
        return None;
    };
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("wasm_c_mod.wasm");
    let dir = Kind::C.dir();
    let mut cmd = Command::new("clang");
    cmd.args(["--target=wasm32", "-nostdlib", "-O2", "-mbulk-memory", "-Wl,--no-entry", "-Wl,--export=efe_scratch"]);
    if let Some(p) = ld {
        // (`--ld-path` is ignored by the wasm driver; an absolute `-fuse-ld` is not.)
        cmd.arg(format!("-fuse-ld={}", p.display()));
    }
    let o = cmd
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/efe-guest/include"))
        .arg("-I")
        .arg(&dir)
        .arg(dir.join("mod.c"))
        .arg("-o")
        .arg(&out)
        .output()
        .expect("run clang");
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        if err.contains("unknown target") || err.contains("No available targets") {
            eprintln!("SKIPPED: this clang cannot target wasm32: {err}");
            return None;
        }
        panic!("clang failed:\n{err}");
    }
    Some(std::fs::read(&out).expect("c_mod wasm"))
}

/// The built wasm of a mod, built once per test process.
fn wasm(kind: Kind) -> Option<&'static [u8]> {
    static R: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    static C: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    match kind {
        Kind::Rust => R.get_or_init(build_rust),
        Kind::C => C.get_or_init(build_c),
    }
    .as_deref()
}

// ---------------------------------------------------------------- mod.toml, translation, package

fn toml_table(kind: Kind) -> toml::Table {
    std::fs::read_to_string(kind.dir().join("mod.toml")).expect("mod.toml").parse().expect("mod.toml is TOML")
}

/// The manifest of `mod.toml` via the real `read_mod_toml` parser.
fn manifest(kind: Kind) -> Manifest {
    let text = std::fs::read_to_string(kind.dir().join("mod.toml")).expect("mod.toml");
    let m = read_mod_toml(&text).unwrap_or_else(|e| panic!("{e}")).manifest;
    m.validate().expect("manifest");
    m
}

fn api_params(v: &toml::Value) -> Vec<Param> {
    v.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let (name, ty) = p.as_str().expect("a string").split_once(": ").expect("`name: type`");
            Param::new(name, ApiType::parse(ty).unwrap_or_else(|| panic!("type `{ty}`")))
        })
        .collect()
}

/// The `[[provide]]` tables and `[build] scratch` as the translator's fallback.
fn fallback(kind: Kind) -> efe_wasm::Fallback {
    let prep = prepare_mod(&FsSource::new(kind.dir()), &BuildLimits::default()).unwrap_or_else(|e| panic!("{e}"));
    to_fallback(&prep)
}

/// Translates the mod's wasm against the merged index. The C mod gets its provides from
/// `mod.toml` (clang cannot emit `efe.provide`); the Rust mod's come from its own section.
fn translate(kind: Kind, wasm: &[u8]) -> Module {
    let opts = efe_wasm::TranslateOptions {
        fallback: if kind == Kind::C { fallback(kind) } else { Default::default() },
        require_efe_api: true,
        ..Default::default()
    };
    efe_wasm::translate(wasm, &indices().merged, &opts).unwrap_or_else(|e| panic!("translate {}: {e}", kind.id()))
}

/// The `.efmod` of a translated module (what `efescript pack` will write in E7.4c).
fn package(kind: Kind, module: &Module) -> Vec<u8> {
    let m = manifest(kind);
    check_module(&m, module).expect("the manifest covers the module's imports");
    Package::write(&m, &module.encode(), &BTreeMap::new()).expect("write the package")
}

// ---------------------------------------------------------------- the driver (.efa)

/// A small `.efa` mod that calls the provides of `ns` (the test is the caller): frame 1 reports
/// `checksum("hello")`, `sum_squares(10)` and `boom(6)`; frame 2 calls `boom(13)`, which traps.
fn driver_package(ns: &str, with: &ApiIndex) -> Vec<u8> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("script_demo_wasm_driver_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("mod.toml"),
        format!("[mod]\nid = \"driver\"\nversion = \"0.1.0\"\ngame = \"script_demo\"\n\n[dependencies]\n{ns} = \"^1\"\n\n[permissions]\nnamespaces = [\"game\"]\n"),
    )
    .unwrap();
    let src = r#"
import NS.sum_squares
import NS.checksum
import NS.boom
import game.report

name db "hello"
global mut phase: i32 = 0

export fn frame(dt: f32) {
    reg p, tmp, v, a
    global.get p, phase
    i32.add tmp, p, 1
    global.set phase, tmp
    br_if p, second
    call NS.checksum(name, name.len) -> v
    call game.report(v)
    const.i32 a, 10
    call NS.sum_squares(a) -> v
    call game.report(v)
    const.i32 a, 6
    call NS.boom(a) -> tmp
    i64.extend_i32_s v, tmp
    call game.report(v)
    ret
second:
    i32.sub tmp, p, 1
    br_if tmp, done
    const.i32 a, 13
    call NS.boom(a) -> tmp
done:
    ret
}
"#;
    std::fs::write(dir.join("src/main.efa"), src.replace("NS", ns)).unwrap();
    let packed = efescript::pack::pack_dir(&dir, &[with]).unwrap_or_else(|e| panic!("pack the driver: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    packed.bytes
}

// ---------------------------------------------------------------- the run

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn load(e: &mut Engine, label: &str, bytes: &[u8]) -> ModuleId {
    match e.load_input(LoadInput::Package { label, bytes }) {
        Ok(l) => l.id(),
        Err(err) => panic!("load {label}: {err}"),
    }
}

/// Everything the two mods have in common, on the interpreter or the JIT.
fn run_end_to_end(kind: Kind, jit: bool) {
    let Some(bytes) = wasm(kind) else { return };
    let ix = indices();
    let module = translate(kind, bytes);
    let pkg = package(kind, &module);
    let own = Package::read(&pkg, &ReadLimits::default()).expect("read back").index();
    let with = ApiIndex::merge(&[&ix.merged, &own]).expect("merge the mod's own index");
    let driver = driver_package(kind.id(), &with);

    let (mut e, score) = engine(jit);
    let id = kind.id();
    e.set_mod_policy(move |r: &ModRequest<'_>| {
        let namespaces: &[&str] = match r.manifest.id.as_str() {
            "inventory" => &["engine.log", "engine.events"],
            m if m == id => &["engine.events", "game"],
            "driver" => &["game"],
            other => return Err(format!("{other} is not welcome")),
        };
        Ok(Grant { namespaces: namespaces.iter().map(|s| s.to_string()).collect(), jit, ..Grant::default() })
    });
    let inventory = load(&mut e, "inventory.efmod", &ix.inventory);
    let wasm_id = load(&mut e, "wasm.efmod", &pkg);
    let driver_id = load(&mut e, "driver.efmod", &driver);
    assert_eq!(e.scripting().runs_on_jit(wasm_id), jit, "the wasm mod runs on the {}", if jit { "JIT" } else { "interpreter" });

    // The mod's provides are in the engine's index, with the docs and types of mod.toml.
    let idx = e.api_index();
    for p in toml_table(kind)["provide"].as_array().unwrap() {
        let f = idx.function(id, p["name"].as_str().unwrap()).unwrap_or_else(|| panic!("{id}.{} in the index", p["name"]));
        assert_eq!((f.owner.as_str(), f.doc.as_str()), (id, p["doc"].as_str().unwrap()));
    }

    // frame 1: score_add(55) = 55, give("sword", {7, 5}) = len 5 + count 5 = 10, count back as 6;
    //          driver: checksum("hello") = 1617, sum_squares(10) = 385, boom(6) = 12
    // frame 2: the event inventory.given: 1 event, id 7, count 6; driver: boom(13) traps
    e.run_frames(1, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(score.reports(), [55, 10, 6, 1617, 385, 12]);
    assert_eq!(score.get(), 55);
    e.run_frames(1, DT);
    assert_eq!(score.reports(), [55, 10, 6, 1617, 385, 12, 1, 7, 6]);

    // the trap names the wasm function
    let errs = errors(&e);
    assert!(errs.iter().any(|l| l.contains("boom")), "the trap error names the function: {errs:?}");
    eprintln!("{} trap error: {errs:?}", id);

    // the game keeps running; nothing more is reported
    e.run_frames(3, DT);
    assert!(e.is_running());
    assert_eq!(score.reports().len(), 9);

    // unload: the driver depends on the wasm mod, which depends on inventory
    e.unload_module(driver_id).expect("unload the driver");
    e.unload_module(wasm_id).expect("unload the wasm mod");
    let idx = e.api_index();
    assert!(idx.function(id, "boom").is_none() && idx.namespaces.iter().all(|n| n.name != id));
    e.unload_module(inventory).expect("unload inventory");
    e.run_frames(1, DT);
    assert!(e.is_running());
}

#[test]
fn the_rust_mod_runs_end_to_end() {
    run_end_to_end(Kind::Rust, false);
}

#[test]
fn the_c_mod_runs_end_to_end() {
    run_end_to_end(Kind::C, false);
}

#[cfg(feature = "jit")]
#[test]
fn the_rust_mod_runs_end_to_end_on_the_jit() {
    run_end_to_end(Kind::Rust, true);
}

#[cfg(feature = "jit")]
#[test]
fn the_c_mod_runs_end_to_end_on_the_jit() {
    run_end_to_end(Kind::C, true);
}

// ---------------------------------------------------------------- the translated modules

fn check_module_sections(kind: Kind) {
    let Some(bytes) = wasm(kind) else { return };
    let ix = indices();
    let module = translate(kind, bytes);
    // `decode` runs `Module::verify`.
    let decoded = Module::decode(&module.encode()).expect("the translated module verifies");
    assert_eq!(decoded.data(), module.data());
    let d = module.data();

    // section 12: the hash of the index the bindings were generated from, and what is imported
    let api = d.api.as_ref().expect("section 12");
    assert_eq!(api.hash, ix.merged.hash());
    for ns in ["engine.events", "game", "inventory"] {
        assert!(api.namespaces.iter().any(|(n, _)| n == ns), "{ns} in {:?}", api.namespaces);
    }

    // section 14: the one event the mod reads, with its payload
    let events: Vec<String> = d.events.iter().map(|e| e.qualified()).collect();
    assert_eq!(events, ["inventory.given"]);
    assert_eq!(d.events[0].payload, Some(("Item".to_string(), 8)));

    // section 13 is what mod.toml declares: same names, docs, parameters and results
    let iface = d.interface.as_ref().expect("section 13");
    let t = toml_table(kind);
    let declared = t["provide"].as_array().unwrap();
    assert_eq!(iface.provides.len(), declared.len(), "{:?}", iface.provides.iter().map(|p| &p.name).collect::<Vec<_>>());
    for p in declared {
        let got = iface.provide(p["name"].as_str().unwrap()).unwrap_or_else(|| panic!("provide {}", p["name"]));
        assert_eq!(got.doc, p["doc"].as_str().unwrap());
        assert_eq!(got.params, api_params(&p["params"]), "{}", got.name);
        assert_eq!(got.results, api_params(&p["results"]), "{}", got.name);
    }
    assert_eq!(iface.scratch.map(|(_, n)| n), Some(t["build"]["scratch"].as_integer().unwrap() as u32));
    assert!(iface.events.is_empty());
    // the mod's own index: the provides as functions of the namespace, and it is a valid index
    let own = iface.index(kind.id(), 1, "doc");
    own.validate().expect("the mod's own index is valid");
    assert_eq!(own.functions.len(), declared.len());
    assert_eq!(own.function(kind.id(), "checksum").unwrap().signature_text(), "(name: str) -> (result: i64)");

    // it is the module of the package, which the engine accepts
    let pkg = package(kind, &module);
    let read = Package::read(&pkg, &ReadLimits::default()).expect("read the package back");
    assert_eq!(read.manifest.id, kind.id());
    assert_eq!(read.module.data(), module.data());
}

#[test]
fn the_translated_rust_module_verifies_and_its_sections_match_the_declarations() {
    check_module_sections(Kind::Rust);
}

#[test]
fn the_translated_c_module_verifies_and_its_sections_match_the_declarations() {
    check_module_sections(Kind::C);
}

/// The Rust mod also works with the provides given from `mod.toml` (they equal its section).
#[test]
fn the_rust_mod_section_and_the_fallback_agree() {
    let Some(bytes) = wasm(Kind::Rust) else { return };
    let opts = efe_wasm::TranslateOptions { fallback: fallback(Kind::Rust), ..Default::default() };
    let both = efe_wasm::translate(bytes, &indices().merged, &opts).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(both.data().interface, translate(Kind::Rust, bytes).data().interface);
}

/// E7.4c: `efescript::pack::pack_dir` packs a wasm mod folder directly, and `Engine::load`
/// loads it as a folder mod through `ScriptCache::translate_wasm` (second engine hits disk).
#[test]
fn staged_wasm_mod_folders_pack_and_load_through_script_cache() {
    let ix = indices();
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("wasm_folder_e74c_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cache_dir = root.join("cache");

    for kind in [Kind::Rust, Kind::C] {
        let Some(bytes) = wasm(kind) else { continue };
        let stage = root.join(kind.id());
        let toml_text = std::fs::read_to_string(kind.dir().join("mod.toml")).unwrap();
        let mt = read_mod_toml(&toml_text).unwrap();
        let wasm_rel = mt.build.wasm_path().expect("wasm_path");
        let wasm_dest = stage.join(wasm_rel);
        std::fs::create_dir_all(wasm_dest.parent().unwrap()).unwrap();
        std::fs::write(stage.join("mod.toml"), &toml_text).unwrap();
        std::fs::write(&wasm_dest, bytes).unwrap();

        // Pack with efescript::pack::pack_dir.
        let packed = efescript::pack::pack_dir(&stage, &[&ix.merged]).unwrap_or_else(|e| panic!("pack {}: {e}", kind.id()));
        assert_eq!(packed.manifest.id, kind.id());
        assert_eq!(packed.module.data().interface, translate(kind, bytes).data().interface);

        // Load directly as a mod folder with ScriptCache enabled (first run translates, second hits disk).
        let setup_engine = |cache_dir: &Path| {
            let cfg = EngineConfig {
                fixed_dt: DT,
                script_cache: CacheConfig { disk: CacheDir::Path(cache_dir.to_path_buf()), ..Default::default() },
                ..EngineConfig::headless()
            };
            let mut e = Engine::with_config(cfg).expect("headless engine");
            setup(&mut e);
            let id = kind.id();
            e.set_mod_policy(move |r: &ModRequest<'_>| {
                let namespaces: &[&str] = match r.manifest.id.as_str() {
                    "inventory" => &["engine.log", "engine.events"],
                    m if m == id => &["engine.events", "game"],
                    other => return Err(format!("unexpected {other}")),
                };
                Ok(Grant { namespaces: namespaces.iter().map(|s| s.to_string()).collect(), ..Grant::default() })
            });
            e.load_input(LoadInput::Package { label: "inventory.efmod", bytes: &ix.inventory }).unwrap();
            e
        };

        let mut e1 = setup_engine(&cache_dir);
        let loaded = e1.load(&stage).unwrap_or_else(|e| panic!("load folder {}: {e}", kind.id()));
        assert_eq!(e1.package(loaded.id()).unwrap().manifest.id, kind.id());
        let s1 = e1.cache().stats();
        assert_eq!((s1.compiles, s1.disk_hits), (1, 0), "{}: {s1:?}", kind.id());

        let mut e2 = setup_engine(&cache_dir);
        e2.load(&stage).unwrap_or_else(|e| panic!("load cached {}: {e}", kind.id()));
        let s2 = e2.cache().stats();
        assert_eq!((s2.compiles, s2.disk_hits), (0, 1), "{}: {s2:?}", kind.id());
    }
    let _ = std::fs::remove_dir_all(&root);
}
