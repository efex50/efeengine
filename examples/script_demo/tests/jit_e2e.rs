//! With feature `jit`: the demo's `mover.efa` runs on the JIT and gives exactly the interpreter's
//! results. Run with `cargo test -p script_demo --features jit`.
#![cfg(all(feature = "jit", not(target_family = "wasm")))]

use std::path::Path;

use efeengine_core::cache::{CacheConfig, CacheDir};
use efeengine_core::efevm_jit::native_runs;
use efeengine_core::prelude::*;
use efeengine_core::scripting::{JitConfig, Script, ScriptBackend};
use script_demo::{Modules, setup, spawn_world};

/// Positions and states after 20 frames, the score, and whether the module was on the JIT.
fn run(backend: ScriptBackend) -> (Vec<(f32, u64)>, i64, bool, u64) {
    let cfg = EngineConfig {
        fixed_dt: 1.0 / 60.0,
        script_cache: CacheConfig {
            disk: CacheDir::Off,
            ..Default::default()
        },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    let score = setup(&mut e);
    e.scripting_mut().set_backend(backend, JitConfig::default());
    let id = e
        .load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scripts")
                .join("mover.efa"),
        )
        .expect("load mover")
        .id();
    spawn_world(&mut e, &Modules { mover: Some(id) });
    let before = native_runs();
    e.run_frames(20, 1.0 / 60.0);
    let mut out: Vec<(f32, u64)> = e
        .world()
        .query::<(&Transform, &Script)>()
        .iter()
        .map(|(t, s)| (t.position.x, s.state as u64))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    (
        out,
        score.get(),
        e.scripting().runs_on_jit(id),
        native_runs() - before,
    )
}

#[test]
fn the_mover_runs_on_the_jit_with_the_interpreters_results() {
    let (a, score_a, jit_a, native_a) = run(ScriptBackend::Interpreter);
    let (b, score_b, jit_b, native_b) = run(ScriptBackend::Jit);
    assert!(!jit_a && native_a == 0);
    assert!(
        jit_b && native_b >= 60,
        "3 entities x 20 frames ran natively ({native_b})"
    );
    assert_eq!(a, b);
    assert_eq!(score_a, score_b);
    assert_eq!(a.len(), 3);
}
