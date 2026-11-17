//! E8.13: typed script events. A game registers events (`Engine::register_event`), a mod
//! provides events in its interface (section 13), modules list the events they use in section
//! 14 and call `engine.events.send/count/read` with the event's id in that list. Headless;
//! modules are built with `ModuleBuilder` (`event_ref`; the `.efa` syntax comes with the
//! assembler's `import event`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use efeengine_core::efemod::{Dependency, Manifest, Package, VersionReq};
use efeengine_core::efevm::ValType::{F32, I32};
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;
use efeengine_core::mods::{Grant, LoadInput, ModError, ModOptions, ModRequest};
use efeengine_core::scripting::{LogLevel, ModuleId, ScriptError, ScriptEventError};

/// Test shim: load one `.efmod` through `load_input`.
trait LoadPackage {
    fn load_package(&mut self, bytes: &[u8]) -> Result<ModuleId, ModError>;
}

impl LoadPackage for Engine {
    fn load_package(&mut self, bytes: &[u8]) -> Result<ModuleId, ModError> {
        match self.load_input(LoadInput::Package { label: "test.efmod", bytes }) {
            Ok(l) => Ok(l.id()),
            Err(efeengine_core::mods::LoadError::Mod(e)) => Err(e),
            Err(efeengine_core::mods::LoadError::Script(e)) => Err(ModError::Script(e)),
            Err(e) => panic!("unexpected load error: {e}"),
        }
    }
}

type Reports = Arc<Mutex<Vec<i64>>>;

/// A headless engine with the namespaces `test` (`report(v: i64)`) and `game`, the struct
/// `Score { points: i32, who: i32 }` (8 bytes) and the events `game.score` (payload `Score`) and
/// `game.reset` (no payload).
fn engine() -> (Engine, Reports) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
    e.define_namespace("test", 1, "testgame", "Helpers of the test game.").unwrap();
    e.define_namespace("game", 1, "testgame", "Events of the test game.").unwrap();
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(HostFn::new("test", "report").doc("Record `v` for the test.").param("v", ApiType::I64), move |_c, a, _r| {
        g.lock().unwrap().push(a.i64(0)?);
        Ok(HostOk::Continue)
    })
    .unwrap();
    let mut score = StructDesc::new("Score", 8, 4)
        .field(FieldDesc::new("points", 0, Prim::I32, 1))
        .field(FieldDesc::new("who", 4, Prim::I32, 1))
        .doc("A score change.");
    score.owner = "testgame".into();
    e.register_struct(score).unwrap();
    e.register_event(EventDesc::new("game", "score").payload("Score").doc("The score changed.")).unwrap();
    e.register_event(EventDesc::new("game", "reset").doc("The game restarted.")).unwrap();
    (e, got)
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn warnings(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Warn).map(|l| l.text.clone()).collect()
}

fn func_type(e: &Engine, ns: &str, name: &str) -> FuncType {
    e.api_index().function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is not in the index")).func_type()
}

fn import(mb: &mut ModuleBuilder, e: &Engine, ns: &str, name: &str) -> u32 {
    mb.import(ns, name, func_type(e, ns, name))
}

enum A {
    I32(i32),
    I64(i64),
}

/// Constants into `base..`, then `call func, base`; the result (if any) is in `base`.
fn call_with(f: &mut FunctionBuilder, func: u32, base: u16, args: &[A]) {
    for (i, a) in args.iter().enumerate() {
        match a {
            A::I32(v) => f.const_i32(base + i as u16, *v),
            A::I64(v) => f.const_i64(base + i as u16, *v),
        }
    }
    f.call(func, base);
}

fn report_i32(f: &mut FunctionBuilder, report: u32, src: u16) {
    f.op2(Op::I64ExtendI32S, 60, src);
    f.call(report, 60);
}

