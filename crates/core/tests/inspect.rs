//! Reflection for tools: entity rows, inspected components, field writes (docs/IN_GAME_EDITOR.md).

use efeengine_core::ecs::{SceneError, SceneValue};
use efeengine_core::engine::inspect::{EntityRow, InspectedComponent};
use efeengine_core::prelude::*;

#[script_component(name = "Coin")]
#[derive(Default, Debug)]
struct Coin {
    value: i32,
    ratio: f32,
}

#[script_component(name = "Flag")]
#[derive(Default, Debug)]
struct Flag {
    on: ScriptBool,
}

fn engine() -> Engine {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
    e.register::<Coin>().unwrap();
    e.register::<Flag>().unwrap();
    e
}

fn tr(x: f32, y: f32) -> Transform {
    Transform { position: Vec2::new(x, y), ..Default::default() }
}

fn comp<'a>(c: &'a [InspectedComponent], name: &str) -> &'a InspectedComponent {
    c.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no component `{name}`"))
}

fn names(rows: &[EntityRow]) -> Vec<&str> {
    rows.iter().map(|r| r.name.as_str()).collect()
}

#[test]
fn inspect_lists_registry_components_with_fields_in_declared_order() {
    let mut e = engine();
    let a = e.world_mut().spawn((Name::new("hero"), tr(1.5, 2.0), Coin { value: 7, ratio: 0.5 }, EditorId { id: 9 }));
    let comps = e.inspect_entity(a).expect("alive");
    let names: Vec<&str> = comps.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"Transform") && names.contains(&"Coin") && names.contains(&"EditorId"));
    assert!(!names.contains(&"Name"), "Rust-only components are not fields: {names:?}");
    let t = comp(&comps, "Transform");
    assert!(!t.readonly);
    let fields: Vec<&str> = t.fields.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(fields, ["position", "rotation", "scale"], "declared order, not alphabetical");
    assert_eq!(t.fields[0].1, SceneValue::List(vec![SceneValue::Float(1.5), SceneValue::Float(2.0)]));
    let coin = comp(&comps, "Coin");
    assert_eq!(coin.fields[0], ("value".to_string(), SceneValue::Int(7)));
    assert_eq!(coin.fields[1], ("ratio".to_string(), SceneValue::Float(0.5)));
    assert_eq!(comp(&comps, "EditorId").fields[0].1, SceneValue::UInt(9));
}

#[test]
fn a_dead_entity_has_nothing_to_inspect() {
    let mut e = engine();
    let a = e.world_mut().spawn((Coin::default(),));
    e.world_mut().despawn(a).unwrap();
    assert!(e.inspect_entity(a).is_none());
}

#[test]
fn readonly_components_are_flagged() {
    let mut e = engine();
    let a = e.world_mut().spawn((tr(0.0, 0.0), GlobalTransform::__identity_for_tests()));
    let comps = e.inspect_entity(a).unwrap();
    assert!(comp(&comps, "GlobalTransform").readonly);
    assert!(!comp(&comps, "Transform").readonly);
    let err = e.set_component_field(a, "GlobalTransform", "translation", SceneValue::List(vec![SceneValue::Float(1.0), SceneValue::Float(1.0)]));
    assert!(matches!(err, Err(SceneError::Field { .. })), "{err:?}");
}

