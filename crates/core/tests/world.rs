//! World, Commands and type registry tests (ROADMAP 3.1-3.4). Headless: no SDL or GPU.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use efeengine_core::efevm::Prim;
use efeengine_core::prelude::*;
use efeengine_core::ecs::{EngineState};
use efeengine_core::ecs::{CompError, TypeError, entity_from_bits, entity_to_bits};

fn engine() -> Engine {
    Engine::with_config(EngineConfig::headless()).expect("headless engine")
}

#[test]
fn spawn_in_startup_is_visible_in_the_next_stage() {
    let mut e = engine();
    let spawned = Rc::new(Cell::new(None::<Entity>));
    let seen = Rc::new(Cell::new(0usize));
    let (s1, s2, seen2) = (spawned.clone(), spawned.clone(), seen.clone());
    e.add_system(Stage::Startup, move |cx| {
        let ent = cx.spawn((Transform::default(), MeshHandle { id: 7 }));
        s1.set(Some(ent));
        // Reserved but not yet applied: no components in this stage.
        assert!(cx.world.get::<&Transform>(ent).is_err());
    });
    e.add_system(Stage::PreUpdate, move |cx| {
        let ent = s2.get().unwrap();
        assert!(cx.world.contains(ent));
        assert_eq!(cx.world.get::<&MeshHandle>(ent).unwrap().id, 7);
        seen2.set(cx.world.query::<(&Transform, &MeshHandle)>().iter().count());
    });
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(seen.get(), 1);
    assert_eq!(e.world().len(), 1);
}

#[test]
fn insert_remove_despawn_through_commands() {
    let mut e = engine();
    let ent = e.world_mut().spawn((Transform::default(),));
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let l = log.clone();
    let frame = Rc::new(Cell::new(0));
    let f = frame.clone();
    e.add_system(Stage::Update, move |cx| {
        match f.get() {
            0 => {
                cx.commands.insert(ent, Velocity { linear: Vec2::X, angular: 0.0 });
                cx.commands.insert(ent, Name::new("player"));
                assert!(cx.world.get::<&Velocity>(ent).is_err(), "not applied yet");
            }
            1 => {
                assert_eq!(cx.world.get::<&Velocity>(ent).unwrap().linear, Vec2::X);
                assert_eq!(cx.world.get::<&Name>(ent).unwrap().0, "player");
                cx.commands.remove::<Velocity>(ent);
                // removing something that is not there is ignored
                cx.commands.remove::<MeshHandle>(ent);
            }
            2 => {
                assert!(cx.world.get::<&Velocity>(ent).is_err());
                assert!(cx.world.get::<&Transform>(ent).is_ok());
                cx.commands.despawn(ent);
                // ops after the despawn (same buffer) hit a dead entity: ignored
                cx.commands.insert(ent, Velocity::default());
                cx.commands.despawn(ent);
            }
            _ => {
                assert!(!cx.world.contains(ent));
                l.borrow_mut().push("done");
            }
        }
        f.set(f.get() + 1);
    });
    e.run_frames(4, 1.0 / 60.0);
    assert_eq!(*log.borrow(), ["done"]);
}

#[test]
fn despawn_inside_a_query_is_safe() {
    let mut e = engine();
    for i in 0..10 {
        e.world_mut().spawn((Transform::from_position(Vec2::new(i as f32, 0.0)), MeshHandle { id: i }));
    }
    e.add_system(Stage::Update, |cx| {
        for (ent, m) in cx.world.query::<(Entity, &MeshHandle)>().iter() {
            if m.id % 2 == 0 {
                cx.commands.despawn(ent);
            }
        }
    });
    e.run_frames(1, 1.0 / 60.0);
    let mut left: Vec<u32> = e.world().query::<&MeshHandle>().iter().map(|m| m.id).collect();
    left.sort();
    assert_eq!(left, [1, 3, 5, 7, 9]);
}

#[test]
fn commands_on_dead_and_stale_entities_are_ignored() {
    let mut e = engine();
    let ent = e.world_mut().spawn((Transform::default(),));
    e.world_mut().despawn(ent).unwrap();
    let reused = e.world_mut().spawn((Transform::default(),)); // same id, new generation
    let tid = e.types().of::<Transform>().unwrap().id();
    e.add_system(Stage::Update, move |cx| {
        cx.commands.insert(ent, Velocity::default());
        cx.commands.remove::<Transform>(ent);
        cx.commands.insert_raw(ent, tid, vec![0; size_of::<Transform>()]);
        cx.commands.remove_raw(ent, tid);
        cx.commands.despawn(ent);
        // unknown id and wrong size on a live entity: logged, skipped
        cx.commands.insert_raw(reused, 0xdead_beef, vec![1, 2, 3]);
        cx.commands.insert_raw(reused, tid, vec![1, 2, 3]);
    });
    e.run_frames(1, 1.0 / 60.0);
    assert!(e.world().contains(reused));
    assert_eq!(*e.world().get::<&Transform>(reused).unwrap(), Transform::default());
    assert!(e.world().get::<&Velocity>(reused).is_err());
}

