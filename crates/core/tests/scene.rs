//! Scene files (docs/SCENES.md, phase S1): load, save, round trip, validation. Headless.

use efeengine_core::ecs::{Scene, SceneError};
use efeengine_core::prelude::*;

/// A coin.
#[script_component(name = "Coin")]
#[derive(Default, Debug)]
struct Coin {
    value: i32,
    taken: u32,
    weight: f32,
}

fn engine() -> Engine {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
    e.register::<Coin>().unwrap();
    e.add_system(Stage::Update, |_| {}).name("coin_collect");
    e.add_system(Stage::Update, |_| {}).name("hud");
    e
}

const SCENE: &str = r#"Scene(
    format: 1,
    name: "level_1",
    requires: [],
    systems: ["coin_collect", "hud"],
    entities: [
        (components: {
            "Transform": { "position": [1.5, -2.0], "rotation": 0.1, "scale": [1.0, 1.0] },
            "Coin": { "value": 5, "taken": 0, "weight": 0.25 },
        }),
        (components: {
            "Transform": { "position": [3.0, 4.0], "rotation": 0.0, "scale": [2.0, 2.0] },
        }),
    ],
)"#;

fn dump(e: &Engine) -> String {
    e.capture_scene("level_1").to_ron()
}

#[test]
fn scene_loads_saves_and_round_trips_byte_for_byte() {
    let mut a = engine();
    let ents = a.load_scene_str(SCENE).expect("scene loads");
    assert_eq!(ents.len(), 2);
    assert_eq!(a.world().get::<&Coin>(ents[0]).unwrap().value, 5);
    assert_eq!(a.world().get::<&Transform>(ents[1]).unwrap().scale, Vec2::new(2.0, 2.0));

    let dir = std::env::temp_dir().join(format!("efe_scene_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("level_1.ron");
    a.save_scene(&path).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();

    // Reload into a fresh engine, save again: identical bytes.
    let mut b = engine();
    b.load_scene_file(&path).expect("saved scene loads");
    let dir2 = dir.join("second");
    std::fs::create_dir_all(&dir2).unwrap();
    let path2 = dir2.join("level_1.ron");
    b.save_scene(&path2).unwrap();
    assert_eq!(first, std::fs::read_to_string(&path2).unwrap());
    assert_eq!(dump(&a), dump(&b));
    // Saved text parses back to the same value.
    let scene = Scene::from_ron(&first).unwrap();
    assert_eq!(scene.to_ron(), first);
    assert_eq!(scene.entities.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_files_fail_without_changing_the_world() {
    let mut e = engine();
    e.load_scene_str(SCENE).unwrap();
    let before = dump(&e);
    let count = e.world().len();

    let cases: [(&str, &str); 7] = [
        ("Scene(format: 1, name: \"x\"", "cannot parse"),
        ("Scene(format: 9, name: \"x\")", "format 9"),
        ("Scene(format: 1, name: \"x\", requires: [\"nope\"])", "requires `nope`"),
        ("Scene(format: 1, name: \"x\", systems: [\"ghost\"])", "unknown system `ghost`"),
        ("Scene(format: 1, name: \"x\", entities: [(components: {}), (components: {\"Nope\": {}})])", "entity 1: unknown component `Nope`"),
        (
            "Scene(format: 1, name: \"x\", entities: [(components: {\"Coin\": {\"value\": 1}}), (components: {\"Coin\": {\"valu\": 1}})])",
            "entity 1: `Coin`: field `valu`",
        ),
        ("Scene(format: 1, name: \"x\", entities: [(components: {\"Coin\": {\"value\": 1}}), (components: {\"Coin\": {\"value\": 5000000000}})])", "field `value`"),
    ];
    for (text, want) in cases {
        let err = e.load_scene_str(text).expect_err(text);
        assert!(err.to_string().contains(want), "`{err}` should contain `{want}`");
        assert_eq!(e.world().len(), count, "world changed by: {text}");
        assert_eq!(dump(&e), before);
    }
    assert!(matches!(e.load_scene_file("/no/such/scene.ron"), Err(SceneError::Io { .. })));
}