#[test]
fn rows_are_a_stable_tree_with_names() {
    let mut e = engine();
    let root = e.world_mut().spawn((Name::new("root"), tr(0.0, 0.0), EditorId { id: 2 }));
    let child_b = e.world_mut().spawn((Name::new("child_b"), tr(1.0, 0.0), Parent::new(root), EditorId { id: 30 }));
    let child_a = e.world_mut().spawn((Name::new("child_a"), tr(2.0, 0.0), Parent::new(root), EditorId { id: 20 }));
    let grand = e.world_mut().spawn((tr(3.0, 0.0), Parent::new(child_a)));
    let first = e.world_mut().spawn((Name::new("first"), tr(0.0, 0.0), EditorId { id: 1 }));
    let noid = e.world_mut().spawn((Coin::default(),));
    // not editable: no registered component
    e.world_mut().spawn((Name::new("hidden"),));

    let rows = e.editable_entities();
    assert_eq!(rows.len(), 6);
    // roots by EditorId (no id last), children after their parent, children by EditorId
    let order: Vec<Entity> = rows.iter().map(|r| r.entity).collect();
    assert_eq!(order, [first, root, child_a, grand, child_b, noid]);
    let depth: Vec<usize> = rows.iter().map(|r| r.depth).collect();
    assert_eq!(depth, [0, 0, 1, 2, 1, 0]);
    assert_eq!(rows[1].children, [child_a, child_b]);
    assert_eq!(rows[2].parent, Some(root));
    assert_eq!(rows[1].editor_id, Some(2));
    assert_eq!(rows[5].editor_id, None);
    // names: Name, else the first registry component with the entity index, else Entity(n)
    assert_eq!(rows[0].name, "first");
    assert!(rows[3].name.starts_with("Transform #"), "{}", rows[3].name);
    assert!(rows[5].name.starts_with("Coin #"), "{}", rows[5].name);
    assert!(!names(&rows).contains(&"hidden"));
    // stable between calls
    assert_eq!(e.editable_entities(), rows);
}

#[test]
fn a_missing_parent_makes_a_root_and_a_cycle_loses_nothing() {
    let mut e = engine();
    let gone = e.world_mut().spawn((Coin::default(),));
    let orphan = e.world_mut().spawn((Name::new("orphan"), tr(0.0, 0.0), Parent::new(gone)));
    e.world_mut().despawn(gone).unwrap();
    let a = e.world_mut().spawn((Name::new("a"), tr(0.0, 0.0), EditorId { id: 1 }));
    let b = e.world_mut().spawn((Name::new("b"), tr(0.0, 0.0), EditorId { id: 2 }, Parent::new(a)));
    e.world_mut().insert_one(a, Parent::new(b)).unwrap();
    let rows = e.editable_entities();
    assert_eq!(rows.len(), 3, "nothing is hidden");
    assert_eq!(rows.iter().find(|r| r.entity == orphan).unwrap().parent, None);
}

#[test]
fn scene_owned_is_reported() {
    let mut e = engine();
    let o = e.spawn_scene_owned();
    let plain = e.world_mut().spawn((Coin::default(),));
    let rows = e.editable_entities();
    assert!(rows.iter().find(|r| r.entity == o).unwrap().scene_owned);
    assert!(!rows.iter().find(|r| r.entity == plain).unwrap().scene_owned);
}

#[test]
fn set_component_field_writes_through_the_codec() {
    let mut e = engine();
    let a = e.world_mut().spawn((tr(1.0, 2.0), Coin { value: 1, ratio: 0.0 }, Flag::default()));
    e.set_component_field(a, "Coin", "value", SceneValue::Int(42)).unwrap();
    e.set_component_field(a, "Flag", "on", SceneValue::Bool(true)).unwrap();
    assert_eq!(e.world().get::<&Coin>(a).unwrap().value, 42);
    assert!(e.world().get::<&Flag>(a).unwrap().on.get());
    // a list field; the other fields stay
    e.set_component_field(a, "Transform", "position", SceneValue::List(vec![SceneValue::Float(5.0), SceneValue::Float(-1.0)])).unwrap();
    assert_eq!(e.world().get::<&Transform>(a).unwrap().position, Vec2::new(5.0, -1.0));
    assert_eq!(e.world().get::<&Transform>(a).unwrap().scale, Vec2::ONE);
    // what was written is what inspect reads
    let comps = e.inspect_entity(a).unwrap();
    assert_eq!(comp(&comps, "Coin").fields[0].1, SceneValue::Int(42));
}

#[test]
fn set_component_field_errors_change_nothing() {
    let mut e = engine();
    let a = e.world_mut().spawn((Coin { value: 3, ratio: 0.0 },));
    let before = e.snapshot().scene.to_ron();
    assert!(matches!(e.set_component_field(a, "Nope", "x", SceneValue::Int(1)), Err(SceneError::UnknownComponent { .. })));
    assert!(matches!(e.set_component_field(a, "Transform", "position", SceneValue::Int(1)), Err(SceneError::Insert { .. })), "the entity lacks it");
    assert!(matches!(e.set_component_field(a, "Coin", "nope", SceneValue::Int(1)), Err(SceneError::Field { .. })));
    // wrong shape and out of range
    assert!(matches!(e.set_component_field(a, "Coin", "value", SceneValue::List(vec![])), Err(SceneError::Field { .. })));
    assert!(matches!(e.set_component_field(a, "Coin", "value", SceneValue::Int(i64::MAX)), Err(SceneError::Field { .. })));
    let dead = e.world_mut().spawn((Coin::default(),));
    e.world_mut().despawn(dead).unwrap();
    assert!(e.set_component_field(dead, "Coin", "value", SceneValue::Int(1)).is_err());
    assert_eq!(e.snapshot().scene.to_ron(), before);
}

