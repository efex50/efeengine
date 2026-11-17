//! E8 T13: `.efmod` packages in the engine: policy and grant, id clash, game check, dependency
//! resolution, cross-mod calls, index, unload, assets, memory limit, directory loading and
//! hot reload. Headless; modules are built with `ModuleBuilder`, packages with `Package::write`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use efeengine_core::efemod::{Dependency, Manifest, Package, VersionReq};
use efeengine_core::efevm::ValType::{F32, I32, I64};
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;

/// Test shim: load one `.efmod` through `load_input`.
trait LoadPackage {
    fn load_package(&mut self, bytes: &[u8]) -> Result<ModuleId, ModError>;
}

impl LoadPackage for Engine {
    fn load_package(&mut self, bytes: &[u8]) -> Result<ModuleId, ModError> {
        match self.load_input(LoadInput::Package { label: "test.efmod", bytes }) {
            Ok(l) => Ok(l.id()),
            Err(efeengine_core::prelude::LoadError::Mod(e)) => Err(e),
            Err(efeengine_core::prelude::LoadError::Script(e)) => Err(ModError::Script(e)),
            Err(e) => panic!("unexpected load error: {e}"),
        }
    }
}

/// A one-shot mod source: hands its items out on the first `list`.
struct VecSource(Vec<ModItem>);

impl ModSource for VecSource {
    fn list(&mut self) -> Vec<ModItem> {
        std::mem::take(&mut self.0)
    }
}

fn load_packages(e: &mut Engine, packages: Vec<(String, Vec<u8>)>) -> LoadReport {
    e.add_mod_source(VecSource(packages.into_iter().map(|(label, b)| ModItem { label, body: ModBody::Package(b) }).collect()));
    e.load_mods()
}

type Reports = Arc<Mutex<Vec<i64>>>;

fn engine() -> (Engine, Reports) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
    e.define_namespace("test", 1, "testgame", "Helpers of the test game.").unwrap();
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(HostFn::new("test", "report").doc("Record `v` for the test.").param("v", ApiType::I64), move |_c, a, _r| {
        g.lock().unwrap().push(a.i64(0)?);
        Ok(HostOk::Continue)
    })
    .unwrap();
    (e, got)
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn grant(ns: &[&str]) -> Grant {
    Grant { namespaces: ns.iter().map(|s| s.to_string()).collect(), ..Grant::default() }
}

/// A policy that grants `ns` to everyone and counts its calls.
fn counting(ns: &'static [&'static str]) -> (impl FnMut(&ModRequest<'_>) -> Result<Grant, String>, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    (
        move |_r: &ModRequest<'_>| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(grant(ns))
        },
        n,
    )
}

fn manifest(id: &str, version: &str) -> Manifest {
    Manifest::new(id, version.parse().unwrap())
}

fn dep(id: &str, req: &str) -> Dependency {
    Dependency { id: id.into(), req: req.parse::<VersionReq>().unwrap() }
}

fn pack(m: &Manifest, module: &Module, assets: &[(&str, &[u8])]) -> Vec<u8> {
    let assets: BTreeMap<String, Vec<u8>> = assets.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect();
    Package::write(m, &module.encode(), &assets).unwrap()
}

fn param(name: &str, ty: ApiType) -> Param {
    Param::new(name, ty)
}

fn provide(name: &str, func: u32, doc: &str, params: Vec<Param>, results: Vec<Param>) -> Provide {
    Provide { name: name.into(), func, doc: doc.into(), params, results }
}

fn stat() -> StructDesc {
    StructDesc::new("Stat", 4, 4).field(FieldDesc::new("hp", 0, Prim::I32, 1)).doc("A stat.")
}

fn func_type(e: &Engine, ns: &str, name: &str) -> FuncType {
    e.api_index().function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is not in the index")).func_type()
}

