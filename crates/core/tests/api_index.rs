//! E4.6 (engine half): the engine's host registry is the API index. Headless.

use std::collections::BTreeSet;

use efeengine_core::efevm::*;
use efeengine_core::prelude::*;

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
    assert!(callable.len() >= 28, "{}", callable.len());
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
        "engine.mods.export",
    ];
    assert_eq!(names, expected.iter().map(|s| s.to_string()).collect());
    for ns in &idx.namespaces {
        assert_eq!((ns.version, ns.owner.as_str()), (1, "engine"), "{}", ns.name);
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
    assert_eq!(r.signature_text(), "(name: str, index: i64, buf: bytes) -> (len: i64)");
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
    assert_eq!(e.types().entries().len(), 5);
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
    assert_eq!(e.api_index().types.len(), 5);
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
