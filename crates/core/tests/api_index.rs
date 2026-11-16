//! E4.6 (engine half): the engine's host registry is the API index. Headless.

use std::collections::BTreeSet;

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

fn engine() -> Engine {
    Engine::with_config(EngineConfig { title: "index_game".into(), ..EngineConfig::headless() }).expect("headless engine")
}

/// A game component.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
struct Follow {
    /// The entity to follow.
    #[script(entity)]
    target: u64,
    speed: f32,
    _pad: u32,
}

#[test]
fn every_callable_function_has_an_index_entry_and_vice_versa() {
    let mut e = engine();
    // a game function and a game component too
    e.define_namespace("game", 2, "index_game", "Game functions.").unwrap();
    e.register_host_fn(
        HostFn::new("game", "score").doc("The score.").result("score", ApiType::I64),
        |_c, _a, r| {
            r.set_i64(0, 7)?;
            Ok(HostOk::Continue)
        },
    )
    .unwrap();
    e.register_component_owned::<Follow>("index_game").unwrap();

    let imports = e.scripting().registry().imports();
    let idx = e.api_index();
    let callable: Vec<(String, String, FuncType)> = imports.names().map(|(ns, n, t)| (ns.to_string(), n.to_string(), t.clone())).collect();
    assert!(callable.len() >= 27, "{}", callable.len());
    for (ns, name, ty) in &callable {
        let f = idx.function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is callable but not in the index"));
        assert!(!f.doc.trim().is_empty(), "{ns}.{name} has no doc");
        assert_eq!(&f.func_type(), ty, "{ns}.{name}");
        assert!(!f.owner.is_empty());
        assert_eq!(imports.namespace_version(ns), idx.namespace(ns).map(|n| n.version));
    }
    for f in &idx.functions {
        assert!(callable.iter().any(|(ns, n, _)| *ns == f.namespace && *n == f.name), "{} is in the index but not callable", f.qualified());
    }
    assert_eq!(imports.api_hash(), Some(idx.hash()));
    idx.validate().unwrap();
}

#[test]
fn engine_namespaces_and_functions_are_exactly_the_documented_ones() {
    let idx = engine().api_index();
    let names: BTreeSet<String> = idx.qualified_names().into_iter().collect();
    let expected = [
        "engine.world.spawn", "engine.world.despawn", "engine.world.alive", "engine.world.get", "engine.world.set",
        "engine.world.has", "engine.world.insert", "engine.world.remove", "engine.world.query", "engine.world.component_id",
        "engine.input.key_down", "engine.input.key_pressed", "engine.input.key_released", "engine.input.mouse_x",
        "engine.input.mouse_y", "engine.input.mouse_down",
        "engine.time.delta", "engine.time.elapsed", "engine.time.frame", "engine.time.fixed_delta",
        "engine.events.send", "engine.events.count", "engine.events.read",
        "engine.log.info", "engine.log.warn", "engine.log.error",
        "engine.mods.asset_len", "engine.mods.asset_read", "engine.mods.meta_f64",
        "engine.script.attach",
        "engine.random.f32", "engine.random.range", "engine.random.seed",
        "engine.math.sin", "engine.math.cos", "engine.math.atan2",
    ];
    assert_eq!(names, expected.iter().map(|s| s.to_string()).collect());
    for ns in &idx.namespaces {
        // `engine.mods` is version 2: `export` was removed, provides are declared in section 13, assets are read with `asset_len` / `asset_read`
        // `engine.events` is version 2: events are typed (ids of the module's events section, struct payloads)
        let version = if matches!(ns.name.as_str(), "engine.mods" | "engine.events") { 2 } else { 1 };
        assert_eq!((ns.version, ns.owner.as_str()), (version, "engine"), "{}", ns.name);
        assert!(!ns.doc.is_empty());
    }
    for f in &idx.functions {
        assert_eq!(f.owner, "engine");
        assert!(f.params.iter().chain(&f.results).all(|p| !p.name.is_empty()));
    }
    let get = idx.function("engine.world", "get").unwrap();
    assert_eq!(get.signature_text(), "(entity: entity, component: i32, out: ptr) -> (status: i32)");
    let q = idx.function("engine.world", "query").unwrap();
    assert_eq!(q.signature_text(), "(component: i32, out: ptr, cap: i64) -> (count: i64)");
    let r = idx.function("engine.events", "read").unwrap();
    assert_eq!(r.signature_text(), "(ev: i32, index: i32, out: ptr) -> (status: i32)");
    assert_eq!(idx.function("engine.events", "send").unwrap().signature_text(), "(ev: i32, payload: ptr)");
    assert_eq!(idx.function("engine.events", "count").unwrap().signature_text(), "(ev: i32) -> (count: i32)");
    // the engine registers no events of its own (yet)
    assert!(idx.events.is_empty());
}