/// A module that provides nothing and imports nothing.
fn empty() -> Module {
    let mut mb = ModuleBuilder::new();
    let mut f = FunctionBuilder::new(2);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

/// A module whose `init` logs `msg` with `engine.log.info`.
fn logger(e: &Engine, msg: &str) -> Module {
    let mut mb = ModuleBuilder::new();
    let info = mb.import("engine.log", "info", func_type(e, "engine.log", "info"));
    mb.memory(1, None);
    mb.data(100, msg.as_bytes());
    let mut f = FunctionBuilder::new(16);
    f.const_i64(10, 100);
    f.const_i64(11, msg.len() as i64);
    f.call(info, 10);
    f.ret(0);
    let id = mb.add_func(FuncType::new([], []), f);
    mb.export_func("init", id);
    mb.finish().unwrap()
}

/// The provider `a`: `add(a, b)`, `len(s: str)`, `bump(s: &Stat)` (hp += 1, returns the old
/// value) and, with `who`, `who_len()` = the length of its own asset `who`.
fn lib_a(who: bool) -> Module {
    let mut mb = ModuleBuilder::new();
    let asset_len = who.then(|| mb.import("engine.mods", "asset_len", FuncType::new([I64, I64], [I64])));
    mb.memory(1, None);
    mb.data(100, b"who");
    let mut f = FunctionBuilder::new(4);
    f.op3(Op::I64Add, 2, 0, 1);
    f.ret(2);
    let add = mb.add_func(FuncType::new([I64, I64], [I64]), f);
    let mut f = FunctionBuilder::new(4);
    f.ret(1);
    let len = mb.add_func(FuncType::new([I64, I64], [I64]), f);
    let mut f = FunctionBuilder::new(4);
    f.load(Op::I32Load, 1, 0, 0);
    f.const_i32(2, 1);
    f.op3(Op::I32Add, 2, 1, 2);
    f.store(Op::I32Store, 2, 0, 0);
    f.ret(1);
    let bump = mb.add_func(FuncType::new([I64], [I32]), f);
    let mut provides = vec![
        provide("add", add, "Add two numbers.", vec![param("a", ApiType::I64), param("b", ApiType::I64)], vec![param("sum", ApiType::I64)]),
        provide("len", len, "Length of a string.", vec![param("s", ApiType::Str)], vec![param("n", ApiType::I64)]),
        provide("bump", bump, "Increment hp; returns the old value.", vec![param("stat", ApiType::Struct("Stat".into()))], vec![param("old", ApiType::I32)]),
    ];
    if let Some(asset_len) = asset_len {
        let mut f = FunctionBuilder::new(8);
        f.const_i64(1, 100);
        f.const_i64(2, 3);
        f.call(asset_len, 1);
        f.ret(1);
        let who = mb.add_func(FuncType::new([], [I64]), f);
        provides.push(provide("who_len", who, "The length of the provider's own asset `who`.", vec![], vec![param("n", ApiType::I64)]));
    }
    mb.interface(Interface { scratch: Some((1024, 4096)), structs: vec![stat()], provides, ..Default::default() });
    mb.finish().unwrap()
}

fn a_manifest(version: &str, who: bool) -> Manifest {
    let mut m = manifest("a", version);
    if who {
        m.permissions.namespaces = vec!["engine.mods".into()];
    }
    m
}

fn a_package(version: &str) -> Vec<u8> {
    pack(&a_manifest(version, false), &lib_a(false), &[])
}

fn report(f: &mut FunctionBuilder, report: u32, src: u16) {
    f.mov(60, src);
    f.call(report, 60);
}

fn report_i32(f: &mut FunctionBuilder, report: u32, src: u16) {
    f.op2(Op::I64ExtendI32S, 60, src);
    f.call(report, 60);
}

/// A consumer of `a` with a `frame`: reports `a.len("hello")`, `a.bump(stat)` and the stat's hp
/// afterwards, and `a.who_len()` when asked, then its own `engine.mods.asset_len("who")`.
fn consumer(e: &Engine, who: bool) -> Module {
    let mut mb = ModuleBuilder::new();
    let len = mb.import("a", "len", func_type(e, "a", "len"));
    let bump = mb.import("a", "bump", func_type(e, "a", "bump"));
    let rep = mb.import("test", "report", func_type(e, "test", "report"));
    let who_fns = who.then(|| {
        (mb.import("a", "who_len", func_type(e, "a", "who_len")), mb.import("engine.mods", "asset_len", func_type(e, "engine.mods", "asset_len")))
    });
    mb.memory(1, None);
    mb.data(100, b"hello");
    mb.data(200, &41i32.to_le_bytes());
    mb.data(300, b"who");
    let mut f = FunctionBuilder::new(64);
    f.const_i64(10, 100);
    f.const_i64(11, 5);
    f.call(len, 10);
    report(&mut f, rep, 10);
    f.const_i64(10, 200);
    f.call(bump, 10);
    report_i32(&mut f, rep, 10);
    f.const_i64(13, 200);
    f.load(Op::I32Load, 12, 13, 0);
    report_i32(&mut f, rep, 12);
    if let Some((who_len, asset_len)) = who_fns {
        f.call(who_len, 10);
        report(&mut f, rep, 10);
        f.const_i64(10, 300);
        f.const_i64(11, 3);
        f.call(asset_len, 10);
        report(&mut f, rep, 10);
    }
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

fn b_manifest(who: bool) -> Manifest {
    let mut m = manifest("b", "1.0.0");
    m.dependencies = vec![dep("a", "^1")];
    m.permissions.namespaces = vec!["test".into()];
    if who {
        m.permissions.namespaces.push("engine.mods".into());
    }
    m
}

// ---------------------------------------------------------------------------------------------

#[test]
fn load_package_gives_package_info_and_the_grant_and_runs_init() {
    let (mut e, _) = engine();
    e.set_mod_policy(|r: &ModRequest<'_>| {
        assert_eq!((r.manifest.id.as_str(), r.imports), ("logmod", ["engine.log".to_string()].as_slice()));
        Ok(Grant { namespaces: vec!["engine.log".into()], budget: 500, memory_pages: 4, ..Grant::default() })
    });
    let mut m = manifest("logmod", "1.2.3");
    m.permissions.namespaces = vec!["engine.log".into()];
    let id = e.load_package(&pack(&m, &logger(&e, "hi from init"), &[("x.txt", b"X")])).unwrap();
    let info = e.package(id).unwrap();
    assert_eq!(info.manifest, m);
    assert_eq!(info.grant, Grant { namespaces: vec!["engine.log".into()], budget: 500, memory_pages: 4, ..Grant::default() });
    assert_eq!(info.assets.len(), 1);
    let mi = e.scripting().modules().into_iter().find(|x| x.id == id).unwrap();
    assert_eq!((mi.name.as_str(), mi.budget), ("logmod", 500));
    assert!(e.scripting().log_lines().any(|l| l.text == "[logmod] hi from init"), "init ran at once");
    assert!(e.package(ModuleId(99)).is_none());
}

#[test]
fn the_default_policy_grants_nothing() {
    let (mut e, _) = engine();
    let mut m = manifest("logmod", "1.0.0");
    m.permissions.namespaces = vec!["engine.log".into()];
    let err = e.load_package(&pack(&m, &logger(&e, "x"), &[])).unwrap_err();
    match &err {
        ModError::NotGranted { id, namespaces } => assert_eq!((id.as_str(), namespaces.as_slice()), ("logmod", ["engine.log".to_string()].as_slice())),
        other => panic!("{other}"),
    }
    assert!(err.to_string().contains("engine.log"));
    assert!(e.scripting().module_id("logmod").is_none());
    // the same package under a policy that grants engine.log loads
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine.log"])));
    e.load_package(&pack(&m, &logger(&e, "x"), &[])).unwrap();
}

#[test]
fn a_package_that_only_uses_its_dependencies_loads_under_the_default_policy() {
    let (mut e, got) = engine();
    let a = e.load_package(&a_package("1.0.0")).unwrap();
    assert_eq!(e.package(a).unwrap().grant, Grant::default());
    // `test` is not granted: the consumer must not import it, so only `a` is imported here
    let mut mb = ModuleBuilder::new();
    let add = mb.import("a", "add", func_type(&e, "a", "add"));
    let mut f = FunctionBuilder::new(8);
    f.const_i64(1, 20);
    f.const_i64(2, 22);
    f.call(add, 1);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    let mut m = manifest("c", "1.0.0");
    m.dependencies = vec![dep("a", "^1")];
    let c = e.load_package(&pack(&m, &mb.finish().unwrap(), &[])).unwrap();
    assert!(e.package(c).unwrap().grant.namespaces.is_empty());
    e.run_frames(1, 0.016);
    assert!(errors(&e).is_empty() && got.lock().unwrap().is_empty());
}

#[test]
fn the_grant_is_intersected_with_the_request() {
    let (mut e, _) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine.log", "engine.world", "game"])));
    let mut m = manifest("logmod", "1.0.0");
    m.permissions.namespaces = vec!["engine.log".into(), "engine.time".into()];
    let id = e.load_package(&pack(&m, &logger(&e, "x"), &[])).unwrap();
    assert_eq!(e.package(id).unwrap().grant.namespaces, ["engine.log"]);

    // a broader grant gives what was requested, a narrower request wins
    let (mut e, _) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine"])));
    let id = e.load_package(&pack(&m, &logger(&e, "x"), &[])).unwrap();
    assert_eq!(e.package(id).unwrap().grant.namespaces, ["engine.log", "engine.time"]);

    // an empty request grants nothing even to a generous policy: NotGranted
    let (mut e, _) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["engine.log"])));
    let mut m2 = manifest("logmod", "1.0.0");
    m2.permissions.namespaces = vec!["engine.log".into()];
    assert!(e.load_package(&pack(&m2, &logger(&e, "x"), &[])).is_ok());
}

