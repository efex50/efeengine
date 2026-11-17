//! E8 T9/T10: declared mod provides (section 13) registered by the engine, the copy ABI across
//! mods, dependents, unload and hot reload. Headless; modules are built with `ModuleBuilder`.

use std::sync::{Arc, Mutex};

use efeengine_core::efevm::ValType::{F32, I32, I64};
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;
use efeengine_core::scripting::{LogLevel, ModuleId, Script, ScriptError};
use efeengine_core::mods::{ModOptions};

type Reports = Arc<Mutex<Vec<i64>>>;

fn engine() -> (Engine, Reports) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
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

fn param(name: &str, ty: ApiType) -> Param {
    Param::new(name, ty)
}

fn provide(name: &str, func: u32, doc: &str, params: Vec<Param>, results: Vec<Param>) -> Provide {
    Provide { name: name.into(), func, doc: doc.into(), params, results }
}

/// The struct `Stat { hp: i32 }`, defined by the provider.
fn stat() -> StructDesc {
    StructDesc::new("Stat", 4, 4).field(FieldDesc::new("hp", 0, Prim::I32, 1)).doc("A stat.")
}

const SCRATCH: (u64, u32) = (1024, 4096);

// Function indices of `inv()`.
const ADD: u32 = 0;
const LEN: u32 = 1;
const FIRST: u32 = 2;
const FILL: u32 = 3;
const BUMP: u32 = 4;
const WHERE: u32 = 5;
const SCRIBBLE: u32 = 6;
const TOUCH: u32 = 7;

/// The provider `inv`: scalars, `str`, `bytes` and structs. `transform` is the copy of the
/// engine's `Transform` (or a deliberately wrong one).
fn inv(transform: Option<StructDesc>) -> Module {
    let mut mb = ModuleBuilder::new();
    mb.memory(1, None);
    // add(a, b) = a + b
    let mut f = FunctionBuilder::new(4);
    f.op3(Op::I64Add, 2, 0, 1);
    f.ret(2);
    mb.add_func(FuncType::new([I64, I64], [I64]), f);
    // len(s) = s.len
    let mut f = FunctionBuilder::new(4);
    f.ret(1);
    mb.add_func(FuncType::new([I64, I64], [I64]), f);
    // first(s) = the first byte
    let mut f = FunctionBuilder::new(4);
    f.load(Op::I64Load8U, 2, 0, 0);
    f.ret(2);
    mb.add_func(FuncType::new([I64, I64], [I64]), f);
    // fill(buf): buf[0] = 0x7F; returns buf.len
    let mut f = FunctionBuilder::new(4);
    f.const_i32(2, 0x7F);
    f.store(Op::I32Store8, 2, 0, 0);
    f.ret(1);
    mb.add_func(FuncType::new([I64, I64], [I64]), f);
    // bump(s: &Stat): hp += 1; returns the old hp
    let mut f = FunctionBuilder::new(4);
    f.load(Op::I32Load, 1, 0, 0);
    f.const_i32(2, 1);
    f.op3(Op::I32Add, 2, 1, 2);
    f.store(Op::I32Store, 2, 0, 0);
    f.ret(1);
    mb.add_func(FuncType::new([I64], [I32]), f);
    // where(a: str, b: str) = b.ptr
    let mut f = FunctionBuilder::new(6);
    f.ret(2);
    mb.add_func(FuncType::new([I64, I64, I64, I64], [I64]), f);
    // scribble(buf): buf[0] = 0x7F, then traps
    let mut f = FunctionBuilder::new(4);
    f.const_i32(2, 0x7F);
    f.store(Op::I32Store8, 2, 0, 0);
    f.unreachable();
    mb.add_func(FuncType::new([I64, I64], []), f);
    // touch(t: &Transform): position.x += 1.0
    let mut f = FunctionBuilder::new(4);
    f.load(Op::F32Load, 1, 0, 0);
    f.const_f32(2, 1.0);
    f.op3(Op::F32Add, 1, 1, 2);
    f.store(Op::F32Store, 1, 0, 0);
    f.const_i32(3, 0);
    f.ret(3);
    mb.add_func(FuncType::new([I64], [I32]), f);

    let sp = || ApiType::Str;
    let mut provides = vec![
        provide("add", ADD, "Add two numbers.", vec![param("a", ApiType::I64), param("b", ApiType::I64)], vec![param("sum", ApiType::I64)]),
        provide("len", LEN, "Length of a string.", vec![param("s", sp())], vec![param("n", ApiType::I64)]),
        provide("first", FIRST, "First byte of a string.", vec![param("s", sp())], vec![param("b", ApiType::I64)]),
        provide("fill", FILL, "Write 0x7F into the first byte of a buffer.", vec![param("buf", ApiType::Bytes)], vec![param("n", ApiType::I64)]),
        provide("bump", BUMP, "Increment hp; returns the old value.", vec![param("stat", ApiType::Struct("Stat".into()))], vec![param("old", ApiType::I32)]),
        provide("where", WHERE, "The address of the second string in scratch.", vec![param("a", sp()), param("b", sp())], vec![param("at", ApiType::I64)]),
        provide("scribble", SCRIBBLE, "Write 0x7F then trap.", vec![param("buf", ApiType::Bytes)], vec![]),
    ];
    let mut structs = vec![stat()];
    if let Some(t) = transform {
        structs.push(t);
        provides.push(provide("touch", TOUCH, "Move a transform one unit right.", vec![param("t", ApiType::Struct(structs[1].name.clone()))], vec![param("status", ApiType::I32)]));
    }
    mb.interface(Interface { scratch: Some(SCRATCH), structs, provides, ..Default::default() });
    mb.finish().unwrap()
}

