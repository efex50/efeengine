//! Change detection: ticks, `Changed`/`Added`/`Mut`, and tick bookkeeping across structural changes

use hecs::*;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pos(i32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Vel(i32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tag;

fn pid() -> ComponentId {
    ComponentId::of::<Pos>()
}

fn ticks(world: &World, e: Entity) -> ComponentTicks {
    world.component_ticks(e, pid()).unwrap().unwrap()
}

fn t(n: u32) -> Tick {
    Tick::new(n)
}

fn tracked_world() -> World {
    let mut world = World::new();
    world.track_changes::<Pos>();
    world
}

fn collect_changed(world: &World, last: Tick) -> Vec<Entity> {
    let now = world.change_tick();
    let mut v = world
        .query::<(Entity, Changed<Pos>)>()
        .with_ticks(last, now)
        .iter()
        .map(|(e, ())| e)
        .collect::<Vec<_>>();
    v.sort();
    v
}

fn collect_added(world: &World, last: Tick) -> Vec<Entity> {
    let now = world.change_tick();
    let mut v = world
        .query::<(Entity, Added<Pos>)>()
        .with_ticks(last, now)
        .iter()
        .map(|(e, ())| e)
        .collect::<Vec<_>>();
    v.sort();
    v
}

#[test]
fn tick_wrap_semantics() {
    // plain
    assert!(t(5).is_newer_than(t(4), t(10)));
    assert!(!t(4).is_newer_than(t(4), t(10)));
    assert!(t(10).is_newer_than(t(4), t(10)));
    // around u32::MAX
    let last = t(u32::MAX - 1);
    let now = t(2);
    assert!(t(u32::MAX).is_newer_than(last, now));
    assert!(t(0).is_newer_than(last, now));
    assert!(t(2).is_newer_than(last, now));
    assert!(!t(u32::MAX - 1).is_newer_than(last, now));
    assert!(!t(u32::MAX - 5).is_newer_than(last, now));
    // `Tick::get`/`new` round trip and default
    assert_eq!(t(7).get(), 7);
    assert_eq!(Tick::default(), t(0));
}

#[test]
fn check_tick_clamps_old() {
    let now = t(100);
    let mut old = Tick::new(100u32.wrapping_sub(MAX_CHANGE_AGE + 1));
    assert!(old.check_tick(now));
    assert_eq!(old, Tick::new(100u32.wrapping_sub(MAX_CHANGE_AGE)));
    let mut young = Tick::new(100u32.wrapping_sub(MAX_CHANGE_AGE));
    assert!(!young.check_tick(now));
    assert_eq!(MAX_CHANGE_AGE, u32::MAX - (2 * CHECK_TICK_THRESHOLD - 1));
}

#[test]
fn world_tick_increments_and_wraps() {
    let mut world = World::new();
    assert_eq!(world.change_tick(), t(1));
    assert_eq!(world.increment_change_tick(), t(2));
    assert_eq!(world.change_tick(), t(2));
}

#[test]
fn spawn_stamps_added_and_changed() {
    let mut world = tracked_world();
    world.increment_change_tick();
    let e = world.spawn((Pos(1),));
    assert_eq!(
        ticks(&world, e),
        ComponentTicks {
            added: t(2),
            changed: t(2)
        }
    );
    assert!(world.is_tracked(pid()));
    assert!(!world.is_tracked(ComponentId::of::<Vel>()));
}

#[test]
fn untracked_has_no_ticks() {
    let mut world = World::new();
    let e = world.spawn((Pos(1),));
    assert_eq!(world.component_ticks(e, pid()), Ok(None));
    assert!(world.component_ticks(e, ComponentId::of::<Vel>()).is_err());
    // Untracked: is_changed/is_added are conservatively true.
    assert_eq!(world.is_changed::<Pos>(e, t(1000)), Ok(true));
    assert_eq!(world.is_added::<Pos>(e, t(1000)), Ok(true));
}

#[test]
fn mut_ref_query_stamps_eagerly() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),));
    let b = world.spawn((Pos(2),));
    world.increment_change_tick(); // 2
    world.increment_change_tick(); // 3
    for p in world.query_mut::<&mut Pos>() {
        // Merely fetching mutably counts as a change
        let _ = p;
    }
    assert_eq!(ticks(&world, a).changed, t(3));
    assert_eq!(ticks(&world, a).added, t(1));
    assert_eq!(ticks(&world, b).changed, t(3));
    // a shared query does not stamp
    world.increment_change_tick(); // 4
    for _ in world.query_mut::<&Pos>() {}
    assert_eq!(ticks(&world, a).changed, t(3));
}