#[test]
fn the_policy_can_deny() {
    let (mut e, _) = engine();
    e.set_mod_policy(|r: &ModRequest<'_>| Err(format!("{} is not welcome", r.manifest.id)));
    match e.load_package(&a_package("1.0.0")).unwrap_err() {
        ModError::Denied { id, reason } => assert_eq!((id.as_str(), reason.as_str()), ("a", "a is not welcome")),
        other => panic!("{other}"),
    }
    assert!(e.scripting().module_id("a").is_none());
}

#[test]
fn an_id_that_equals_or_prefixes_a_namespace_is_refused_before_the_policy() {
    let (mut e, _) = engine();
    let (policy, calls) = counting(&[]);
    e.set_mod_policy(policy);
    e.define_namespace("game", 1, "testgame", "The game.").unwrap();
    e.define_namespace("arena.items", 1, "testgame", "Items.").unwrap();
    let pk = |id: &str| pack(&manifest(id, "1.0.0"), &empty(), &[]);

    match e.load_package(&pk("game")).unwrap_err() {
        ModError::IdClash { id, namespace, owner } => assert_eq!((id.as_str(), namespace.as_str(), owner.as_str()), ("game", "game", "testgame")),
        other => panic!("{other}"),
    }
    match e.load_package(&pk("arena")).unwrap_err() {
        ModError::IdClash { id, namespace, owner } => assert_eq!((id.as_str(), namespace.as_str(), owner.as_str()), ("arena", "arena.items", "testgame")),
        other => panic!("{other}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the policy is not asked");
    // `arena_x` merely starts with the same letters: fine
    e.load_package(&pk("arena_x")).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // load_mods checks the same, also before the policy
    let report = load_packages(&mut e, vec![("g".into(), pk("game")), ("ok".into(), pk("fine"))]);
    assert_eq!(report.loaded.len(), 1);
    assert!(matches!(&report.failed[..], [(l, ModError::IdClash { .. })] if l == "g"), "{:?}", report.failed);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // another mod's namespace
    let a = e.load_package(&a_package("1.0.0")).unwrap();
    assert!(e.scripting().is_loaded(a));
    assert!(matches!(e.load_package(&a_package("1.0.0")), Err(ModError::AlreadyLoaded { id, same_contents: true, .. }) if id == "a"));

    // reserved ids cannot even be written
    for id in ["engine", "std", "efe"] {
        assert!(Package::write(&manifest(id, "1.0.0"), &empty().encode(), &BTreeMap::new()).is_err(), "{id}");
    }
}

#[test]
fn the_game_must_match() {
    let (mut e, _) = engine();
    e.set_game_name("mygame");
    let mut m = manifest("g", "1.0.0");
    m.game = Some("other".into());
    match e.load_package(&pack(&m, &empty(), &[])).unwrap_err() {
        ModError::GameMismatch { id, wanted, game } => assert_eq!((id.as_str(), wanted.as_str(), game.as_str()), ("g", "other", "mygame")),
        other => panic!("{other}"),
    }
    m.game = Some("mygame".into());
    e.load_package(&pack(&m, &empty(), &[])).unwrap();
}

#[test]
fn a_missing_or_mismatched_dependency_is_refused_by_name() {
    let (mut e, _) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["test", "engine.mods"])));
    let b = |m: Manifest| pack(&m, &empty(), &[]);
    let mut m = manifest("b", "1.0.0");
    m.dependencies = vec![dep("a", "^1")];
    match e.load_package(&b(m.clone())).unwrap_err() {
        ModError::Resolve(efemod_problem) => assert!(efemod_problem.to_string().contains("`a`"), "{efemod_problem}"),
        other => panic!("{other}"),
    }
    // a bare mod called `a` has no version and never satisfies a dependency
    e.load_mod(&lib_a(false).encode(), ModOptions::new("a")).unwrap();
    assert!(matches!(e.load_package(&b(m.clone())), Err(ModError::Resolve(_))));
    e.unload_module(e.scripting().module_id("a").unwrap()).unwrap();

    e.load_package(&a_package("2.0.0")).unwrap();
    let err = e.load_package(&b(m.clone())).unwrap_err();
    assert!(matches!(&err, ModError::Resolve(p) if p.to_string().contains("2.0.0")), "{err}");
    m.dependencies = vec![dep("a", "^2")];
    e.load_package(&b(m)).unwrap();
}