fn engine_transform(e: &Engine) -> StructDesc {
    e.api_index().struct_type("Transform").unwrap().clone()
}

/// An engine with `inv` loaded as a mod.
fn with_inv() -> (Engine, Reports, ModuleId) {
    let (mut e, got) = engine();
    let t = engine_transform(&e);
    let id = e.load_mod(&inv(Some(t)).encode(), ModOptions::new("inv")).unwrap();
    (e, got, id)
}

enum A {
    I64(i64),
}

fn call_with(f: &mut FunctionBuilder, func: u32, base: u16, args: &[A]) {
    for (i, a) in args.iter().enumerate() {
        let r = base + i as u16;
        match a {
            A::I64(v) => f.const_i64(r, *v),
        }
    }
    f.call(func, base);
}

fn report_i64(f: &mut FunctionBuilder, report: u32, src: u16) {
    f.mov(60, src);
    f.call(report, 60);
}

fn report_i32(f: &mut FunctionBuilder, report: u32, src: u16) {
    f.op2(Op::I64ExtendI32S, 60, src);
    f.call(report, 60);
}

/// A caller module with a `frame(dt)`: imports `ns.name` of the engine's index (typed from it)
/// and `test.report`, has `data` at the given addresses and one page of memory. `body` gets
/// the function indices `[ns.name, test.report]`.
fn caller(e: &Engine, imports: &[(&str, &str)], data: &[(u64, &[u8])], body: impl FnOnce(&mut FunctionBuilder, &[u32])) -> Module {
    let idx = e.api_index();
    let mut mb = ModuleBuilder::new();
    let mut fns = Vec::new();
    for (ns, name) in imports.iter().chain(&[("test", "report")]) {
        let f = idx.function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is not in the index"));
        fns.push(mb.import(ns, name, f.func_type()));
    }
    mb.memory(1, None);
    for (at, bytes) in data {
        mb.data(*at, bytes);
    }
    let mut f = FunctionBuilder::new(64);
    body(&mut f, &fns);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

/// Load `m` as game module `user`, run one frame, return what was reported and the errors.
fn run_user(e: &mut Engine, got: &Reports, m: &Module) -> (Vec<i64>, Vec<String>) {
    got.lock().unwrap().clear();
    let before = errors(e).len();
    let id = e.load_module("user", &m.encode()).unwrap();
    e.run_frames(1, 0.016);
    e.unload_module(id).unwrap();
    let out = got.lock().unwrap().clone();
    (out, errors(e)[before..].to_vec())
}

#[test]
fn provides_are_in_the_index_with_owner_doc_version_and_scalars_work() {
    let (mut e, got) = engine();
    let t = engine_transform(&e);
    e.load_mod(&inv(Some(t)).encode(), ModOptions::new("inv").with_version(3).with_doc("The inventory.")).unwrap();
    let idx = e.api_index();
    idx.validate().unwrap();
    let ns = idx.namespace("inv").unwrap();
    assert_eq!((ns.version, ns.owner.as_str(), ns.doc.as_str()), (3, "inv", "The inventory."));
    let f = idx.function("inv", "add").unwrap();
    assert_eq!((f.owner.as_str(), f.doc.as_str()), ("inv", "Add two numbers."));
    assert_eq!(f.signature_text(), "(a: i64, b: i64) -> (sum: i64)");
    assert_eq!(idx.function("inv", "bump").unwrap().signature_text(), "(stat: &Stat) -> (old: i32)");
    let s = idx.struct_type("Stat").unwrap();
    assert_eq!((s.owner.as_str(), s.size), ("inv", 4));
    assert_eq!(idx.struct_type("Transform").unwrap().owner, "engine", "a copy does not take over the owner");
    // every callable function is documented and in the index
    for (ns, n, ty) in e.scripting().registry().imports().names() {
        let f = idx.function(ns, n).unwrap();
        assert!(!f.doc.is_empty() && &f.func_type() == ty);
    }

    // default doc
    let (mut e2, _) = engine();
    e2.load_mod(&inv(None).encode(), ModOptions::new("inv")).unwrap();
    assert_eq!(e2.api_index().namespace("inv").unwrap().doc, "Functions provided by the mod `inv`.");

    // a caller reaches it
    let m = caller(&e, &[("inv", "add")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(20), A::I64(22)]);
        report_i64(f, fns[1], 10);
    });
    let (out, errs) = run_user(&mut e, &got, &m);
    assert_eq!((out, errs), (vec![42], vec![]));
}

