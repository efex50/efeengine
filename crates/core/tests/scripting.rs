//! Scripting through the engine (E5.1-E5.3): headless, modules built with `ModuleBuilder`.

use std::sync::{Arc, Mutex};

use efeengine_core::efevm::ValType::{F32, I64};
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;

fn engine() -> Engine {
    Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine")
}

type Reports = Arc<Mutex<Vec<i64>>>;

/// An engine with a game namespace `test` and `test.report(v: i64)` collecting values.
fn engine_with_report() -> (Engine, Reports) {
    let mut e = engine();
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

fn warnings(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Warn).map(|l| l.text.clone()).collect()
}

/// The import of `ns.name`, typed from the engine's index (as an assembler with `--api` would).
fn import(mb: &mut ModuleBuilder, e: &Engine, ns: &str, name: &str) -> u32 {
    let idx = e.api_index();
    let f = idx.function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is not in the index"));
    mb.import(ns, name, f.func_type())
}

enum A {
    I32(i32),
    I64(i64),
}

/// Constants into `base..`, then `call func, base`; the result (if any) is in `base`.
fn call_with(f: &mut FunctionBuilder, func: u32, base: u16, args: &[A]) {
    for (i, a) in args.iter().enumerate() {
        let r = base + i as u16;
        match a {
            A::I32(v) => f.const_i32(r, *v),
            A::I64(v) => f.const_i64(r, *v),
        }
    }
    f.call(func, base);
}

/// Report the `i32` in register `src`.
fn report_i32(f: &mut FunctionBuilder, report: u32, src: u16) {
    f.op2(Op::I64ExtendI32S, 60, src);
    f.call(report, 60);
}

/// Report the `i64` in register `src`.
fn report_i64(f: &mut FunctionBuilder, report: u32, src: u16) {
    f.mov(60, src);
    f.call(report, 60);
}

fn tid(e: &Engine, name: &str) -> i32 {
    e.types().by_name(name).unwrap().id() as i32
}

/// `update(entity, dt, state) -> state + 1`: adds `dx` to the entity's `Transform.position.x`
/// with engine.world.get / set.
fn mover(e: &Engine, dx: f32) -> Module {
    let mut mb = ModuleBuilder::new();
    let get = import(&mut mb, e, "engine.world", "get");
    let set = import(&mut mb, e, "engine.world", "set");
    mb.memory(1, None);
    let t = tid(e, "Transform");
    let mut f = FunctionBuilder::new(64);
    f.mov(10, 0);
    f.const_i32(11, t);
    f.const_i64(12, 1024);
    f.call(get, 10);
    f.const_i64(13, 0);
    f.load(Op::F32Load, 14, 13, 1024);
    f.const_f32(15, dx);
    f.op3(Op::F32Add, 14, 14, 15);
    f.store(Op::F32Store, 14, 13, 1024);
    f.mov(10, 0);
    f.const_i32(11, t);
    f.const_i64(12, 1024);
    f.call(set, 10);
    f.const_i64(17, 1);
    f.op3(Op::I64Add, 16, 2, 17);
    f.ret(16);
    let id = mb.add_func(FuncType::new([I64, F32, I64], [I64]), f);
    mb.export_func("update", id);
    mb.finish().unwrap()
}