#[test]
fn load_mods_orders_by_dependencies_skips_dependents_and_reports_duplicates() {
    let (mut e, _) = engine();
    let with = |id: &str, deps: &[&str]| {
        let mut m = manifest(id, "1.0.0");
        m.dependencies = deps.iter().map(|d| dep(d, "*")).collect();
        pack(&m, &empty(), &[])
    };
    // `bad` imports engine.log, which the default policy does not grant
    let mut bm = manifest("bad", "1.0.0");
    bm.permissions.namespaces = vec!["engine.log".into()];
    let bad = pack(&bm, &logger(&e, "x"), &[]);
    let report = load_packages(&mut e, vec![
        ("c".into(), with("c", &["b"])),
        ("b".into(), with("b", &["a"])),
        ("a".into(), with("a", &[])),
        ("d".into(), with("d", &["bad"])),
        ("bad".into(), bad),
        ("dup1".into(), with("dup", &[])),
        ("dup2".into(), with("dup", &[])),
        ("garbage".into(), vec![1, 2, 3]),
        ("ok".into(), with("ok", &[])),
    ]);
    let loaded: Vec<&str> = report.loaded.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(loaded, ["a", "b", "c", "ok"]);
    let failed: BTreeMap<&str, &ModError> = report.failed.iter().map(|(l, e)| (l.as_str(), e)).collect();
    assert_eq!(failed.len(), 5, "{:?}", report.failed);
    assert!(matches!(failed["garbage"], ModError::Package(_)));
    assert!(matches!(failed["dup1"], ModError::Resolve(p) if p.to_string().contains("duplicate")));
    assert!(matches!(failed["dup2"], ModError::Resolve(p) if p.to_string().contains("duplicate")));
    assert!(matches!(failed["bad"], ModError::NotGranted { .. }));
    assert!(matches!(failed["d"], ModError::Resolve(p) if p.to_string().contains("`bad` failed")), "{}", failed["d"]);

    // loading `a` again with the same contents: skipped, not a failure
    let r = load_packages(&mut e, vec![("a2".into(), with("a", &[]))]);
    assert!(r.failed.is_empty() && r.loaded.is_empty(), "{:?}", r.failed);
    assert!(matches!(&r.skipped[..], [(l, _)] if l == "a2"));
}