#[test]
fn str_is_copied_and_checked() {
    let (mut e, got, _) = with_inv();
    // len("hello"), first("hello")
    let m = caller(&e, &[("inv", "len"), ("inv", "first")], &[(100, b"hello")], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(100), A::I64(5)]);
        report_i64(f, fns[2], 10);
        call_with(f, fns[1], 10, &[A::I64(100), A::I64(5)]);
        report_i64(f, fns[2], 10);
    });
    assert_eq!(run_user(&mut e, &got, &m), (vec![5, b'h' as i64], vec![]));

    // invalid UTF-8 traps in the caller, naming the argument and the function
    let m = caller(&e, &[("inv", "len")], &[(100, &[0xFF, 0xFE])], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(100), A::I64(2)]);
        report_i64(f, fns[1], 10);
    });
    let (out, errs) = run_user(&mut e, &got, &m);
    assert!(out.is_empty());
    assert!(errs[0].contains("argument `s` of `inv.len` is not valid UTF-8"), "{errs:?}");

    // a pointer past the end of the caller's memory, and a negative pointer
    for ptr in [65530i64, -8] {
        let m = caller(&e, &[("inv", "len")], &[], |f, fns| {
            call_with(f, fns[0], 10, &[A::I64(ptr), A::I64(16)]);
            report_i64(f, fns[1], 10);
        });
        let (out, errs) = run_user(&mut e, &got, &m);
        assert!(out.is_empty());
        assert!(errs[0].contains("argument `s` of `inv.len`"), "{errs:?}");
    }
    // a negative length
    let m = caller(&e, &[("inv", "len")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(0), A::I64(-1)]);
        report_i64(f, fns[1], 10);
    });
    assert!(run_user(&mut e, &got, &m).1[0].contains("negative pointer or length"));
    assert!(e.is_running());
}

#[test]
fn a_zero_length_str_still_checks_its_pointer() {
    let (mut e, got, _) = with_inv();
    // one page of caller memory: 65536 is the end (accepted for length 0), 65537 is past it
    let m = caller(&e, &[("inv", "len")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(65537), A::I64(0)]);
        report_i64(f, fns[1], 10);
    });
    let (out, errs) = run_user(&mut e, &got, &m);
    assert!(out.is_empty());
    assert!(errs[0].contains("argument `s` of `inv.len`: 0 bytes at address 65537 are out of bounds"), "{errs:?}");
    assert!(e.is_running());
}

#[test]
fn bytes_are_copied_back_after_a_normal_return_only() {
    let (mut e, got, _) = with_inv();
    // fill(buf) writes 0x7F into the first byte; the caller sees it
    let m = caller(&e, &[("inv", "fill")], &[(200, &[1, 2, 3, 4])], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(200), A::I64(4)]);
        report_i64(f, fns[1], 10);
        f.const_i64(11, 0);
        f.load(Op::I64Load8U, 12, 11, 200);
        report_i64(f, fns[1], 12);
        f.load(Op::I64Load8U, 12, 11, 201);
        report_i64(f, fns[1], 12);
    });
    assert_eq!(run_user(&mut e, &got, &m), (vec![4, 0x7F, 2], vec![]));

    // scribble writes and then traps: nothing is copied back, the caller traps with the message
    let m = caller(&e, &[("inv", "scribble"), ("inv", "fill")], &[(200, &[1, 2, 3, 4])], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(200), A::I64(4)]);
        report_i64(f, fns[2], 10);
    });
    let (out, errs) = run_user(&mut e, &got, &m);
    assert!(out.is_empty());
    assert!(errs[0].contains("unreachable"), "{errs:?}");
    // ... and the caller's byte is untouched: a second export of the caller reads it
    let (mut e, got, _) = with_inv();
    let idx = e.api_index();
    let mut mb = ModuleBuilder::new();
    let scribble = mb.import("inv", "scribble", idx.function("inv", "scribble").unwrap().func_type());
    mb.memory(1, None);
    mb.data(200, &[1, 2, 3, 4]);
    let mut f = FunctionBuilder::new(16);
    call_with(&mut f, scribble, 10, &[A::I64(200), A::I64(4)]);
    f.ret(0);
    let frame = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", frame);
    let mut f = FunctionBuilder::new(8);
    f.const_i64(4, 0);
    f.load(Op::I64Load8U, 5, 4, 200);
    f.ret(5);
    let update = mb.add_func(FuncType::new([I64, F32, I64], [I64]), f);
    mb.export_func("update", update);
    let id = e.load_module("user", &mb.finish().unwrap().encode()).unwrap();
    let ent = e.world_mut().spawn((Transform::default(), Script::new(id, "update")));
    e.run_frames(1, 0.016);
    assert!(errors(&e)[0].contains("unreachable"), "{:?}", errors(&e));
    assert_eq!(e.world().get::<&Script>(ent).unwrap().state, 1, "the byte the provider wrote was not copied back");
    let _ = got;
    assert!(e.is_running());
}

