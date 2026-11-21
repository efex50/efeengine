//! Prefabs (docs/SCENES.md, phase S5): files, placement with overrides, propagation, saving
//! placements back, the editor-facing instance API. Headless.

use efeengine_core::ecs::{Scene, SceneError, SceneValue};
use efeengine_core::engine::prefab::{Prefab, PrefabInstance};
use efeengine_core::prelude::*;

#[script_component(name = "Coin")]
#[derive(Default, Debug)]
struct Coin {
    value: i32,
    weight: f32,
    taken: u32,
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("efe_prefab_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn engine(dir: &std::path::Path) -> Engine {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
    e.register::<Coin>().unwrap();
    e.add_system(Stage::Update, |_| {}).name("coin_collect");
    e.add_system(Stage::Update, |_| {}).name("hud");
    e.set_prefab_dir(dir);
    e
}

const COIN: &str = r#"Prefab(
    format: 1,
    name: "Coin",
    entities: [
        (id: Some(1), name: Some("Coin"), components: {
            "Transform": { "position": [0.0, 0.0], "rotation": 0.0, "scale": [1.0, 1.0] },
            "Coin": { "value": 1, "weight": 0.5, "taken": 0 },
        }),
    ],
)"#;

const CRATE: &str = r#"Prefab(
    format: 1,
    name: "Crate",
    entities: [
        (id: Some(1), name: Some("Crate"), components: {
            "Transform": { "position": [0.0, 0.0], "rotation": 0.0, "scale": [1.0, 1.0] },
        }),
        (id: Some(2), name: Some("Lid"), components: {
            "Parent": { "entity": 1 },
            "Transform": { "position": [0.0, 8.0], "rotation": 0.0, "scale": [1.0, 1.0] },
            "Coin": { "value": 7, "weight": 0.0, "taken": 0 },
        }),
    ],
)"#;

fn write(dir: &std::path::Path, name: &str, text: &str) {
    std::fs::write(dir.join(format!("{name}.ron")), text).unwrap();
}

fn coin_of(e: &Engine, ent: Entity) -> (i32, f32) {
    let c = e.world().get::<&Coin>(ent).unwrap();
    (c.value, c.weight)
}

fn pos_of(e: &Engine, ent: Entity) -> Vec2 {
    e.world().get::<&Transform>(ent).unwrap().position
}

const LEVEL: &str = r#"Scene(
    format: 1,
    name: "level",
    entities: [
        (prefab: Some("Coin"), overrides: { "Transform": { "position": [3.0, 4.0] }, "Coin": { "value": 10 } }),
        (prefab: Some("Coin"), name: Some("Gold"), overrides: { "Coin": { "value": 50, "weight": 2.0 } }),
        (components: { "Transform": { "position": [9.0, 9.0], "rotation": 0.0, "scale": [1.0, 1.0] } }),
    ],
)"#;