/// A module whose `frame(dt)` is built by `body`; `setup` declares imports, events and data.
fn with_frame(setup: impl FnOnce(&mut ModuleBuilder) -> Vec<u32>, body: impl FnOnce(&mut FunctionBuilder, &[u32])) -> Module {
    let mut mb = ModuleBuilder::new();
    let fns = setup(&mut mb);
    mb.memory(1, None);
    let mut f = FunctionBuilder::new(64);
    body(&mut f, &fns);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

const SCORE: Option<(&str, u32)> = Some(("Score", 8));

/// Sends `game.score { points: 7, who: 3 }` every frame.
fn score_sender(e: &Engine) -> Module {
    with_frame(
        |mb| {
            mb.event_ref("game", "score", SCORE);
            mb.data(100, &[7, 0, 0, 0, 3, 0, 0, 0]);
            vec![import(mb, e, "engine.events", "send")]
        },
        |f, fns| call_with(f, fns[0], 10, &[A::I32(0), A::I64(100)]),
    )
}

/// Every frame reports the number of `game.score` events, then the first one's `points` and
/// `who` (0, 0 if there is none).
fn score_reader(e: &Engine) -> Module {
    with_frame(
        |mb| {
            mb.event_ref("game", "score", SCORE);
            vec![
                import(mb, e, "engine.events", "count"),
                import(mb, e, "engine.events", "read"),
                import(mb, e, "test", "report"),
            ]
        },
        |f, fns| {
            let (count, read, report) = (fns[0], fns[1], fns[2]);
            call_with(f, count, 10, &[A::I32(0)]);
            report_i32(f, report, 10);
            call_with(f, read, 10, &[A::I32(0), A::I32(0), A::I64(200)]);
            f.const_i64(13, 0);
            f.load(Op::I32Load, 14, 13, 200);
            report_i32(f, report, 14);
            f.load(Op::I32Load, 14, 13, 204);
            report_i32(f, report, 14);
        },
    )
}

// ---------------------------------------------------------------------------------------------

#[test]
fn a_game_event_with_a_struct_payload_goes_from_one_module_to_another_next_frame() {
    let (mut e, got) = engine();
    e.load_module("sender", &score_sender(&e).encode()).unwrap();
    e.load_module("reader", &score_reader(&e).encode()).unwrap();
    let rust_saw = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let rs = rust_saw.clone();
    e.add_system(Stage::PostUpdate, move |cx| {
        for p in cx.script_events("game.score") {
            rs.lock().unwrap().push(p.to_vec());
        }
    });
    e.run_frames(3, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    // frame 0: nothing yet (status -1, memory untouched); frames 1 and 2: one event, payload intact
    assert_eq!(*got.lock().unwrap(), [0, 0, 0, /* */ 1, 7, 3, /* */ 1, 7, 3]);
    // Rust reads the same events by name
    assert_eq!(*rust_saw.lock().unwrap(), [vec![7, 0, 0, 0, 3, 0, 0, 0], vec![7, 0, 0, 0, 3, 0, 0, 0]]);
}

#[test]
fn rust_sends_are_checked_against_the_registered_event() {
    let (mut e, got) = engine();
    e.load_module("reader", &score_reader(&e).encode()).unwrap();
    e.send_script_event("game.score", &[9, 0, 0, 0, 4, 0, 0, 0]).unwrap();
    assert_eq!(e.send_script_event("game.nope", &[]), Err(ScriptEventError::Unknown { event: "game.nope".into() }));
    assert_eq!(
        e.send_script_event("game.score", &[1, 2, 3]),
        Err(ScriptEventError::Payload { event: "game.score".into(), expected: 8, got: 3 })
    );
    assert_eq!(
        e.send_script_event("game.reset", &[0]),
        Err(ScriptEventError::Payload { event: "game.reset".into(), expected: 0, got: 1 })
    );
    e.send_script_event("game.reset", &[]).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(*got.lock().unwrap(), [1, 9, 4], "only the valid send arrived");
}

#[test]
fn registering_events_follows_the_registry_rules() {
    let (mut e, _) = engine();
    assert!(e.register_event(EventDesc::new("game", "score").doc("again")).is_err(), "duplicate");
    assert!(e.register_event(EventDesc::new("nowhere", "x").doc("x")).is_err(), "namespace not defined");
    assert!(e.register_event(EventDesc::new("game", "nodoc")).is_err(), "no doc");
    assert!(e.register_event(EventDesc::new("game", "x").payload("Nope").doc("x")).is_err(), "payload not registered");
    assert!(e.register_event(EventDesc::new("engine.events", "x").doc("x")).is_err(), "engine namespaces are reserved");
    let idx = e.api_index();
    let ev = idx.event("game", "score").unwrap();
    assert_eq!((ev.owner.as_str(), ev.payload.as_deref(), ev.doc.as_str()), ("testgame", Some("Score"), "The score changed."));
    assert_eq!(idx.events.len(), 2);
    idx.validate().unwrap();
}

#[test]
fn an_unregistered_event_is_refused_at_link_with_its_name() {
    let (mut e, _) = engine();
    let m = with_frame(
        |mb| {
            mb.event_ref("game", "gone", None);
            vec![]
        },
        |_, _| {},
    );
    match e.load_module("m", &m.encode()).unwrap_err() {
        ScriptError::Link { errors, .. } => {
            assert_eq!(errors, [LinkError::UnknownEvent { namespace: "game".into(), name: "gone".into() }]);
            assert!(errors[0].to_string().contains("game.gone"), "{}", errors[0]);
        }
        other => panic!("{other}"),
    }
    assert!(e.scripting().modules().is_empty());
}

#[test]
fn a_payload_layout_mismatch_is_refused_at_link() {
    let (mut e, _) = engine();
    for (payload, what) in [(Some(("Score", 4)), "size"), (Some(("Other", 8)), "struct name"), (None, "payload missing")] {
        let m = with_frame(
            |mb| {
                mb.event_ref("game", "score", payload);
                vec![]
            },
            |_, _| {},
        );
        match e.load_module("m", &m.encode()).unwrap_err() {
            ScriptError::Link { errors, .. } => {
                assert!(matches!(&errors[0], LinkError::EventPayload { name, .. } if name == "score"), "{what}: {errors:?}");
                assert!(errors[0].to_string().contains("game.score"), "{}", errors[0]);
            }
            other => panic!("{what}: {other}"),
        }
    }
    // and the other way: the module expects a payload, the event has none
    let m = with_frame(
        |mb| {
            mb.event_ref("game", "reset", Some(("Score", 8)));
            vec![]
        },
        |_, _| {},
    );
    assert!(matches!(e.load_module("m", &m.encode()).unwrap_err(), ScriptError::Link { .. }));
}

#[test]
fn an_unknown_event_id_traps_and_so_does_an_out_of_range_payload() {
    let (mut e, got) = engine();
    for (func, args) in [("send", vec![A::I32(5), A::I64(0)]), ("count", vec![A::I32(-1)]), ("read", vec![A::I32(1), A::I32(0), A::I64(0)])] {
        let m = with_frame(
            |mb| {
                mb.event_ref("game", "score", SCORE);
                vec![import(mb, &e, "engine.events", func), import(mb, &e, "test", "report")]
            },
            |f, fns| {
                call_with(f, fns[0], 10, &args);
                f.const_i64(30, 1);
                f.call(fns[1], 30); // never reached
            },
        );
        let id = e.load_module(&format!("bad_{func}"), &m.encode()).unwrap();
        e.run_frames(1, 1.0 / 60.0);
        let errs = errors(&e);
        let last = errs.last().unwrap();
        assert!(last.contains(&format!("bad_{func}")) && last.contains("no event with id") && last.contains(&format!("engine.events.{func}")), "{last}");
        e.unload_module(id).unwrap();
    }
    assert!(got.lock().unwrap().is_empty());
    // a payload range outside memory (a 64 KiB memory, 8 bytes at 65534) traps with out of bounds
    let before = errors(&e).len();
    for func in ["send", "read"] {
        let m = with_frame(
            |mb| {
                mb.event_ref("game", "score", SCORE);
                vec![import(mb, &e, "engine.events", func)]
            },
            |f, fns| {
                if func == "send" {
                    call_with(f, fns[0], 10, &[A::I32(0), A::I64(65534)]);
                } else {
                    // an event exists, so the copy is attempted
                    call_with(f, fns[0], 10, &[A::I32(0), A::I32(0), A::I64(65534)]);
                }
            },
        );
        e.send_script_event("game.score", &[0; 8]).unwrap();
        let id = e.load_module(&format!("oob_{func}"), &m.encode()).unwrap();
        e.run_frames(2, 1.0 / 60.0);
        assert!(errors(&e)[before..].iter().any(|l| l.contains(&format!("oob_{func}")) && l.contains("out of bounds")), "{func}: {:?}", errors(&e));
        e.unload_module(id).unwrap();
    }
}

/// A frame that sends `game.reset` (no payload, `ptr` 0) `n` times in a loop.
fn spammer(e: &Engine, n: i32) -> Module {
    with_frame(
        |mb| {
            mb.event_ref("game", "reset", None);
            vec![import(mb, e, "engine.events", "send")]
        },
        |f, fns| {
            f.const_i32(10, 0);
            f.const_i32(11, n);
            f.const_i32(12, 1);
            let top = f.new_label();
            f.bind(top);
            f.const_i32(20, 0);
            f.const_i64(21, 0);
            f.call(fns[0], 20);
            f.op3(Op::I32Add, 10, 10, 12);
            f.op3(Op::I32LtS, 13, 10, 11);
            f.br_if(13, top);
        },
    )
}

#[test]
fn sends_over_the_cap_are_dropped_counted_and_logged_once_per_frame() {
    let (mut e, got) = engine();
    e.load_module("spam", &spammer(&e, 5000).encode()).unwrap();
    let counter = with_frame(
        |mb| {
            mb.event_ref("game", "reset", None);
            vec![import(mb, &e, "engine.events", "count"), import(mb, &e, "test", "report")]
        },
        |f, fns| {
            call_with(f, fns[0], 10, &[A::I32(0)]);
            report_i32(f, fns[1], 10);
        },
    );
    e.load_module("counter", &counter.encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(e.scripting().dropped_events_last_frame(), 5000 - 4096);
    let drops = |e: &Engine| warnings(e).into_iter().filter(|l| l.contains("dropped")).collect::<Vec<_>>();
    assert_eq!(drops(&e), ["904 script events dropped this frame (cap 4096)"]);
    e.run_frames(2, 1.0 / 60.0);
    // the cap held: 4096 readable per frame; one more line per frame with drops
    assert_eq!(*got.lock().unwrap(), [0, 4096, 4096]);
    assert_eq!(drops(&e).len(), 3);
    assert_eq!(e.scripting().dropped_events_last_frame(), 904);
    // a frame without drops: the count resets and nothing is logged
    e.unload_module(e.scripting().module_id("spam").unwrap()).unwrap();
    e.load_module("spam2", &spammer(&e, 10).encode()).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(e.scripting().dropped_events_last_frame(), 0);
    assert_eq!(drops(&e).len(), 3);
}

// ---------------------------------------------------------------------------------------------
// Mods that provide events

fn flag() -> StructDesc {
    StructDesc::new("Flag", 4, 4).field(FieldDesc::new("n", 0, Prim::I32, 1)).doc("A flag.")
}

fn provided(name: &str, payload: Option<&str>, doc: &str) -> ProvidedEvent {
    ProvidedEvent { name: name.into(), payload: payload.map(str::to_string), doc: doc.into() }
}

/// A mod `emit` providing the event `ready(Flag)`, the event `done` and `fire(n: i32)`, which
/// sends `ready { n }` (a reference to its own event, section 14 id 0).
fn emitter(e: &Engine, extra_event: bool) -> Module {
    let mut mb = ModuleBuilder::new();
    let send = import(&mut mb, e, "engine.events", "send");
    mb.event_ref("emit", "ready", Some(("Flag", 4)));
    mb.memory(1, None);
    let mut f = FunctionBuilder::new(8);
    f.const_i64(1, 0);
    f.store(Op::I32Store, 0, 1, 100); // *(100) = n
    f.const_i32(2, 0);
    f.const_i64(3, 100);
    f.call(send, 2);
    f.ret(0);
    let fire = mb.add_func(FuncType::new([I32], []), f);
    let mut events = vec![provided("ready", Some("Flag"), "The emitter is ready; the payload says with which number.")];
    if extra_event {
        events.push(provided("done", None, "The emitter is done."));
    }
    mb.interface(Interface {
        structs: vec![flag()],
        provides: vec![Provide {
            name: "fire".into(),
            func: fire,
            doc: "Send `ready` with the number `n`.".into(),
            params: vec![Param::new("n", ApiType::I32)],
            results: vec![],
        }],
        events,
        ..Default::default()
    });
    mb.finish().unwrap()
}

/// Reports, every frame, the number of `emit.ready` events and the first one's `n`.
fn ready_reader(e: &Engine, with_report: bool) -> Module {
    with_frame(
        |mb| {
            mb.event_ref("emit", "ready", Some(("Flag", 4)));
            let mut v = vec![import(mb, e, "engine.events", "count"), import(mb, e, "engine.events", "read")];
            if with_report {
                v.push(import(mb, e, "test", "report"));
            }
            v
        },
        |f, fns| {
            call_with(f, fns[0], 10, &[A::I32(0)]);
            if with_report {
                report_i32(f, fns[2], 10);
            }
            call_with(f, fns[1], 10, &[A::I32(0), A::I32(0), A::I64(200)]);
            f.const_i64(13, 0);
            f.load(Op::I32Load, 14, 13, 200);
            if with_report {
                report_i32(f, fns[2], 14);
            }
        },
    )
}

/// A game module whose `frame` calls `emit.fire(n)`.
fn driver(e: &Engine, n: i32) -> Module {
    with_frame(
        |mb| vec![mb.import("emit", "fire", func_type(e, "emit", "fire"))],
        |f, fns| call_with(f, fns[0], 10, &[A::I32(n)]),
    )
}

fn manifest(id: &str, deps: &[&str], perms: &[&str]) -> Manifest {
    let mut m = Manifest::new(id, "1.0.0".parse().unwrap());
    m.dependencies = deps.iter().map(|d| Dependency { id: d.to_string(), req: "^1".parse::<VersionReq>().unwrap() }).collect();
    m.permissions.namespaces = perms.iter().map(|s| s.to_string()).collect();
    m
}

fn pack(m: &Manifest, module: &Module) -> Vec<u8> {
    Package::write(m, &module.encode(), &BTreeMap::new()).unwrap()
}

fn grant(ns: &[&str]) -> Grant {
    Grant { namespaces: ns.iter().map(|s| s.to_string()).collect(), ..Grant::default() }
}

#[test]
fn a_mod_provides_an_event_and_a_package_under_a_policy_reads_it() {
    let (mut e, got) = engine();
    let requests: Arc<Mutex<Vec<(String, Vec<String>, Vec<String>)>>> = Arc::default();
    let rq = requests.clone();
    e.set_mod_policy(move |r: &ModRequest<'_>| {
        rq.lock().unwrap().push((r.manifest.id.clone(), r.imports.to_vec(), r.events.to_vec()));
        Ok(grant(&["engine.events", "test"]))
    });
    // the emitter package: its own event needs no grant, `engine.events` does
    e.load_package(&pack(&manifest("emit", &[], &["engine.events"]), &emitter(&e, false))).unwrap();
    let idx = e.api_index();
    idx.validate().unwrap();
    let ev = idx.event("emit", "ready").expect("the provided event is in the index");
    assert_eq!((ev.owner.as_str(), ev.payload.as_deref()), ("emit", Some("Flag")));
    assert_eq!(ev.doc, "The emitter is ready; the payload says with which number.");
    assert_eq!(idx.struct_type("Flag").unwrap().owner, "emit");
    // the reader package names the emitter as a dependency: its namespace needs no grant
    let reader = ready_reader(&e, true);
    let rm = manifest("watch", &["emit"], &["engine.events", "test"]);
    e.load_package(&pack(&rm, &reader)).unwrap();
    e.load_module("driver", &driver(&e, 42).encode()).unwrap();
    e.run_frames(3, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    // frame 0: no event yet; frames 1, 2: one event with the payload the provider sent
    assert_eq!(*got.lock().unwrap(), [0, 0, 1, 42, 1, 42]);
    let requests = requests.lock().unwrap();
    assert_eq!(requests[0], ("emit".to_string(), vec!["engine.events".to_string()], Vec::<String>::new()));
    assert_eq!(requests[1], ("watch".to_string(), vec!["engine.events".to_string(), "test".to_string()], vec!["emit".to_string()]));
    // Rust can see the event too, by its qualified name
    assert_eq!(e.scripting().check_event("emit.ready", 4), Ok(()));
}

#[test]
fn an_event_namespace_outside_the_grant_is_refused_and_named() {
    let (mut e, _) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine.events", "test"])));
    // a package reading `game.score` while the policy grants no `game`
    let m = manifest("p", &[], &["engine.events", "test", "game"]);
    match e.load_package(&pack(&m, &score_reader(&e))).unwrap_err() {
        ModError::NotGranted { id, namespaces } => assert_eq!((id.as_str(), namespaces.as_slice()), ("p", ["game".to_string()].as_slice())),
        other => panic!("{other}"),
    }
    assert!(e.scripting().modules().is_empty());
    // the policy sees the event namespace in the request, apart from the imports
    e.set_mod_policy(|r: &ModRequest<'_>| {
        assert_eq!(r.events, ["game".to_string()]);
        Ok(grant(&["engine.events", "test", "game"]))
    });
    e.load_package(&pack(&m, &score_reader(&e))).unwrap();
    // the same rule for a bare mod with an allow-list
    let err = e.load_mod(&score_reader(&e).encode(), ModOptions::new("bare").allow("engine.events").allow("test")).unwrap_err();
    match err {
        ScriptError::Disallowed { module, namespaces } => assert_eq!((module.as_str(), namespaces.as_slice()), ("bare", ["game".to_string()].as_slice())),
        other => panic!("{other}"),
    }
    e.load_mod(&score_reader(&e).encode(), ModOptions::new("bare").allow("engine.events").allow("test").allow("game")).unwrap();
}

#[test]
fn a_game_module_cannot_provide_events_and_a_mod_cannot_clash_with_a_function() {
    let (mut e, _) = engine();
    let mut mb = ModuleBuilder::new();
    mb.interface(Interface { events: vec![provided("x", None, "An event.")], ..Default::default() });
    let m = mb.finish().unwrap();
    assert!(matches!(e.load_module("g", &m.encode()).unwrap_err(), ScriptError::NotAMod(_)));
    e.load_mod(&m.encode(), ModOptions::new("mm")).unwrap();
    assert!(e.api_index().event("mm", "x").is_some());
    // an event named like a provide of the same mod is refused (a failed load leaves nothing)
    let mut mb = ModuleBuilder::new();
    let mut f = FunctionBuilder::new(2);
    f.ret(0);
    let id = mb.add_func(FuncType::new([], []), f);
    mb.interface(Interface {
        provides: vec![Provide { name: "x".into(), func: id, doc: "A function.".into(), params: vec![], results: vec![] }],
        events: vec![provided("x", None, "An event.")],
        ..Default::default()
    });
    let before = e.api_index();
    // the verifier may already refuse the module; if not, the registry refuses the load
    if let Ok(m) = mb.finish() {
        assert!(e.load_mod(&m.encode(), ModOptions::new("clash")).is_err());
    }
    assert_eq!(e.api_index(), before);
}

#[test]
fn a_mod_may_not_refer_to_its_events_wrongly() {
    let (mut e, _) = engine();
    // the module's section 14 says `emit.ready` has a different payload size than the interface
    let mut mb = ModuleBuilder::new();
    mb.event_ref("emit", "ready", Some(("Flag", 8)));
    mb.interface(Interface { structs: vec![flag()], events: vec![provided("ready", Some("Flag"), "Ready.")], ..Default::default() });
    let before = e.api_index();
    match e.load_mod(&mb.finish().unwrap().encode(), ModOptions::new("emit")).unwrap_err() {
        ScriptError::Link { errors, .. } => assert!(matches!(&errors[0], LinkError::EventPayload { name, .. } if name == "ready"), "{errors:?}"),
        other => panic!("{other}"),
    }
    // an own-namespace event the interface does not declare
    let mut mb = ModuleBuilder::new();
    mb.event_ref("emit", "nope", None);
    mb.interface(Interface { events: vec![provided("ready", None, "Ready.")], ..Default::default() });
    match e.load_mod(&mb.finish().unwrap().encode(), ModOptions::new("emit")).unwrap_err() {
        ScriptError::Link { errors, .. } => assert_eq!(errors, [LinkError::UnknownEvent { namespace: "emit".into(), name: "nope".into() }]),
        other => panic!("{other}"),
    }
    assert_eq!(e.api_index(), before, "nothing was registered");
    assert!(e.scripting().modules().is_empty());
}

#[test]
fn the_cascade_covers_event_dependents() {
    let (mut e, _) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine.events"])));
    let before = e.api_index();
    let emit = e.load_package(&pack(&manifest("emit", &[], &["engine.events"]), &emitter(&e, false))).unwrap();
    // the reader only refers to the event: no import from `emit`
    let reader = ready_reader(&e, false);
    let watch = e.load_package(&pack(&manifest("watch", &["emit"], &["engine.events"]), &reader)).unwrap();
    // a game module that refers to the event without importing from the mod
    let plain = e.load_module("plain", &reader.encode()).unwrap();
    match e.unload_module(emit).unwrap_err() {
        ScriptError::Needed { module, by } => assert_eq!((module.as_str(), by.as_slice()), ("emit", ["watch".to_string(), "plain".to_string()].as_slice())),
        other => panic!("{other}"),
    }
    assert!(e.api_index().event("emit", "ready").is_some());
    let removed = e.unload_module_and_dependents(emit).unwrap();
    assert_eq!(removed, [plain, watch, emit]);
    assert_eq!(e.api_index(), before);
    assert!(e.api_index().events.len() == 2);
}

#[cfg(not(target_family = "wasm"))]
mod reload {
    use super::*;

    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn replace(path: &std::path::Path, m: &Module, step: u64) {
        std::fs::write(path, m.encode()).unwrap();
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(60 * step);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    /// The emitter with `ready` over a `Flag` of 4 or 8 bytes (without `fire`).
    fn flagger(events: &[&str], wide: bool) -> Module {
        let mut mb = ModuleBuilder::new();
        let mut flag = StructDesc::new("Flag", if wide { 8 } else { 4 }, if wide { 8 } else { 4 });
        flag = flag.field(FieldDesc::new("n", 0, if wide { Prim::I64 } else { Prim::I32 }, 1)).doc("A flag.");
        mb.interface(Interface {
            structs: vec![flag],
            events: events.iter().map(|n| provided(n, if *n == "ready" { Some("Flag") } else { None }, "An event.")).collect(),
            ..Default::default()
        });
        mb.finish().unwrap()
    }

    #[test]
    fn a_reload_that_drops_or_changes_an_event_a_dependent_uses_is_refused() {
        let dir = Dir(std::env::temp_dir().join(format!("efe_events_{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("emit.efvm");
        let (mut e, _) = engine();
        let first = flagger(&["ready", "done"], false);
        std::fs::write(&path, first.encode()).unwrap();
        let id = e.load_mod(&first.encode(), ModOptions::new("emit")).unwrap();
        e.watch_module(id, &path).unwrap();
        e.load_module("user", &ready_reader(&e, false).encode()).unwrap();
        assert!(e.api_index().event("emit", "done").is_some());

        // dropping an event nobody uses is accepted and unregisters it
        replace(&path, &flagger(&["ready"], false), 1);
        e.run_frames(1, 0.3);
        assert!(errors(&e).is_empty(), "{:?}", errors(&e));
        assert!(e.api_index().event("emit", "done").is_none());
        assert!(e.api_index().event("emit", "ready").is_some());

        // dropping the event the dependent uses is refused, the old version keeps running
        replace(&path, &flagger(&["done"], false), 2);
        e.run_frames(1, 0.3);
        let errs = errors(&e);
        assert_eq!(errs.iter().filter(|l| l.contains("reload failed")).count(), 1, "{errs:?}");
        assert!(errs[0].contains("`user` uses the event `emit.ready`") && errs[0].contains("does not provide"), "{}", errs[0]);
        assert!(e.api_index().event("emit", "ready").is_some());
        assert!(e.api_index().event("emit", "done").is_none());

        // changing its payload (Flag grows to 8 bytes) is refused too
        replace(&path, &flagger(&["ready"], true), 3);
        e.run_frames(1, 0.3);
        let errs = errors(&e);
        assert_eq!(errs.iter().filter(|l| l.contains("reload failed")).count(), 2, "{errs:?}");
        assert!(errs[1].contains("`user` uses the event `emit.ready` with &Flag (4 bytes)") && errs[1].contains("&Flag (8 bytes)"), "{}", errs[1]);
        assert_eq!(e.scripting().registry().imports().event("emit", "ready"), Some(Some(("Flag", 4))));

        // an unchanged event with an added one is accepted
        replace(&path, &flagger(&["ready", "done"], false), 4);
        e.run_frames(1, 0.3);
        assert_eq!(errors(&e).len(), 2, "{:?}", errors(&e));
        assert!(e.api_index().event("emit", "done").is_some());
        e.api_index().validate().unwrap();
    }
}

mod send_now {
    use std::sync::{Arc, Mutex};

    use efeengine_core::prelude::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Ping(u32);

    #[test]
    fn readable_same_frame_by_later_systems_and_dropped_next_frame() {
        let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).unwrap();
        let seen: Arc<Mutex<Vec<(&'static str, Vec<u32>)>>> = Default::default();
        let mut n = 0;
        e.add_system(Stage::PreUpdate, move |cx| {
            n += 1;
            if n == 1 {
                cx.send_now(Ping(1));
                cx.send_batch_now([Ping(2), Ping(3)]);
                cx.send(Ping(9)); // normal send: next frame only
            }
        })
        .order(order::INPUT);
        let s = seen.clone();
        e.add_system(Stage::Update, move |cx| {
            s.lock().unwrap().push(("update", cx.events::<Ping>().iter().map(|p| p.0).collect()));
        });
        let s = seen.clone();
        e.add_system(Stage::PreUpdate, move |cx| {
            s.lock().unwrap().push(("early", cx.events::<Ping>().iter().map(|p| p.0).collect()));
        })
        .order(order::INPUT - 1);
        e.run_frames(2, 1.0 / 60.0);
        let v = seen.lock().unwrap().clone();
        // Frame 1: the earlier system sees nothing, the Update system sees the now-events in order.
        assert_eq!(v[0], ("early", vec![]));
        assert_eq!(v[1], ("update", vec![1, 2, 3]));
        // Frame 2: the now-events are gone, the normal send is readable.
        assert_eq!(v[2], ("early", vec![9]));
        assert_eq!(v[3], ("update", vec![9]));
    }
}