#[test]
fn raw_insert_and_remove_through_commands() {
    let mut e = engine();
    let vid = e.types().by_name("Velocity").unwrap().id();
    let v = Velocity { linear: Vec2::new(1.0, -2.0), angular: 0.5 };
    let bytes = bytemuck::bytes_of(&v).to_vec();
    let spawned = Rc::new(Cell::new(None::<Entity>));
    let (s, frame) = (spawned.clone(), Rc::new(Cell::new(0)));
    e.add_system(Stage::Update, move |cx| {
        match frame.get() {
            0 => {
                let ent = cx.commands.spawn_empty(cx.world);
                cx.commands.insert_raw(ent, vid, bytes.clone());
                s.set(Some(ent));
            }
            1 => cx.commands.remove_raw(s.get().unwrap(), vid),
            _ => {}
        }
        frame.set(frame.get() + 1);
    });
    e.run_frames(1, 1.0 / 60.0);
    let ent = spawned.get().unwrap();
    assert_eq!(*e.world().get::<&Velocity>(ent).unwrap(), v);
    e.run_frames(1, 1.0 / 60.0);
    assert!(e.world().contains(ent));
    assert!(e.world().get::<&Velocity>(ent).is_err());
}

#[test]
fn appended_commands_apply_in_order() {
    // The script system's pattern: fill its own buffer, append it to the stage's.
    let mut world = World::new();
    // The registry only fills through the engine (one path: TypeRegistry + API index).
    let types = engine().types().clone();
    let mut stage = Commands::new();
    let mut other = Commands::new();
    let a = stage.spawn(&world, (MeshHandle { id: 1 },));
    let b = other.spawn(&world, (MeshHandle { id: 2 },));
    other.despawn(a);
    other.quit();
    stage.append(&mut other);
    assert!(other.is_empty());
    assert_eq!(stage.world_ops(), 3);
    stage.apply(&mut world, &types);
    assert_eq!(stage.world_ops(), 0);
    assert!(!stage.is_empty(), "control requests stay for the engine");
    assert!(!world.contains(a));
    assert_eq!(world.get::<&MeshHandle>(b).unwrap().id, 2);
}

#[test]
fn hecs_reserved_entities() {
    // How hecs 0.11 treats reserved entities, which `Commands::spawn` relies on.
    let mut world = World::new();
    let r = world.reserve_entity();
    // A reserved entity already counts as contained, but has no location yet.
    assert!(world.contains(r));
    assert!(world.get::<&Transform>(r).is_err());
    // insert flushes pending reservations first, so inserting into a reserved entity works.
    world.insert(r, (Transform::default(),)).unwrap();
    assert!(world.get::<&Transform>(r).is_ok());

    // A reservation that is never used comes to life empty at the next flush.
    let r2 = world.reserve_entity();
    world.flush();
    assert!(world.contains(r2));
    assert_eq!(world.entity(r2).unwrap().len(), 0);
    // Despawning a reserved (unflushed) entity works too.
    let r3 = world.reserve_entity();
    world.despawn(r3).unwrap();
    assert!(!world.contains(r3));
}

#[test]
fn registry_has_builtin_components_with_valid_layouts() {
    let e = engine();
    let t = e.types();
    for (name, size) in [("Transform", size_of::<Transform>()), ("Velocity", size_of::<Velocity>()), ("MeshHandle", size_of::<MeshHandle>())] {
        let entry = t.by_name(name).unwrap_or_else(|| panic!("{name} registered"));
        assert_eq!(entry.id(), efeengine_core::efevm::api::fnv1a32(name.as_bytes()));
        assert_eq!(entry.size() as usize, size);
        assert_eq!(entry.desc().component, Some(entry.id()));
        assert_eq!(entry.desc().owner, "engine");
        assert!(entry.is_component());
        entry.desc().validate().unwrap();
        assert!(std::ptr::eq(t.get(entry.id()).unwrap(), entry));
    }
    let tr = t.by_name("Transform").unwrap().desc();
    assert_eq!(tr.field_named("scale").unwrap().offset as usize, std::mem::offset_of!(Transform, scale));
    assert_eq!(tr.field_named("scale").unwrap().ty, Prim::F32);
    assert_eq!(t.entries().len(), 10);
}

