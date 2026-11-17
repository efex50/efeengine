//! Change detection (docs/THREADING.md, "Change ticks"): `Changed` / `Added` filters through
//! `Ctx::query` and `ParCtx::query`, command ticks, and `engine.world.changed` / `tick`.

use std::sync::{Arc, Mutex};

use efeengine_core::efevm::ValType::F32;
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
struct Hp {
    v: f32,
    _pad: u32,
}

type Log = Arc<Mutex<Vec<(u32, usize, usize)>>>;

fn engine() -> Engine {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
    e.register_component::<Hp>().unwrap();
    e.track_changes::<Hp>();
    e
}

/// An Update system that logs `(frame, #Changed<Hp>, #Added<Hp>)` each run.
fn watcher(e: &mut Engine, log: &Log) {
    let log = log.clone();
    let mut frame = 0u32;
    e.add_system(Stage::Update, move |cx| {
        let c = cx.query::<(Entity, Changed<Hp>)>().iter().count();
        let a = cx.query::<(Entity, Added<Hp>)>().iter().count();
        log.lock().unwrap().push((frame, c, a));
        frame += 1;
    });
}

#[test]
fn exclusive_system_sees_spawn_once_then_writes() {
    let mut e = engine();
    let log: Log = Default::default();
    watcher(&mut e, &log);
    let ent = e.world_mut().spawn((Hp::default(),));
    // a writer that runs after the watcher, in frame 2 only
    let mut n = 0;
    e.add_system(Stage::PostUpdate, move |cx| {
        if n == 2 {
            cx.world.get::<&mut Hp>(ent).unwrap().v = 5.0;
        }
        n += 1;
    });
    e.run_frames(5, 1.0 / 60.0);
    // frame 0: spawn is added and changed; frame 1: nothing; frame 3: the write of frame 2
    // is a change, not an add.
    assert_eq!(*log.lock().unwrap(), [(0, 1, 1), (1, 0, 0), (2, 0, 0), (3, 1, 0), (4, 0, 0)]);
}

#[test]
fn commands_insert_is_added_on_the_next_run() {
    let mut e = engine();
    let log: Log = Default::default();
    watcher(&mut e, &log);
    let ent = e.world_mut().spawn((Transform::default(),));
    let mut n = 0;
    e.add_system(Stage::Update, move |cx| {
        if n == 1 {
            cx.commands.insert(ent, Hp::default());
        }
        n += 1;
    });
    e.run_frames(4, 1.0 / 60.0);
    assert_eq!(*log.lock().unwrap(), [(0, 0, 0), (1, 0, 0), (2, 1, 1), (3, 0, 0)]);
}

#[test]
fn ctx_ticks_and_changed_helper() {
    let mut e = engine();
    let ent = e.world_mut().spawn((Hp::default(),));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let mut n = 0;
    e.add_system(Stage::Update, move |cx| {
        s.lock().unwrap().push((cx.last_run().get(), cx.this_run().get(), cx.changed::<Hp>(ent)));
        if n == 1 {
            // writing through the ctx query stamps this system's own tick, which its next run
            // does not count as new (a system never sees its own writes as changes)
            for h in cx.query::<&mut Hp>().iter() {
                h.v = 1.0;
            }
        }
        n += 1;
    });
    // a later system writes in frame 2: the watcher sees it on its run in frame 3
    let mut m = 0;
    e.add_system(Stage::PostUpdate, move |cx| {
        if m == 2 {
            cx.world.get::<&mut Hp>(ent).unwrap().v = 9.0;
        }
        m += 1;
    });
    e.run_frames(5, 1.0 / 60.0);
    let v = seen.lock().unwrap().clone();
    assert_eq!(v[0].0, 0, "first run: last_run is 0");
    assert!(v[0].2 && !v[1].2, "{v:?}");
    assert!(!v[2].2, "own write of run 1 is not new for run 2: {v:?}");
    assert!(v[3].2 && !v[4].2, "write of a later system in frame 2 shows in frame 3: {v:?}");
    for w in v.windows(2) {
        assert!(w[1].0 == w[0].1 && w[1].1 > w[0].1, "last_run is the previous this_run: {v:?}");
    }
}

#[test]
fn par_writer_then_par_reader_in_a_later_batch() {
    for threads in [Threads::Single, Threads::Fixed(4)] {
        let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, threads, ..EngineConfig::headless() }).unwrap();
        e.register_component::<Hp>().unwrap();
        e.track_changes::<Hp>();
        let ents: Vec<Entity> = (0..10).map(|_| e.world_mut().spawn((Hp::default(), Transform::default()))).collect();
        let mut n = 0u32;
        // writer: touches the even entities on odd frames
        let evens: Vec<Entity> = ents.iter().copied().step_by(2).collect();
        e.add_par_system(Stage::Update, Access::new().named("writer").write::<Hp>(), move |cx| {
            if n % 2 == 1 {
                for ent in &evens {
                    cx.get::<&mut Hp>(*ent).unwrap().v += 1.0;
                }
            }
            n += 1;
        });
        // unrelated par system in the same batch as the writer
        e.add_par_system(Stage::Update, Access::new().named("other").write::<Transform>(), |cx| {
            for t in cx.query::<&mut Transform>().iter() {
                t.position.x += 1.0;
            }
        });
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        e.add_par_system(Stage::Update, Access::new().named("reader").read::<Hp>(), move |cx| {
            let c = cx.query::<(Entity, &Hp, Changed<Hp>)>().iter().count();
            l.lock().unwrap().push((c, cx.last_run().get(), cx.this_run().get()));
        });
        assert_eq!(e.batch_count(Stage::Update), 3, "script system, writer + other, reader");
        e.run_frames(5, 1.0 / 60.0);
        let v = log.lock().unwrap().clone();
        let counts: Vec<usize> = v.iter().map(|x| x.0).collect();
        // frame 0: everything is new; frame 1: writer ran before the reader in the same frame
        assert_eq!(counts, [10, 5, 0, 5, 0], "{threads:?}");
        for w in v.windows(2) {
            assert_eq!(w[1].1, w[0].2, "last_run is the previous this_run");
        }
    }
}

