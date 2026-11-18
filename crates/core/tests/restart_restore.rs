//! Seamless restart restore (docs/EDITOR.md E6.2): the lenient decoder. Headless.

use efeengine_core::ecs::{RestartSnapshot, SceneOwned};
use efeengine_core::prelude::*;

#[script_component(name = "Coin")]
#[derive(Default, Debug, Clone)]
struct Coin {
    value: i32,
    bonus: i32,
}

#[script_component(name = "Follow")]
#[derive(Default, Debug, Clone)]
struct Follow {
    #[script(entity)]
    target: u64,
}

fn engine() -> Engine {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
    e.register::<Coin>().unwrap();
    e.register::<Follow>().unwrap();
    e
}

fn snap(text: &str) -> RestartSnapshot {
    RestartSnapshot::from_ron(text).unwrap()
}

/// Written by an older build: `Coin` had `value` and `old_field`, no `bonus`; `Gone` is a
/// component the game no longer has; the camera-like entity has a Transform.
const OLD: &str = r#"RestartSnapshot(
    scene: Scene(format: 1, name: "level", entities: [
        (id: Some(10), components: {
            "Coin": { "value": 7, "old_field": 3 },
            "Gone": { "x": 1 },
        }),
        (id: Some(20), components: {
            "Transform": { "position": [12.5, -3.0], "rotation": 0.5, "scale": [2.0, 2.0] },
            "Follow": { "target": 10 },
        }),
    ]),
    owners: [Some((1, "level")), None],
)"#;

fn find<T: efeengine_core::ecs::Component + Clone>(e: &Engine) -> Vec<T> {
    e.world().query::<&T>().iter().map(|c| c.clone()).collect()
}

#[test]
fn missing_field_is_zero_unknown_dropped_component_skipped() {
    let mut e = engine();
    let report = e.restore_lenient(&snap(OLD)).unwrap();
    assert_eq!(report.entities, 2);
    let w = report.warnings.join("\n");
    assert!(w.contains("`old_field` no longer exists"), "{w}");
    assert!(w.contains("new field `bonus`"), "{w}");
    assert!(w.contains("`Gone`: no such component"), "{w}");
    let snapshot = e.restart_snapshot();
    let coin = snapshot.scene.entities.iter().find_map(|x| x.components.get("Coin")).unwrap();
    assert_eq!(format!("{:?}", coin["value"]), "Int(7)");
    assert_eq!(format!("{:?}", coin["bonus"]), "Int(0)");
}

#[test]
fn camera_like_entity_keeps_its_values_and_references_are_remapped() {
    let mut e = engine();
    e.restore_lenient(&snap(OLD)).unwrap();
    let t = e.world().query::<&Transform>().iter().next().map(|t| (t.position.x, t.position.y, t.rotation, t.scale.x)).unwrap();
    assert_eq!(t, (12.5, -3.0, 0.5, 2.0));
    let coin_entity = e.world().query::<(Entity, &Coin)>().iter().next().map(|(en, _)| en).unwrap();
    let follow = find::<Follow>(&e);
    assert_eq!(follow[0].target, efeengine_core::ecs::entity_to_bits(coin_entity));
}

#[test]
fn restore_replaces_the_current_world_and_reowns_the_active_scene() {
    let mut e = engine();
    e.load_scene_str(r#"Scene(format: 1, name: "level", entities: [(components: { "Coin": { "value": 99 } })])"#).unwrap();
    e.restore_lenient(&snap(OLD)).unwrap();
    // The freshly loaded entity is gone, the restored scene entity belongs to the active scene.
    assert_eq!(find::<Coin>(&e).len(), 1);
    assert_eq!(find::<Coin>(&e)[0].value, 7);
    let owned: Vec<String> = e.world().query::<&SceneOwned>().iter().map(|o| format!("{}#{}", o.scene_name(), o.scene_id())).collect();
    assert_eq!(owned, vec!["level#1".to_string()]);
}

#[test]
fn wrong_type_value_is_zero_not_an_error() {
    let mut e = engine();
    let text = r#"RestartSnapshot(scene: Scene(format: 1, name: "x", entities: [
        (components: { "Coin": { "value": [1, 2], "bonus": 5 } }),
    ]), owners: [None])"#;
    let report = e.restore_lenient(&snap(text)).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("`value` does not fit")), "{:?}", report.warnings);
    assert_eq!(find::<Coin>(&e)[0].bonus, 5);
}

#[test]
fn bad_format_fails_and_leaves_the_world() {
    let mut e = engine();
    e.restore_lenient(&snap(OLD)).unwrap();
    let bad = r#"RestartSnapshot(scene: Scene(format: 99, name: "x", entities: []), owners: [])"#;
    assert!(e.restore_lenient(&snap(bad)).is_err());
    assert_eq!(find::<Coin>(&e).len(), 1);
}

#[test]
fn strict_load_still_rejects_unknown_fields() {
    let mut e = engine();
    let err = e.load_scene_str(r#"Scene(format: 1, name: "s", entities: [(components: { "Coin": { "nope": 1 } })])"#);
    assert!(err.is_err());
}

#[test]
fn round_trips_through_ron() {
    let mut e = engine();
    e.restore_lenient(&snap(OLD)).unwrap();
    let s = e.restart_snapshot();
    assert_eq!(RestartSnapshot::from_ron(&s.to_ron()).unwrap().to_ron(), s.to_ron());
    let mut e2 = engine();
    e2.restore_lenient(&s).unwrap();
    assert_eq!(e2.restart_snapshot().scene.entities.len(), 2);
}
