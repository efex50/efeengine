//! The efevm JIT inside the engine (feature `jit`, native only): game code runs on it, mods
//! only with `Grant::jit`, `ScriptBackend::Interpreter` keeps everything interpreted, and hot
//! reload re-enables it. The "ran natively" check reads the process-wide counter of
//! `efevm_jit::native_runs`, so the tests take a lock and compare deltas.
#![cfg(all(feature = "jit", not(target_arch = "wasm32")))]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use efeengine_core::cache::{CacheConfig, CacheDir};
use efeengine_core::efa::asm::MemSource;
use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::efevm_jit::native_runs;
use efeengine_core::mods::{Grant, LoadInput, ModRequest};
use efeengine_core::prelude::*;
use efeengine_core::scripting::{JitConfig, LogLevel, ModuleId, ScriptBackend};

type Reports = Arc<Mutex<Vec<i64>>>;

static SERIAL: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

fn temp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "efe-jit-it-{}-{name}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn engine_cfg(cfg: EngineConfig) -> (Engine, Reports) {
    let cfg = EngineConfig {
        fixed_dt: 1.0 / 60.0,
        script_cache: CacheConfig {
            disk: CacheDir::Off,
            ..Default::default()
        },
        ..cfg
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    e.define_namespace("test", 1, "testgame", "Helpers of the test game.")
        .unwrap();
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(
        HostFn::new("test", "report")
            .doc("Record `v`.")
            .param("v", ApiType::I64),
        move |_c, a, _r| {
            g.lock().unwrap().push(a.i64(0)?);
            Ok(HostOk::Continue)
        },
    )
    .unwrap();
    (e, got)
}

fn engine() -> (Engine, Reports) {
    engine_cfg(EngineConfig::headless())
}

/// `init` reports `n`; `frame` reports `n + 1000` (a loop of 100 adds first, so it is real work).
fn script(n: i64) -> String {
    format!(
        "import test.report\n\
         export fn init() {{\n    reg x\n    const.i64 x, {n}\n    call test.report(x)\n    ret\n}}\n\
         export fn frame(dt: f32) {{\n    reg x\n    const.i64 x, {}\n    call test.report(x)\n    ret\n}}\n",
        n + 1000
    )
}

fn mod_files(id: &str, n: i64) -> MemSource {
    MemSource::new()
        .with("mod.toml", &format!("[mod]\nid = \"{id}\"\nversion = \"1.0.0\"\n\n[permissions]\nnamespaces = [\"test\"]\n"))
        .with("src/main.efa", &script(n))
}

fn load_mod_files(e: &mut Engine, files: &MemSource) -> ModuleId {
    e.load_input(LoadInput::Folder {
        label: "m",
        files,
        assets: &[],
    })
    .unwrap_or_else(|err| panic!("{err}"))
    .id()
}

fn policy(jit: bool) -> impl FnMut(&ModRequest<'_>) -> Result<Grant, String> {
    move |_r: &ModRequest<'_>| {
        Ok(Grant {
            namespaces: vec!["test".into()],
            jit,
            ..Grant::default()
        })
    }
}

fn take(got: &Reports) -> Vec<i64> {
    std::mem::take(&mut *got.lock().unwrap())
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting()
        .log_lines()
        .filter(|l| l.level == LogLevel::Error)
        .map(|l| l.text.clone())
        .collect()
}