#[test]
fn json_round_trip_keeps_the_hash() {
    let mut e = engine();
    e.register_component::<Follow>().unwrap();
    let idx = e.api_index();
    let text = e.export_api_index();
    let back = ApiIndex::from_json(&text).expect("the export parses");
    assert_eq!(back.hash(), idx.hash());
    assert_eq!(back, idx);
    assert_eq!(back.game, "index_game");
    e.set_game_name("renamed");
    assert_eq!(e.api_index().game, "renamed");
    assert_eq!(e.api_index().hash(), idx.hash(), "the game name is not hashed");
}

#[test]
fn every_registered_component_is_in_the_index_with_its_layout() {
    let mut e = engine();
    let id = e.register_component::<Follow>().unwrap();
    let mut desc = StructDesc::new("ModStat", 4, 4).field(FieldDesc::new("hp", 0, Prim::I32, 1));
    desc.owner = "somemod".into();
    e.register_struct(desc).unwrap();

    let idx = e.api_index();
    assert_eq!(e.types().entries().len(), 11); // 9 built-ins + Follow + ModStat
    for entry in e.types().entries() {
        let t = idx.struct_type(entry.name()).unwrap_or_else(|| panic!("{} is registered but not in the index", entry.name()));
        assert_eq!(t, entry.desc());
        if entry.is_component() {
            assert_eq!(t.component, Some(entry.id()), "{}", entry.name());
        } else {
            assert_eq!(t.component, None);
        }
    }
    for t in &idx.types {
        assert!(e.types().by_name(&t.name).is_some(), "{} is in the index but not registered", t.name);
    }
    // built-ins
    for (name, size) in [("Transform", size_of::<Transform>()), ("Velocity", size_of::<Velocity>()), ("MeshHandle", size_of::<MeshHandle>())] {
        let t = idx.struct_type(name).unwrap();
        assert_eq!((t.size as usize, t.owner.as_str()), (size, "engine"));
        assert_eq!(t.component, Some(e.types().by_name(name).unwrap().id()));
    }
    let tr = idx.struct_type("Transform").unwrap();
    assert_eq!(tr.fields.iter().map(|f| (f.name.as_str(), f.offset, f.ty, f.count)).collect::<Vec<_>>(), [
        ("position", 0, Prim::F32, 2),
        ("rotation", 8, Prim::F32, 1),
        ("scale", 12, Prim::F32, 2),
    ]);
    // a game component with docs, ids equal to the one engine.world.component_id returns
    let f = idx.struct_type("Follow").unwrap();
    assert_eq!((f.component, f.doc.as_str(), f.owner.as_str()), (Some(id), "A game component.", "engine"));
    assert_eq!(f.field_named("target").unwrap().ty, Prim::Entity);
    assert_eq!(idx.struct_type("ModStat").unwrap().owner, "somemod");
    // a duplicate does not leave a half-registered type behind
    assert!(e.register_component::<Follow>().is_err());
    assert_eq!(e.api_index().types.len(), 11);
}