#[test]
fn structs_own_and_copied_are_copied_in_and_out() {
    let (mut e, got, _) = with_inv();
    // Stat { hp } at 300 = 41 -> bump returns 41 and leaves 42 in the caller's memory
    let m = caller(&e, &[("inv", "bump")], &[(300, &41i32.to_le_bytes())], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(300)]);
        report_i32(f, fns[1], 10);
        f.const_i64(11, 0);
        f.load(Op::I32Load, 12, 11, 300);
        report_i32(f, fns[1], 12);
    });
    assert_eq!(run_user(&mut e, &got, &m), (vec![41, 42], vec![]));

    // a copied struct (the engine's Transform): position.x += 1
    let tr = Transform { position: Vec2::new(5.0, 6.0), ..Transform::default() };
    let bytes = bytemuck::bytes_of(&tr).to_vec();
    let m = caller(&e, &[("inv", "touch")], &[(400, &bytes)], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(400)]);
        f.const_i64(11, 0);
        f.load(Op::F32Load, 12, 11, 400);
        f.load(Op::F32Load, 13, 11, 404);
        f.op2(Op::I64TruncF32S, 14, 12);
        f.op2(Op::I64TruncF32S, 15, 13);
        report_i64(f, fns[1], 14);
        report_i64(f, fns[1], 15);
    });
    assert_eq!(run_user(&mut e, &got, &m), (vec![6, 6], vec![]));
}

#[test]
fn a_copied_struct_that_differs_or_is_missing_is_refused_by_name() {
    let (mut e, _) = engine();
    let ids = e.api_index();
    let mods_before = e.scripting().modules().len();

    // wrong size
    let mut t = engine_transform(&e);
    t.size += 4;
    let err = e.load_mod(&inv(Some(t)).encode(), ModOptions::new("inv")).unwrap_err();
    match &err {
        ScriptError::StructMismatch { name, expected, found } => {
            assert_eq!(name, "Transform");
            assert!(expected.contains("size 20") && found.contains("size 24"), "{expected} / {found}");
        }
        other => panic!("{other}"),
    }
    assert!(err.to_string().contains("`Transform`"), "{err}");

    // a field at another offset
    let mut t = engine_transform(&e);
    t.fields[1].ty = Prim::I32;
    assert!(matches!(e.load_mod(&inv(Some(t)).encode(), ModOptions::new("inv")), Err(ScriptError::StructMismatch { .. })));

    // not registered at all
    let mut t = engine_transform(&e);
    t.name = "Nowhere".into();
    match e.load_mod(&inv(Some(t)).encode(), ModOptions::new("inv")) {
        Err(ScriptError::StructMismatch { name, expected, .. }) => assert_eq!((name.as_str(), expected.as_str()), ("Nowhere", "not registered")),
        other => panic!("{other:?}"),
    }
    // nothing was left behind, and no module id was used
    assert_eq!(e.api_index(), ids);
    assert_eq!(e.scripting().modules().len(), mods_before);
}

#[test]
fn scratch_overflow_traps_in_the_caller_before_the_provider_runs() {
    let (mut e, got, _) = with_inv();
    // 5000 bytes of zeros are valid UTF-8 and in the caller's memory, but the scratch has 4096
    let m = caller(&e, &[("inv", "len")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(0), A::I64(5000)]);
        report_i64(f, fns[1], 10);
    });
    let (out, errs) = run_user(&mut e, &got, &m);
    assert!(out.is_empty());
    assert!(errs[0].contains("argument `s` of `inv.len` needs 5000 bytes of scratch, the mod has 4096"), "{errs:?}");
    // exactly the scratch size fits
    let m = caller(&e, &[("inv", "len")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(0), A::I64(4096)]);
        report_i64(f, fns[1], 10);
    });
    assert_eq!(run_user(&mut e, &got, &m), (vec![4096], vec![]));
    // the second argument does not fit after the first: `needs` counts up to its end
    let m = caller(&e, &[("inv", "where")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(0), A::I64(3000), A::I64(0), A::I64(3000)]);
        report_i64(f, fns[1], 10);
    });
    let (_, errs) = run_user(&mut e, &got, &m);
    assert!(errs[0].contains("argument `b` of `inv.where` needs 6000 bytes of scratch, the mod has 4096"), "{errs:?}");
}

#[test]
fn arguments_are_placed_left_to_right_aligned_to_8_and_empty_ones_get_the_cursor() {
    let (mut e, got, _) = with_inv();
    let s = SCRATCH.0 as i64;
    let mut run = |a_len: i64, b_len: i64| {
        let m = caller(&e, &[("inv", "where")], &[], |f, fns| {
            call_with(f, fns[0], 10, &[A::I64(0), A::I64(a_len), A::I64(0), A::I64(b_len)]);
            report_i64(f, fns[1], 10);
        });
        let (out, errs) = run_user(&mut e, &got, &m);
        assert!(errs.is_empty(), "{errs:?}");
        out[0]
    };
    assert_eq!(run(5, 3), s + 8, "b starts at align8(5)");
    assert_eq!(run(8, 3), s + 8);
    assert_eq!(run(9, 3), s + 16);
    assert_eq!(run(0, 3), s, "an empty first argument takes no space");
    assert_eq!(run(3, 0), s + 3, "an empty argument gets the cursor itself");
    assert_eq!(run(0, 0), s);
}

