//! Runtime-registered (dynamic) components

use hecs::*;

fn info(size: usize, align: usize) -> DynamicInfo {
    DynamicInfo { size, align }
}

fn t(n: u32) -> Tick {
    Tick::new(n)
}

#[test]
fn register_validates_layout() {
    let mut world = World::new();
    for bad in [
        info(4, 0),
        info(4, 3),
        info(6, 4),
        info(8, 8192),
        info(1 << 40, 1),
    ] {
        assert_eq!(
            world.register_dynamic(1, bad, false),
            Err(DynamicError::BadLayout)
        );
    }
    assert!(world.dynamic_info(1).is_none());
    let id = world.register_dynamic(1, info(8, 4), false).unwrap();
    assert_eq!(
        world.register_dynamic(1, info(8, 4), false),
        Err(DynamicError::AlreadyRegistered(1))
    );
    assert_eq!(world.dynamic_info(1), Some((id, info(8, 4))));
    assert!(matches!(
        id,
        ComponentId::Dynamic(DynamicId {
            id: 1,
            generation: _
        })
    ));
    // zero-sized and maximal alignment are fine
    world.register_dynamic(2, info(0, 1), false).unwrap();
    world.register_dynamic(3, info(4096, 4096), false).unwrap();
}

#[test]
fn insert_get_remove_replace() {
    let mut world = World::new();
    world.register_dynamic(7, info(4, 4), false).unwrap();
    let e = world.spawn(());
    let f = world.spawn(());
    assert!(!world.has_dynamic(e, 7));
    assert_eq!(world.get_dynamic(e, 7).err(), Some(DynamicError::Missing));

    world.insert_dynamic(e, 7, &[1, 2, 3, 4]).unwrap();
    world.insert_dynamic(f, 7, &[9, 9, 9, 9]).unwrap();
    assert!(world.has_dynamic(e, 7));
    assert_eq!(&*world.get_dynamic(e, 7).unwrap(), &[1, 2, 3, 4]);
    assert_eq!(&*world.get_dynamic(f, 7).unwrap(), &[9, 9, 9, 9]);

    // replace in place
    world.insert_dynamic(e, 7, &[5, 6, 7, 8]).unwrap();
    assert_eq!(&*world.get_dynamic(e, 7).unwrap(), &[5, 6, 7, 8]);

    // mutate through the guard
    {
        let mut m = world.get_dynamic_mut(e, 7).unwrap();
        m[0] = 100;
        assert_eq!(m.len(), 4);
    }
    assert_eq!(world.get_dynamic(e, 7).unwrap()[0], 100);

    // errors
    assert_eq!(
        world.insert_dynamic(e, 7, &[1, 2, 3]),
        Err(DynamicError::WrongSize {
            expected: 4,
            got: 3
        })
    );
    assert_eq!(
        world.insert_dynamic(e, 8, &[]),
        Err(DynamicError::Unregistered(8))
    );
    assert_eq!(
        world.get_dynamic(e, 8).err(),
        Some(DynamicError::Unregistered(8))
    );
    assert_eq!(
        world.remove_dynamic(e, 8),
        Err(DynamicError::Unregistered(8))
    );

    world.remove_dynamic(e, 7).unwrap();
    assert!(!world.has_dynamic(e, 7));
    assert_eq!(world.remove_dynamic(e, 7), Err(DynamicError::Missing));
    // f is unaffected (it was swap-moved)
    assert_eq!(&*world.get_dynamic(f, 7).unwrap(), &[9, 9, 9, 9]);

    world.despawn(e).unwrap();
    assert_eq!(
        world.insert_dynamic(e, 7, &[0; 4]),
        Err(DynamicError::NoSuchEntity)
    );
    assert_eq!(
        world.get_dynamic(e, 7).err(),
        Some(DynamicError::NoSuchEntity)
    );
    assert_eq!(
        world.remove_dynamic(e, 7),
        Err(DynamicError::NoSuchEntity)
    );
    assert!(!world.has_dynamic(e, 7));
}

