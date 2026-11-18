//! Snapshots (docs/SCENES.md, phase S3): snapshot, change the world, restore. Headless.

use efeengine_core::ecs::{entity_from_bits, entity_to_bits, Scene, SceneOwned};
use efeengine_core::prelude::*;

#[script_component(name = "Coin")]
#[derive(Default, Debug)]
struct Coin {
    value: i32,
}

/// Points at another entity.
#[script_component(name = "Follow")]
#[derive(Default, Debug)]
struct Follow {
    #[script(entity)]
    target: u64,
}

fn engine() -> Engine {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
    e.register::<Coin>().unwrap();
    e.register::<Follow>().unwrap();
    for n in ["a_sys", "b_sys", "other"] {
        e.add_system(Stage::Update, |_| {}).name(n);
    }
    e.add_system(Stage::PostUpdate, |_| {}).name("late");
    e
}

fn scene(text: &str) -> Scene {
    Scene::from_ron(text).unwrap()
}

const LEVEL: &str = r#"Scene(format: 1, name: "level", systems: ["a_sys"], entities: [
    (components: { "Coin": { "value": 1 }, "Transform": { "position": [1.0, 2.0], "rotation": 0.0, "scale": [1.0, 1.0] } }),
    (components: { "Coin": { "value": 2 } }),
])"#;

/// The snapshot's content as a sorted list, without entity ids (restored entities get new ids
/// and so a new order).
fn content(e: &Engine) -> Vec<String> {
    let s = e.snapshot();
    let mut v: Vec<String> = s.scene.entities.iter().zip(&s.owners).map(|(ent, o)| format!("{:?} {:?}", ent.components, o)).collect();
    v.sort();
    v
}

fn flags(e: &Engine) -> Vec<(String, bool)> {
    let mut v = Vec::new();
    for st in [Stage::Update, Stage::PostUpdate] {
        for (n, en) in e.system_states(st) {
            v.push((n.to_string(), en));
        }
    }
    v
}

fn owned(e: &Engine) -> Vec<String> {
    let mut v: Vec<String> = e.world().query::<&SceneOwned>().iter().map(|o| format!("{}#{}", o.scene_name(), o.scene_id())).collect();
    v.sort();
    v
}