#[test]
fn the_copy_is_charged_to_the_providers_budget() {
    let (mut e, got) = engine();
    // `add` runs 2 instructions; a call with no copy costs 1 + 2 = 3 units
    e.load_mod(&inv(None).encode(), ModOptions::new("inv").with_budget(10)).unwrap();
    let m = caller(&e, &[("inv", "add")], &[], |f, fns| {
        for _ in 0..4 {
            call_with(f, fns[0], 10, &[A::I64(1), A::I64(2)]);
            report_i64(f, fns[1], 10);
        }
    });
    let (out, errs) = run_user(&mut e, &got, &m);
    assert_eq!(out, [3, 3, 3], "the 4th call needs 3 units, 1 is left");
    assert!(errs[0].contains("out of budget in `inv.add`"), "{errs:?}");

    // `len` of 1000 bytes: 1 + 1000/256 = 4 units to copy, 1 to run (`ret`)
    for (budget, ok) in [(5u64, true), (4, false), (3, false)] {
        let (mut e, got) = engine();
        e.load_mod(&inv(None).encode(), ModOptions::new("inv").with_budget(budget)).unwrap();
        let m = caller(&e, &[("inv", "len")], &[], |f, fns| {
            call_with(f, fns[0], 10, &[A::I64(0), A::I64(1000)]);
            report_i64(f, fns[1], 10);
        });
        let (out, errs) = run_user(&mut e, &got, &m);
        assert_eq!(out == [1000], ok, "budget {budget}: {out:?} {errs:?}");
        if !ok {
            assert!(errs[0].contains("out of budget in `inv.len`"), "{errs:?}");
        }
    }
    // `bytes` count twice (in and out): 1 + 2*512/256 = 5 units to copy, 3 to run fill
    for (budget, ok) in [(8u64, true), (7, false)] {
        let (mut e, got) = engine();
        e.load_mod(&inv(None).encode(), ModOptions::new("inv").with_budget(budget)).unwrap();
        let m = caller(&e, &[("inv", "fill")], &[], |f, fns| {
            call_with(f, fns[0], 10, &[A::I64(0), A::I64(512)]);
            report_i64(f, fns[1], 10);
        });
        let (out, errs) = run_user(&mut e, &got, &m);
        assert_eq!(out == [512], ok, "budget {budget}: {out:?} {errs:?}");
    }
}

#[test]
fn nested_calls_work_and_a_busy_module_is_refused() {
    let (mut e, got, _) = with_inv();
    // mod `mid` provides twice(x) = inv.add(x, x) and sum_len(s) = inv.len(s) (copied again)
    let idx = e.api_index();
    let add = idx.function("inv", "add").unwrap().func_type();
    let len = idx.function("inv", "len").unwrap().func_type();
    let mut mb = ModuleBuilder::new();
    let i_add = mb.import("inv", "add", add);
    let i_len = mb.import("inv", "len", len);
    mb.memory(1, None);
    let mut f = FunctionBuilder::new(8);
    f.mov(1, 0);
    f.call(i_add, 0);
    f.ret(0);
    let twice = mb.add_func(FuncType::new([I64], [I64]), f);
    let mut f = FunctionBuilder::new(8);
    f.call(i_len, 0);
    f.ret(0);
    let slen = mb.add_func(FuncType::new([I64, I64], [I64]), f);
    mb.interface(Interface {
        scratch: Some((512, 256)),
        structs: vec![],
        provides: vec![
            provide("twice", twice, "x + x via inv.add.", vec![param("x", ApiType::I64)], vec![param("y", ApiType::I64)]),
            provide("slen", slen, "Length of s via inv.len.", vec![param("s", ApiType::Str)], vec![param("n", ApiType::I64)]),
        ],
        ..Default::default()
    });
    e.load_mod(&mb.finish().unwrap().encode(), ModOptions::new("mid").allow("inv")).unwrap();

    // user -> mid -> inv, with a string copied twice
    let m = caller(&e, &[("mid", "twice"), ("mid", "slen")], &[(100, b"abcdef")], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(21)]);
        report_i64(f, fns[2], 10);
        call_with(f, fns[1], 10, &[A::I64(100), A::I64(6)]);
        report_i64(f, fns[2], 10);
    });
    assert_eq!(run_user(&mut e, &got, &m), (vec![42, 6], vec![]));
    // mid's scratch is smaller than inv's: 300 bytes do not fit in mid
    let m = caller(&e, &[("mid", "slen")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(0), A::I64(300)]);
        report_i64(f, fns[1], 10);
    });
    let (_, errs) = run_user(&mut e, &got, &m);
    assert!(errs[0].contains("argument `s` of `mid.slen` needs 300 bytes of scratch, the mod has 256"), "{errs:?}");
}

#[test]
fn a_game_module_with_an_interface_is_refused() {
    let (mut e, _) = engine();
    let before = e.api_index();
    let err = e.load_module("gm", &inv(None).encode()).unwrap_err();
    assert!(matches!(err, ScriptError::NotAMod(ref n) if n == "gm"), "{err}");
    assert!(err.to_string().contains("load_mod"));
    assert_eq!(e.api_index(), before);
    assert!(e.scripting().modules().is_empty());
}