fn with_a_and_b(who: bool) -> (Engine, Reports, ModuleId, ModuleId) {
    let (mut e, got) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["test", "engine.mods"])));
    let a = if who {
        e.load_package(&pack(&a_manifest("1.0.0", true), &lib_a(true), &[("who", b"xyz")])).unwrap()
    } else {
        e.load_package(&a_package("1.0.0")).unwrap()
    };
    let b = e.load_package(&pack(&b_manifest(who), &consumer(&e, who), &[])).unwrap();
    (e, got, a, b)
}

#[test]
fn a_mod_calls_another_mods_provide_with_a_str_and_a_struct() {
    let (mut e, got, _, _) = with_a_and_b(false);
    e.run_frames(1, 0.016);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(*got.lock().unwrap(), [5, 41, 42], "len(\"hello\"), the old hp, the hp copied back");
}

#[test]
fn the_index_lists_both_mods_and_unload_is_needed_then_cascades() {
    let (mut e, _, a, b) = with_a_and_b(false);
    let before_both = e.api_index();
    before_both.validate().unwrap();
    for f in ["add", "len", "bump"] {
        assert_eq!(before_both.function("a", f).unwrap().owner, "a");
    }
    assert_eq!(before_both.namespace("a").unwrap().owner, "a");
    assert_eq!(before_both.struct_type("Stat").unwrap().owner, "a");
    // every callable function is in the index with a doc
    for (ns, n, ty) in e.scripting().registry().imports().names() {
        let f = before_both.function(ns, n).unwrap();
        assert!(!f.doc.is_empty() && &f.func_type() == ty, "{ns}.{n}");
    }

    let err = e.unload_module(a).unwrap_err();
    assert!(matches!(&err, ScriptError::Needed { module, by } if module == "a" && by == &["b".to_string()]), "{err}");
    assert!(e.api_index().function("a", "add").is_some() && e.package(a).is_some());

    assert_eq!(e.unload_module_and_dependents(a).unwrap(), [b, a]);
    let idx = e.api_index();
    assert!(idx.namespace("a").is_none() && idx.function("a", "add").is_none() && idx.struct_type("Stat").is_none());
    assert!(e.package(a).is_none() && e.package(b).is_none(), "unload drops the package info");
    idx.validate().unwrap();
}