#[test]
fn byte_read_write_insert_round_trip() {
    let mut e = engine();
    let ent = e.world_mut().spawn((Transform::default(),));
    let entry_id = e.types().of::<Transform>().unwrap().id();
    let EngineState { world, types, .. } = &mut e.state;
    let tr = types.get(entry_id).unwrap();

    let mut buf = vec![0u8; tr.size() as usize];
    tr.read(world, ent, &mut buf).unwrap();
    assert_eq!(bytemuck::pod_read_unaligned::<Transform>(&buf), Transform::default());

    let t2 = Transform { position: Vec2::new(3.0, 4.0), rotation: 1.5, scale: Vec2::splat(2.0) };
    tr.write(world, ent, bytemuck::bytes_of(&t2)).unwrap();
    assert_eq!(*world.get::<&Transform>(ent).unwrap(), t2);
    tr.read(world, ent, &mut buf).unwrap();
    assert_eq!(buf, bytemuck::bytes_of(&t2));

    // unaligned source bytes are fine
    let mut unaligned = vec![0u8; buf.len() + 1];
    unaligned[1..].copy_from_slice(&buf);
    tr.write(world, ent, &unaligned[1..]).unwrap();

    // insert onto an entity without the component; write needs it to exist
    let vel = types.of::<Velocity>().unwrap();
    let v = Velocity { linear: Vec2::Y, angular: -1.0 };
    assert!(!vel.has(world, ent));
    assert_eq!(vel.write(world, ent, bytemuck::bytes_of(&v)), Err(CompError::Missing("Velocity".into())));
    let mut vbuf = [0u8; 12];
    assert_eq!(vel.read(world, ent, &mut vbuf), Err(CompError::Missing("Velocity".into())));
    vel.insert(world, ent, bytemuck::bytes_of(&v)).unwrap();
    assert!(vel.has(world, ent));
    assert_eq!(*world.get::<&Velocity>(ent).unwrap(), v);
    vel.remove(world, ent).unwrap();
    assert!(!vel.has(world, ent));
    assert_eq!(vel.remove(world, ent), Err(CompError::Missing("Velocity".into())));
}

#[test]
fn wrong_length_and_stale_entity_are_errors_not_panics() {
    let mut e = engine();
    let ent = e.world_mut().spawn((Transform::default(),));
    let EngineState { world, types, .. } = &mut e.state;
    let tr = types.of::<Transform>().unwrap();
    let n = tr.size() as usize;
    for len in [0, n - 1, n + 1] {
        let bytes = vec![0u8; len];
        let want = CompError::WrongSize { name: "Transform".into(), expected: n as u32, got: len };
        assert_eq!(tr.write(world, ent, &bytes), Err(want.clone()));
        assert_eq!(tr.insert(world, ent, &bytes), Err(want.clone()));
        let mut out = vec![0u8; len];
        assert_eq!(tr.read(world, ent, &mut out), Err(want));
    }

    let bits = entity_to_bits(ent);
    world.despawn(ent).unwrap();
    let stale = entity_from_bits(bits).expect("well-formed bits");
    let mut out = vec![0u8; n];
    assert_eq!(tr.read(world, stale, &mut out), Err(CompError::NoEntity));
    assert_eq!(tr.write(world, stale, &out), Err(CompError::NoEntity));
    assert_eq!(tr.insert(world, stale, &out), Err(CompError::NoEntity));
    assert_eq!(tr.remove(world, stale), Err(CompError::NoEntity));
    assert!(!tr.has(world, stale));
    assert_eq!(entity_from_bits(0), None);
}

#[test]
fn engine_register_component_and_struct() {
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
    let mut e = engine();
    let id = e.register_component::<Follow>().unwrap();
    let entry = e.types().get(id).unwrap();
    assert_eq!(entry.name(), "Follow");
    assert_eq!(entry.desc().doc, "A game component.");
    let target = entry.desc().field_named("target").unwrap();
    assert_eq!((target.ty, target.doc.as_str()), (Prim::Entity, "The entity to follow."));
    assert!(matches!(e.register_component::<Follow>(), Err(TypeError::DuplicateType(_))));

    let desc = efeengine_core::efevm::StructDesc::new("Follow", 4, 4);
    assert!(matches!(e.register_struct(desc), Err(TypeError::DuplicateName(_))));
    let mut desc = efeengine_core::efevm::StructDesc::new("ModStat", 4, 4)
        .field(efeengine_core::efevm::FieldDesc::new("hp", 0, Prim::I32, 1));
    desc.owner = "mymod".into();
    let sid = e.register_struct(desc).unwrap();
    assert_eq!(e.types().get(sid).unwrap().desc().owner, "mymod");
    assert!(!e.types().get(sid).unwrap().is_component());
}