#[test]
fn world_get_mut_and_query_one_stamp() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),));
    world.increment_change_tick(); // 2
    {
        let mut p = world.get::<&mut Pos>(a).unwrap();
        p.0 += 1;
    }
    assert_eq!(ticks(&world, a).changed, t(2));
    world.increment_change_tick(); // 3
    {
        let mut q = world.query_one::<&mut Pos>(a);
        q.get().unwrap().0 += 1;
    }
    assert_eq!(ticks(&world, a).changed, t(3));
    world.increment_change_tick(); // 4
    world.query_one_mut::<&mut Pos>(a).unwrap().0 += 1;
    assert_eq!(ticks(&world, a).changed, t(4));
    assert_eq!(world.is_changed::<Pos>(a, t(3)), Ok(true));
    assert_eq!(world.is_changed::<Pos>(a, t(4)), Ok(false));
    assert_eq!(world.is_added::<Pos>(a, t(0)), Ok(true));
    assert_eq!(world.is_added::<Pos>(a, t(1)), Ok(false));
}

#[test]
fn mut_wrapper_stamps_only_on_deref_mut() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),));
    let b = world.spawn((Pos(2),));
    world.increment_change_tick(); // 2
    world.increment_change_tick(); // 3
    {
        let mut q = world.query::<(Entity, Mut<Pos>)>();
        for (e, mut p) in q.iter() {
            assert_eq!(p.0, if e == a { 1 } else { 2 });
            if e == a {
                p.0 += 10; // DerefMut
            }
        }
    }
    assert_eq!(ticks(&world, a).changed, t(3));
    assert_eq!(ticks(&world, b).changed, t(1));
    assert_eq!(world.get::<&Pos>(a).unwrap().0, 11);

    // set_changed / bypass / into_inner / is_changed / is_added
    world.increment_change_tick(); // 4
    {
        let mut q = world.query::<(Entity, Mut<Pos>)>().with_ticks(t(3), t(4));
        for (e, mut p) in q.iter() {
            if e == a {
                // changed at 3, last_run 3: not new. added at 1: not new
                assert!(!p.is_changed());
                assert!(!p.is_added());
                p.bypass_change_detection().0 += 1;
            } else {
                assert!(!p.is_changed());
                p.set_changed();
                assert_eq!(p.into_inner().0, 2);
            }
        }
    }
    assert_eq!(ticks(&world, a).changed, t(3)); // bypassed
    assert_eq!(ticks(&world, b).changed, t(4)); // set_changed / into_inner

    // is_changed with an older window sees them
    {
        let mut q = world.query::<Mut<Pos>>().with_ticks(t(0), t(4));
        assert!(q.iter().all(|p| p.is_changed() && p.is_added()));
    }
}

#[test]
fn mut_on_untracked_is_conservative() {
    let mut world = World::new();
    world.spawn((Vel(1),));
    let mut q = world.query::<Mut<Vel>>().with_ticks(t(5), t(6));
    let mut v = q.iter().next().unwrap();
    assert!(v.is_changed());
    assert!(v.is_added());
    v.0 = 4;
    assert_eq!(v.0, 4);
}