// ----- step 2: add / remove component, entity actions ----------------------------------------

use efeengine_core::engine::inspect::EntityData;

fn ron(e: &Engine) -> String {
    e.snapshot().scene.to_ron()
}

#[test]
fn entity_fields_are_flagged() {
    let mut e = engine();
    let root = e.world_mut().spawn((tr(0.0, 0.0),));
    let kid = e.world_mut().spawn((tr(0.0, 0.0), Parent::new(root)));
    let comps = e.inspect_entity(kid).unwrap();
    assert_eq!(comp(&comps, "Parent").entity_fields, ["entity"]);
    assert!(comp(&comps, "Transform").entity_fields.is_empty());
}

#[test]
fn add_component_default_uses_defaults_for_engine_types_and_zeros_for_the_rest() {
    let mut e = engine();
    let a = e.world_mut().spawn((EditorId { id: 1 },));
    assert!(e.addable_components(a).contains(&"Transform".to_string()));
    assert!(!e.addable_components(a).contains(&"EditorId".to_string()), "EditorId is managed");
    assert!(!e.addable_components(a).contains(&"GlobalTransform".to_string()), "read-only is never offered");
    e.add_component_default(a, "Transform").unwrap();
    assert_eq!(e.world().get::<&Transform>(a).unwrap().scale, Vec2::ONE, "Default, not zeros");
    e.add_component_default(a, "Coin").unwrap();
    assert_eq!(e.world().get::<&Coin>(a).unwrap().value, 0);
    e.add_component_default(a, "Sprite").unwrap();
    assert_eq!(e.world().get::<&Sprite>(a).unwrap().color, [1.0; 4]);
    assert!(!e.addable_components(a).contains(&"Coin".to_string()), "now present");
    // errors change nothing
    let before = ron(&e);
    assert!(matches!(e.add_component_default(a, "Coin"), Err(SceneError::Insert { .. })), "already there");
    assert!(matches!(e.add_component_default(a, "Nope"), Err(SceneError::UnknownComponent { .. })));
    assert!(e.add_component_default(a, "GlobalTransform").is_err(), "read-only");
    let dead = e.world_mut().spawn((Coin::default(),));
    e.world_mut().despawn(dead).unwrap();
    assert!(e.add_component_default(dead, "Flag").is_err());
    assert!(e.addable_components(dead).is_empty());
    assert_eq!(ron(&e), before);
}

#[test]
fn remove_component_removes_and_refuses_readonly_and_missing() {
    let mut e = engine();
    let a = e.world_mut().spawn((tr(1.0, 2.0), Coin::default(), GlobalTransform::__identity_for_tests()));
    e.remove_component(a, "Coin").unwrap();
    assert!(e.world().get::<&Coin>(a).is_err());
    assert!(e.world().get::<&Transform>(a).is_ok(), "the others stay");
    assert!(e.remove_component(a, "Coin").is_err(), "already gone");
    assert!(e.remove_component(a, "GlobalTransform").is_err(), "read-only");
    assert!(e.world().get::<&GlobalTransform>(a).is_ok());
    assert!(matches!(e.remove_component(a, "Nope"), Err(SceneError::UnknownComponent { .. })));
}

#[test]
fn editor_ids_are_allocated_above_the_largest() {
    let mut e = engine();
    assert_eq!(e.next_editor_id(), 1);
    let a = e.world_mut().spawn((tr(0.0, 0.0), EditorId { id: 7 }));
    let b = e.world_mut().spawn((tr(0.0, 0.0),));
    assert_eq!(e.next_editor_id(), 8);
    assert_eq!(e.ensure_editor_id(a), Some(7), "an existing id is kept");
    assert_eq!(e.ensure_editor_id(b), Some(8));
    assert_eq!(e.world().get::<&EditorId>(b).unwrap().id, 8);
    assert_eq!(e.next_editor_id(), 9);
}