#[test]
fn restore_puts_the_world_back() {
    let mut e = engine();
    e.set_system_enabled("b_sys", false).unwrap();
    e.load_scene(&scene(LEVEL)).unwrap();
    // A loose entity the scene does not own: the snapshot covers it too.
    e.world_mut().spawn((Coin { value: 99 },));
    e.run_frames(3, 1.0 / 60.0);

    let snap = e.snapshot();
    let world_before = content(&e);
    let flags_before = flags(&e);
    let owned_before = owned(&e);
    assert_eq!(snap.tick, e.change_tick().get());
    assert_eq!(snap.scene.entities.len(), 3);

    // Change everything: move, despawn, spawn, toggle systems, switch the scene.
    let coins: Vec<Entity> = e.world().query::<(Entity, &Coin)>().iter().map(|(en, _)| en).collect();
    for en in &coins {
        e.world_mut().get::<&mut Coin>(*en).unwrap().value += 100;
    }
    e.world_mut().despawn(coins[0]).unwrap();
    e.world_mut().spawn((Coin { value: -1 },));
    e.set_system_enabled("a_sys", false).unwrap();
    e.set_system_enabled("b_sys", true).unwrap();
    e.set_system_enabled("late", false).unwrap();
    e.load_scene(&scene(r#"Scene(format: 1, name: "other", systems: ["other"], entities: [(components: { "Coin": { "value": 7 } })])"#)).unwrap();
    assert_ne!(flags(&e), flags_before);
    assert_eq!(e.current_scene(), Some("other"));

    e.restore(&snap).unwrap();
    assert_eq!(content(&e), world_before, "entities equal the snapshot");
    assert_eq!(flags(&e), flags_before, "system flags restored");
    assert_eq!(owned(&e), owned_before, "ownership restored");
    assert_eq!(e.current_scene(), Some("level"));
    assert_eq!(e.world().query::<&Coin>().iter().count(), 3);

    // The restored scene is the active one: unloading removes exactly its entities.
    e.unload_scene();
    assert_eq!(e.world().query::<&Coin>().iter().count(), 1);
    assert!(!e.is_system_enabled("a_sys").unwrap());
}

#[test]
fn entity_references_survive_restore() {
    let mut e = engine();
    e.load_scene(&scene(LEVEL)).unwrap();
    let ents: Vec<Entity> = {
        let mut v: Vec<(i32, Entity)> = e.world().query::<(Entity, &Coin)>().iter().map(|(en, c)| (c.value, en)).collect();
        v.sort_by_key(|(c, _)| *c);
        v.into_iter().map(|(_, en)| en).collect()
    };
    // Entity 0 follows entity 1; entity 1 follows entity 0.
    e.world_mut().insert_one(ents[0], Follow { target: entity_to_bits(ents[1]) }).unwrap();
    e.world_mut().insert_one(ents[1], Follow { target: entity_to_bits(ents[0]) }).unwrap();
    let snap = e.snapshot();

    // Make the ids different: churn the world, then restore.
    for _ in 0..5 {
        e.world_mut().spawn((Coin { value: 0 },));
    }
    e.restore(&snap).unwrap();

    let mut by_value = std::collections::BTreeMap::new();
    for (en, c, f) in e.world().query::<(Entity, &Coin, &Follow)>().iter() {
        by_value.insert(c.value, (en, f.target));
    }
    let (e1, t1) = by_value[&1];
    let (e2, t2) = by_value[&2];
    assert_eq!(entity_from_bits(t1), Some(e2), "1 still follows 2");
    assert_eq!(entity_from_bits(t2), Some(e1), "2 still follows 1");
    assert!(e.world().contains(e1) && e.world().contains(e2));
}

#[test]
fn invalid_snapshot_changes_nothing() {
    let mut e = engine();
    e.load_scene(&scene(LEVEL)).unwrap();
    let mut snap = e.snapshot();
    let before = e.snapshot().scene.to_ron();
    snap.scene.entities[0].components.get_mut("Coin").unwrap().insert("bogus".into(), efeengine_core::ecs::SceneValue::Int(1));
    assert!(e.restore(&snap).is_err());
    let mut snap = e.snapshot();
    snap.systems.push((Stage::Update, "gone".into(), true));
    assert!(e.restore(&snap).is_err());
    assert_eq!(e.snapshot().scene.to_ron(), before);
}

// ---- E0.3 editor ids, E0.4 resource snapshots ----

#[derive(Clone, Debug, PartialEq)]
struct Score(u32);

#[derive(Clone, Debug, PartialEq)]
struct Unregistered(u32);

#[test]
fn editor_id_survives_restore_and_lookup_works() {
    let mut e = engine();
    e.load_scene_str(r#"Scene(format: 1, name: "ed", entities: [
        (components: { "EditorId": { "id": 77 }, "Coin": { "value": 1 } }),
        (components: { "EditorId": { "id": 5 }, "Coin": { "value": 2 } }),
    ])"#)
    .unwrap();
    let before = e.entity_by_editor_id(77).unwrap();
    assert_eq!(e.world().get::<&Coin>(before).unwrap().value, 1);
    let snap = e.snapshot();
    assert!(snap.scene.entities.iter().any(|x| x.components.contains_key("EditorId")));
    e.world_mut().despawn(before).unwrap();
    assert!(e.entity_by_editor_id(77).is_none());
    e.restore(&snap).unwrap();
    let after = e.entity_by_editor_id(77).unwrap();
    assert_eq!(e.world().get::<&EditorId>(after).unwrap().id, 77);
    assert_eq!(e.world().get::<&Coin>(after).unwrap().value, 1);
    assert_eq!(e.world().get::<&Coin>(e.entity_by_editor_id(5).unwrap()).unwrap().value, 2);
    assert!(e.entity_by_editor_id(6).is_none());
}

#[test]
fn duplicate_editor_id_is_an_error() {
    let mut e = engine();
    let err = e
        .load_scene_str(r#"Scene(format: 1, name: "dup", entities: [
            (components: { "EditorId": { "id": 3 } }),
            (components: { "EditorId": { "id": 3 } }),
        ])"#)
        .unwrap_err();
    assert!(err.to_string().contains("EditorId 3"), "{err}");
    assert_eq!(e.world().iter().count(), 0);
}

#[test]
fn registered_resource_is_restored_and_gaps_are_listed() {
    let mut e = engine();
    e.insert_resource(Score(1));
    e.insert_resource(Unregistered(1));
    e.register_snapshot_resource::<Score>();
    let snap = e.snapshot();
    assert!(snap.gaps.iter().any(|n| n.ends_with("Unregistered")));
    assert!(!snap.gaps.iter().any(|n| n.ends_with("Score")));
    assert!(e.snapshot_gaps().iter().any(|n| n.ends_with("Unregistered")));
    e.resources_mut().get_mut::<Score>().unwrap().0 = 9;
    e.resources_mut().get_mut::<Unregistered>().unwrap().0 = 9;
    e.restore(&snap).unwrap();
    assert_eq!(e.resources().get::<Score>().as_deref(), Some(&Score(1)));
    assert_eq!(e.resources().get::<Unregistered>().as_deref(), Some(&Unregistered(9)));
}