fn set_mtime_ahead(path: &std::path::Path, secs: u64) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn a_game_script_runs_on_the_jit_by_default() {
    let _g = lock();
    let (mut e, got) = engine();
    assert_eq!(e.config().script_backend, ScriptBackend::Auto);
    let dir = temp("game");
    std::fs::write(dir.join("g.efa"), script(7)).unwrap();
    let before = native_runs();
    let id = e.load(dir.join("g.efa")).unwrap().id();
    assert_eq!(take(&got), [7], "init ran");
    assert!(e.scripting().runs_on_jit(id));
    assert!(native_runs() > before, "init ran natively");
    let mid = native_runs();
    e.run_frames(3, 1.0 / 60.0);
    assert_eq!(take(&got), [1007, 1007, 1007]);
    assert!(native_runs() >= mid + 3, "every frame ran natively");
    assert!(errors(&e).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_interpreter_choice_stays_interpreted() {
    let _g = lock();
    let (mut e, got) = engine();
    e.scripting_mut()
        .set_backend(ScriptBackend::Interpreter, JitConfig::default());
    let dir = temp("interp");
    std::fs::write(dir.join("g.efa"), script(7)).unwrap();
    let before = native_runs();
    let id = e.load(dir.join("g.efa")).unwrap().id();
    e.run_frames(3, 1.0 / 60.0);
    assert_eq!(take(&got), [7, 1007, 1007, 1007], "same results");
    assert!(!e.scripting().runs_on_jit(id));
    assert_eq!(native_runs(), before, "nothing ran natively");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_mod_without_the_grant_runs_interpreted_and_with_it_natively() {
    let _g = lock();
    let (mut e, got) = engine();
    e.set_mod_policy(policy(false));
    let before = native_runs();
    let plain = load_mod_files(&mut e, &mod_files("plain", 1));
    e.run_frames(2, 1.0 / 60.0);
    assert_eq!(take(&got), [1, 1001, 1001]);
    assert!(!e.scripting().runs_on_jit(plain));
    assert_eq!(
        native_runs(),
        before,
        "a mod without the grant never runs natively"
    );
    assert!(!e.package(plain).unwrap().grant.jit);

    e.set_mod_policy(policy(true));
    let fast = load_mod_files(&mut e, &mod_files("fast", 2));
    assert!(e.package(fast).unwrap().grant.jit);
    assert!(e.scripting().runs_on_jit(fast));
    assert!(!e.scripting().runs_on_jit(plain));
    let mid = native_runs();
    e.run_frames(2, 1.0 / 60.0);
    let mut v = take(&got);
    v.sort();
    assert_eq!(v, [2, 1001, 1001, 1002, 1002]);
    assert!(native_runs() >= mid + 2, "the granted mod ran natively");
    assert!(errors(&e).is_empty());
}

#[test]
fn hot_reload_keeps_the_choice() {
    let _g = lock();
    let (mut e, got) = engine();
    // game code: reloaded from its file
    let dir = temp("reload");
    let file = dir.join("g.efa");
    std::fs::write(&file, script(1)).unwrap();
    let id = e.load(&file).unwrap().id();
    e.watch_source(id, &file).unwrap();
    assert!(e.scripting().runs_on_jit(id));
    std::fs::write(&file, script(2)).unwrap();
    set_mtime_ahead(&file, 10);
    let before = native_runs();
    e.run_frames(30, 1.0 / 60.0);
    assert!(take(&got).contains(&2), "the new version's init ran");
    assert!(
        e.scripting().runs_on_jit(id),
        "the reloaded instance is on the JIT again"
    );
    assert!(native_runs() > before);

    // a mod with the grant keeps it across a reload, one without never gets it
    for jit in [true, false] {
        e.set_mod_policy(policy(jit));
        let name = format!("m{jit}");
        let mid = load_mod_files(&mut e, &mod_files(&name, 10));
        assert_eq!(e.scripting().runs_on_jit(mid), jit);
        take(&got);
        // module ids change on a mod reload
        let r = e
            .reload_mod_input(LoadInput::Folder {
                label: "m",
                files: &mod_files(&name, 20),
                assets: &[],
            })
            .unwrap_or_else(|err| panic!("{err}"));
        assert_ne!(r.id(), mid);
        assert_eq!(
            e.scripting().runs_on_jit(r.id()),
            jit,
            "after the reload, jit={jit}"
        );
        e.run_frames(1, 1.0 / 60.0);
        assert!(take(&got).contains(&20));
    }
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn changing_the_backend_re_applies_to_loaded_modules() {
    let _g = lock();
    let (mut e, got) = engine();
    e.set_mod_policy(policy(true));
    let dir = temp("switch");
    std::fs::write(dir.join("g.efa"), script(5)).unwrap();
    let game = e.load(dir.join("g.efa")).unwrap().id();
    let m = load_mod_files(&mut e, &mod_files("sw", 6));
    assert!(e.scripting().runs_on_jit(game) && e.scripting().runs_on_jit(m));
    e.scripting_mut()
        .set_backend(ScriptBackend::Interpreter, JitConfig::default());
    assert!(!e.scripting().runs_on_jit(game) && !e.scripting().runs_on_jit(m));
    take(&got);
    let before = native_runs();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(native_runs(), before);
    assert_eq!(take(&got).len(), 2, "both still run");
    e.scripting_mut().set_backend(
        ScriptBackend::Jit,
        JitConfig {
            optimize: false,
            ..JitConfig::default()
        },
    );
    assert!(e.scripting().runs_on_jit(game) && e.scripting().runs_on_jit(m));
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(take(&got).len(), 2);
    assert!(native_runs() > before);
    let _ = std::fs::remove_dir_all(dir);
}