#[test]
fn bytes_may_be_unaligned_input_and_output_is_aligned() {
    let mut world = World::new();
    world.register_dynamic(1, info(32, 16), false).unwrap();
    // an odd-offset source slice
    let backing = [0u8; 64];
    let mut src = backing;
    for (i, b) in src.iter_mut().enumerate() {
        *b = i as u8;
    }
    let mut es = Vec::new();
    for i in 0..130 {
        // enough to force several reallocations of the column
        let e = world.spawn((i as u64,));
        world.insert_dynamic(e, 1, &src[1..33]).unwrap();
        es.push(e);
    }
    for (i, &e) in es.iter().enumerate() {
        let r = world.get_dynamic(e, 1).unwrap();
        assert_eq!(r.as_ptr() as usize % 16, 0, "row {i} not 16-byte aligned");
        assert_eq!(&*r, &src[1..33]);
    }
    // grow again through typed inserts that move the entities
    for &e in &es {
        world.insert_one(e, 1u8).unwrap();
    }
    for &e in &es {
        let r = world.get_dynamic(e, 1).unwrap();
        assert_eq!(r.as_ptr() as usize % 16, 0);
        assert_eq!(&*r, &src[1..33]);
    }
    // whole column view
    let mut seen = 0;
    for arch in world.archetypes() {
        if let Some((id, _)) = world.dynamic_info(1) {
            if let Some(col) = arch.dynamic_column(id) {
                assert_eq!(col.len(), arch.len() as usize * 32);
                assert_eq!(col.as_ptr() as usize % 16, 0);
                for row in col.chunks(32) {
                    assert_eq!(row, &src[1..33]);
                    seen += 1;
                }
            }
        }
    }
    assert_eq!(seen, 130);
    // typed ids are not exposed as bytes
    for arch in world.archetypes() {
        assert!(arch.dynamic_column(ComponentId::of::<u64>()).is_none());
    }
}

#[test]
fn big_alignment() {
    let mut world = World::new();
    world.register_dynamic(1, info(4096, 4096), false).unwrap();
    let a = world.spawn(());
    let b = world.spawn(());
    let mut bytes = vec![0u8; 4096];
    bytes[0] = 1;
    bytes[4095] = 2;
    world.insert_dynamic(a, 1, &bytes).unwrap();
    bytes[0] = 3;
    world.insert_dynamic(b, 1, &bytes).unwrap();
    for (e, first) in [(a, 1u8), (b, 3u8)] {
        let r = world.get_dynamic(e, 1).unwrap();
        assert_eq!(r.as_ptr() as usize % 4096, 0);
        assert_eq!((r[0], r[4095]), (first, 2));
    }
}

#[test]
fn zero_sized_dynamic() {
    let mut world = World::new();
    world.register_dynamic(1, info(0, 16), true).unwrap();
    let es = (0..100)
        .map(|i| {
            let e = world.spawn((i,));
            world.insert_dynamic(e, 1, &[]).unwrap();
            e
        })
        .collect::<Vec<_>>();
    for &e in &es {
        assert!(world.has_dynamic(e, 1));
        let r = world.get_dynamic(e, 1).unwrap();
        assert_eq!(r.len(), 0);
        assert_eq!(r.as_ptr() as usize % 16, 0);
    }
    assert_eq!(world.query_dynamic(&[1]).count(), 100);
    world.remove_dynamic(es[50], 1).unwrap();
    assert_eq!(world.query_dynamic(&[1]).count(), 99);
    for &e in &es {
        world.despawn(e).unwrap();
    }
    assert_eq!(world.len(), 0);
}