#[test]
fn assets_are_read_by_the_game_and_by_the_mod_itself() {
    let (mut e, got, a, b) = with_a_and_b(true);
    assert_eq!(e.package_asset(a, "who"), Some(&b"xyz"[..]));
    assert_eq!(e.package_asset(a, "nope"), None);
    assert_eq!(e.package_asset(b, "who"), None);
    e.run_frames(1, 0.016);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    // a.who_len reads the provider's own asset (3), b's own `who` does not exist (-1)
    assert_eq!(*got.lock().unwrap(), [5, 41, 42, 3, -1]);
    let idx = e.api_index();
    for f in ["asset_len", "asset_read"] {
        assert!(!idx.function("engine.mods", f).unwrap().doc.is_empty());
    }
    assert_eq!(idx.namespace("engine.mods").unwrap().version, 2);
}

#[test]
fn asset_read_copies_what_fits_and_returns_the_full_length() {
    let (mut e, got) = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(grant(&["test", "engine.mods"])));
    let mut mb = ModuleBuilder::new();
    let len = mb.import("engine.mods", "asset_len", func_type(&e, "engine.mods", "asset_len"));
    let read = mb.import("engine.mods", "asset_read", func_type(&e, "engine.mods", "asset_read"));
    let rep = mb.import("test", "report", func_type(&e, "test", "report"));
    mb.memory(1, None);
    mb.data(100, b"data/x.bin");
    mb.data(200, b"nope");
    let mut f = FunctionBuilder::new(64);
    f.const_i64(10, 100);
    f.const_i64(11, 10);
    f.call(len, 10);
    report(&mut f, rep, 10);
    // asset_read into 4 bytes at 300
    f.const_i64(10, 100);
    f.const_i64(11, 10);
    f.const_i64(12, 300);
    f.const_i64(13, 4);
    f.call(read, 10);
    report(&mut f, rep, 10);
    f.const_i64(14, 300);
    f.load(Op::I64Load8U, 15, 14, 0);
    report(&mut f, rep, 15);
    f.load(Op::I64Load8U, 15, 14, 4);
    report(&mut f, rep, 15);
    // a missing asset
    f.const_i64(10, 200);
    f.const_i64(11, 4);
    f.call(len, 10);
    report(&mut f, rep, 10);
    f.const_i64(10, 200);
    f.const_i64(11, 4);
    f.const_i64(12, 300);
    f.const_i64(13, 4);
    f.call(read, 10);
    report(&mut f, rep, 10);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    let mut m = manifest("rd", "1.0.0");
    m.permissions.namespaces = vec!["test".into(), "engine.mods".into()];
    let module = mb.finish().unwrap();
    let rd = e.load_package(&pack(&m, &module, &[("data/x.bin", &[10, 20, 30, 40, 50, 60])])).unwrap();
    e.run_frames(1, 0.016);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(*got.lock().unwrap(), [6, 6, 10, 0, -1, -1], "len, full length, the 4 bytes that fit, the byte after stays 0");

    // a module that is not a package gets -1
    e.unload_module(rd).unwrap();
    got.lock().unwrap().clear();
    let mut mb = ModuleBuilder::new();
    let len = mb.import("engine.mods", "asset_len", func_type(&e, "engine.mods", "asset_len"));
    let rep = mb.import("test", "report", func_type(&e, "test", "report"));
    mb.memory(1, None);
    mb.data(100, b"data/x.bin");
    let mut f = FunctionBuilder::new(64);
    f.const_i64(10, 100);
    f.const_i64(11, 10);
    f.call(len, 10);
    report(&mut f, rep, 10);
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    e.load_module("plain", &mb.finish().unwrap().encode()).unwrap();
    e.run_frames(1, 0.016);
    assert_eq!(*got.lock().unwrap(), [-1]);
}