#[test]
fn a_new_entity_has_transform_id_and_name_and_follows_the_scene() {
    let mut e = engine();
    e.world_mut().spawn((tr(0.0, 0.0), EditorId { id: 4 }));
    let n = e.new_editor_entity();
    assert_eq!(e.world().get::<&EditorId>(n).unwrap().id, 5);
    assert_eq!(e.world().get::<&Name>(n).unwrap().0, "entity 5");
    assert!(e.world().get::<&Transform>(n).is_ok());
    assert!(!e.is_scene_owned(n), "no scene loaded: a free entity");
    assert!(e.editable_entities().iter().any(|r| r.entity == n));

    e.load_scene_str("(format: 1, name: \"s\", entities: [])").unwrap();
    let m = e.new_editor_entity();
    assert!(e.is_scene_owned(m), "scene loaded: owned by it");
    assert_eq!(e.world().get::<&EditorId>(m).unwrap().id, 6);
}

#[test]
fn duplicate_copies_components_with_a_new_id() {
    let mut e = engine();
    let root = e.world_mut().spawn((tr(0.0, 0.0), EditorId { id: 1 }));
    let a = e.world_mut().spawn((Name::new("a"), tr(3.0, 4.0), Coin { value: 9, ratio: 0.5 }, EditorId { id: 2 }, Parent::new(root)));
    let d = e.duplicate_entity(a).unwrap();
    assert_ne!(d, a);
    assert_eq!(e.world().get::<&EditorId>(d).unwrap().id, 3);
    assert_eq!(e.world().get::<&Name>(d).unwrap().0, "a copy");
    assert_eq!(e.world().get::<&Coin>(d).unwrap().value, 9);
    assert_eq!(e.world().get::<&Transform>(d).unwrap().position, Vec2::new(3.0, 4.0));
    assert_eq!(e.world().get::<&Parent>(d).unwrap().entity(), Some(root), "a sibling");
    // the original is untouched
    assert_eq!(e.world().get::<&EditorId>(a).unwrap().id, 2);
    let dead = e.world_mut().spawn((Coin::default(),));
    e.world_mut().despawn(dead).unwrap();
    assert!(e.duplicate_entity(dead).is_err());
}

#[test]
fn delete_takes_the_children_and_spawn_entity_data_brings_the_tree_back() {
    let mut e = engine();
    let root = e.world_mut().spawn((Name::new("root"), tr(0.0, 0.0), EditorId { id: 1 }));
    let kid = e.world_mut().spawn((Name::new("kid"), tr(1.0, 0.0), EditorId { id: 2 }, Parent::new(root)));
    let _grand = e.world_mut().spawn((Name::new("grand"), tr(2.0, 0.0), EditorId { id: 3 }, Parent::new(kid)));
    let other = e.world_mut().spawn((Name::new("other"), tr(5.0, 0.0), EditorId { id: 4 }));
    let before = e.editable_entities().iter().map(|r| (r.name.clone(), r.editor_id, r.depth)).collect::<Vec<_>>();
    let comps_before = e.inspect_entity(kid).unwrap();

    assert_eq!(e.entity_tree(root).len(), 3);
    let data = e.delete_entity_tree(root);
    assert_eq!(data.len(), 3);
    assert_eq!(data[0].editor_id(), Some(1));
    assert_eq!(e.editable_entities().len(), 1, "only `other` is left");
    assert!(e.world().contains(other));

    let back = e.spawn_entity_data(&data).unwrap();
    assert_eq!(back.len(), 3);
    let after = e.editable_entities().iter().map(|r| (r.name.clone(), r.editor_id, r.depth)).collect::<Vec<_>>();
    assert_eq!(after, before, "names, ids and the tree shape are back");
    let kid2 = e.entity_by_editor_id(2).unwrap();
    assert_eq!(e.world().get::<&Parent>(kid2).unwrap().entity(), e.entity_by_editor_id(1), "Parent follows the respawned parent");
    let comps_after = e.inspect_entity(kid2).unwrap();
    // everything but the (new) parent bits is the same
    for (a, b) in comps_before.iter().zip(&comps_after) {
        if a.name != "Parent" {
            assert_eq!(a, b);
        }
    }
}