#[test]
fn mixes_with_rust_components() {
    use std::sync::Arc;
    let mut world = World::new();
    world.register_dynamic(5, info(8, 8), false).unwrap();
    let marker = Arc::new(());
    let e = world.spawn((1i32, String::from("hi"), marker.clone()));
    let other = world.spawn((2i32,));
    world.insert_dynamic(e, 5, &42u64.to_le_bytes()).unwrap();
    assert_eq!(*world.get::<&i32>(e).unwrap(), 1);
    assert_eq!(&*world.get::<&String>(e).unwrap(), "hi");
    assert_eq!(world.get_dynamic(e, 5).unwrap()[..], 42u64.to_le_bytes());

    // typed insert/remove moves keep the dynamic bytes
    world.insert_one(e, 3.5f32).unwrap();
    world.remove_one::<String>(e).unwrap();
    assert_eq!(world.get_dynamic(e, 5).unwrap()[..], 42u64.to_le_bytes());
    assert_eq!(*world.get::<&f32>(e).unwrap(), 3.5);

    // typed queries only see typed components; the entity still matches
    let mut seen = world
        .query::<(Entity, &i32)>()
        .iter()
        .map(|(e, &i)| (e, i))
        .collect::<Vec<_>>();
    seen.sort();
    assert_eq!(seen.len(), 2);
    assert!(world.get::<&u64>(e).is_err());

    // entity_ref reports dynamic component ids
    let ids = world.entity(e).unwrap().component_types().collect::<Vec<_>>();
    let (dyn_id, _) = world.dynamic_info(5).unwrap();
    assert!(ids.contains(&dyn_id));
    assert!(ids.contains(&ComponentId::of::<i32>()));
    assert!(world.entity(e).unwrap().has_component(dyn_id));
    assert!(!world.entity(other).unwrap().has_component(dyn_id));
    assert!(world.archetypes().any(|a| a.has_component(dyn_id)));
    assert!(world
        .archetypes()
        .any(|a| a.component_types().any(|c| c == dyn_id)));

    // despawn drops the Rust components
    assert_eq!(Arc::strong_count(&marker), 2);
    world.despawn(e).unwrap();
    assert_eq!(Arc::strong_count(&marker), 1);
}

#[test]
fn swap_remove_keeps_dynamic_bytes() {
    let mut world = World::new();
    world.register_dynamic(1, info(2, 2), false).unwrap();
    let es = (0..3u8)
        .map(|i| {
            let e = world.spawn((i,));
            world.insert_dynamic(e, 1, &[i, i + 10]).unwrap();
            e
        })
        .collect::<Vec<_>>();
    world.despawn(es[0]).unwrap();
    assert_eq!(&*world.get_dynamic(es[1], 1).unwrap(), &[1, 11]);
    assert_eq!(&*world.get_dynamic(es[2], 1).unwrap(), &[2, 12]);
    assert_eq!(*world.get::<&u8>(es[2]).unwrap(), 2);
}

#[test]
fn query_dynamic_is_ordered_and_deterministic() {
    fn build() -> (World, Vec<Entity>) {
        let mut world = World::new();
        world.register_dynamic(1, info(4, 4), false).unwrap();
        world.register_dynamic(2, info(1, 1), false).unwrap();
        let mut es = Vec::new();
        for i in 0..6u32 {
            let e = world.spawn((i,));
            es.push(e);
        }
        // Archetype A: {u32, dyn1}; archetype B: {u32, dyn1, dyn2}; one entity only has dyn2
        for &e in &es[0..2] {
            world.insert_dynamic(e, 1, &[0; 4]).unwrap();
        }
        for &e in &es[2..4] {
            world.insert_dynamic(e, 1, &[0; 4]).unwrap();
            world.insert_dynamic(e, 2, &[0]).unwrap();
        }
        world.insert_dynamic(es[4], 2, &[0]).unwrap();
        (world, es)
    }
    let (world, es) = build();
    let q1 = world.query_dynamic(&[1]).collect::<Vec<_>>();
    assert_eq!(q1, vec![es[0], es[1], es[2], es[3]]);
    let q12 = world.query_dynamic(&[1, 2]).collect::<Vec<_>>();
    assert_eq!(q12, vec![es[2], es[3]]);
    let q21 = world.query_dynamic(&[2, 1]).collect::<Vec<_>>();
    assert_eq!(q21, q12);
    assert_eq!(world.query_dynamic(&[2]).count(), 3);
    assert_eq!(world.query_dynamic(&[99]).count(), 0);
    assert_eq!(world.query_dynamic(&[1, 99]).count(), 0);
    // identical op sequences give identical results
    let (world2, es2) = build();
    assert_eq!(es, es2);
    assert_eq!(
        world2.query_dynamic(&[2]).collect::<Vec<_>>(),
        world.query_dynamic(&[2]).collect::<Vec<_>>()
    );
}