#[test]
fn changed_and_added_filters() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),)); // tick 1
    world.increment_change_tick(); // 2
    let b = world.spawn((Pos(2),)); // added 2
    world.increment_change_tick(); // 3
    world.query_one_mut::<&mut Pos>(a).unwrap().0 += 1; // a changed 3
    let c = world.spawn((Pos(3), Vel(0))); // added 3

    assert_eq!(collect_added(&world, t(0)), sorted(&[a, b, c]));
    assert_eq!(collect_added(&world, t(1)), sorted(&[b, c]));
    assert_eq!(collect_added(&world, t(2)), sorted(&[c]));
    assert_eq!(collect_added(&world, t(3)), sorted(&[]));
    assert_eq!(collect_changed(&world, t(0)), sorted(&[a, b, c]));
    assert_eq!(collect_changed(&world, t(2)), sorted(&[a, c]));
    assert_eq!(collect_changed(&world, t(3)), sorted(&[]));

    // query_since uses the world's tick as this_run
    let mut got = world
        .query_since::<(Entity, Changed<Pos>)>(t(2))
        .iter()
        .map(|(e, ())| e)
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(got, sorted(&[a, c]));

    // len() counts matching rows, exactly
    let mut q = world.query::<(Entity, Changed<Pos>)>().with_ticks(t(2), t(3));
    let mut iter = q.iter();
    assert_eq!(iter.len(), 2);
    assert_eq!(iter.size_hint(), (2, Some(2)));
    iter.next();
    assert_eq!(iter.len(), 1);
    drop(iter);
    drop(q);

    // works with other terms and via fold/for_each
    let mut q = world.query::<(&Pos, &Vel, Changed<Pos>)>().with_ticks(t(2), t(3));
    assert_eq!(q.iter().count(), 1);
    let mut n = 0;
    q.iter().for_each(|_| n += 1);
    assert_eq!(n, 1);

    drop(q);

    // QueryMut variant
    let n = world
        .query_mut::<(&Pos, Added<Pos>)>()
        .with_ticks(t(2), t(3))
        .into_iter()
        .count();
    assert_eq!(n, 1);
}


fn sorted(es: &[Entity]) -> Vec<Entity> {
    let mut v = es.to_vec();
    v.sort();
    v
}

#[test]
fn filters_in_or_and_wrappers() {
    let mut world = World::new();
    world.track_changes::<Pos>();
    world.track_changes::<Vel>();
    let a = world.spawn((Pos(0), Vel(0))); // 1
    world.increment_change_tick(); // 2
    let b = world.spawn((Pos(0),)); // Pos added 2
    let c = world.spawn((Vel(0),)); // Vel added 2
    world.increment_change_tick(); // 3
    let q = |world: &World| {
        let mut v = world
            .query::<(Entity, Or<Changed<Pos>, Changed<Vel>>)>()
            .with_ticks(t(1), t(3))
            .iter()
            .map(|(e, _)| e)
            .collect::<Vec<_>>();
        v.sort();
        v
    };
    assert_eq!(q(&world), sorted(&[b, c]));
    // Without + filter on the main part
    let n = world
        .query::<Without<(Entity, Changed<Pos>), &Vel>>()
        .with_ticks(t(1), t(3))
        .iter()
        .count();
    assert_eq!(n, 1);
    let _ = a;
}

#[test]
fn view_and_query_one_respect_filters() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),));
    world.increment_change_tick(); // 2
    let b = world.spawn((Pos(2),));
    {
        let view = world
            .view::<(&Pos, Changed<Pos>)>()
            .with_ticks(t(1), t(2));
        assert!(view.get(a).is_none());
        assert!(!view.contains(a));
        assert_eq!(view.get(b).map(|(p, ())| p.0), Some(2));
        assert!(view.contains(b));
    }
    {
        let mut q = world.query_one::<(&Pos, Changed<Pos>)>(a).with_ticks(t(1), t(2));
        assert_eq!(q.get().err(), Some(QueryOneError::Unsatisfied));
    }
    {
        let mut q = world.query_one::<(&Pos, Changed<Pos>)>(b).with_ticks(t(1), t(2));
        assert_eq!(q.get().map(|(p, ())| p.0), Ok(2));
    }
    // the failed attempt must have released its borrow
    world.query_one_mut::<&mut Pos>(a).unwrap().0 = 5;
}

