//! Script lanes (docs/PARALLEL_SPEC.md section 7): buffered `engine.world.set` with
//! read-your-own-writes, the merge in lane order, lanes joined by provides, spawn pools.

use std::sync::{Arc, Mutex};

use efeengine_core::efevm::ValType::{F32, I64};
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;

type Reports = Arc<Mutex<Vec<i64>>>;

fn engine_with(threads: Threads) -> (Engine, Reports) {
    let cfg = EngineConfig { fixed_dt: 1.0 / 60.0, threads, ..EngineConfig::headless() };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    e.define_namespace("test", 1, "testgame", "Helpers of the test game.").unwrap();
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(
        HostFn::new("test", "report").doc("Record `v` for the test.").param("v", ApiType::I64),
        move |_c, a, _r| {
            g.lock().unwrap().push(a.i64(0)?);
            Ok(HostOk::Continue)
        },
    )
    .unwrap();
    (e, got)
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn import(mb: &mut ModuleBuilder, e: &Engine, ns: &str, name: &str) -> u32 {
    let f = e.api_index().function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is not in the index")).func_type();
    mb.import(ns, name, f)
}

fn tid(e: &Engine, name: &str) -> i32 {
    e.types().by_name(name).unwrap().id() as i32
}

/// `frame(dt)`: set `Transform.position.x = x` of entity `ent`; with `report`, read it back
/// afterwards and report the f32 bits (read-your-own-writes).
fn setter(e: &Engine, ent: Entity, x: f32, report: bool) -> Module {
    let mut mb = ModuleBuilder::new();
    let get = import(&mut mb, e, "engine.world", "get");
    let set = import(&mut mb, e, "engine.world", "set");
    let rep = import(&mut mb, e, "test", "report");
    mb.memory(1, None);
    let (t, bits) = (tid(e, "Transform"), entity_to_bits(ent) as i64);
    let mut f = FunctionBuilder::new(64);
    f.const_i64(10, bits);
    f.const_i32(11, t);
    f.const_i64(12, 1024);
    f.call(get, 10);
    f.const_i64(13, 0);
    f.const_f32(14, x);
    f.store(Op::F32Store, 14, 13, 1024);
    f.const_i64(10, bits);
    f.const_i32(11, t);
    f.const_i64(12, 1024);
    f.call(set, 10);
    if report {
        f.const_i64(10, bits);
        f.const_i32(11, t);
        f.const_i64(12, 1024);
        f.call(get, 10);
        f.load(Op::I32Load, 14, 13, 1024);
        f.op2(Op::I64ExtendI32S, 60, 14);
        f.call(rep, 60);
    }
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

/// `frame(dt)`: report the f32 bits of `Transform.position.x` of `ent`.
fn reader(e: &Engine, ent: Entity) -> Module {
    let mut mb = ModuleBuilder::new();
    let get = import(&mut mb, e, "engine.world", "get");
    let rep = import(&mut mb, e, "test", "report");
    mb.memory(1, None);
    let (t, bits) = (tid(e, "Transform"), entity_to_bits(ent) as i64);
    let mut f = FunctionBuilder::new(64);
    f.const_i64(10, bits);
    f.const_i32(11, t);
    f.const_i64(12, 1024);
    f.call(get, 10);
    f.const_i64(13, 0);
    f.load(Op::I32Load, 14, 13, 1024);
    f.op2(Op::I64ExtendI32S, 60, 14);
    f.call(rep, 60);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

/// `frame(dt)`: `n` times `engine.world.spawn` + `insert Transform` (x = `marker`, from memory
/// that the module fills); with `report`, reports the last handle.
fn spawner(e: &Engine, n: usize, marker: f32, report_last: bool) -> Module {
    let mut mb = ModuleBuilder::new();
    let spawn = import(&mut mb, e, "engine.world", "spawn");
    let insert = import(&mut mb, e, "engine.world", "insert");
    let rep = import(&mut mb, e, "test", "report");
    mb.memory(1, None);
    let t = tid(e, "Transform");
    let mut f = FunctionBuilder::new(64);
    f.const_i64(13, 0);
    f.const_f32(14, marker);
    f.store(Op::F32Store, 14, 13, 1024);
    for _ in 0..n {
        f.call(spawn, 20);
        f.mov(30, 20);
        f.const_i32(31, t);
        f.const_i64(32, 1024);
        f.call(insert, 30);
        if report_last {
            f.mov(60, 20);
        }
    }
    if report_last {
        f.call(rep, 60);
    }
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

fn x_of(e: &Engine, ent: Entity) -> f32 {
    e.world().get::<&Transform>(ent).unwrap().position.x
}

#[test]
fn a_lane_reads_its_own_writes_and_another_lane_sees_the_snapshot() {
    for threads in [Threads::Single, Threads::Fixed(4)] {
        let (mut e, got) = engine_with(threads);
        let ent = e.world_mut().spawn((Transform::from_position(Vec2::new(1.0, 0.0)),));
        // module 0 writes 5 and reads it back; module 1 (another lane) only reads
        e.load_module("w", &setter(&e, ent, 5.0, true).encode()).unwrap();
        e.load_module("r", &reader(&e, ent).encode()).unwrap();
        e.run_frames(1, 1.0 / 60.0);
        assert!(errors(&e).is_empty(), "{:?}", errors(&e));
        assert_eq!(e.script_stats().lanes, 2);
        let mut v = got.lock().unwrap().clone();
        // `w` read back its own 5.0; `r` still saw the start-of-frame 1.0 (order of the two reports
        // depends on the threads, the values do not)
        v.sort();
        assert_eq!(v, [1.0f32.to_bits() as i64, 5.0f32.to_bits() as i64], "{threads:?}");
        assert_eq!(x_of(&e, ent), 5.0);
        // next frame the write is visible to everyone
        got.lock().unwrap().clear();
        e.run_frames(1, 1.0 / 60.0);
        assert_eq!(*got.lock().unwrap().iter().min().unwrap(), 5.0f32.to_bits() as i64);
    }
}

#[test]
fn two_modules_setting_the_same_component_count_a_conflict_and_the_higher_id_wins() {
    for threads in [Threads::Single, Threads::Fixed(4)] {
        let (mut e, _got) = engine_with(threads);
        let ent = e.world_mut().spawn((Transform::default(),));
        e.load_module("a", &setter(&e, ent, 3.0, false).encode()).unwrap();
        e.load_module("b", &setter(&e, ent, 7.0, false).encode()).unwrap();
        e.run_frames(1, 1.0 / 60.0);
        assert_eq!(e.script_stats().write_conflicts, 1, "{threads:?}");
        assert_eq!(x_of(&e, ent), 7.0, "{threads:?}");
        assert!(e.scripting().log_lines().any(|l| l.level == LogLevel::Warn && l.text.contains("write conflicts")));
        // the counter is per frame
        e.run_frames(1, 1.0 / 60.0);
        assert_eq!(e.script_stats().write_conflicts, 1);
    }
}

#[test]
fn modules_that_write_different_entities_do_not_conflict() {
    let (mut e, _got) = engine_with(Threads::Fixed(2));
    let a = e.world_mut().spawn((Transform::default(),));
    let b = e.world_mut().spawn((Transform::default(),));
    e.load_module("a", &setter(&e, a, 3.0, false).encode()).unwrap();
    e.load_module("b", &setter(&e, b, 7.0, false).encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(e.script_stats().write_conflicts, 0);
    assert_eq!((x_of(&e, a), x_of(&e, b)), (3.0, 7.0));
}

/// A provider module `p` (`id(x) -> x`) and a module importing it.
fn provider() -> Module {
    use efevm::{ApiType, Interface, Param, Provide};
    let mut mb = ModuleBuilder::new();
    let mut f = FunctionBuilder::new(2);
    f.ret(0);
    let id = mb.add_func(FuncType::new([I64], [I64]), f);
    mb.interface(Interface {
        provides: vec![Provide {
            name: "id".into(),
            func: id,
            doc: "Identity.".into(),
            params: vec![Param::new("x", ApiType::I64)],
            results: vec![Param::new("y", ApiType::I64)],
        }],
        ..Default::default()
    });
    mb.finish().unwrap()
}

fn importer() -> Module {
    let mut mb = ModuleBuilder::new();
    let id = mb.import("p", "id", FuncType::new([I64], [I64]));
    let mut f = FunctionBuilder::new(4);
    f.const_i64(2, 1);
    f.call(id, 2);
    f.ret(0);
    let fid = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", fid);
    mb.finish().unwrap()
}

fn idle() -> Module {
    let mut mb = ModuleBuilder::new();
    let mut f = FunctionBuilder::new(2);
    f.ret(0);
    let fid = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", fid);
    mb.finish().unwrap()
}

#[test]
fn modules_joined_by_provides_share_a_lane() {
    let (mut e, _got) = engine_with(Threads::Fixed(4));
    let p = e.load_mod(&provider().encode(), ModOptions::new("p")).unwrap();
    let z = e.load_module("z", &idle().encode()).unwrap();
    let q = e.load_mod(&importer().encode(), ModOptions::new("q").allow("p")).unwrap();
    assert_eq!(e.scripting().lanes(), vec![vec![p, q], vec![z]]);
    e.run_frames(2, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(e.script_stats().lanes, 2);
    // unloading the dependent splits nothing but removes it from the lane
    e.unload_module(q).unwrap();
    assert_eq!(e.scripting().lanes(), vec![vec![p], vec![z]]);
}

/// Handles and the component data of every entity, sorted.
fn world_dump(e: &Engine) -> Vec<(u64, u32)> {
    let mut v: Vec<(u64, u32)> = e.world().query::<(Entity, &Transform)>().iter().map(|(en, t)| (entity_to_bits(en), t.position.x.to_bits())).collect();
    v.sort();
    v
}

#[test]
fn spawn_ids_are_equal_for_single_and_fixed_4() {
    let run = |threads| {
        let (mut e, _got) = engine_with(threads);
        for (i, name) in ["s0", "s1", "s2", "s3"].iter().enumerate() {
            e.load_module(name, &spawner(&e, 3 + i, 10.0 + i as f32, false).encode()).unwrap();
        }
        e.run_frames(3, 1.0 / 60.0);
        assert!(errors(&e).is_empty(), "{:?}", errors(&e));
        world_dump(&e)
    };
    let single = run(Threads::Single);
    assert_eq!(single.len(), 3 * (3 + 4 + 5 + 6));
    for _ in 0..5 {
        assert_eq!(run(Threads::Fixed(4)), single);
    }
}

#[test]
fn an_empty_spawn_pool_returns_the_null_handle_and_counts_the_overflow() {
    let (mut e, got) = engine_with(Threads::Single);
    e.load_module("many", &spawner(&e, 70, 1.0, true).encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(e.script_stats().spawn_overflows, 6, "pool of 64, 70 spawns");
    assert_eq!(*got.lock().unwrap(), [0], "the last handle is the null handle");
    assert_eq!(world_dump(&e).len(), 64);
    assert!(e.scripting().log_lines().any(|l| l.level == LogLevel::Warn && l.text.contains("spawn pool is empty")));
    // the pool follows the demand: 2 * 70 next frame
    got.lock().unwrap().clear();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(e.script_stats().spawn_overflows, 0);
    assert_ne!(got.lock().unwrap()[0], 0);
    assert_eq!(world_dump(&e).len(), 64 + 70);
}

#[test]
fn unused_pool_entities_do_not_stay_alive() {
    let (mut e, _got) = engine_with(Threads::Single);
    e.load_module("idle", &idle().encode()).unwrap();
    e.run_frames(3, 1.0 / 60.0);
    assert_eq!(e.world().len(), 0);
}

#[test]
fn engine_random_is_per_module_and_independent_of_threads() {
    // two modules each draw one number per frame; the draws do not depend on the other module
    let draws = |threads, with_other: bool| {
        let (mut e, got) = engine_with(threads);
        let draw = |tag: i64, e: &Engine| {
            let mut mb = ModuleBuilder::new();
            let r = import(&mut mb, e, "engine.random", "range");
            let rep = import(&mut mb, e, "test", "report");
            let mut f = FunctionBuilder::new(64);
            f.const_i64(10, 1_000_000);
            f.call(r, 10);
            f.const_i64(11, tag);
            f.op3(Op::I64Mul, 12, 10, 11);
            f.mov(60, 12);
            f.call(rep, 60);
            f.ret(0);
            let id = mb.add_func(FuncType::new([F32], []), f);
            mb.export_func("frame", id);
            mb.finish().unwrap()
        };
        e.load_module("d1", &draw(1, &e).encode()).unwrap();
        if with_other {
            e.load_module("d2", &draw(1_000_000_007, &e).encode()).unwrap();
        }
        e.run_frames(4, 1.0 / 60.0);
        let v = got.lock().unwrap().clone();
        // d1's draws are the values below 1_000_000
        let mut mine: Vec<i64> = v.into_iter().filter(|x| *x < 1_000_000).collect();
        mine.sort();
        mine
    };
    let alone = draws(Threads::Single, false);
    assert_eq!(alone.len(), 4);
    assert_eq!(draws(Threads::Fixed(4), true), alone);
}