#[test]
fn unregister_strips_and_reregister_gets_new_layout() {
    let mut world = World::new();
    let old_id = world.register_dynamic(9, info(4, 4), false).unwrap();
    let mut es = Vec::new();
    for i in 0..5u32 {
        let e = world.spawn((i, i as u8));
        world.insert_dynamic(e, 9, &i.to_le_bytes()).unwrap();
        es.push(e);
    }
    let bare = world.spawn((100u32,));
    let archetypes_before = world.archetypes().count();

    assert_eq!(world.unregister_dynamic(9), Ok(5));
    assert_eq!(world.unregister_dynamic(9), Err(DynamicError::Unregistered(9)));
    assert!(world.dynamic_info(9).is_none());
    for &e in &es {
        assert!(!world.has_dynamic(e, 9));
        assert_eq!(
            world.get_dynamic(e, 9).err(),
            Some(DynamicError::Unregistered(9))
        );
        assert!(world.entity(e).unwrap().component_types().all(|c| c != old_id));
    }
    // typed data intact, in the original order
    let mut vals = world
        .query::<(Entity, &u32, &u8)>()
        .iter()
        .map(|(e, &a, &b)| (e, a, b))
        .collect::<Vec<_>>();
    vals.sort_by_key(|x| x.1);
    assert_eq!(vals.len(), 5);
    for (i, (e, a, b)) in vals.iter().enumerate() {
        assert_eq!((*e, *a, *b), (es[i], i as u32, i as u8));
    }
    assert!(world.contains(bare));
    // no entity references old archetypes any more
    assert!(world
        .archetypes()
        .filter(|a| a.has_component(old_id))
        .all(|a| a.is_empty()));

    // Re-register the same id with a different, larger layout
    let new_id = world.register_dynamic(9, info(16, 16), false).unwrap();
    assert_ne!(old_id, new_id);
    for &e in &es {
        world.insert_dynamic(e, 9, &[7u8; 16]).unwrap();
    }
    assert_eq!(
        world.insert_dynamic(es[0], 9, &[7u8; 4]),
        Err(DynamicError::WrongSize {
            expected: 16,
            got: 4
        })
    );
    for &e in &es {
        let r = world.get_dynamic(e, 9).unwrap();
        assert_eq!(&*r, &[7u8; 16]);
        assert_eq!(r.as_ptr() as usize % 16, 0);
    }
    // new archetypes were created for the new component id; the old, empty ones were not reused
    assert!(world.archetypes().count() > archetypes_before);
    assert!(world
        .archetypes()
        .filter(|a| a.has_component(old_id))
        .all(|a| a.is_empty()));
    assert_eq!(world.query_dynamic(&[9]).count(), 5);

    // unregister with nothing to strip
    world.register_dynamic(10, info(1, 1), false).unwrap();
    assert_eq!(world.unregister_dynamic(10), Ok(0));
}

#[test]
fn unregister_everything_leaves_empty_archetype_entities() {
    let mut world = World::new();
    world.register_dynamic(1, info(1, 1), false).unwrap();
    let es = (0..70)
        .map(|_| {
            let e = world.spawn(());
            world.insert_dynamic(e, 1, &[3]).unwrap();
            e
        })
        .collect::<Vec<_>>();
    assert_eq!(world.unregister_dynamic(1), Ok(70));
    assert_eq!(world.len(), 70);
    for &e in &es {
        assert!(world.entity(e).unwrap().is_empty());
    }
    // entities can be despawned / reused normally afterwards
    for &e in &es {
        world.despawn(e).unwrap();
    }
    assert_eq!(world.len(), 0);
}

#[test]
#[should_panic]
fn mut_while_shared_borrow_panics() {
    let mut world = World::new();
    world.register_dynamic(1, info(1, 1), false).unwrap();
    let e = world.spawn(());
    world.insert_dynamic(e, 1, &[1]).unwrap();
    let _r = world.get_dynamic(e, 1).unwrap();
    let _m = world.get_dynamic_mut(e, 1);
}

#[test]
#[should_panic]
fn shared_while_mut_borrow_panics() {
    let mut world = World::new();
    world.register_dynamic(1, info(1, 1), false).unwrap();
    let e = world.spawn(());
    world.insert_dynamic(e, 1, &[1]).unwrap();
    let _m = world.get_dynamic_mut(e, 1).unwrap();
    let _r = world.get_dynamic(e, 1);
}