#[test]
#[should_panic]
fn changed_with_mut_same_component_conflicts() {
    let mut world = tracked_world();
    world.spawn((Pos(1),));
    let mut q = world.query::<(&mut Pos, Changed<Pos>)>();
    let _ = q.iter().count();
}

#[test]
fn untracked_changed_matches_everything() {
    let mut world = World::new();
    let a = world.spawn((Vel(1),));
    let b = world.spawn((Vel(2), Tag));
    world.spawn((Pos(1),));
    world.increment_change_tick();
    let mut got = world
        .query::<(Entity, Changed<Vel>)>()
        .with_ticks(t(100), t(101))
        .iter()
        .map(|(e, ())| e)
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(got, sorted(&[a, b]));
    assert_eq!(
        world
            .query::<Added<Tag>>()
            .with_ticks(t(100), t(101))
            .iter()
            .count(),
        1
    );
}

#[test]
fn ticks_survive_archetype_move() {
    let mut world = World::new();
    world.track_changes::<Pos>();
    world.track_changes::<Vel>();
    let a = world.spawn((Pos(1),)); // 1
    world.increment_change_tick(); // 2
    world.query_one_mut::<&mut Pos>(a).unwrap().0 = 2; // changed 2
    world.increment_change_tick(); // 3
    world.insert_one(a, Vel(9)).unwrap(); // move; Vel new at 3
    assert_eq!(
        ticks(&world, a),
        ComponentTicks {
            added: t(1),
            changed: t(2)
        }
    );
    assert_eq!(
        world
            .component_ticks(a, ComponentId::of::<Vel>())
            .unwrap()
            .unwrap(),
        ComponentTicks {
            added: t(3),
            changed: t(3)
        }
    );
    assert_eq!(world.get::<&Pos>(a).unwrap().0, 2);
    world.increment_change_tick(); // 4
    world.remove_one::<Vel>(a).unwrap(); // move back
    assert_eq!(
        ticks(&world, a),
        ComponentTicks {
            added: t(1),
            changed: t(2)
        }
    );
}

#[test]
fn ticks_survive_swap_remove() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),)); // 1
    world.increment_change_tick();
    let b = world.spawn((Pos(2),)); // 2
    world.increment_change_tick();
    let c = world.spawn((Pos(3),)); // 3
    world.increment_change_tick();
    world.query_one_mut::<&mut Pos>(c).unwrap().0 = 30; // c changed 4
    world.despawn(a).unwrap(); // c moves into a's row
    assert_eq!(
        ticks(&world, b),
        ComponentTicks {
            added: t(2),
            changed: t(2)
        }
    );
    assert_eq!(
        ticks(&world, c),
        ComponentTicks {
            added: t(3),
            changed: t(4)
        }
    );
    assert_eq!(world.get::<&Pos>(c).unwrap().0, 30);

    // same through `remove` (archetype move out of a tracked archetype)
    let d = world.spawn((Pos(4), Vel(0))); // 4
    let e = world.spawn((Pos(5), Vel(0))); // 4
    world.increment_change_tick();
    world.query_one_mut::<&mut Pos>(e).unwrap().0 = 50; // e changed 5
    world.remove_one::<Vel>(d).unwrap(); // swap-remove in the (Pos, Vel) archetype
    assert_eq!(
        ticks(&world, e),
        ComponentTicks {
            added: t(4),
            changed: t(5)
        }
    );
    assert_eq!(
        ticks(&world, d),
        ComponentTicks {
            added: t(4),
            changed: t(4)
        }
    );
}