#[test]
fn a_failed_registration_leaves_the_index_and_the_module_table_unchanged() {
    let (mut e, _) = engine();
    // the game already has a struct called Stat
    let mut d = stat();
    d.owner = "testgame".into();
    e.register_struct(d).unwrap();
    let before = e.api_index();
    let err = e.load_mod(&inv(None).encode(), ModOptions::new("inv")).unwrap_err();
    assert!(matches!(err, ScriptError::Registry(RegistryError::DuplicateStruct { .. })), "{err}");
    assert_eq!(e.api_index(), before, "the game's struct stays, nothing of inv was registered");
    assert!(e.scripting().modules().is_empty(), "no slot, so no id was used");
    assert!(e.types().by_name("Stat").is_some());

    // a function that is already registered under the mod's namespace (owned by the same name)
    let (mut e, _) = engine();
    e.define_namespace("inv", 1, "inv", "The game also owns this.").unwrap();
    e.register_host_fn(HostFn::new("inv", "add").doc("Taken."), |_c, _a, _r| Ok(HostOk::Continue)).unwrap();
    let before = e.api_index();
    assert!(matches!(
        e.load_mod(&inv(None).encode(), ModOptions::new("inv")),
        Err(ScriptError::Registry(RegistryError::DuplicateFunction { .. }))
    ));
    assert_eq!(e.api_index(), before);
    // other owner's namespace of the same name
    let (mut e, _) = engine();
    e.define_namespace("inv", 1, "someone", "Theirs.").unwrap();
    assert!(matches!(e.load_mod(&inv(None).encode(), ModOptions::new("inv")), Err(ScriptError::NameTaken(_))));
    // after a failure the next load gets the first id
    let (mut e, _) = engine();
    let mut nope = StructDesc::new("Nope", 4, 4);
    nope.owner = "engine".into();
    assert!(e.load_mod(&inv(Some(nope)).encode(), ModOptions::new("x")).is_err());
    assert_eq!(e.load_mod(&inv(None).encode(), ModOptions::new("inv")).unwrap(), ModuleId(0));
}

/// `inv` plus a module `user` that calls `inv.add` every frame.
fn with_inv_and_user() -> (Engine, Reports, ModuleId, ModuleId) {
    let (mut e, got) = engine();
    let inv_id = e.load_mod(&inv(None).encode(), ModOptions::new("inv")).unwrap();
    let m = caller(&e, &[("inv", "add")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(20), A::I64(22)]);
        report_i64(f, fns[1], 10);
    });
    let user_id = e.load_module("user", &m.encode()).unwrap();
    (e, got, inv_id, user_id)
}

#[test]
fn unload_is_refused_while_dependents_are_loaded_and_the_cascade_removes_both() {
    let (mut e, got, inv_id, user_id) = with_inv_and_user();
    e.run_frames(1, 0.016);
    assert_eq!(*got.lock().unwrap(), [42]);
    let before = e.api_index();
    let err = e.unload_module(inv_id).unwrap_err();
    match &err {
        ScriptError::Needed { module, by } => assert_eq!((module.as_str(), by.as_slice()), ("inv", ["user".to_string()].as_slice())),
        other => panic!("{other}"),
    }
    assert!(err.to_string().contains("`inv`") && err.to_string().contains("user"));
    assert!(e.scripting().is_loaded(inv_id));
    assert_eq!(e.api_index(), before);

    let removed = e.unload_module_and_dependents(inv_id).unwrap();
    assert_eq!(removed, [user_id, inv_id], "dependents first, the module last");
    assert!(!e.scripting().is_loaded(inv_id) && !e.scripting().is_loaded(user_id));
    let idx = e.api_index();
    assert!(idx.namespace("inv").is_none() && idx.function("inv", "add").is_none() && idx.struct_type("Stat").is_none());
    // the name and the registrations can be used again
    e.load_mod(&inv(None).encode(), ModOptions::new("inv")).unwrap();
    assert!(e.api_index().function("inv", "add").is_some());
    // a module nobody depends on unloads on its own
    let again = e.scripting().module_id("inv").unwrap();
    e.unload_module(again).unwrap();
}

#[test]
fn the_cascade_is_transitive_and_in_reverse_load_order() {
    let (mut e, _, inv_id, user_id) = with_inv_and_user();
    // a third module depends on `user`? game modules provide nothing, so chain through mods:
    // mid imports inv.add; user2 imports mid.twice
    let idx = e.api_index();
    let add = idx.function("inv", "add").unwrap().func_type();
    let mut mb = ModuleBuilder::new();
    let i_add = mb.import("inv", "add", add);
    let mut f = FunctionBuilder::new(8);
    f.mov(1, 0);
    f.call(i_add, 0);
    f.ret(0);
    let twice = mb.add_func(FuncType::new([I64], [I64]), f);
    mb.interface(Interface {
        provides: vec![provide("twice", twice, "x + x.", vec![param("x", ApiType::I64)], vec![param("y", ApiType::I64)])],
        ..Default::default()
    });
    let mid = e.load_mod(&mb.finish().unwrap().encode(), ModOptions::new("mid").allow("inv")).unwrap();
    let m = caller(&e, &[("mid", "twice")], &[], |f, fns| {
        call_with(f, fns[0], 10, &[A::I64(1)]);
        report_i64(f, fns[1], 10);
    });
    let user2 = e.load_module("user2", &m.encode()).unwrap();
    // mid is needed by user2 only
    match e.unload_module(mid).unwrap_err() {
        ScriptError::Needed { by, .. } => assert_eq!(by, ["user2"]),
        other => panic!("{other}"),
    }
    let removed = e.unload_module_and_dependents(inv_id).unwrap();
    assert_eq!(removed, [user2, mid, user_id, inv_id]);
    assert!(e.scripting().modules().iter().all(|m| !m.loaded));
    assert!(e.unload_module_and_dependents(inv_id).is_err(), "already unloaded");
}

