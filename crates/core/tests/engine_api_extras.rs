//! `engine.random`, `engine.math`, `Engine::watch_source` (hot reload of `.efa` game scripts)
//! and `Commands::unload_mods`. Headless.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use efeengine_core::efa::asm::MemSource;
use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::prelude::*;

type Reports = Arc<Mutex<Vec<i64>>>;

fn temp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("efe-extras-{}-{name}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
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
    (e, got)
}

const RANDOM_MATH: &str = "\
import test.report
import engine.random.f32
import engine.random.range
import engine.random.seed
import engine.math.sin
import engine.math.cos
import engine.math.atan2

export fn init() {
    reg a, b, x, v, s, one
    call engine.random.seed(7)
    call engine.random.range(1000) -> a
    call engine.random.seed(7)
    call engine.random.range(1000) -> b
    call test.report(a)
    call test.report(b)
    ; f32 in [0, 1): scaled by 1000 and truncated is in 0..=999
    call engine.random.f32() -> x
    f32.mul x, x, 1000.0
    i64.trunc_f32_s v, x
    call test.report(v)
    ; sin(0.5) * 1000 = 479, cos(0) = 1, atan2(1, 1) * 1000 = 785
    call engine.math.sin(0.5) -> s
    f32.mul s, s, 1000.0
    i64.trunc_f32_s v, s
    call test.report(v)
    call engine.math.cos(0.0) -> s
    i64.trunc_f32_s v, s
    call test.report(v)
    call engine.math.atan2(1.0, 1.0) -> s
    f32.mul s, s, 1000.0
    i64.trunc_f32_s v, s
    call test.report(v)
    ; range(0) is 0
    call engine.random.range(0) -> a
    call test.report(a)
    ret
}
";

#[test]
fn random_is_seedable_and_math_has_trigonometry() {
    let (mut e, got) = engine();
    let dir = temp("rand");
    std::fs::write(dir.join("rm.efa"), RANDOM_MATH).unwrap();
    e.load(dir.join("rm.efa")).unwrap_or_else(|err| panic!("{err}"));
    let v = got.lock().unwrap().clone();
    assert_eq!(v.len(), 7, "{v:?}");
    assert!((0..1000).contains(&v[0]));
    assert_eq!(v[0], v[1], "same seed, same number");
    assert!((0..1000).contains(&v[2]));
    assert_eq!(v[3..], [479, 1, 785, 0]);
    let _ = std::fs::remove_dir_all(dir);
}

fn set_mtime_ahead(path: &std::path::Path, secs: u64) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(secs)).unwrap();
}

#[test]
fn watch_source_recompiles_an_efa_script_with_its_use_files_in_place() {
    let (mut e, got) = engine();
    let dir = temp("watch");
    let main = dir.join("main.efa");
    let helper = dir.join("helper.efa");
    std::fs::write(&helper, "pub const VALUE = 1\n").unwrap();
    let script = |v: &str| {
        format!("use \"helper.efa\" as h\nimport test.report\nexport fn init() {{\n    reg x\n    const.i64 x, {v}\n    call test.report(x)\n    ret\n}}\n")
    };
    std::fs::write(&main, script("h.VALUE")).unwrap();
    let id = e.load(&main).unwrap().id();
    e.watch_source(id, &main).unwrap();
    assert_eq!(*got.lock().unwrap(), [1]);

    // an edit of a `use`d file is picked up too (the root is unchanged)
    std::fs::write(&helper, "pub const VALUE = 2\n").unwrap();
    set_mtime_ahead(&helper, 10);
    e.run_frames(30, 1.0 / 60.0);
    assert_eq!(*got.lock().unwrap(), [1, 2]);
    assert!(e.scripting().log_lines().any(|l| l.level == LogLevel::Info && l.text.contains("recompiled and reloaded")));

    // a broken edit is logged and the old version keeps running
    std::fs::write(&main, "this is not efa\n").unwrap();
    set_mtime_ahead(&main, 20);
    e.run_frames(30, 1.0 / 60.0);
    assert_eq!(*got.lock().unwrap(), [1, 2]);
    assert!(e.scripting().log_lines().any(|l| l.level == LogLevel::Error && l.text.contains("keeping the old version")));
    assert!(e.is_running());

    // fixed again: reloaded under the same module id
    std::fs::write(&main, script("3")).unwrap();
    set_mtime_ahead(&main, 30);
    e.run_frames(30, 1.0 / 60.0);
    assert_eq!(*got.lock().unwrap(), [1, 2, 3]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_system_can_queue_unload_mods() {
    let (mut e, _got) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant::default()));
    let files = MemSource::new()
        .with("mod.toml", "[mod]\nid = \"tiny\"\nversion = \"1.0.0\"\n")
        .with("src/main.efa", "export fn frame(dt: f32) {\n    ret\n}\n");
    let id = e.load_input(LoadInput::Folder { label: "tiny", files: &files, assets: &[] }).unwrap().id();
    assert!(e.package(id).is_some());
    let go = Arc::new(Mutex::new(false));
    let g = go.clone();
    e.add_system(Stage::Update, move |ctx| {
        if *g.lock().unwrap() {
            ctx.commands.unload_mods();
        }
    });
    e.run_frames(2, 1.0 / 60.0);
    assert!(e.package(id).is_some(), "nothing queued yet");
    *go.lock().unwrap() = true;
    e.run_frames(2, 1.0 / 60.0);
    assert!(e.package(id).is_none(), "unloaded");
}

