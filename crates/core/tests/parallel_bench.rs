//! Benchmark of the parallel schedule (run with
//! `cargo test -p efeengine-core --release --test parallel_bench -- --ignored --nocapture`):
//! 10k entities with `Transform`, `Velocity` and `Sprite`, the built-in velocity system plus two
//! par systems doing per-entity math on disjoint components. Prints ms per frame.

use std::time::Instant;

use efeengine_core::prelude::*;

const ENTITIES: usize = 10_000;
const WARMUP: usize = 30;
const FRAMES: usize = 300;

#[derive(Clone, Copy)]
struct Wave(f32);
#[derive(Clone, Copy)]
struct Decay(f32);

fn crunch(mut x: f32) -> f32 {
    for i in 0..64 {
        x = (x * 1.0001 + i as f32 * 0.01).sin().abs() + x.sqrt() * 0.5;
    }
    x
}

fn build(threads: Threads) -> Engine {
    let cfg = EngineConfig { fixed_dt: 1.0 / 60.0, threads, ..EngineConfig::headless() };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    for i in 0..ENTITIES {
        let f = i as f32;
        e.world_mut().spawn((
            Transform::from_position(Vec2::new(f % 100.0, f / 100.0)),
            Velocity { linear: Vec2::new(1.0, 0.5), angular: 0.0 },
            Sprite::new(TextureHandle { id: 0 }),
            Wave(f),
            Decay(1.0 + f),
        ));
    }
    e.add_par_system(
        Stage::Update,
        Access::new().named("wave").write::<Wave>(),
        |ctx| ctx.par_for_each::<&mut Wave, _>(512, |w| w.0 = crunch(w.0)),
    );
    e.add_par_system(
        Stage::Update,
        Access::new().named("decay").write::<Decay>(),
        |ctx| ctx.par_for_each::<&mut Decay, _>(512, |d| d.0 = crunch(d.0 + 1.0)),
    );
    e
}

fn ms_per_frame(threads: Threads) -> f64 {
    let mut e = build(threads);
    e.run_frames(WARMUP, 1.0 / 60.0);
    let t = Instant::now();
    e.run_frames(FRAMES, 1.0 / 60.0);
    t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64
}

#[test]
#[ignore]
fn parallel_schedule_ms_per_frame() {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    println!("{ENTITIES} entities, {FRAMES} frames after {WARMUP} warm-up, {cores} available cores");
    let mut cases = vec![("Single".to_string(), Threads::Single), ("Fixed(2)".into(), Threads::Fixed(2)), ("Fixed(4)".into(), Threads::Fixed(4))];
    cases.push((format!("Fixed({cores}) (available_parallelism)"), Threads::Fixed(cores)));
    let mut base = 0.0;
    for (name, th) in cases {
        let ms = ms_per_frame(th);
        if base == 0.0 {
            base = ms;
        }
        println!("{name:<40} {ms:8.3} ms/frame   x{:.2}", base / ms);
    }
}