#[test]
fn the_granted_memory_pages_limit_the_instance() {
    let (mut e, _) = engine();
    let two_pages = {
        let mut mb = ModuleBuilder::new();
        mb.memory(2, None);
        mb.finish().unwrap()
    };
    let one_page = {
        let mut mb = ModuleBuilder::new();
        mb.memory(1, None);
        mb.finish().unwrap()
    };
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant { memory_pages: 1, ..Grant::default() }));
    let err = e.load_package(&pack(&manifest("big", "1.0.0"), &two_pages, &[])).unwrap_err();
    assert!(matches!(&err, ModError::Script(ScriptError::Link { .. })), "{err}");
    assert!(e.scripting().module_id("big").is_none());
    e.load_package(&pack(&manifest("small", "1.0.0"), &one_page, &[])).unwrap();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant { memory_pages: 2, ..Grant::default() }));
    e.load_package(&pack(&manifest("big", "1.0.0"), &two_pages, &[])).unwrap();
}

#[cfg(not(target_family = "wasm"))]
mod files {
    use super::*;

    pub struct Dir(pub std::path::PathBuf);
    impl Dir {
        pub fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("efe_packages_{}_{tag}", std::process::id()));
            std::fs::remove_dir_all(&p).ok();
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// Write `bytes` to `path` with a modification time `step` minutes in the future.
    pub fn replace(path: &std::path::Path, bytes: &[u8], step: u64) {
        std::fs::write(path, bytes).unwrap();
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(60 * step);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn dir_source_loads_efmod_files_sorted_and_not_recursive() {
        let dir = Dir::new("dir");
        let mut m = manifest("b", "1.0.0");
        m.dependencies = vec![dep("a", "^1")];
        // `b` sorts before... nothing: `a.efmod` < `b.efmod`, but resolution does not rely on it
        std::fs::write(dir.0.join("z_a.efmod"), a_package("1.0.0")).unwrap();
        std::fs::write(dir.0.join("b.efmod"), pack(&m, &empty(), &[])).unwrap();
        std::fs::write(dir.0.join("notes.txt"), b"not a package").unwrap();
        std::fs::write(dir.0.join("broken.efmod"), b"garbage").unwrap();
        std::fs::create_dir_all(dir.0.join("sub")).unwrap();
        std::fs::write(dir.0.join("sub").join("c.efmod"), pack(&manifest("c", "1.0.0"), &empty(), &[])).unwrap();

        let (mut e, _) = engine();
        e.add_mod_source(DirSource::new(&dir.0));
        let report = e.load_mods();
        let loaded: Vec<&str> = report.loaded.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(loaded, ["z_a.efmod", "b.efmod"], "dependencies first");
        assert!(matches!(&report.failed[..], [(l, ModError::Package(_))] if l == "broken.efmod"), "{:?}", report.failed);
        assert!(e.scripting().module_id("a").is_some() && e.scripting().module_id("c").is_none());

        let (mut e2, _) = engine();
        e2.add_mod_source(DirSource::new(dir.0.join("missing")));
        let report = e2.load_mods();
        assert!(report.loaded.is_empty() && report.failed.is_empty());
    }

    /// `p` with `add(x) = x + k` as provide `bump_it` of mod `p`.
    fn prov(k: i64, doc: &str) -> Module {
        let mut mb = ModuleBuilder::new();
        let mut f = FunctionBuilder::new(4);
        f.const_i64(1, k);
        f.op3(Op::I64Add, 2, 0, 1);
        f.ret(2);
        let id = mb.add_func(FuncType::new([I64], [I64]), f);
        mb.interface(Interface {
            provides: vec![provide("inc", id, doc, vec![param("a", ApiType::I64)], vec![param("r", ApiType::I64)])],
            ..Default::default()
        });
        mb.finish().unwrap()
    }

    #[test]
    fn watch_package_reloads_and_refuses_an_id_change() {
        let dir = Dir::new("watch");
        let path = dir.0.join("p.efmod");
        let (mut e, _) = engine();
        let (policy, asked) = counting(&[]);
        e.set_mod_policy(policy);
        let pk = |id: &str, version: &str, k: i64, doc: &str| pack(&manifest(id, version), &prov(k, doc), &[("v", version.as_bytes())]);
        let v1 = pk("p", "1.0.0", 1, "Old doc.");
        std::fs::write(&path, &v1).unwrap();
        let id = e.load_package(&v1).unwrap();
        e.watch_package(id, &path).unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // a game module is not a package
        let g = e.load_module("plain", &empty().encode()).unwrap();
        assert!(e.watch_package(g, &path).is_err());

        replace(&path, &pk("p", "1.1.0", 10, "New doc."), 1);
        e.run_frames(1, 0.3);
        assert!(errors(&e).is_empty(), "{:?}", errors(&e));
        assert_eq!(e.package(id).unwrap().manifest.version.to_string(), "1.1.0");
        assert_eq!(e.package_asset(id, "v"), Some(&b"1.1.0"[..]));
        assert_eq!(e.api_index().function("p", "inc").unwrap().doc, "New doc.");
        assert_eq!(asked.load(Ordering::SeqCst), 2, "the policy is asked again");
        assert!(e.scripting().log_lines().any(|l| l.text.contains("[p] reloaded")));

        // a changed id is refused, the old version keeps running
        replace(&path, &pk("other", "9.0.0", 99, "Other."), 2);
        e.run_frames(1, 0.3);
        let errs = errors(&e);
        assert!(errs.iter().any(|t| t.contains("[p] reload failed") && t.contains("`other`")), "{errs:?}");
        assert_eq!(e.package(id).unwrap().manifest.version.to_string(), "1.1.0");
        assert_eq!(e.api_index().function("p", "inc").unwrap().doc, "New doc.");
        assert!(e.api_index().namespace("other").is_none());

        // a new version that the policy denies keeps the old one too
        e.set_mod_policy(|_: &ModRequest<'_>| Err("no more".to_string()));
        replace(&path, &pk("p", "1.2.0", 5, "Third."), 3);
        e.run_frames(1, 0.3);
        assert!(errors(&e).iter().any(|t| t.contains("no more")));
        assert_eq!(e.package(id).unwrap().manifest.version.to_string(), "1.1.0");
    }
}