#[test]
fn spawn_entity_data_refuses_a_used_editor_id_and_keeps_the_owner() {
    let mut e = engine();
    e.load_scene_str("(format: 1, name: \"s\", entities: [])").unwrap();
    let n = e.new_editor_entity();
    assert!(e.is_scene_owned(n));
    let data = e.delete_entity_tree(n);
    let back = e.spawn_entity_data(&data).unwrap();
    assert!(e.is_scene_owned(back[0]), "the owner tag is part of the copy");
    let before = ron(&e);
    assert!(matches!(e.spawn_entity_data(&data), Err(SceneError::DuplicateEditorId { .. })), "the id is in use");
    assert_eq!(ron(&e), before, "nothing spawned");
}

#[test]
fn entity_data_roundtrips_a_copy_with_a_new_id() {
    let mut e = engine();
    let a = e.world_mut().spawn((tr(1.0, 1.0), EditorId { id: 1 }));
    let mut d: EntityData = e.capture_entity_data(a).unwrap();
    d.set_editor_id(50);
    assert_eq!(d.editor_id(), Some(50));
    let n = e.spawn_entity_data(&[d]).unwrap()[0];
    assert_eq!(e.world().get::<&EditorId>(n).unwrap().id, 50);
}

#[test]
fn clear_editable_world_unloads_the_scene_and_removes_every_entity() {
    let mut e = engine();
    e.load_scene_str("(format: 1, name: \"s\", entities: [(components: {\"Transform\": {\"position\": [1.0, 2.0], \"rotation\": 0.0, \"scale\": [1.0, 1.0]}})])").unwrap();
    e.world_mut().spawn((Name::new("free"), tr(0.0, 0.0)));
    let hidden = e.world_mut().spawn((Name::new("hidden"),));
    assert_eq!(e.editable_entities().len(), 2);
    e.clear_editable_world();
    assert!(e.editable_entities().is_empty());
    assert_eq!(e.current_scene(), None);
    assert!(e.world().contains(hidden), "entities a scene would not capture stay");
}

#[test]
fn a_world_scene_keeps_parents_and_unowned_entities_through_a_file() {
    let mut e = engine();
    let sun = e.world_mut().spawn((tr(-1.0, 0.0), EditorId { id: 2 }, Coin { value: 5, ratio: 1.5 }));
    e.world_mut().spawn((tr(9.0, 0.0), EditorId { id: 3 }, Parent::new(sun)));
    e.world_mut().spawn((tr(0.0, 0.0), EditorId { id: 1 }));
    let scene = e.capture_world_scene("w");
    let ids: Vec<Option<u64>> = scene.entities.iter().map(|s| s.id).collect();
    assert_eq!(ids, [Some(1), Some(2), Some(3)], "entities carry ids 1, 2, ... in file order");
    // Parent (EditorId 3) holds the file id of the sun (EditorId 2), not raw entity bits
    assert_eq!(scene.entities[2].components["Parent"]["entity"], SceneValue::UInt(2));
    // order by EditorId: 1, 2, 3
    assert_eq!(scene.entities[0].components["EditorId"]["id"], SceneValue::UInt(1));
    assert_eq!(scene.entities[2].components["EditorId"]["id"], SceneValue::UInt(3));
    let before = e.editable_entities().iter().map(|r| (r.editor_id, r.depth)).collect::<Vec<_>>();

    let path = std::env::temp_dir().join(format!("efe_inspect_world_{}.ron", std::process::id()));
    e.save_world_scene(&path).unwrap();
    e.clear_editable_world();
    let spawned = e.load_scene_file(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(spawned.len(), 3);
    let after = e.editable_entities().iter().map(|r| (r.editor_id, r.depth)).collect::<Vec<_>>();
    assert_eq!(after, before, "the parent link survived the file");
    assert_eq!(e.world().get::<&Coin>(e.entity_by_editor_id(2).unwrap()).unwrap().value, 5);
    assert!(e.is_scene_owned(e.entity_by_editor_id(3).unwrap()));
}