#[test]
fn a_script_attaches_its_own_entry_to_an_entity_it_spawned() {
    let (mut e, got) = engine();
    let dir = temp("attach");
    let src = "\
import test.report
import engine.world.spawn
import engine.script.attach

tick_name db \"tick\"
nope_name db \"nope\"

export fn init() {
    reg ent, st
    call engine.world.spawn() -> ent
    call engine.script.attach(ent, tick_name, tick_name.len) -> st
    call test.report(st)
    call engine.script.attach(ent, nope_name, nope_name.len) -> st
    call test.report(st)
    ret
}

export fn tick(entity: i64, dt: f32, state: i64) -> i64 {
    i64.add state, state, 1
    ret state
}
";
    std::fs::write(dir.join("s.efa"), src).unwrap();
    e.load(dir.join("s.efa")).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(*got.lock().unwrap(), [0, 2], "tick attaches, nope does not exist");
    e.run_frames(5, 1.0 / 60.0);
    let states: Vec<i64> = e.world().query::<&Script>().iter().map(|s| s.state).collect();
    assert_eq!(states, [5]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_mod_reads_numbers_of_its_own_meta_table() {
    let (mut e, got) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant { namespaces: vec!["engine.mods".into(), "test".into()], ..Grant::default() }));
    let main = "\
import test.report
import engine.mods.meta_f64

k_speed db \"difficulty.speed\"
k_hard db \"difficulty.hard\"
k_missing db \"difficulty.nope\"
k_text db \"name\"

export fn init() {
    reg v, i
    call engine.mods.meta_f64(k_speed, k_speed.len, 1.0) -> v
    f64.mul v, v, 100.0
    i64.trunc_f64_s i, v
    call test.report(i)
    call engine.mods.meta_f64(k_hard, k_hard.len, 7.0) -> v
    i64.trunc_f64_s i, v
    call test.report(i)
    call engine.mods.meta_f64(k_missing, k_missing.len, 7.0) -> v
    i64.trunc_f64_s i, v
    call test.report(i)
    call engine.mods.meta_f64(k_text, k_text.len, 9.0) -> v
    i64.trunc_f64_s i, v
    call test.report(i)
    ret
}
";
    let files = MemSource::new()
        .with("mod.toml", "[mod]\nid = \"m\"\nversion = \"1.0.0\"\nname = \"text\"\n[permissions]\nnamespaces = [\"engine.mods\", \"test\"]\n[meta]\nname = \"x\"\n[meta.difficulty]\nspeed = 1.5\nhard = true\n")
        .with("src/main.efa", main);
    e.load_input(LoadInput::Folder { label: "m", files: &files, assets: &[] }).unwrap_or_else(|err| panic!("{err}"));
    let errs: Vec<String> = e.scripting().log_lines().map(|l| l.text.clone()).collect();
    assert_eq!(*got.lock().unwrap(), [150, 1, 7, 9], "{errs:?}");
}