#[test]
fn spawn_batch_and_growth() {
    let mut world = tracked_world();
    world.increment_change_tick(); // 2
    let es = world.spawn_batch((0..200).map(|i| (Pos(i),))).collect::<Vec<_>>();
    for (i, &e) in es.iter().enumerate() {
        assert_eq!(world.get::<&Pos>(e).unwrap().0, i as i32);
        assert_eq!(
            ticks(&world, e),
            ComponentTicks {
                added: t(2),
                changed: t(2)
            }
        );
    }
    // growth past the first 64 with distinct ticks per row
    let mut w2 = tracked_world();
    let mut all = Vec::new();
    for i in 0..150u32 {
        w2.increment_change_tick();
        all.push((w2.spawn((Pos(i as i32),)), w2.change_tick()));
    }
    for (e, tick) in all {
        assert_eq!(ticks(&w2, e).added, tick);
    }
}

#[test]
fn clear_and_reuse() {
    let mut world = tracked_world();
    for _ in 0..70 {
        world.spawn((Pos(1),));
    }
    world.clear();
    assert_eq!(world.len(), 0);
    world.increment_change_tick();
    let e = world.spawn((Pos(2),));
    assert_eq!(ticks(&world, e).added, t(2));
    assert!(world.is_tracked(pid()));
}

#[test]
fn column_batch_ticks() {
    let mut world = tracked_world();
    let make = |n: u32| {
        let mut ty = ColumnBatchType::new();
        ty.add::<Pos>();
        ty.add::<Vel>();
        let b = ty.into_batch(n);
        {
            let mut w = b.writer::<Pos>().unwrap();
            let mut v = b.writer::<Vel>().unwrap();
            for i in 0..n {
                w.push(Pos(i as i32)).unwrap();
                v.push(Vel(0)).unwrap();
            }
        }
        b.build().unwrap()
    };
    world.increment_change_tick(); // 2
    // brand-new archetype
    let first = world.spawn_column_batch(make(3)).collect::<Vec<_>>();
    for &e in &first {
        assert_eq!(ticks(&world, e).added, t(2));
        assert_eq!(ticks(&world, e).changed, t(2));
    }
    world.increment_change_tick(); // 3
    // merged into the existing archetype
    let second = world.spawn_column_batch(make(70)).collect::<Vec<_>>();
    for &e in &second {
        assert_eq!(ticks(&world, e).added, t(3));
    }
    for &e in &first {
        assert_eq!(ticks(&world, e).added, t(2), "old rows keep their ticks");
    }
    world.increment_change_tick(); // 4
    let handles = [world.reserve_entity(), world.reserve_entity()];
    world.flush();
    world.spawn_column_batch_at(&handles, make(2));
    for &e in &handles {
        assert_eq!(ticks(&world, e).added, t(4));
        assert!(world.get::<&Vel>(e).is_ok());
    }
}

#[test]
fn insert_replace_keeps_added() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1), Vel(1))); // 1
    world.increment_change_tick(); // 2
    world.insert_one(a, Pos(2)).unwrap(); // replace in place
    assert_eq!(
        ticks(&world, a),
        ComponentTicks {
            added: t(1),
            changed: t(2)
        }
    );
    world.increment_change_tick(); // 3
    // replacing while also moving archetypes (adds Tag)
    world.insert(a, (Pos(3), Tag)).unwrap();
    assert_eq!(
        ticks(&world, a),
        ComponentTicks {
            added: t(1),
            changed: t(3)
        }
    );
    assert_eq!(world.get::<&Pos>(a).unwrap().0, 3);
}

