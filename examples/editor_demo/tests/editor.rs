//! The demo with the Editor plugin, headless: modes freeze and restore the demo world.

use efeengine_core::prelude::*;
use efeengine_ui::editor::{EditorMode, EditorPanel, EditorSelection, EditorState};
use efeengine_ui::{DebugMenuPlugin, EditorPlugin, UiTest};

use editor_demo::Spin;

fn demo() -> UiTest {
    let mut t = UiTest::new(|e| {
        e.add_plugin(DebugMenuPlugin).expect("debug menu");
        e.add_plugin(EditorPlugin::new()).expect("editor");
        editor_demo::build_game(e);
    });
    t.step(3);
    t
}

fn rotation(t: &UiTest, name: &str) -> f32 {
    let w = t.engine.world();
    w.query::<(&Name, &Transform)>().iter().find(|(n, _)| n.0 == name).map(|(_, tr)| tr.rotation).expect(name)
}

fn press(t: &mut UiTest, key: &str) {
    t.click(&format!("debug_menu/editor/toolbar/{key}"));
    t.step(2);
}

#[test]
fn the_demo_is_frozen_in_edit_runs_in_play_and_stop_puts_it_back() {
    let mut t = demo();
    t.resource_mut::<efeengine_ui::UiViews>().set_enabled("debug_menu", true);
    t.step(2);
    t.click("debug_menu/tab_editor");
    t.step(2);
    assert_eq!(t.resource::<EditorState>().mode, EditorMode::Edit);
    t.step(30);
    assert_eq!(rotation(&t, "sun"), 0.0, "frozen in Edit");
    assert_eq!(t.engine.world().query::<&Spin>().iter().count(), 3);

    press(&mut t, "play");
    t.step(30);
    assert_ne!(rotation(&t, "sun"), 0.0, "the spin system runs in Play");

    press(&mut t, "stop");
    assert_eq!(t.resource::<EditorState>().mode, EditorMode::Edit);
    assert_eq!(rotation(&t, "sun"), 0.0, "Stop restored the world");
    t.step(30);
    assert_eq!(rotation(&t, "sun"), 0.0);

    // the hierarchy lists the demo, planet under sun
    let rows = t.resource::<EditorPanel>().rows.clone();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["camera", "sun", "planet", "crate", "gem"]);
    assert_eq!(rows[2].depth, 1);
    let _ = t.resource::<EditorSelection>();
}


fn bare() -> Engine {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("engine");
    editor_demo::register_game(&mut e);
    e
}

fn manifest(p: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(p)
}

#[test]
fn the_committed_scene_is_what_the_demo_world_saves() {
    let mut e = bare();
    editor_demo::spawn_demo_world(&mut e);
    let dir = std::env::temp_dir().join(format!("editor_demo_scene_{}", std::process::id()));
    let fresh = dir.join("demo.ron");
    std::fs::create_dir_all(&dir).unwrap();
    e.save_world_scene(&fresh).expect("save");
    let want = std::fs::read_to_string(&fresh).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    if std::env::var_os("EFE_BLESS_SCENES").is_some() {
        std::fs::create_dir_all(manifest("scenes")).unwrap();
        std::fs::write(manifest("scenes/demo.ron"), &want).unwrap();
    }
    let have = std::fs::read_to_string(manifest("scenes/demo.ron")).unwrap_or_default();
    assert_eq!(have, want, "scenes/demo.ron is out of date; EFE_BLESS_SCENES=1 rewrites it. Expected:\n{want}");
    assert!(!manifest("scenes/demo.ron.names").exists(), "names live in the scene file now, the sidecar is gone");
}

#[test]
fn the_demo_starts_from_the_scene_file() {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("engine");
    editor_demo::build_game(&mut e);
    assert!(editor_demo::scene_path().is_some(), "the demo scene is committed");
    let rows = e.editable_entities();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["camera", "sun", "planet", "crate", "gem"], "names come from the scene file");
    assert!(rows.iter().all(|r| r.scene_owned), "the file's entities belong to its scene");
    assert_eq!(rows[2].parent, Some(rows[1].entity), "planet is still under sun");
    assert_eq!(e.current_scene(), Some("demo"));
}

#[test]
fn an_edited_demo_saves_through_the_editor_and_loads_back() {
    let dir = std::env::temp_dir().join(format!("editor_demo_roundtrip_{}", std::process::id()));
    let path = dir.join("edited.ron").to_string_lossy().to_string();
    let mut t = demo();
    t.resource_mut::<efeengine_ui::UiViews>().set_enabled("debug_menu", true);
    t.step(2);
    t.click("debug_menu/tab_editor");
    t.step(3);
    let sun = t.engine.entity_by_editor_id(2).expect("sun");
    t.engine.set_component_field(sun, "Spin", "speed", efeengine_core::ecs::SceneValue::Float(2.5)).unwrap();
    t.set_text("debug_menu/editor/scene/path_row/path", &path);
    t.step(3);
    t.click("debug_menu/editor/scene/bar/save_as");
    t.step(4);
    assert!(std::path::Path::new(&path).exists(), "saved");

    let mut other = bare();
    let n = other.load_scene_file(&path).expect("load").len();
    assert_eq!(n, 5);
    let sun2 = other.entity_by_editor_id(2).unwrap();
    assert_eq!(other.world().get::<&Spin>(sun2).unwrap().speed, 2.5);
    assert_eq!(other.world().get::<&Name>(sun2).unwrap().0, "sun");
    std::fs::remove_dir_all(&dir).ok();
}