/// A module whose `frame(dt)` is built by `body`; imports are declared by `imports`.
fn with_frame(imports: impl FnOnce(&mut ModuleBuilder) -> Vec<u32>, body: impl FnOnce(&mut FunctionBuilder, &[u32])) -> Module {
    let mut mb = ModuleBuilder::new();
    let fns = imports(&mut mb);
    mb.memory(1, None);
    let mut f = FunctionBuilder::new(64);
    body(&mut f, &fns);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

#[test]
fn script_moves_its_entitys_transform() {
    let mut e = engine();
    let id = e.load_module("mover", &mover(&e, 1.5).encode()).unwrap();
    let ent = e.world_mut().spawn((Transform::default(), Script::new(id, "update")));
    e.run_frames(5, 1.0 / 60.0);
    assert_eq!(e.world().get::<&Transform>(ent).unwrap().position, Vec2::new(7.5, 0.0));
    assert_eq!(e.world().get::<&Script>(ent).unwrap().state, 5);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    // the entity of another script is independent
    let other = e.world_mut().spawn((Transform::from_position(Vec2::new(10.0, 3.0)), Script::new(id, "update")));
    e.run_frames(2, 1.0 / 60.0);
    assert_eq!(e.world().get::<&Transform>(other).unwrap().position, Vec2::new(13.0, 3.0));
    assert_eq!(e.world().get::<&Script>(other).unwrap().state, 2);
}

#[test]
fn spawn_and_insert_apply_after_the_stage() {
    let mut e = engine();
    let vid = tid(&e, "Velocity");
    let m = {
        let mut mb = ModuleBuilder::new();
        let spawn = import(&mut mb, &e, "engine.world", "spawn");
        let insert = import(&mut mb, &e, "engine.world", "insert");
        mb.memory(1, None);
        let v: Vec<u8> = [1.0f32, 2.0, 3.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        mb.data(2048, &v);
        let mut f = FunctionBuilder::new(32);
        f.call(spawn, 5);
        f.mov(6, 5);
        f.const_i32(7, vid);
        f.const_i64(8, 2048);
        f.call(insert, 6);
        f.ret(0);
        let id = mb.add_func(FuncType::new([F32], []), f);
        mb.export_func("frame", id);
        mb.finish().unwrap()
    };
    e.load_module("spawner", &m.encode()).unwrap();
    let seen = Arc::new(Mutex::new(Vec::<(&'static str, usize)>::new()));
    for (stage, name) in [(Stage::Update, "update"), (Stage::PostUpdate, "post")] {
        let s = seen.clone();
        e.add_system(stage, move |cx| s.lock().unwrap().push((name, cx.world.query::<&Velocity>().iter().count())));
    }
    e.run_frames(1, 1.0 / 60.0);
    // not visible in the Update stage that spawned it, visible in the next stage
    assert_eq!(*seen.lock().unwrap(), [("update", 0), ("post", 1)]);
    let (_, v) = e.world_mut().query_mut::<(Entity, &Velocity)>().into_iter().next().map(|(a, b)| (a, *b)).unwrap();
    assert_eq!(v, Velocity { linear: Vec2::new(1.0, 2.0), angular: 3.0 });
}

#[test]
fn stale_forged_and_unknown_handles_give_status_codes() {
    let (mut e, got) = engine_with_report();
    let live = e.world_mut().spawn((Transform::default(),));
    let stale = e.world_mut().spawn((Transform::default(),));
    e.world_mut().despawn(stale).unwrap();
    let (live_b, stale_b) = (entity_to_bits(live) as i64, entity_to_bits(stale) as i64);
    let (t, v) = (tid(&e, "Transform"), tid(&e, "Velocity"));
    let m = with_frame(
        |mb| {
            let mut v = vec![];
            for n in ["get", "set", "despawn", "alive", "has", "query", "component_id"] {
                v.push(import(mb, &e, "engine.world", n));
            }
            v.push(import(mb, &e, "test", "report"));
            mb.data(3000, b"Transform");
            v
        },
        |f, fns| {
            let (get, set, despawn, alive, has, query, cid, report) = (fns[0], fns[1], fns[2], fns[3], fns[4], fns[5], fns[6], fns[7]);
            call_with(f, get, 10, &[A::I64(stale_b), A::I32(t), A::I64(1024)]);
            report_i32(f, report, 10); // 1
            call_with(f, set, 10, &[A::I64(stale_b), A::I32(t), A::I64(1024)]);
            report_i32(f, report, 10); // 1
            call_with(f, despawn, 10, &[A::I64(stale_b)]);
            report_i32(f, report, 10); // 1
            call_with(f, alive, 10, &[A::I64(stale_b)]);
            report_i32(f, report, 10); // 0
            call_with(f, alive, 10, &[A::I64(live_b)]);
            report_i32(f, report, 10); // 1
            call_with(f, get, 10, &[A::I64(live_b), A::I32(0x1234), A::I64(1024)]);
            report_i32(f, report, 10); // 3 unknown id
            call_with(f, get, 10, &[A::I64(live_b), A::I32(v), A::I64(1024)]);
            report_i32(f, report, 10); // 2 missing
            call_with(f, get, 10, &[A::I64(0), A::I32(t), A::I64(1024)]);
            report_i32(f, report, 10); // forged bits 0 -> 1
            call_with(f, get, 10, &[A::I64(-1), A::I32(t), A::I64(1024)]);
            report_i32(f, report, 10); // forged bits all ones -> 1
            call_with(f, has, 10, &[A::I64(live_b), A::I32(t)]);
            report_i32(f, report, 10); // 1
            call_with(f, query, 10, &[A::I32(0x1234), A::I64(2048), A::I64(4)]);
            report_i64(f, report, 10); // -1
            call_with(f, query, 10, &[A::I32(t), A::I64(2048), A::I64(4)]);
            report_i64(f, report, 10); // 1 (only the live entity)
            call_with(f, cid, 10, &[A::I64(3000), A::I64(9)]);
            report_i32(f, report, 10); // Transform id
            call_with(f, get, 10, &[A::I64(live_b), A::I32(t), A::I64(1024)]);
            report_i32(f, report, 10); // 0
        },
    );
    e.load_module("probe", &m.encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(*got.lock().unwrap(), [1, 1, 1, 0, 1, 3, 2, 1, 1, 1, -1, 1, t as i64, 0]);
}

#[test]
fn guest_out_of_bounds_pointer_traps_and_the_engine_keeps_running() {
    let (mut e, got) = engine_with_report();
    let live = e.world_mut().spawn((Transform::default(),));
    let (live_b, t) = (entity_to_bits(live) as i64, tid(&e, "Transform"));
    let m = with_frame(
        |mb| vec![import(mb, &e, "engine.world", "get"), import(mb, &e, "test", "report")],
        |f, fns| {
            call_with(f, fns[0], 10, &[A::I64(live_b), A::I32(t), A::I64(0x7fff_ffff_0000)]);
            report_i32(f, fns[1], 10); // never reached
        },
    );
    e.load_module("bad", &m.encode()).unwrap();
    let frames = Arc::new(Mutex::new(0u32));
    let fr = frames.clone();
    e.add_system(Stage::PostUpdate, move |_| *fr.lock().unwrap() += 1);
    e.run_frames(3, 1.0 / 60.0);
    assert_eq!(*frames.lock().unwrap(), 3);
    assert!(e.is_running());
    assert!(got.lock().unwrap().is_empty());
    let errs = errors(&e);
    assert_eq!(errs.len(), 3, "{errs:?}");
    assert!(errs[0].contains("[bad]") && errs[0].contains("out of bounds"), "{}", errs[0]);
}

#[test]
fn events_read_with_a_negative_buffer_traps() {
    let mut e = engine();
    let m = with_frame(
        |mb| {
            mb.data(3000, b"ping");
            vec![import(mb, &e, "engine.events", "read")]
        },
        |f, fns| {
            // name = (3000, 4), index 0, buf = (4096, -1): the length has bit 63 set
            call_with(f, fns[0], 10, &[A::I64(3000), A::I64(4), A::I64(0), A::I64(4096), A::I64(-1)]);
        },
    );
    e.load_module("bad", &m.encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    let errs = errors(&e);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("[bad]") && errs[0].contains("out of bounds"), "{}", errs[0]);
}

#[test]
fn world_query_checks_the_output_range_even_without_matches() {
    let (mut e, got) = engine_with_report();
    // no entity has a Transform, so nothing would be written
    let t = tid(&e, "Transform");
    let m = with_frame(
        |mb| vec![import(mb, &e, "engine.world", "query"), import(mb, &e, "test", "report")],
        |f, fns| {
            // cap <= 0 writes nothing and needs no range: returns 0
            call_with(f, fns[0], 10, &[A::I32(t), A::I64(65530), A::I64(-1)]);
            report_i64(f, fns[1], 10);
            // cap 4 at 65530 ends past the 64 KiB memory: traps, with no match
            call_with(f, fns[0], 10, &[A::I32(t), A::I64(65530), A::I64(4)]);
            report_i64(f, fns[1], 10); // never reached
        },
    );
    e.load_module("q", &m.encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(*got.lock().unwrap(), [0]);
    let errs = errors(&e);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("out of bounds"), "{}", errs[0]);
}

#[test]
fn budget_exhaustion_skips_the_module_but_not_the_frame() {
    let mut e = engine();
    let spin = {
        let mut mb = ModuleBuilder::new();
        let mut f = FunctionBuilder::new(4);
        let top = f.new_label();
        f.bind(top);
        f.jmp(top);
        let id = mb.add_func(FuncType::new([F32], []), f);
        mb.export_func("frame", id);
        mb.finish().unwrap()
    };
    let spin_id = e.load_module("spin", &spin.encode()).unwrap();
    e.scripting_mut().set_budget(spin_id, 10_000).unwrap();
    let mover_id = e.load_module("mover", &mover(&e, 1.0).encode()).unwrap();
    let ent = e.world_mut().spawn((Transform::default(), Script::new(mover_id, "update")));
    let frames = Arc::new(Mutex::new(0u32));
    let fr = frames.clone();
    e.add_system(Stage::PostUpdate, move |_| *fr.lock().unwrap() += 1);
    e.run_frames(4, 1.0 / 60.0);
    assert_eq!(*frames.lock().unwrap(), 4);
    assert_eq!(e.world().get::<&Transform>(ent).unwrap().position.x, 4.0, "the other module still runs");
    let w = warnings(&e);
    assert_eq!(w.len(), 4, "{w:?}");
    assert!(w[0].contains("[spin]") && w[0].contains("budget of 10000"), "{}", w[0]);
}

#[test]
fn a_script_with_an_entry_that_runs_out_skips_the_modules_other_scripts() {
    // An entry that loops forever: the first script burns the module's budget, so the second
    // is skipped this frame (the state of the second stays 0).
    let mut e = engine();
    let m = {
        let mut mb = ModuleBuilder::new();
        let mut f = FunctionBuilder::new(8);
        let top = f.new_label();
        f.bind(top);
        f.jmp(top);
        let id = mb.add_func(FuncType::new([I64, F32, I64], [I64]), f);
        mb.export_func("update", id);
        mb.finish().unwrap()
    };
    let id = e.load_module("loop", &m.encode()).unwrap();
    e.scripting_mut().set_budget(id, 5_000).unwrap();
    e.world_mut().spawn((Script::new(id, "update"),));
    e.world_mut().spawn((Script::new(id, "update"),));
    e.run_frames(2, 0.016);
    assert_eq!(warnings(&e).len(), 2, "one warning per frame, the second script is not even tried");
}

#[test]
fn disallowed_namespace_and_link_errors_are_reported_by_name() {
    let mut e = engine();
    let m = {
        let mut mb = ModuleBuilder::new();
        import(&mut mb, &e, "engine.world", "spawn");
        import(&mut mb, &e, "engine.log", "info");
        mb.memory(1, None);
        mb.finish().unwrap()
    };
    let err = e.load_mod(&m.encode(), ModOptions::new("greedy").allow("engine.log")).unwrap_err();
    assert!(matches!(&err, ScriptError::Disallowed { namespaces, .. } if namespaces == &["engine.world".to_string()]), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("greedy") && text.contains("engine.world") && !text.contains("engine.log"), "{text}");
    // a prefix must end at a dot boundary
    assert!(e.load_mod(&m.encode(), ModOptions::new("m2").allow("engine.wor")).is_err());
    // allowed: loads
    e.load_mod(&m.encode(), ModOptions::new("fine").allow("engine")).unwrap();

    // link errors: all problems at once
    let bad = {
        let mut mb = ModuleBuilder::new();
        mb.import("nope", "missing", FuncType::new([], []));
        mb.import("engine.time", "delta", FuncType::new([], [I64])); // host: () -> f32
        mb.memory(1, None);
        mb.finish().unwrap()
    };
    let err = e.load_module("linker", &bad.encode()).unwrap_err();
    let ScriptError::Link { errors, .. } = &err else { panic!("{err:?}") };
    assert_eq!(errors.len(), 2, "{errors:?}");
    let text = err.to_string();
    assert!(text.contains("nope.missing") && text.contains("engine.time.delta"), "{text}");
    // names are checked too
    assert!(matches!(e.load_module("not a name", &bad.encode()), Err(ScriptError::InvalidName(_))));
    assert!(matches!(e.load_module("engine", &m.encode()), Err(ScriptError::NameTaken(_))));
}

/// A mod `adder` with `add(a, b) -> a + b`, exported with `engine.mods.export` from `init`.
fn adder(e: &Engine) -> Module {
    let mut mb = ModuleBuilder::new();
    let export = import(&mut mb, e, "engine.mods", "export");
    mb.memory(1, None);
    mb.data(100, b"add");
    mb.data(110, b"Add two numbers.");
    let mut init = FunctionBuilder::new(8);
    call_with(&mut init, export, 0, &[A::I64(100), A::I64(3), A::I64(110), A::I64(16)]);
    init.ret(0);
    let init_id = mb.add_func(FuncType::new([], []), init);
    let mut add = FunctionBuilder::new(4);
    add.op3(Op::I64Add, 2, 0, 1);
    add.ret(2);
    let add_id = mb.add_func(FuncType::new([I64, I64], [I64]), add);
    mb.reg_name(add_id, 0, "a");
    mb.reg_name(add_id, 1, "b");
    mb.export_func("init", init_id);
    mb.export_func("add", add_id);
    mb.finish().unwrap()
}

#[test]
fn mod_exports_appear_in_the_index_are_callable_and_vanish_on_unload() {
    let (mut e, got) = engine_with_report();
    let a = e.load_mod(&adder(&e).encode(), ModOptions::new("adder").with_version(3).allow("engine.mods")).unwrap();

    let idx = e.api_index();
    let f = idx.function("adder", "add").expect("exported function is in the index");
    assert_eq!(f.owner, "adder");
    assert_eq!(f.doc, "Add two numbers.");
    assert_eq!(f.params.iter().map(|p| (p.name.as_str(), &p.ty)).collect::<Vec<_>>(), [("a", &ApiType::I64), ("b", &ApiType::I64)]);
    assert_eq!(f.results.iter().map(|p| (p.name.as_str(), &p.ty)).collect::<Vec<_>>(), [("ret", &ApiType::I64)]);
    let ns = idx.namespace("adder").unwrap();
    assert_eq!((ns.version, ns.owner.as_str()), (3, "adder"));
    idx.validate().unwrap();

    // module B imports it (typed from the index) and calls it every frame
    let b = with_frame(
        |mb| vec![import(mb, &e, "adder", "add"), import(mb, &e, "test", "report")],
        |f, fns| {
            call_with(f, fns[0], 10, &[A::I64(20), A::I64(22)]);
            report_i64(f, fns[1], 10);
        },
    );
    e.load_module("user", &b.encode()).unwrap();
    e.run_frames(2, 1.0 / 60.0);
    assert_eq!(*got.lock().unwrap(), [42, 42]);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));

    e.unload_module(a).unwrap();
    let idx = e.api_index();
    assert!(idx.function("adder", "add").is_none() && idx.namespace("adder").is_none());
    assert!(!e.scripting().is_loaded(a));
    e.run_frames(2, 1.0 / 60.0);
    assert!(e.is_running());
    assert_eq!(*got.lock().unwrap(), [42, 42], "the call after unload traps");
    let errs = errors(&e);
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs[0].contains("[user]") && errs[0].contains("module adder unloaded"), "{}", errs[0]);
    // a new module cannot link against the gone function
    assert!(matches!(e.load_module("user2", &b.encode()), Err(ScriptError::Link { .. })));
    // the name can be used again
    e.load_mod(&adder(&e).encode(), ModOptions::new("adder").allow("engine.mods")).unwrap();
    assert!(e.api_index().function("adder", "add").is_some());
}

#[test]
fn export_outside_init_or_of_a_missing_function_is_refused() {
    let (mut e, got) = engine_with_report();
    let m = {
        let mut mb = ModuleBuilder::new();
        let export = import(&mut mb, &e, "engine.mods", "export");
        let report = import(&mut mb, &e, "test", "report");
        mb.memory(1, None);
        mb.data(100, b"nothing");
        mb.data(110, b"Doc.");
        let mut f = FunctionBuilder::new(64);
        call_with(&mut f, export, 0, &[A::I64(100), A::I64(7), A::I64(110), A::I64(4)]);
        report_i32(&mut f, report, 0);
        f.ret(0);
        let id = mb.add_func(FuncType::new([F32], []), f);
        mb.export_func("frame", id);
        mb.finish().unwrap()
    };
    e.load_module("exp", &m.encode()).unwrap();
    e.run_frames(1, 0.016);
    assert_eq!(*got.lock().unwrap(), [-1]);
    assert!(e.api_index().namespace("exp").is_none());
}

#[test]
fn game_functions_are_in_the_index_with_their_owner() {
    let (e, _) = engine_with_report();
    let idx = e.api_index();
    let f = idx.function("test", "report").unwrap();
    assert_eq!((f.owner.as_str(), f.doc.as_str()), ("testgame", "Record `v` for the test."));
    assert_eq!(f.params[0].name, "v");
    assert_eq!(f.signature_text(), "(v: i64)");
    // duplicates and engine-owned namespaces are refused
    let mut e = e;
    assert!(e.register_host_fn(HostFn::new("test", "report").doc("again"), |_c, _a, _r| Ok(HostOk::Continue)).is_err());
    assert!(e.define_namespace("sneaky", 1, "engine", "x").is_err());
    assert!(e.register_host_fn(HostFn::new("nowhere", "f").doc("x"), |_c, _a, _r| Ok(HostOk::Continue)).is_err());
    assert!(e.register_host_fn(HostFn::new("test", "nodoc"), |_c, _a, _r| Ok(HostOk::Continue)).is_err());
}

#[test]
fn events_time_and_input_reach_scripts() {
    let (mut e, got) = engine_with_report();
    let m = with_frame(
        |mb| {
            let mut v = vec![];
            for (ns, n) in [("engine.events", "count"), ("engine.events", "read"), ("engine.events", "send"), ("engine.time", "frame"), ("engine.input", "key_down")] {
                v.push(import(mb, &e, ns, n));
            }
            v.push(import(mb, &e, "test", "report"));
            mb.data(100, b"ping");
            mb.data(110, &[1, 2, 3, 4]);
            mb.data(120, b"from_rust");
            mb.data(140, b"Space");
            v
        },
        |f, fns| {
            let (count, read, send, frame, key, report) = (fns[0], fns[1], fns[2], fns[3], fns[4], fns[5]);
            call_with(f, count, 10, &[A::I64(100), A::I64(4)]);
            report_i64(f, report, 10);
            call_with(f, read, 10, &[A::I64(100), A::I64(4), A::I64(0), A::I64(200), A::I64(2)]);
            report_i64(f, report, 10); // full length 4 (only 2 bytes copied), or -1
            call_with(f, count, 10, &[A::I64(120), A::I64(9)]);
            report_i64(f, report, 10);
            call_with(f, send, 10, &[A::I64(100), A::I64(4), A::I64(110), A::I64(4)]);
            f.call(frame, 10);
            report_i64(f, report, 10);
            call_with(f, key, 10, &[A::I64(140), A::I64(5)]);
            report_i32(f, report, 10);
        },
    );
    e.load_module("evt", &m.encode()).unwrap();
    let rust_saw = Arc::new(Mutex::new(Vec::<(String, Vec<u8>)>::new()));
    let rs = rust_saw.clone();
    e.add_system(Stage::PostUpdate, move |cx| {
        for ev in cx.events::<ScriptEvent>().iter().filter(|ev| ev.name == "ping") {
            rs.lock().unwrap().push((ev.name.clone(), ev.data.clone()));
        }
        if cx.time.frame() == 0 {
            cx.send(ScriptEvent { name: "from_rust".into(), data: vec![] });
        }
    });
    e.run_frames(1, 1.0 / 60.0);
    e.inject(InputEvent::new(InputEventKind::key_pressed(KeyCode::Space)));
    e.run_frames(2, 1.0 / 60.0);
    // frame 0: no events yet; frame 1: ping (sent in frame 0) and from_rust readable, Space down
    assert_eq!(*got.lock().unwrap(), [0, -1, 0, 0, 0, /* frame 1 */ 1, 4, 1, 1, 1, /* frame 2 */ 1, 4, 0, 2, 1]);
    // Rust saw the script's ping in frames 1 and 2, with its payload
    let saw = rust_saw.lock().unwrap();
    assert_eq!(saw.len(), 2);
    assert_eq!(saw[0], ("ping".to_string(), vec![1, 2, 3, 4]));
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn hot_reload_swaps_the_module_and_resets_script_state() {
    let dir = std::env::temp_dir().join(format!("efe_hot_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hot.efvm");
    let mut e = engine();
    std::fs::write(&path, mover(&e, 1.0).encode()).unwrap();
    let id = e.load_module_file(&path).unwrap();
    assert_eq!(e.scripting().module_id("hot"), Some(id));
    e.watch_module(id, &path).unwrap();
    let ent = e.world_mut().spawn((Transform::default(), Script::new(id, "update")));
    e.run_frames(3, 0.3);
    assert_eq!(e.world().get::<&Transform>(ent).unwrap().position.x, 3.0);
    assert_eq!(e.world().get::<&Script>(ent).unwrap().state, 3);

    // a broken file: the old module keeps running, the failure is logged once
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    let touch = |t: std::time::SystemTime| std::fs::File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
    std::fs::write(&path, b"not a module").unwrap();
    touch(later);
    e.run_frames(3, 0.3);
    assert_eq!(e.world().get::<&Transform>(ent).unwrap().position.x, 6.0);
    assert_eq!(errors(&e).iter().filter(|l| l.contains("reload failed")).count(), 1, "{:?}", errors(&e));

    // a good file: swapped, state reset to 0, new behaviour (+10)
    std::fs::write(&path, mover(&e, 10.0).encode()).unwrap();
    touch(later + std::time::Duration::from_secs(60));
    e.run_frames(1, 0.3);
    assert_eq!(e.world().get::<&Transform>(ent).unwrap().position.x, 16.0);
    assert_eq!(e.world().get::<&Script>(ent).unwrap().state, 1, "reset to 0, then one call");
    assert!(e.scripting().log_lines().any(|l| l.text.contains("reloaded")));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unknown_entry_and_unloaded_module_are_skipped_with_one_warning() {
    let mut e = engine();
    let id = e.load_module("mover", &mover(&e, 1.0).encode()).unwrap();
    e.world_mut().spawn((Script::new(id, "nope"),));
    e.world_mut().spawn((Script::new(ModuleId(99), "update"),));
    e.run_frames(5, 0.016);
    assert_eq!(warnings(&e).len(), 2, "{:?}", warnings(&e));
    assert!(errors(&e).is_empty());
}