#[test]
fn exchange_ticks() {
    let mut world = World::new();
    world.track_changes::<Pos>();
    world.track_changes::<Vel>();
    world.track_changes::<Tag>();
    let a = world.spawn((Pos(1), Vel(1))); // 1
    world.increment_change_tick(); // 2
    // remove Vel, add Tag: Pos retained
    let v = world.exchange_one::<Vel, Tag>(a, Tag).unwrap();
    assert_eq!(v, Vel(1));
    assert_eq!(
        ticks(&world, a),
        ComponentTicks {
            added: t(1),
            changed: t(1)
        }
    );
    assert_eq!(
        world
            .component_ticks(a, ComponentId::of::<Tag>())
            .unwrap()
            .unwrap()
            .added,
        t(2)
    );
    world.increment_change_tick(); // 3
    // exchange to the same archetype: removed and re-added, so counts as new
    let tag = world.exchange_one::<Tag, Tag>(a, Tag).unwrap();
    assert_eq!(tag, Tag);
    assert_eq!(
        world
            .component_ticks(a, ComponentId::of::<Tag>())
            .unwrap()
            .unwrap(),
        ComponentTicks {
            added: t(3),
            changed: t(3)
        }
    );
    assert_eq!(ticks(&world, a).added, t(1));
}

#[test]
fn take_and_respawn() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),));
    world.increment_change_tick(); // 2
    let mut other = tracked_world();
    other.increment_change_tick();
    other.increment_change_tick();
    other.increment_change_tick(); // 4
    let taken = world.take(a).unwrap();
    let b = other.spawn(taken);
    assert_eq!(ticks(&other, b).added, t(4));
    assert_eq!(world.len(), 0);
}

#[test]
fn retrofit_tracking_on_existing_archetypes() {
    let mut world = World::new();
    let es = (0..100).map(|i| world.spawn((Pos(i),))).collect::<Vec<_>>();
    let f = world.spawn((Pos(1), Vel(2)));
    for _ in 0..4 {
        world.increment_change_tick();
    } // 5
    assert_eq!(world.component_ticks(es[0], pid()), Ok(None));
    world.track_changes::<Pos>();
    world.track_changes::<Pos>(); // idempotent
    for &e in es.iter().chain([&f]) {
        assert_eq!(
            ticks(&world, e),
            ComponentTicks {
                added: t(5),
                changed: t(5)
            }
        );
    }
    // everything looks added once...
    assert_eq!(collect_added(&world, t(4)).len(), 101);
    world.increment_change_tick(); // 6
    // ...and not afterwards
    assert_eq!(collect_added(&world, t(5)).len(), 0);
    // and the retrofitted storage keeps working: growth, moves, removal
    for i in 0..100 {
        world.spawn((Pos(i),));
    }
    world.insert_one(es[3], Vel(0)).unwrap();
    world.despawn(es[0]).unwrap();
    assert_eq!(collect_added(&world, t(5)).len(), 100);
    assert_eq!(ticks(&world, es[3]).added, t(5));
    assert_eq!(ticks(&world, es[99]).added, t(5));
}

#[test]
fn check_change_ticks_clamps_stale() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),));
    let b = world.spawn((Pos(2),));
    // Forge a stamp far in the past by running a query in a very different tick window.
    let stale = Tick::new(world.change_tick().get().wrapping_sub(MAX_CHANGE_AGE + 5));
    {
        let mut q = world.query_one::<&mut Pos>(a).with_ticks(t(0), stale);
        q.get().unwrap().0 = 7;
    }
    assert_eq!(ticks(&world, a).changed, stale);
    // Not yet due: nothing happens
    world.maybe_check_change_ticks();
    assert_eq!(ticks(&world, a).changed, stale);
    world.check_change_ticks();
    let clamped = Tick::new(world.change_tick().get().wrapping_sub(MAX_CHANGE_AGE));
    assert_eq!(ticks(&world, a).changed, clamped);
    assert_eq!(ticks(&world, b).changed, t(1));
}

