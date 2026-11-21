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