#[cfg(not(target_family = "wasm"))]
mod reload {
    use super::*;

    struct Dir(std::path::PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("efe_provides_{}_{tag}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        fn write(&self, name: &str, m: &Module) -> std::path::PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, m.encode()).unwrap();
            p
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// Replace the file and make the watcher see a newer modification time (`step` seconds
    /// further into the future each time).
    fn replace(path: &std::path::Path, m: &Module, step: u64) {
        std::fs::write(path, m.encode()).unwrap();
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(60 * step);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    /// A provider `m` with the given provides over functions `k(a) = a + k` (one per name).
    fn prov(names: &[(&str, i64)], doc: &str) -> Module {
        let mut mb = ModuleBuilder::new();
        let mut provides = Vec::new();
        for (name, k) in names {
            let mut f = FunctionBuilder::new(4);
            f.const_i64(1, *k);
            f.op3(Op::I64Add, 2, 0, 1);
            f.ret(2);
            let id = mb.add_func(FuncType::new([I64], [I64]), f);
            provides.push(provide(name, id, doc, vec![param("a", ApiType::I64)], vec![param("r", ApiType::I64)]));
        }
        mb.interface(Interface { provides, ..Default::default() });
        mb.finish().unwrap()
    }

    fn user_of(e: &Engine, fname: &str) -> Module {
        caller(e, &[("m", fname)], &[], |f, fns| {
            call_with(f, fns[0], 10, &[A::I64(100)]);
            report_i64(f, fns[1], 10);
        })
    }

    fn provider_file(dir: &Dir, m: &Module) -> (Engine, Reports, ModuleId, std::path::PathBuf) {
        let (mut e, got) = engine();
        let path = dir.write("m.efvm", m);
        let id = e.load_mod(&m.encode(), ModOptions::new("m").allow("m")).unwrap();
        e.watch_module(id, &path).unwrap();
        (e, got, id, path)
    }

    #[test]
    fn reload_adds_replaces_and_unregisters_provides() {
        let dir = Dir::new("rules");
        let (mut e, got, _, path) = provider_file(&dir, &prov(&[("inc", 1), ("dec", -1)], "Old doc."));
        assert!(e.api_index().function("m", "dec").is_some());

        // changed code and doc, one provide dropped, one added: no dependents, so accepted
        replace(&path, &prov(&[("inc", 10), ("twice_inc", 20)], "New doc."), 1);
        e.run_frames(1, 0.3);
        assert!(errors(&e).is_empty(), "{:?}", errors(&e));
        let idx = e.api_index();
        assert!(idx.function("m", "dec").is_none(), "a dropped provide is unregistered");
        assert_eq!(idx.function("m", "inc").unwrap().doc, "New doc.");
        assert!(idx.function("m", "twice_inc").is_some());
        idx.validate().unwrap();
        let m = user_of(&e, "inc");
        assert_eq!(run_user(&mut e, &got, &m), (vec![110], vec![]), "the new code runs");
        assert!(e.scripting().log_lines().any(|l| l.text.contains("reloaded")));
    }

    #[test]
    fn a_dependent_that_a_reload_would_break_keeps_the_old_version_running() {
        let dir = Dir::new("refuse");
        let (mut e, got, _, path) = provider_file(&dir, &prov(&[("inc", 1), ("other", 5)], "Doc."));
        let m = user_of(&e, "inc");
        e.load_module("user", &m.encode()).unwrap();
        e.run_frames(1, 0.3);
        assert_eq!(*got.lock().unwrap(), [101]);

        // 1. a provide the dependent imports disappears
        replace(&path, &prov(&[("other", 5)], "Doc."), 1);
        e.run_frames(1, 0.3);
        let errs = errors(&e);
        assert_eq!(errs.iter().filter(|l| l.contains("reload failed")).count(), 1, "{errs:?}");
        assert!(errs[0].contains("`user` imports `m.inc`") && errs[0].contains("does not provide"), "{}", errs[0]);
        assert!(e.api_index().function("m", "inc").is_some());
        e.run_frames(1, 0.3);
        assert_eq!(*got.lock().unwrap(), [101, 101, 101], "the old version still serves the dependent");

        // 2. its lowered signature changes (i64 -> i32)
        let mut mb = ModuleBuilder::new();
        let mut f = FunctionBuilder::new(4);
        f.ret(0);
        let id = mb.add_func(FuncType::new([I32], [I32]), f);
        mb.interface(Interface {
            provides: vec![provide("inc", id, "Doc.", vec![param("a", ApiType::I32)], vec![param("r", ApiType::I32)])],
            ..Default::default()
        });
        replace(&path, &mb.finish().unwrap(), 2);
        e.run_frames(1, 0.3);
        let errs = errors(&e);
        assert_eq!(errs.iter().filter(|l| l.contains("reload failed")).count(), 2, "{errs:?}");
        assert!(errs[1].contains("`user` imports `m.inc` as"), "{}", errs[1]);
        assert_eq!(e.api_index().function("m", "inc").unwrap().signature_text(), "(a: i64) -> (r: i64)");

        // 3. a compatible reload (same signature, other code, extra provide) is accepted
        replace(&path, &prov(&[("inc", 1000), ("extra", 1)], "Doc."), 3);
        e.run_frames(2, 0.3);
        assert_eq!(errors(&e).len(), 2);
        assert_eq!(got.lock().unwrap().last(), Some(&1100));
        assert!(e.api_index().function("m", "extra").is_some());
        // and dropping a provide nobody imports (`extra`) is accepted too
        replace(&path, &prov(&[("inc", 1000)], "Doc."), 4);
        e.run_frames(1, 0.3);
        assert_eq!(errors(&e).len(), 2, "{:?}", errors(&e));
        assert!(e.api_index().function("m", "extra").is_none());
        assert!(e.api_index().function("m", "inc").is_some());
    }

    /// A mod with provide `put(&Stat)` where Stat is `{hp: i32}` or, changed, `{hp: i64}`.
    fn stat_mod(wide: bool) -> Module {
        let mut mb = ModuleBuilder::new();
        mb.memory(1, None);
        let mut f = FunctionBuilder::new(4);
        f.const_i32(1, 0);
        f.ret(1);
        let id = mb.add_func(FuncType::new([I64], [I32]), f);
        let d = if wide {
            StructDesc::new("Stat", 8, 8).field(FieldDesc::new("hp", 0, Prim::I64, 1))
        } else {
            StructDesc::new("Stat", 4, 4).field(FieldDesc::new("hp", 0, Prim::I32, 1))
        };
        mb.interface(Interface {
            scratch: Some((64, 64)),
            structs: vec![d],
            provides: vec![provide("put", id, "Put.", vec![param("s", ApiType::Struct("Stat".into()))], vec![param("r", ApiType::I32)])],
            ..Default::default()
        });
        mb.finish().unwrap()
    }

    #[test]
    fn a_struct_a_dependent_copied_cannot_change_layout() {
        let dir = Dir::new("struct");
        let (mut e, _, _, path) = provider_file(&dir, &stat_mod(false));
        // the dependent mod copies Stat from `m` and imports m.put
        let put = e.api_index().function("m", "put").unwrap().func_type();
        let mut mb = ModuleBuilder::new();
        let i_put = mb.import("m", "put", put);
        mb.memory(1, None);
        let mut f = FunctionBuilder::new(4);
        f.call(i_put, 0);
        f.ret(0);
        let id = mb.add_func(FuncType::new([I64], [I32]), f);
        let mut copy = e.api_index().struct_type("Stat").unwrap().clone();
        copy.owner = "m".into();
        mb.interface(Interface {
            scratch: Some((64, 64)),
            structs: vec![copy],
            provides: vec![provide("fwd", id, "Forward.", vec![param("s", ApiType::Struct("Stat".into()))], vec![param("r", ApiType::I32)])],
            ..Default::default()
        });
        e.load_mod(&mb.finish().unwrap().encode(), ModOptions::new("dep").allow("m")).unwrap();

        replace(&path, &stat_mod(true), 1);
        e.run_frames(1, 0.3);
        let errs = errors(&e);
        assert!(errs.iter().any(|l| l.contains("`dep` uses struct `Stat`") && l.contains("layout")), "{errs:?}");
        assert_eq!(e.api_index().struct_type("Stat").unwrap().size, 4, "the old layout stays");
        // the dependent also stops the unload of `m`
        let m_id = e.scripting().module_id("m").unwrap();
        match e.unload_module(m_id).unwrap_err() {
            ScriptError::Needed { by, .. } => assert_eq!(by, ["dep"]),
            other => panic!("{other}"),
        }
    }

    #[test]
    fn a_reload_into_a_module_that_calls_itself_gets_the_reentrance_error() {
        let dir = Dir::new("reenter");
        let (mut e, got, _, path) = provider_file(&dir, &prov(&[("inc", 1)], "Doc."));
        // v2 of `inc` calls `m.inc` (itself, through the registry) before returning
        let own = e.api_index().function("m", "inc").unwrap().func_type();
        let mut mb = ModuleBuilder::new();
        let i_inc = mb.import("m", "inc", own);
        let mut f = FunctionBuilder::new(4);
        f.call(i_inc, 0);
        f.ret(0);
        let id = mb.add_func(FuncType::new([I64], [I64]), f);
        mb.interface(Interface {
            provides: vec![provide("inc", id, "Doc.", vec![param("a", ApiType::I64)], vec![param("r", ApiType::I64)])],
            ..Default::default()
        });
        replace(&path, &mb.finish().unwrap(), 1);
        e.run_frames(1, 0.3);
        assert!(errors(&e).is_empty(), "{:?}", errors(&e));
        let m = user_of(&e, "inc");
        let (out, errs) = run_user(&mut e, &got, &m);
        assert!(out.is_empty());
        assert!(errs[0].contains("module m is already running (re-entrant call)"), "{errs:?}");
        assert!(e.is_running());
    }
}