#[test]
fn one_prefab_placed_twice_with_different_overrides() {
    let dir = tmp("twice");
    write(&dir, "Coin", COIN);
    let mut e = engine(&dir);
    let ents = e.load_scene_str(LEVEL).expect("level loads");
    assert_eq!(ents.len(), 3);
    assert_eq!(coin_of(&e, ents[0]), (10, 0.5), "override value, prefab weight");
    assert_eq!(pos_of(&e, ents[0]), Vec2::new(3.0, 4.0));
    assert_eq!(coin_of(&e, ents[1]), (50, 2.0));
    assert_eq!(pos_of(&e, ents[1]), Vec2::new(0.0, 0.0), "no override: the prefab's position");
    assert_eq!(e.world().get::<&Name>(ents[0]).unwrap().0, "Coin");
    assert_eq!(e.world().get::<&Name>(ents[1]).unwrap().0, "Gold", "the entry's name overrides the root's");
    assert_eq!(pos_of(&e, ents[2]), Vec2::new(9.0, 9.0));
    let t = e.prefab_instance(ents[0]).expect("placed entities remember their prefab");
    assert_eq!((t.prefab(), t.local(), t.is_root()), ("Coin", 1, true));
    assert!(e.prefab_instance(ents[2]).is_none());
    let mut over = e.prefab_overrides(ents[0]);
    over.sort();
    assert_eq!(over, vec![("Coin".into(), "value".into()), ("Transform".into(), "position".into())]);
    assert_eq!(e.prefab_base_field(ents[0], "Coin", "value"), Some(SceneValue::Int(1)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_prefab_with_children_and_child_overrides() {
    let dir = tmp("tree");
    write(&dir, "Crate", CRATE);
    let mut e = engine(&dir);
    let s = r##"Scene(format: 1, name: "s", entities: [
        (prefab: Some("Crate"), overrides: { "Transform": { "position": [5.0, 5.0] }, "#2:Coin": { "value": 99 } }),
        (prefab: Some("Crate")),
    ])"##;
    let ents = e.load_scene_str(s).unwrap();
    assert_eq!(ents.len(), 4, "two entities per placement, root first");
    let (r1, l1, r2, l2) = (ents[0], ents[1], ents[2], ents[3]);
    assert_eq!(e.world().get::<&Parent>(l1).unwrap().entity(), Some(r1));
    assert_eq!(e.world().get::<&Parent>(l2).unwrap().entity(), Some(r2), "each copy has its own parent link");
    assert_eq!(coin_of(&e, l1).0, 99);
    assert_eq!(coin_of(&e, l2).0, 7);
    assert_eq!(e.prefab_instance_root(l1), Some(r1));
    assert_eq!(e.prefab_instance_entities(r2), vec![r2, l2]);
    assert_eq!(e.prefab_instance(l1).unwrap().local(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_overrides_are_errors_that_name_scene_entry_and_field() {
    let dir = tmp("bad");
    write(&dir, "Coin", COIN);
    let mut e = engine(&dir);
    let load = |e: &mut Engine, ov: &str| {
        e.load_scene_str(&format!(r#"Scene(format: 1, name: "lvl", entities: [
            (components: {{ "Transform": {{ "position": [0.0, 0.0], "rotation": 0.0, "scale": [1.0, 1.0] }} }}),
            (prefab: Some("Coin"), overrides: {{ {ov} }}),
        ])"#))
    };
    let before = e.world().len();
    match load(&mut e, r#""Coin": { "nope": 1 }"#) {
        Err(SceneError::Field { scene, entity, component, field, .. }) => assert_eq!((scene.as_str(), entity, component.as_str(), field.as_str()), ("lvl", 1, "Coin", "nope")),
        other => panic!("{other:?}"),
    }
    match load(&mut e, r#""Coin": { "value": 99999999999 }"#) {
        Err(SceneError::Field { entity, field, .. }) => assert_eq!((entity, field.as_str()), (1, "value")),
        other => panic!("{other:?}"),
    }
    match load(&mut e, r#""Mystery": { "x": 1 }"#) {
        Err(SceneError::UnknownComponent { scene, entity, component }) => assert_eq!((scene.as_str(), entity, component.as_str()), ("lvl", 1, "Mystery")),
        other => panic!("{other:?}"),
    }
    let err = e.load_scene_str(r#"Scene(format: 1, name: "lvl", entities: [(prefab: Some("Ghost"))])"#).unwrap_err();
    assert!(matches!(&err, SceneError::Prefab { entity: 0, prefab, .. } if prefab == "Ghost"), "{err}");
    let err = e.load_scene_str(r#"Scene(format: 1, name: "lvl", entities: [(overrides: { "Coin": { "value": 1 } })])"#).unwrap_err();
    assert!(err.to_string().contains("needs a `prefab`"), "{err}");
    assert_eq!(e.world().len(), before, "a bad scene changes nothing");
    assert!(e.current_scene().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_broken_prefab_file_is_reported_with_its_path() {
    let dir = tmp("broken");
    write(&dir, "Coin", &COIN.replace("\"value\": 1", "\"value\": 1, \"bogus\": 2"));
    let mut e = engine(&dir);
    let err = e.load_scene_str(r#"Scene(format: 1, name: "lvl", entities: [(prefab: Some("Coin"))])"#).unwrap_err();
    assert!(matches!(&err, SceneError::Prefab { .. }) && err.to_string().contains("bogus"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn editing_the_prefab_changes_every_placement_and_overrides_stay() {
    let dir = tmp("propagate");
    write(&dir, "Coin", COIN);
    let mut e = engine(&dir);
    let scene = Scene::from_ron(LEVEL).unwrap();
    e.load_scene(&scene).unwrap();
    write(&dir, "Coin", &COIN.replace("\"weight\": 0.5", "\"weight\": 0.75").replace("\"value\": 1,", "\"value\": 2,"));
    let mut e2 = engine(&dir);
    let ents = e2.load_scene(&scene).unwrap();
    assert_eq!(coin_of(&e2, ents[0]), (10, 0.75), "value stays overridden, weight follows the prefab");
    assert_eq!(coin_of(&e2, ents[1]), (50, 2.0));
    // unlike a placement with no override of `value`
    let s = e2.load_scene_str(r#"Scene(format: 1, name: "x", entities: [(prefab: Some("Coin"))])"#).unwrap();
    assert_eq!(coin_of(&e2, s[0]), (2, 0.75));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_writes_the_prefab_reference_and_only_the_overridden_fields() {
    let dir = tmp("save");
    write(&dir, "Coin", COIN);
    write(&dir, "Crate", CRATE);
    let mut e = engine(&dir);
    let s = r##"Scene(format: 1, name: "lvl", entities: [
        (prefab: Some("Coin"), overrides: { "Transform": { "position": [3.0, 4.0] } }),
        (prefab: Some("Crate"), overrides: { "#2:Coin": { "value": 99 } }),
        (components: { "Transform": { "position": [9.0, 9.0], "rotation": 0.0, "scale": [1.0, 1.0] } }),
    ])"##;
    e.load_scene_str(s).unwrap();
    // an edit in the live world is an override too
    let coin = e.world().query::<(Entity, &Coin)>().iter().find(|(_, c)| c.value == 1).map(|(en, _)| en).unwrap();
    e.set_component_field(coin, "Coin", "weight", SceneValue::Float(3.0)).unwrap();
    let saved = e.capture_world_scene("lvl");
    assert_eq!(saved.entities.len(), 3, "a placement is one entry: {}", saved.to_ron());
    let placed: Vec<_> = saved.entities.iter().filter(|x| x.prefab.is_some()).collect();
    assert_eq!(placed.len(), 2);
    for p in &placed {
        assert!(p.components.is_empty(), "no full data for a placement");
    }
    let coin_entry = placed.iter().find(|p| p.prefab.as_deref() == Some("Coin")).unwrap();
    assert_eq!(coin_entry.overrides.keys().collect::<Vec<_>>(), ["Coin", "Transform"]);
    assert_eq!(coin_entry.overrides["Coin"].keys().collect::<Vec<_>>(), ["weight"]);
    assert_eq!(coin_entry.overrides["Transform"].keys().collect::<Vec<_>>(), ["position"]);
    let crate_entry = placed.iter().find(|p| p.prefab.as_deref() == Some("Crate")).unwrap();
    assert_eq!(crate_entry.overrides.keys().collect::<Vec<_>>(), ["#2:Coin"]);
    assert_eq!(crate_entry.overrides["#2:Coin"].keys().collect::<Vec<_>>(), ["value"]);
    // and the file reloads to the same world
    let text = saved.to_ron();
    let mut e2 = engine(&dir);
    e2.load_scene_str(&text).unwrap();
    assert_eq!(e2.capture_world_scene("lvl").to_ron(), text);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_incomplete_instance_is_saved_as_plain_entities() {
    let dir = tmp("incomplete");
    write(&dir, "Crate", CRATE);
    let mut e = engine(&dir);
    let ents = e.load_scene_str(r#"Scene(format: 1, name: "lvl", entities: [(prefab: Some("Crate"))])"#).unwrap();
    assert_eq!(e.capture_world_scene("lvl").entities.iter().filter(|x| x.prefab.is_some()).count(), 1);
    e.world_mut().despawn(ents[1]).unwrap();
    let saved = e.capture_world_scene("lvl");
    assert!(saved.entities.iter().all(|x| x.prefab.is_none()), "{}", saved.to_ron());
    assert_eq!(saved.entities.len(), 1);
    // unpacking one entity group
    let mut e = engine(&dir);
    let ents = e.load_scene_str(r#"Scene(format: 1, name: "lvl", entities: [(prefab: Some("Crate"))])"#).unwrap();
    let removed = e.unpack_prefab(ents[1]);
    assert_eq!(removed.len(), 2, "unpack takes the whole instance");
    assert!(e.prefab_instance(ents[0]).is_none());
    let saved = e.capture_world_scene("lvl");
    assert_eq!(saved.entities.len(), 2);
    assert!(saved.entities.iter().all(|x| x.prefab.is_none()));
    for (en, t) in removed {
        e.set_prefab_instance(en, Some(t));
    }
    assert_eq!(e.capture_world_scene("lvl").entities.len(), 1, "linked again");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_as_prefab_place_and_apply() {
    let dir = tmp("apply");
    let mut e = engine(&dir);
    // build a small tree by hand
    let root = e.new_editor_entity();
    let child = e.new_editor_entity();
    e.world_mut().insert_one(child, Parent::new(root)).unwrap();
    e.add_component_default(child, "Coin").unwrap();
    e.set_component_field(child, "Coin", "value", SceneValue::Int(4)).unwrap();
    e.set_entity_name(root, "Box");
    let (prefab, tree) = e.capture_prefab(root, "Box").unwrap();
    assert_eq!(tree, vec![root, child]);
    assert_eq!(prefab.entities.len(), 2);
    assert!(prefab.entities.iter().all(|x| !x.components.contains_key("EditorId")), "no editor ids in prefabs");
    assert!(!prefab.entities[0].components.contains_key("Parent"));
    e.save_prefab(&prefab).unwrap();
    assert!(dir.join("Box.ron").exists());
    assert_eq!(e.prefab_names(), vec!["Box".to_string()]);
    e.link_prefab(&tree, &prefab).unwrap();
    assert!(e.prefab_overrides(child).is_empty());
    // the file reads back to the same prefab
    assert_eq!(e.load_prefab("Box").unwrap(), prefab);
    // two more copies
    let a = e.place_prefab("Box").unwrap();
    let b = e.place_prefab("Box").unwrap();
    assert_eq!((a.len(), b.len()), (2, 2));
    assert!(a.iter().all(|x| e.world().get::<&EditorId>(*x).is_ok()), "placed entities get editor ids");
    // one copy overrides the child's value
    e.set_component_field(b[1], "Coin", "value", SceneValue::Int(77)).unwrap();
    assert_eq!(e.prefab_overrides(b[1]), vec![("Coin".to_string(), "value".to_string())]);
    // apply from the first copy after changing its child
    e.set_component_field(child, "Coin", "value", SceneValue::Int(5)).unwrap();
    e.set_component_field(child, "Coin", "weight", SceneValue::Float(1.5)).unwrap();
    let (changed, tree2) = e.capture_prefab(root, "Box").unwrap();
    e.save_prefab(&changed).unwrap();
    let rep = e.apply_prefab_to_world(&changed, &tree2).unwrap();
    assert_eq!(rep.stale, 0);
    assert_eq!(coin_of(&e, a[1]), (5, 1.5), "an untouched copy follows");
    assert_eq!(coin_of(&e, b[1]), (77, 1.5), "an overridden field stays, the other follows");
    assert!(e.prefab_overrides(child).is_empty(), "the source has no overrides left");
    // undo data: values come back
    for (en, comp, f, old) in rep.values.iter().rev() {
        e.set_component_field(*en, comp, f, old.clone()).unwrap();
    }
    for (en, tag) in rep.tags.iter().rev() {
        e.set_prefab_instance(*en, tag.clone());
    }
    assert_eq!(coin_of(&e, a[1]), (4, 0.0), "undo data restores the old values");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshots_keep_prefab_links() {
    let dir = tmp("snap");
    write(&dir, "Crate", CRATE);
    let mut e = engine(&dir);
    let ents = e.load_scene_str(r#"Scene(format: 1, name: "lvl", entities: [(prefab: Some("Crate"), overrides: { "Transform": { "position": [2.0, 2.0] } })])"#).unwrap();
    let snap = e.snapshot();
    e.world_mut().despawn(ents[0]).unwrap();
    e.restore(&snap).unwrap();
    let root = e.world().query::<(Entity, &PrefabInstance)>().iter().find(|(_, t)| t.is_root()).map(|(en, _)| en).expect("the link survived");
    assert_eq!(e.prefab_instance_entities(root).len(), 2);
    assert_eq!(e.prefab_overrides(root), vec![("Transform".to_string(), "position".to_string())]);
    // a restart snapshot too (through text)
    let rs = e.restart_snapshot();
    let text = rs.to_ron();
    let mut e2 = engine(&dir);
    e2.restore_lenient(&efeengine_core::ecs::RestartSnapshot::from_ron(&text).unwrap()).unwrap();
    let root2 = e2.world().query::<(Entity, &PrefabInstance)>().iter().find(|(_, t)| t.is_root()).map(|(en, _)| en).unwrap();
    assert_eq!(e2.prefab_instance_entities(root2).len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registered_prefabs_work_without_files() {
    let mut e = Engine::with_config(EngineConfig::headless()).unwrap();
    e.register::<Coin>().unwrap();
    e.register_prefab(Prefab::from_ron(COIN).unwrap()).unwrap();
    assert!(e.prefab_dir().is_none());
    assert_eq!(e.prefab_names(), vec!["Coin".to_string()]);
    let ents = e.load_scene_str(r#"Scene(format: 1, name: "s", entities: [(prefab: Some("Coin"))])"#).unwrap();
    assert_eq!(coin_of(&e, ents[0]), (1, 0.5));
    assert!(e.register_prefab(Prefab::from_ron(&COIN.replace("\"Coin\": {", "\"Nope\": {")).unwrap()).is_err());
}

#[test]
fn scene_system_list_is_editable_and_saved() {
    let mut e = Engine::with_config(EngineConfig::headless()).unwrap();
    e.add_system(Stage::Update, |_| {}).name("coin_collect");
    e.add_system(Stage::Update, |_| {}).name("hud");
    e.add_system(Stage::Update, |_| {}).name("spawner");
    e.set_global_system("hud").unwrap();
    assert_eq!(e.set_scene_system_entries(vec![], false).unwrap_err().to_string(), "no scene is loaded");
    e.load_scene_str(r#"Scene(format: 1, name: "s", requires: [], systems: ["coin_collect"], entities: [])"#).unwrap();
    let rows = e.registered_systems();
    let find = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
    assert!(find("coin_collect").in_scene && !find("spawner").in_scene);
    assert!(find("hud").global);
    assert_eq!(e.scene_entries_with("spawner", true), vec!["coin_collect".to_string(), "spawner".to_string()]);
    assert!(e.set_scene_system_entries(vec!["nope".into()], false).is_err());
    assert_eq!(e.scene_system_entries(), vec!["coin_collect".to_string()], "a bad list changes nothing");
    // list only: flags untouched
    let en = e.is_system_enabled("spawner").unwrap();
    e.set_scene_system_entries(e.scene_entries_with("spawner", true), false).unwrap();
    assert_eq!(e.is_system_enabled("spawner"), Some(en));
    assert_eq!(e.scene_systems(), vec!["coin_collect".to_string(), "spawner".to_string()]);
    // with flags: unlisting disables
    e.set_scene_system_entries(e.scene_entries_with("coin_collect", false), true).unwrap();
    assert_eq!(e.is_system_enabled("coin_collect"), Some(false));
    // the saved scene carries the list (and `requires` survives)
    let saved = e.capture_world_scene("s");
    assert_eq!(saved.systems, vec!["spawner".to_string()]);
    let again = Scene::from_ron(&saved.to_ron()).unwrap();
    assert_eq!(again.systems, saved.systems);
}

#[test]
fn unlisting_a_system_covered_by_a_plugin_entry_expands_the_entry() {
    struct P;
    impl Plugin for P {
        fn name(&self) -> &str {
            "pp"
        }
        fn build(&self, e: &mut Engine) {
            e.add_system(Stage::Update, |_| {}).name("pa");
            e.add_system(Stage::Update, |_| {}).name("pb");
        }
    }
    let mut e = Engine::with_config(EngineConfig::headless()).unwrap();
    e.add_plugin(P).unwrap();
    e.load_scene_str(r#"Scene(format: 1, name: "s", systems: ["plugin:pp"], entities: [])"#).unwrap();
    assert_eq!(e.scene_systems(), vec!["pa".to_string(), "pb".to_string()]);
    assert_eq!(e.scene_entries_with("pa", false), vec!["pb".to_string()]);
    assert_eq!(e.scene_entries_with("pb", true), vec!["plugin:pp".to_string()], "already covered");
    e.set_scene_system_entries(e.scene_entries_with("pa", false), false).unwrap();
    assert_eq!(e.capture_world_scene("s").systems, vec!["pb".to_string()]);
}