#[test]
fn track_before_and_after_archetype_creation() {
    // Tracked set applies to archetypes created later, for every type in them.
    let mut world = World::new();
    world.track_changes::<Vel>();
    world.increment_change_tick(); // 2
    let e = world.spawn((Pos(1), Vel(2), Tag));
    assert!(world.component_ticks(e, pid()).unwrap().is_none());
    assert_eq!(
        world
            .component_ticks(e, ComponentId::of::<Vel>())
            .unwrap()
            .unwrap()
            .added,
        t(2)
    );
    // zero-sized components can be tracked as well
    world.track_changes::<Tag>();
    assert_eq!(
        world
            .component_ticks(e, ComponentId::of::<Tag>())
            .unwrap()
            .unwrap()
            .added,
        t(2)
    );
}

#[test]
fn prepared_query_with_filters() {
    let mut world = tracked_world();
    let a = world.spawn((Pos(1),));
    world.increment_change_tick();
    world.increment_change_tick(); // 3
    let b = world.spawn((Pos(2),));
    let mut prepared = PreparedQuery::<(Entity, Changed<Pos>)>::new();
    let got = prepared
        .query(&world)
        .with_ticks(t(1), t(3))
        .iter()
        .map(|(e, ())| e)
        .collect::<Vec<_>>();
    assert_eq!(got, vec![b]);
    let all = prepared
        .query_mut(&mut world)
        .map(|(e, ())| e)
        .collect::<Vec<_>>();
    assert_eq!(all.len(), 2);
    assert!(all.contains(&a));
}

#[test]
fn batched_iteration_with_filters() {
    let mut world = tracked_world();
    for i in 0..10 {
        world.spawn((Pos(i),));
    }
    world.increment_change_tick(); // 2
    for i in 0..10 {
        world.spawn((Pos(i),));
    }
    let mut q = world
        .query::<&Pos>()
        .with::<Added<Pos>>()
        .with_ticks(t(1), t(2));
    let n: usize = q.iter_batched(4).map(|b| b.count()).sum();
    // `with` does not apply the right-hand filter (documented): all 20 are yielded
    assert_eq!(n, 20);
    let mut q = world
        .query::<(&Pos, Added<Pos>)>()
        .with_ticks(t(1), t(2));
    let n: usize = q.iter_batched(4).map(|b| b.count()).sum();
    assert_eq!(n, 10);
}

#[cfg(feature = "macros")]
mod derive {
    use super::*;

    #[derive(Query)]
    struct Movers<'a> {
        pos: Mut<'a, Pos>,
        vel: &'a Vel,
    }

    #[derive(Query)]
    enum PosOrVel<'a> {
        A { p: Mut<'a, Pos> },
        B(#[allow(dead_code)] &'a Vel),
    }

    #[test]
    fn derive_query_with_mut_and_filters() {
        let mut world = World::new();
        world.track_changes::<Pos>();
        world.track_changes::<Vel>();
        let a = world.spawn((Pos(1), Vel(1)));
        world.increment_change_tick(); // 2
        let b = world.spawn((Pos(2), Vel(2)));
        {
            // filters compose around derived queries
            let mut q = world
                .query::<(Movers, Added<Vel>)>()
                .with_ticks(t(1), t(2));
            let mut n = 0;
            for (m, ()) in q.iter() {
                n += 1;
                let mut pos = m.pos;
                assert!(pos.is_added());
                pos.0 += m.vel.0;
            }
            assert_eq!(n, 1);
        }
        assert_eq!(world.get::<&Pos>(a).unwrap().0, 1);
        assert_eq!(world.get::<&Pos>(b).unwrap().0, 4);
        assert_eq!(ticks(&world, b).changed, t(2));
        assert_eq!(ticks(&world, a).changed, t(1));

        // enum: `Mut` inside a variant only stamps when written
        world.increment_change_tick(); // 3
        let mut q = world.query::<PosOrVel>().with_ticks(t(2), t(3));
        for x in q.iter() {
            match x {
                PosOrVel::A { mut p } => {
                    assert!(!p.is_changed());
                    p.set_changed();
                }
                PosOrVel::B(_) => unreachable!(),
            }
        }
        drop(q);
        assert_eq!(ticks(&world, a).changed, t(3));
        assert_eq!(ticks(&world, b).changed, t(3));
    }
}
