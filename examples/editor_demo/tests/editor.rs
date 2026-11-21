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