#[test]
fn registered_structs_can_be_parameter_types() {
    let mut e = engine();
    let mut desc = StructDesc::new("Stat", 4, 4).field(FieldDesc::new("hp", 0, Prim::I32, 1));
    desc.owner = "index_game".into();
    e.register_struct(desc).unwrap();
    e.define_namespace("game", 1, "index_game", "Game functions.").unwrap();
    e.register_host_fn(
        HostFn::new("game", "heal").doc("Heal a stat.").param("stat", ApiType::Struct("Stat".into())),
        |_c, _a, _r| Ok(HostOk::Continue),
    )
    .unwrap();
    // an unregistered struct is refused: the type table is the only source of layouts
    assert!(e
        .register_host_fn(HostFn::new("game", "bad").doc("x").param("s", ApiType::Struct("Nope".into())), |_c, _a, _r| Ok(HostOk::Continue))
        .is_err());
    assert_eq!(e.api_index().function("game", "heal").unwrap().signature_text(), "(stat: &Stat)");
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn export_file_is_written_and_parses() {
    let dir = std::env::temp_dir().join(format!("efe_api_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = engine();
    e.define_namespace("game", 1, "index_game", "Game functions.").unwrap();
    e.register_host_fn(HostFn::new("game", "ping").doc("Ping."), |_c, _a, _r| Ok(HostOk::Continue)).unwrap();

    // the function `EFE_API_EXPORT` uses
    let direct = dir.join("direct.efapi");
    e.write_api_index(&direct).unwrap();
    let parsed = ApiIndex::from_json(&std::fs::read_to_string(&direct).unwrap()).unwrap();
    assert_eq!(parsed, e.api_index());
    assert!(parsed.function("game", "ping").is_some());

    // game_loop with an export path writes the index and returns without running a frame
    let looped = dir.join("looped.efapi");
    let frames = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let f = frames.clone();
    e.add_system(Stage::Update, move |_| f.set(f.get() + 1));
    e.set_api_export_path(Some(looped.clone()));
    e.game_loop();
    assert_eq!(frames.get(), 0);
    let parsed = ApiIndex::from_json(&std::fs::read_to_string(&looped).unwrap()).unwrap();
    assert_eq!(parsed.game, "index_game");
    assert!(parsed.function("game", "ping").is_some());
    assert!(parsed.function("engine.world", "get").is_some());
    std::fs::remove_dir_all(&dir).ok();
}

/// A mod `inv` providing `len(s: str) -> n: i64` and `put(stat: &Stat)`, with an own struct.
fn inv_mod() -> Module {
    let mut mb = ModuleBuilder::new();
    mb.memory(1, None);
    let mut f = FunctionBuilder::new(4);
    f.ret(1);
    let len = mb.add_func(FuncType::new([ValType::I64, ValType::I64], [ValType::I64]), f);
    let mut f = FunctionBuilder::new(4);
    f.ret(0);
    let put = mb.add_func(FuncType::new([ValType::I64], []), f);
    mb.interface(Interface {
        scratch: Some((256, 128)),
        structs: vec![StructDesc::new("InvStat", 4, 4).field(FieldDesc::new("hp", 0, Prim::I32, 1)).doc("A stat.")],
        provides: vec![
            Provide {
                name: "len".into(),
                func: len,
                doc: "Length of a string.".into(),
                params: vec![Param::new("s", ApiType::Str)],
                results: vec![Param::new("n", ApiType::I64)],
            },
            Provide {
                name: "put".into(),
                func: put,
                doc: "Store a stat.".into(),
                params: vec![Param::new("stat", ApiType::Struct("InvStat".into()))],
                results: vec![],
            },
        ],
        ..Default::default()
    });
    mb.finish().unwrap()
}

#[test]
fn a_loaded_mods_provides_are_callable_and_in_the_index_and_unload_restores_it() {
    let mut e = engine();
    let before = e.api_index();
    let id = e.load_mod(&inv_mod().encode(), ModOptions::new("inv").with_version(2)).unwrap();

    let imports = e.scripting().registry().imports();
    let idx = e.api_index();
    idx.validate().unwrap();
    // every callable function is in the index with a doc, and the other way round
    let callable: Vec<(String, String, FuncType)> = imports.names().map(|(ns, n, t)| (ns.to_string(), n.to_string(), t.clone())).collect();
    for (ns, name, ty) in &callable {
        let f = idx.function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is callable but not in the index"));
        assert!(!f.doc.trim().is_empty(), "{ns}.{name} has no doc");
        assert_eq!(&f.func_type(), ty);
    }
    for f in &idx.functions {
        assert!(callable.iter().any(|(ns, n, _)| *ns == f.namespace && *n == f.name), "{} is in the index but not callable", f.qualified());
    }
    assert_eq!(imports.api_hash(), Some(idx.hash()));
    // the mod's namespace, functions and struct, with owner and doc
    let ns = idx.namespace("inv").unwrap();
    assert_eq!((ns.version, ns.owner.as_str()), (2, "inv"));
    assert_eq!(idx.function("inv", "len").unwrap().signature_text(), "(s: str) -> (n: i64)");
    assert_eq!(idx.function("inv", "put").unwrap().signature_text(), "(stat: &InvStat)");
    assert!(idx.functions.iter().filter(|f| f.namespace == "inv").all(|f| f.owner == "inv" && !f.doc.is_empty()));
    assert_eq!(idx.struct_type("InvStat").unwrap().owner, "inv");
    // it matches the index the module derives from its own interface
    let own = inv_mod().data().interface.as_ref().unwrap().index("inv", 2, "Functions provided by the mod `inv`.");
    assert_eq!(own.functions, idx.functions.iter().filter(|f| f.namespace == "inv").cloned().collect::<Vec<_>>());
    assert_eq!(own.types, idx.types.iter().filter(|t| t.owner == "inv").cloned().collect::<Vec<_>>());

    e.unload_module(id).unwrap();
    assert_eq!(e.api_index(), before, "unload restores the index");
    assert_eq!(e.api_index().hash(), before.hash());
}

/// E8.6: a loaded package's provides and own structs are in the index with owner `<id>`, every
/// callable function is documented and listed, every listed function is callable, and unload
/// leaves the index as it was.
#[test]
fn a_loaded_package_is_in_the_index_and_unload_restores_it() {
    use efeengine_core::efemod::{Manifest, Package};
    let mut e = engine();
    let before = e.api_index();

    let mut mb = ModuleBuilder::new();
    let mut f = FunctionBuilder::new(4);
    f.load(Op::I32Load, 1, 0, 0);
    f.ret(1);
    let id = mb.add_func(FuncType::new([ValType::I64], [ValType::I32]), f);
    mb.memory(1, None);
    mb.interface(Interface {
        scratch: Some((1024, 1024)),
        structs: vec![StructDesc::new("Pack", 4, 4).field(FieldDesc::new("n", 0, Prim::I32, 1)).doc("A pack.")],
        provides: vec![Provide {
            name: "first".into(),
            func: id,
            doc: "The first field.".into(),
            params: vec![Param::new("p", ApiType::Struct("Pack".into()))],
            results: vec![Param::new("n", ApiType::I32)],
        }],
        ..Default::default()
    });
    let manifest = Manifest::new("pk", "1.0.0".parse().unwrap());
    let bytes = Package::write(&manifest, &mb.finish().unwrap().encode(), &Default::default()).unwrap();
    let mid = e.load_package(&bytes).unwrap();

    let idx = e.api_index();
    idx.validate().unwrap();
    assert_eq!(idx.function("pk", "first").unwrap().owner, "pk");
    assert_eq!(idx.struct_type("Pack").unwrap().owner, "pk");
    assert_eq!(idx.namespace("pk").unwrap().owner, "pk");
    let imports = e.scripting().registry().imports();
    let callable: Vec<(String, String)> = imports.names().map(|(ns, n, _)| (ns.to_string(), n.to_string())).collect();
    for (ns, n) in &callable {
        assert!(!idx.function(ns, n).unwrap().doc.trim().is_empty(), "{ns}.{n}");
    }
    for f in &idx.functions {
        assert!(callable.iter().any(|(ns, n)| *ns == f.namespace && *n == f.name), "{} is listed but not callable", f.qualified());
    }

    e.unload_module(mid).unwrap();
    assert_eq!(e.api_index(), before);
}

/// Every registered event is in the index with a doc and the other way round (E8.13), also
/// after a package that provides an event is loaded and unloaded.
#[test]
fn every_registered_event_is_in_the_index_with_a_doc_and_vice_versa() {
    use efeengine_core::efemod::{Manifest, Package};
    let mut e = engine();
    e.define_namespace("game", 1, "index_game", "Game events.").unwrap();
    let mut desc = StructDesc::new("Hit", 4, 4).field(FieldDesc::new("dmg", 0, Prim::I32, 1)).doc("A hit.");
    desc.owner = "index_game".into();
    e.register_struct(desc).unwrap();
    e.register_event(EventDesc::new("game", "hit").payload("Hit").doc("Something was hit.")).unwrap();
    e.register_event(EventDesc::new("game", "over").doc("The game is over.")).unwrap();
    let before = e.api_index();

    // the registry's own table, as the linker sees it, equals the index
    let check = |e: &Engine, expected: &[&str]| {
        let imports = e.scripting().registry().imports();
        let idx = e.api_index();
        idx.validate().unwrap();
        let registered: Vec<(String, String, Option<(String, u32)>)> =
            imports.events().map(|(ns, n, p)| (ns.to_string(), n.to_string(), p.map(|(s, z)| (s.to_string(), z)))).collect();
        for (ns, n, payload) in &registered {
            let ev = idx.event(ns, n).unwrap_or_else(|| panic!("{ns}.{n} is registered but not in the index"));
            assert!(!ev.doc.trim().is_empty(), "{ns}.{n} has no doc");
            assert!(!ev.owner.is_empty());
            assert_eq!(ev.payload.as_deref(), payload.as_ref().map(|(s, _)| s.as_str()));
            if let Some((s, size)) = payload {
                assert_eq!(idx.struct_type(s).map(|t| t.size), Some(*size));
            }
        }
        for ev in &idx.events {
            assert!(registered.iter().any(|(ns, n, _)| *ns == ev.namespace && *n == ev.name), "{} is in the index but not registered", ev.qualified());
            assert!(e.scripting().registry().event(&ev.namespace, &ev.name).is_some());
        }
        let names: Vec<String> = idx.events.iter().map(|ev| ev.qualified()).collect();
        assert_eq!(names, expected);
        assert_eq!(imports.api_hash(), Some(idx.hash()));
    };
    check(&e, &["game.hit", "game.over"]);

    let mut mb = ModuleBuilder::new();
    mb.interface(Interface {
        structs: vec![StructDesc::new("Pack", 4, 4).field(FieldDesc::new("n", 0, Prim::I32, 1)).doc("A pack.")],
        events: vec![ProvidedEvent { name: "opened".into(), payload: Some("Pack".into()), doc: "The pack was opened.".into() }],
        ..Default::default()
    });
    let manifest = Manifest::new("pk", "1.0.0".parse().unwrap());
    let bytes = Package::write(&manifest, &mb.finish().unwrap().encode(), &Default::default()).unwrap();
    let id = e.load_package(&bytes).unwrap();
    check(&e, &["game.hit", "game.over", "pk.opened"]);
    let ev = e.api_index().event("pk", "opened").unwrap().clone();
    assert_eq!((ev.owner.as_str(), ev.payload.as_deref()), ("pk", Some("Pack")));

    e.unload_module(id).unwrap();
    check(&e, &["game.hit", "game.over"]);
    assert_eq!(e.api_index(), before);
}