// ---- scripts ----

fn import(mb: &mut ModuleBuilder, e: &Engine, name: &str) -> u32 {
    let f = e.api_index().function("engine.world", name).unwrap().func_type();
    mb.import("engine.world", name, f)
}

type Reports = Arc<Mutex<Vec<i64>>>;

fn engine_with_report() -> (Engine, Reports) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).unwrap();
    e.define_namespace("test", 1, "testgame", "Helpers.").unwrap();
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(HostFn::new("test", "report").doc("Record.").param("v", ApiType::I64), move |_c, a, _r| {
        g.lock().unwrap().push(a.i64(0)?);
        Ok(HostOk::Continue)
    })
    .unwrap();
    (e, got)
}

/// `frame(dt)`: report `changed(entity, Transform, mem[4096])`, then store `tick()` at 4096.
fn probe(e: &Engine, ent: Entity) -> Module {
    let mut mb = ModuleBuilder::new();
    let changed = import(&mut mb, e, "changed");
    let tick = import(&mut mb, e, "tick");
    let idx = e.api_index();
    let report = mb.import("test", "report", idx.function("test", "report").unwrap().func_type());
    mb.memory(1, None);
    let t = e.types().by_name("Transform").unwrap().id() as i32;
    let mut f = FunctionBuilder::new(64);
    f.const_i64(10, entity_to_bits(ent) as i64);
    f.const_i32(11, t);
    f.const_i64(13, 0);
    f.load(Op::I32Load, 12, 13, 4096);
    f.call(changed, 10);
    f.op2(Op::I64ExtendI32S, 60, 10);
    f.call(report, 60);
    f.call(tick, 20);
    f.const_i64(13, 0);
    f.store(Op::I32Store, 20, 13, 4096);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

#[test]
fn script_changed_alternates_with_a_rust_mover() {
    let (mut e, got) = engine_with_report();
    let ent = e.world_mut().spawn((Transform::default(),));
    // moves the entity in PreUpdate of every other frame
    let mut n = 0;
    e.add_system(Stage::PreUpdate, move |cx| {
        if n % 2 == 0 {
            cx.world.get::<&mut Transform>(ent).unwrap().position.x += 1.0;
        }
        n += 1;
    });
    e.load_module("probe", &probe(&e, ent).encode()).unwrap();
    e.run_frames(6, 1.0 / 60.0);
    let errs: Vec<_> = e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect();
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(*got.lock().unwrap(), [1, 0, 1, 0, 1, 0]);
}

#[test]
fn script_changed_status_codes() {
    let (mut e, got) = engine_with_report();
    let live = e.world_mut().spawn((Transform::default(),));
    let stale = e.world_mut().spawn((Transform::default(),));
    e.world_mut().despawn(stale).unwrap();
    let t = e.types().by_name("Transform").unwrap().id() as i32;
    let vel = e.types().by_name("Velocity").unwrap().id() as i32;
    let mut mb = ModuleBuilder::new();
    let changed = import(&mut mb, &e, "changed");
    let tick = import(&mut mb, &e, "tick");
    let report = mb.import("test", "report", e.api_index().function("test", "report").unwrap().func_type());
    mb.memory(1, None);
    let mut f = FunctionBuilder::new(64);
    let call = |f: &mut FunctionBuilder, bits: u64, id: i32, since: i32| {
        f.const_i64(10, bits as i64);
        f.const_i32(11, id);
        f.const_i32(12, since);
        f.call(changed, 10);
        f.op2(Op::I64ExtendI32S, 60, 10);
        f.call(report, 60);
    };
    call(&mut f, entity_to_bits(stale), t, 0); // -1
    call(&mut f, entity_to_bits(live), vel, 0); // -2
    call(&mut f, entity_to_bits(live), 0x1234, 0); // -3
    call(&mut f, entity_to_bits(live), t, 0); // 1
    f.call(tick, 20);
    f.op2(Op::I64ExtendI32S, 60, 20);
    f.call(report, 60);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    e.load_module("status", &mb.finish().unwrap().encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    let g = got.lock().unwrap().clone();
    assert_eq!(&g[..4], [-1, -2, -3, 1]);
    assert!(g[4] >= 2 && g[4] <= e.change_tick().get() as i64);
}