#[test]
fn shared_borrows_coexist_and_release() {
    let mut world = World::new();
    world.register_dynamic(1, info(1, 1), false).unwrap();
    let e = world.spawn(());
    world.insert_dynamic(e, 1, &[1]).unwrap();
    {
        let a = world.get_dynamic(e, 1).unwrap();
        let b = world.get_dynamic(e, 1).unwrap();
        assert_eq!(a[0] + b[0], 2);
    }
    world.get_dynamic_mut(e, 1).unwrap()[0] = 5;
    assert_eq!(world.get_dynamic(e, 1).unwrap()[0], 5);
}

#[test]
fn dynamic_ticks() {
    let mut world = World::new();
    let id = world.register_dynamic(1, info(4, 4), true).unwrap();
    assert!(world.is_tracked(id));
    world.register_dynamic(2, info(4, 4), false).unwrap();
    let e = world.spawn(());
    world.increment_change_tick(); // 2
    world.insert_dynamic(e, 1, &[0; 4]).unwrap();
    world.insert_dynamic(e, 2, &[0; 4]).unwrap();
    let ticks = |world: &World| world.component_ticks(e, id).unwrap().unwrap();
    assert_eq!(
        ticks(&world),
        ComponentTicks {
            added: t(2),
            changed: t(2)
        }
    );
    // untracked dynamic component: present, no ticks
    assert_eq!(
        world.component_ticks(e, world.dynamic_info(2).unwrap().0),
        Ok(None)
    );
    world.increment_change_tick(); // 3
    world.insert_dynamic(e, 1, &[1; 4]).unwrap(); // replace
    assert_eq!(
        ticks(&world),
        ComponentTicks {
            added: t(2),
            changed: t(3)
        }
    );
    world.increment_change_tick(); // 4
    drop(world.get_dynamic(e, 1).unwrap()); // reading does not stamp
    assert_eq!(ticks(&world).changed, t(3));
    drop(world.get_dynamic_mut(e, 1).unwrap());
    assert_eq!(ticks(&world).changed, t(4));
    assert_eq!(ticks(&world).added, t(2));

    // ticks move with the entity across archetype changes, and swap-removes
    let f = world.spawn(());
    world.increment_change_tick(); // 5
    world.insert_dynamic(f, 1, &[2; 4]).unwrap();
    world.insert_one(e, 77u32).unwrap();
    assert_eq!(
        ticks(&world),
        ComponentTicks {
            added: t(2),
            changed: t(4)
        }
    );
    world.despawn(e).unwrap();
    assert_eq!(
        world.component_ticks(f, id).unwrap().unwrap(),
        ComponentTicks {
            added: t(5),
            changed: t(5)
        }
    );
}

#[test]
fn tracking_enabled_after_registration_retrofits() {
    let mut world = World::new();
    let id = world.register_dynamic(1, info(2, 2), false).unwrap();
    let e = world.spawn(());
    world.insert_dynamic(e, 1, &[1, 2]).unwrap();
    world.increment_change_tick();
    world.increment_change_tick(); // 3
    assert_eq!(world.component_ticks(e, id), Ok(None));
    world.track_changes_id(id);
    assert_eq!(
        world.component_ticks(e, id).unwrap().unwrap(),
        ComponentTicks {
            added: t(3),
            changed: t(3)
        }
    );
    assert_eq!(&*world.get_dynamic(e, 1).unwrap(), &[1, 2]);
}

#[test]
fn clear_keeps_registrations_and_tracking() {
    let mut world = World::new();
    let id = world.register_dynamic(1, info(4, 4), true).unwrap();
    let e = world.spawn((5u32,));
    world.insert_dynamic(e, 1, &[1; 4]).unwrap();
    world.clear();
    assert_eq!(world.len(), 0);
    assert_eq!(world.dynamic_info(1).map(|x| x.0), Some(id));
    assert!(world.is_tracked(id));
    let e2 = world.spawn((6u32,));
    world.insert_dynamic(e2, 1, &[2; 4]).unwrap();
    assert_eq!(&*world.get_dynamic(e2, 1).unwrap(), &[2; 4]);
    assert!(world.component_ticks(e2, id).unwrap().is_some());
}

