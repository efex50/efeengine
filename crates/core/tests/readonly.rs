//! Read-only components: scripts can read them, `set` / `insert` / `remove` fail with status 5.

use std::sync::{Arc, Mutex};

use efeengine_core::efevm::ValType::{F32, I64};
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;
use efeengine_core::scripting::{LogLevel, ModuleId, Script};

#[script_component(readonly)]
#[derive(Default, Debug, PartialEq)]
struct Locked {
    v: f32,
}

fn engine() -> (Engine, Arc<Mutex<Vec<i64>>>) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
    e.register::<Locked>().unwrap();
    e.define_namespace("test", 1, "testgame", "Helpers.").unwrap();
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(HostFn::new("test", "report").doc("Record `v`.").param("v", ApiType::I64), move |_c, a, _r| {
        g.lock().unwrap().push(a.i64(0)?);
        Ok(HostOk::Continue)
    })
    .unwrap();
    (e, got)
}

/// `update(entity, dt, state)`: reports the statuses of get, set, insert, remove on component `cid`.
fn module(e: &Engine, cid: i32) -> Module {
    let idx = e.api_index();
    let mut mb = ModuleBuilder::new();
    let mut imp = |ns: &str, n: &str| mb.import(ns, n, idx.function(ns, n).unwrap().func_type());
    let (get, set, insert, remove, report) = (imp("engine.world", "get"), imp("engine.world", "set"), imp("engine.world", "insert"), imp("engine.world", "remove"), imp("test", "report"));
    mb.memory(1, None);
    let mut f = FunctionBuilder::new(64);
    for (func, args) in [(get, 3), (set, 3), (insert, 3), (remove, 2)] {
        f.mov(10, 0);
        f.const_i32(11, cid);
        if args == 3 {
            f.const_i64(12, 1024);
        }
        f.call(func, 10);
        f.op2(Op::I64ExtendI32S, 20, 10);
        f.mov(21, 20);
        f.call(report, 21);
    }
    f.const_i64(17, 1);
    f.op3(Op::I64Add, 16, 2, 17);
    f.ret(16);
    let id = mb.add_func(FuncType::new([I64, F32, I64], [I64]), f);
    mb.export_func("update", id);
    mb.finish().unwrap()
}

fn statuses(e: &mut Engine, got: &Arc<Mutex<Vec<i64>>>, comp: &str, ent: impl FnOnce(&mut Engine, ModuleId)) -> Vec<i64> {
    let cid = e.types().by_name(comp).unwrap().id() as i32;
    let m = module(e, cid);
    let id = e.load_module("ro", &m.encode()).unwrap();
    ent(e, id);
    e.run_frames(1, 1.0 / 60.0);
    let v = got.lock().unwrap().clone();
    assert!(e.scripting().log_lines().all(|l| l.level != LogLevel::Error), "script errors");
    v
}

#[test]
fn scripts_read_but_cannot_write_a_readonly_component() {
    let (mut e, got) = engine();
    let s = statuses(&mut e, &got, "Locked", |e, id| {
        e.world_mut().spawn((Locked { v: 1.0 }, Script::new(id, "update")));
    });
    // get ok, set / insert / remove -> 5
    assert_eq!(s, [0, 5, 5, 5]);
    // nothing changed
    let mut q = e.world().query::<&Locked>();
    assert_eq!(q.iter().map(|l| l.v).collect::<Vec<_>>(), [1.0]);
}

#[test]
fn writable_components_still_work() {
    let (mut e, got) = engine();
    let s = statuses(&mut e, &got, "Velocity", |e, id| {
        e.world_mut().spawn((Velocity::default(), Script::new(id, "update")));
    });
    assert_eq!(s, [0, 0, 0, 0]);
}

#[test]
fn global_transform_is_readonly_in_the_index() {
    let (e, _) = engine();
    let idx = e.api_index();
    let ro = |n: &str| idx.types.iter().find(|t| t.name == n).unwrap().readonly;
    assert!(ro("GlobalTransform") && ro("Locked"));
    assert!(!ro("Transform") && !ro("Sprite"));
    let json = idx.to_json();
    assert_eq!(json.matches("\"readonly\": true").count(), 2, "{json}");
    let back = ApiIndex::from_json(&json).unwrap();
    assert!(back.types.iter().find(|t| t.name == "Locked").unwrap().readonly);
    assert!(e.types().by_name("GlobalTransform").unwrap().readonly());
}