#[test]
fn take_into_world_with_same_registration() {
    let mut a = World::new();
    let mut b = World::new();
    a.register_dynamic(1, info(4, 4), false).unwrap();
    b.register_dynamic(1, info(4, 4), false).unwrap();
    let e = a.spawn((1u8,));
    a.insert_dynamic(e, 1, &[4, 3, 2, 1]).unwrap();
    let moved = b.spawn(a.take(e).unwrap());
    assert_eq!(&*b.get_dynamic(moved, 1).unwrap(), &[4, 3, 2, 1]);
    assert_eq!(*b.get::<&u8>(moved).unwrap(), 1);
    assert_eq!(a.len(), 0);
}

#[test]
#[should_panic(expected = "not registered in this world")]
fn take_into_world_with_different_layout_panics() {
    let mut a = World::new();
    let mut b = World::new();
    a.register_dynamic(1, info(4, 4), false).unwrap();
    b.register_dynamic(1, info(8, 8), false).unwrap(); // same id and generation, other layout
    let e = a.spawn(());
    a.insert_dynamic(e, 1, &[1; 4]).unwrap();
    b.spawn(a.take(e).unwrap());
}

#[test]
#[should_panic(expected = "not registered in this world")]
fn take_into_world_without_registration_panics() {
    let mut a = World::new();
    let mut b = World::new();
    a.register_dynamic(1, info(4, 4), false).unwrap();
    let e = a.spawn(());
    a.insert_dynamic(e, 1, &[1; 4]).unwrap();
    let existing = b.spawn(());
    b.insert(existing, a.take(e).unwrap()).unwrap();
}

#[test]
fn many_entities_many_ids() {
    let mut world = World::new();
    for id in 0..5u32 {
        world
            .register_dynamic(id, info(8 << id, 8 << id), id % 2 == 0)
            .unwrap();
    }
    let es = (0..40u32).map(|i| world.spawn((i,))).collect::<Vec<_>>();
    for (i, &e) in es.iter().enumerate() {
        for id in 0..5u32 {
            if (i as u32 + id) % 3 != 0 {
                let size = 8usize << id;
                world.insert_dynamic(e, id, &vec![(i as u8) ^ (id as u8); size]).unwrap();
            }
        }
    }
    // remove some, replace some
    for (i, &e) in es.iter().enumerate().step_by(4) {
        let _ = world.remove_dynamic(e, (i % 5) as u32);
    }
    for (i, &e) in es.iter().enumerate() {
        for id in 0..5u32 {
            let size = 8usize << id;
            let expect_present = (i as u32 + id) % 3 != 0 && !(i % 4 == 0 && (i % 5) as u32 == id);
            assert_eq!(world.has_dynamic(e, id), expect_present, "entity {i} id {id}");
            if expect_present {
                let r = world.get_dynamic(e, id).unwrap();
                assert_eq!(r.len(), size);
                assert!(r.iter().all(|&b| b == (i as u8) ^ (id as u8)));
                assert_eq!(r.as_ptr() as usize % size, 0);
            }
        }
        assert_eq!(*world.get::<&u32>(e).unwrap(), i as u32);
    }
}

#[test]
fn rust_components_dropped_exactly_once_across_dynamic_moves() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct D(#[allow(dead_code)] u64);
    impl Drop for D {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    let mut world = World::new();
    world.register_dynamic(1, info(8, 8), true).unwrap();
    world.track_changes::<D>();
    let es: Vec<_> = (0..5).map(|i| world.spawn((D(i), i as u32))).collect();
    for &e in &es {
        world.insert_dynamic(e, 1, &[7; 8]).unwrap();
    }
    world.remove_dynamic(es[1], 1).unwrap();
    world.insert_dynamic(es[1], 1, &[1; 8]).unwrap();
    assert_eq!(world.unregister_dynamic(1).unwrap(), 5);
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);
    world.despawn(es[2]).unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    drop(world);
    assert_eq!(DROPS.load(Ordering::SeqCst), 5);
}
