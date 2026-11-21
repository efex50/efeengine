use super::panel::format_value;
use super::*;
use crate::UiViews;
use crate::debug_menu::{DebugMenuPlugin, VIEW_RUNS};
use crate::testing::UiTest;
use efeengine_core::ecs::{SceneValue, entity_to_bits};

struct Ticks(u32);
struct Kept(u32);
/// `DebugDraw2D::len()` as seen just before the render step empties it.
struct Drawn(usize);

fn game_tick(cx: &mut Ctx) {
    if let Some(t) = cx.resources.get_mut::<Ticks>() {
        t.0 += 1;
    }
    for t in cx.world.query_mut::<&mut Transform>() {
        t.position.x += 1.0;
    }
}

fn keep_running(cx: &mut Ctx) {
    if let Some(t) = cx.resources.get_mut::<Kept>() {
        t.0 += 1;
    }
}

fn watch_draw(cx: &mut Ctx) {
    let n = cx.resources.get::<DebugDraw2D>().map_or(0, |d| d.len());
    if let Some(d) = cx.resources.get_mut::<Drawn>() {
        d.0 = n;
    }
}

fn world_setup(e: &mut Engine) {
    e.insert_resource(Ticks(0));
    e.insert_resource(Kept(0));
    e.insert_resource(Drawn(0));
    e.add_system(Stage::Update, game_tick).name("game_tick");
    e.add_system(Stage::Update, keep_running).name("keep_running");
    e.set_global_system("keep_running").unwrap();
    e.add_system(Stage::RenderExtract, watch_draw).order(-10).name("watch_draw");
    let w = e.world_mut();
    w.spawn((Camera::orthographic_2d(), Transform::default()));
    let hero = w.spawn((Name::new("hero"), EditorId { id: 1 }, Transform::from_position(Vec2::new(1.5, 2.0)), Sprite::default().with_size(Vec2::new(10.0, 20.0))));
    w.spawn((Name::new("pet"), EditorId { id: 2 }, Transform::default(), Parent::new(hero)));
    w.spawn((Name::new("rock"), EditorId { id: 3 }, Transform::default()));
}

fn editor_with(plugin: EditorPlugin) -> UiTest {
    crate::debug_menu::serial();
    let mut t = UiTest::new(|e| {
        e.add_plugin(DebugMenuPlugin).expect("debug menu");
        e.add_plugin(plugin).expect("editor");
        world_setup(e);
    });
    t.resource_mut::<UiViews>().set_enabled("debug_menu", true);
    t.step(2);
    t
}

fn editor() -> UiTest {
    let mut t = editor_with(EditorPlugin::new());
    t.click("debug_menu/tab_editor");
    t.step(3);
    t
}

fn press(t: &mut UiTest, key: &str) {
    t.click(&format!("debug_menu/editor/toolbar/{key}"));
    t.step(2);
}

fn ticks(t: &UiTest) -> u32 {
    t.resource::<Ticks>().0
}

fn entity(t: &UiTest, name: &str) -> Entity {
    t.engine.world().query::<(Entity, &Name)>().iter().find(|(_, n)| n.0 == name).map(|(e, _)| e).unwrap_or_else(|| panic!("no entity `{name}`"))
}

fn x_of(t: &UiTest, name: &str) -> f32 {
    t.engine.world().get::<&Transform>(entity(t, name)).unwrap().position.x
}

fn row_key(t: &UiTest, name: &str) -> String {
    format!("debug_menu/editor/hierarchy/list/e{}/row", entity_to_bits(entity(t, name)))
}

fn mode(t: &UiTest) -> EditorMode {
    t.resource::<EditorState>().mode
}

#[test]
fn the_tab_exists_only_with_the_plugin() {
    crate::debug_menu::serial();
    let mut plain = UiTest::new(|e| {
        e.add_plugin(DebugMenuPlugin).unwrap();
    });
    plain.resource_mut::<UiViews>().set_enabled("debug_menu", true);
    plain.step(3);
    assert!(plain.ui().find("debug_menu/tab_editor").is_none());

    let mut t = editor_with(EditorPlugin::new());
    assert_eq!(t.text("debug_menu/tab_editor"), "Editor");
    for k in ["tab_logs", "tab_edits", "tab_scripts"] {
        assert!(t.ui().find(&format!("debug_menu/{k}")).is_some());
    }
    t.click("debug_menu/tab_editor");
    t.step(2);
    assert_eq!(t.resource::<crate::debug_menu::DebugState>().tab, DebugTab::Extra(TAB));
    assert!(t.ui().has_class(t.id("debug_menu/tab_editor"), "tab-active"));
    assert!(!t.ui().has_class(t.id("debug_menu/tab_logs"), "tab-active"));
    for k in ["toolbar/edit", "toolbar/play", "toolbar/pause", "toolbar/step", "toolbar/stop", "options/keep", "options/status", "hierarchy/filter", "hierarchy/list", "inspector"] {
        assert!(t.ui().find(&format!("debug_menu/editor/{k}")).is_some(), "Editor tab has `{k}`");
    }
    // other tabs still work and the editor body goes away
    t.click("debug_menu/tab_logs");
    t.step(2);
    assert!(t.ui().find("debug_menu/editor").is_none() && t.ui().find("debug_menu/log_scroll").is_some());
}

#[test]
fn edit_freezes_the_game_and_only_the_game() {
    let t = editor();
    assert_eq!(mode(&t), EditorMode::Edit);
    assert_eq!(t.engine.is_system_enabled("game_tick"), Some(false));
    assert_eq!(t.engine.is_system_enabled("keep_running"), Some(true), "global systems keep running");
    assert_eq!(t.engine.is_system_enabled("propagate_transforms"), Some(true));
    assert_eq!(t.engine.is_system_enabled("editor_update"), Some(true));
    assert_eq!(t.engine.is_system_enabled("debug_menu_update"), Some(true));
    assert_eq!(t.engine.is_system_enabled("script_system"), Some(false));
    let before = (ticks(&t), t.resource::<Kept>().0);
    let mut t = t;
    t.step(20);
    assert_eq!(ticks(&t), before.0, "the game does not tick");
    assert_eq!(t.resource::<Kept>().0, before.1 + 20, "a global system does");
    assert!(t.resource::<EditorState>().frozen >= 2);
    assert_eq!(t.text("debug_menu/editor/toolbar/edit"), "Edit");
    assert!(t.ui().has_class(t.id("debug_menu/editor/toolbar/edit"), "tab-active"));
    assert!(t.text("debug_menu/editor/options/status").starts_with("Edit:"));
}

#[test]
fn play_runs_the_game_and_stop_restores_the_world_and_the_flags() {
    let mut t = editor();
    let (x0, flags0) = (x_of(&t, "hero"), t.engine.system_states(Stage::Update).into_iter().map(|(n, e)| (n.to_string(), e)).collect::<Vec<_>>());
    press(&mut t, "play");
    assert_eq!(mode(&t), EditorMode::Play);
    assert!(t.ui().has_class(t.id("debug_menu/editor/toolbar/play"), "tab-active"));
    assert!(!t.ui().has_class(t.id("debug_menu/editor/toolbar/edit"), "tab-active"));
    assert_eq!(t.engine.is_system_enabled("game_tick"), Some(true));
    let n = ticks(&t);
    t.step(10);
    assert_eq!(ticks(&t), n + 10);
    assert!(x_of(&t, "hero") > x0 + 5.0);

    press(&mut t, "stop");
    assert_eq!(mode(&t), EditorMode::Edit);
    assert_eq!(x_of(&t, "hero"), x0, "the world is back (the entity is a new one, found by name)");
    assert_eq!(t.engine.world().query::<&Name>().iter().count(), 3, "names came back by EditorId");
    let flags1: Vec<(String, bool)> = t.engine.system_states(Stage::Update).into_iter().map(|(n, e)| (n.to_string(), e)).collect();
    assert_eq!(flags1, flags0, "system flags are exactly as Edit had them");
    let n = ticks(&t);
    t.step(10);
    assert_eq!(ticks(&t), n);
    // the parent link survived the restore
    let pet = entity(&t, "pet");
    assert_eq!(t.engine.world().get::<&Parent>(pet).unwrap().entity(), Some(entity(&t, "hero")));
}

#[test]
fn pause_keeps_the_world_and_step_runs_one_frame() {
    let mut t = editor();
    press(&mut t, "play");
    t.step(5);
    press(&mut t, "pause");
    assert_eq!(mode(&t), EditorMode::Paused);
    assert!(t.ui().has_class(t.id("debug_menu/editor/toolbar/pause"), "tab-active"));
    assert_eq!(t.engine.is_system_enabled("game_tick"), Some(false));
    let (n, x) = (ticks(&t), x_of(&t, "hero"));
    t.step(10);
    assert_eq!((ticks(&t), x_of(&t, "hero")), (n, x), "paused: nothing moves, nothing is restored");

    press(&mut t, "step");
    t.step(10);
    assert_eq!(ticks(&t), n + 1, "one frame of game systems");
    assert_eq!(mode(&t), EditorMode::Paused);
    assert_eq!(t.engine.is_system_enabled("game_tick"), Some(false), "frozen again");
    press(&mut t, "step");
    t.step(5);
    assert_eq!(ticks(&t), n + 2);

    // resume: runs again, no new snapshot (Stop goes back to the first one)
    press(&mut t, "play");
    assert_eq!(mode(&t), EditorMode::Play);
    t.step(5);
    assert!(ticks(&t) >= n + 2 + 5, "running again");
    press(&mut t, "pause");
    press(&mut t, "stop");
    assert_eq!(mode(&t), EditorMode::Edit);
    assert_eq!(x_of(&t, "hero"), 1.5, "restored to the state before the first Play");
    assert_eq!(t.engine.is_system_enabled("game_tick"), Some(false));
}

#[test]
fn step_and_pause_do_nothing_in_edit() {
    let mut t = editor();
    press(&mut t, "step");
    press(&mut t, "pause");
    t.step(5);
    assert_eq!((mode(&t), ticks(&t)), (EditorMode::Edit, 0));
    press(&mut t, "stop");
    assert_eq!(mode(&t), EditorMode::Edit);
}

#[test]
fn keep_changes_stops_without_restoring() {
    let mut t = editor();
    t.set_checked("debug_menu/editor/options/keep", true);
    t.step(2);
    assert!(t.resource::<EditorState>().keep_changes);
    let x0 = x_of(&t, "hero");
    press(&mut t, "play");
    t.step(10);
    press(&mut t, "stop");
    assert_eq!(mode(&t), EditorMode::Edit);
    assert!(x_of(&t, "hero") > x0 + 5.0, "the changes stayed");
    assert_eq!(t.engine.is_system_enabled("game_tick"), Some(false), "but the game is frozen again");
    let n = ticks(&t);
    t.step(5);
    assert_eq!(ticks(&t), n);
    // and Play works again
    press(&mut t, "play");
    t.step(3);
    assert!(ticks(&t) > n);
}

#[test]
fn a_game_system_that_was_disabled_before_edit_stays_disabled_in_play() {
    crate::debug_menu::serial();
    let mut t = UiTest::new(|e| {
        e.add_plugin(DebugMenuPlugin).unwrap();
        world_setup(e);
        e.set_system_enabled("game_tick", false).unwrap();
        e.add_plugin(EditorPlugin::new()).unwrap();
    });
    t.resource_mut::<UiViews>().set_enabled("debug_menu", true);
    t.step(2);
    t.click("debug_menu/tab_editor");
    t.step(3);
    press(&mut t, "play");
    t.step(5);
    assert_eq!(ticks(&t), 0, "Play re-enables exactly what Edit froze");
    assert_eq!(t.engine.is_system_enabled("game_tick"), Some(false));
}

#[test]
fn start_in_play_freezes_nothing_until_stop() {
    let mut t = editor_with(EditorPlugin::new().start_in(EditorMode::Play));
    t.step(5);
    assert_eq!(mode(&t), EditorMode::Play);
    assert!(ticks(&t) >= 5);
    t.click("debug_menu/tab_editor");
    t.step(2);
    press(&mut t, "stop");
    assert_eq!(mode(&t), EditorMode::Edit);
    let n = ticks(&t);
    t.step(5);
    assert_eq!(ticks(&t), n, "no snapshot to restore, but the game is frozen");
}

#[test]
fn the_hierarchy_lists_the_world_as_a_tree_and_a_click_selects() {
    let mut t = editor();
    let rows = t.resource::<EditorPanel>().rows.clone();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(&names[..3], ["hero", "pet", "rock"]);
    assert_eq!(rows.len(), 4, "the camera is listed too, last (no EditorId)");
    assert_eq!(rows[1].depth, 1);
    for n in ["hero", "pet", "rock"] {
        assert!(t.ui().find(&row_key(&t, n)).is_some(), "row for {n}");
    }
    assert_eq!(t.resource::<EditorSelection>().entity, None);
    assert_eq!(t.text("debug_menu/editor/inspector/none"), "(select an entity)");

    let rock = row_key(&t, "rock");
    t.click(&rock);
    t.step(2);
    assert_eq!(t.resource::<EditorSelection>().entity, Some(entity(&t, "rock")));
    assert_eq!(t.resource::<EditorSelection>().editor_id, Some(3));
    assert!(t.ui().has_class(t.id(&rock), "tab-active"));
    assert!(!t.ui().has_class(t.id(&row_key(&t, "hero")), "tab-active"));
    t.click(&row_key(&t, "hero"));
    t.step(2);
    assert!(t.ui().has_class(t.id(&row_key(&t, "hero")), "tab-active"));
    assert!(!t.ui().has_class(t.id(&rock), "tab-active"), "one row selected at a time");
}

/// `(component, [(field, value)])` as the inspector shows them.
fn shown(t: &UiTest) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    for i in 0.. {
        let base = format!("debug_menu/editor/inspector/c{i}");
        if t.ui().find(&base).is_none() {
            break;
        }
        let mut fields = Vec::new();
        for j in 0.. {
            let f = format!("{base}/f{j}");
            if t.ui().find(&f).is_none() {
                break;
            }
            fields.push((t.text(&format!("{f}/name")), t.text(&format!("{f}/value"))));
        }
        out.push((t.text(&format!("{base}/head")), fields));
    }
    out
}

#[test]
fn the_inspector_shows_components_and_fields_of_the_selection() {
    let mut t = editor();
    t.click(&row_key(&t, "hero"));
    t.step(3);
    let comps = shown(&t);
    let tr = comps.iter().find(|(n, _)| n == "Transform").expect("Transform shown");
    assert_eq!(tr.1[0], ("position".to_string(), "[1.5, 2]".to_string()));
    assert_eq!(tr.1[1].0, "rotation");
    assert_eq!(tr.1[2], ("scale".to_string(), "[1, 1]".to_string()));
    assert!(comps.iter().any(|(n, _)| n == "Sprite"));
    let ids = comps.iter().find(|(n, _)| n == "EditorId").unwrap();
    assert_eq!(ids.1, [("id".to_string(), "1".to_string())]);
    assert!(comps.iter().all(|(n, _)| n != "Name"));
    // read-only components are marked
    assert!(comps.iter().any(|(n, _)| n.ends_with("(read-only)")), "{:?}", comps.iter().map(|c| &c.0).collect::<Vec<_>>());
    // another entity: another list
    t.click(&row_key(&t, "rock"));
    t.step(3);
    assert!(shown(&t).iter().all(|(n, _)| n != "Sprite"));
    assert!(t.ui().find("debug_menu/editor/inspector/none").is_none());
}

#[test]
fn values_are_formatted_short() {
    assert_eq!(format_value(&SceneValue::Float(1.5)), "1.5");
    assert_eq!(format_value(&SceneValue::Float(2.0)), "2");
    assert_eq!(format_value(&SceneValue::Float(0.33333)), "0.333");
    assert_eq!(format_value(&SceneValue::Float(-0.0001)), "0");
    assert_eq!(format_value(&SceneValue::Float(-0.25)), "-0.25");
    assert_eq!(format_value(&SceneValue::Int(-4)), "-4");
    assert_eq!(format_value(&SceneValue::Bool(true)), "true");
    assert_eq!(format_value(&SceneValue::List(vec![SceneValue::UInt(1), SceneValue::Float(2.5)])), "[1, 2.5]");
}

#[test]
fn the_filter_keeps_matches_and_their_ancestors() {
    let mut t = editor();
    t.set_text("debug_menu/editor/hierarchy/filter", "pet");
    t.step(3);
    assert_eq!(t.resource::<EditorPanel>().filter, "pet");
    assert!(t.ui().find(&row_key(&t, "pet")).is_some());
    assert!(t.ui().find(&row_key(&t, "hero")).is_some(), "the parent of a match stays");
    assert!(t.ui().find(&row_key(&t, "rock")).is_none());
    t.set_text("debug_menu/editor/hierarchy/filter", "ZZZ");
    t.step(3);
    assert_eq!(t.text("debug_menu/editor/hierarchy/list/no_match"), "(no match)");
    t.set_text("debug_menu/editor/hierarchy/filter", "");
    t.step(3);
    assert!(t.ui().find(&row_key(&t, "rock")).is_some());
}

#[test]
fn the_selection_survives_stop_by_editor_id() {
    let mut t = editor();
    t.click(&row_key(&t, "hero"));
    t.step(3);
    let old = entity(&t, "hero");
    press(&mut t, "play");
    t.step(3);
    press(&mut t, "stop");
    t.step(3);
    let new = entity(&t, "hero");
    assert_ne!(old, new, "restore makes new entities");
    assert_eq!(t.resource::<EditorSelection>().entity, Some(new));
    assert!(t.ui().has_class(t.id(&row_key(&t, "hero")), "tab-active"));
    assert!(!shown(&t).is_empty());
}

#[test]
fn a_selection_whose_entity_is_gone_clears() {
    let mut t = editor();
    t.click(&row_key(&t, "rock"));
    t.step(3);
    let rock = entity(&t, "rock");
    t.engine.world_mut().despawn(rock).unwrap();
    t.step(30);
    assert_eq!(t.resource::<EditorSelection>().entity, None);
    assert_eq!(t.text("debug_menu/editor/inspector/none"), "(select an entity)");
}

#[test]
fn the_selected_sprite_gets_an_outline() {
    let mut t = editor();
    t.step(3);
    assert_eq!(t.resource::<Drawn>().0, 0, "nothing selected, nothing drawn");
    t.click(&row_key(&t, "hero"));
    t.step(3);
    assert_eq!(t.resource::<Drawn>().0, 4, "four edges around the sprite");
    t.click(&row_key(&t, "rock"));
    t.step(3);
    assert_eq!(t.resource::<Drawn>().0, 0, "rock has no sprite");
}

#[test]
fn the_panel_reads_the_world_throttled_and_only_while_shown() {
    let mut t = editor_with(EditorPlugin::new());
    // not on the tab: the world is not read
    assert!(t.resource::<EditorPanel>().rows.is_empty());
    t.step(40);
    assert!(t.resource::<EditorPanel>().rows.is_empty());
    t.click("debug_menu/tab_editor");
    t.step(2);
    assert_eq!(t.resource::<EditorPanel>().rows.len(), 4, "opening the tab reads at once");
    t.engine.world_mut().spawn((Name::new("late"), Transform::default(), EditorId { id: 9 }));
    t.step(3);
    assert_eq!(t.resource::<EditorPanel>().rows.len(), 4, "a few frames later: not read every frame");
    t.step(30);
    assert_eq!(t.resource::<EditorPanel>().rows.len(), 5);
    assert!(t.ui().find(&row_key(&t, "late")).is_some());
}

#[test]
fn an_idle_editor_does_not_rebuild_the_view() {
    let mut t = editor();
    t.click(&row_key(&t, "hero"));
    t.step(30);
    let runs = || VIEW_RUNS.load(std::sync::atomic::Ordering::Relaxed);
    let before = runs();
    let tick = |t: &UiTest| (t.engine.resources().changed_tick::<EditorPanel>(), t.engine.resources().changed_tick::<EditorState>(), t.engine.resources().changed_tick::<EditorSelection>(), t.engine.resources().changed_tick::<crate::debug_menu::DebugState>(), t.engine.resources().changed_tick::<crate::debug_menu::Fps>());
    let ticks0 = tick(&t);
    t.step(180);
    assert_eq!(tick(&t), ticks0, "which resource changed");
    assert_eq!(runs(), before, "Edit mode, nothing changes: the view stays asleep");
    // playing with nothing moving is idle too (the hero moves: filter it out by freezing the tick)
    press(&mut t, "play");
    press(&mut t, "pause");
    t.step(30);
    let before = runs();
    t.step(180);
    assert_eq!(runs(), before, "Paused: still asleep");
    // a change wakes it
    t.set_text("debug_menu/editor/hierarchy/filter", "r");
    t.step(2);
    assert!(runs() > before);
}

#[test]
fn requests_can_be_sent_as_events() {
    struct Send(Option<EditorRequest>);
    fn go(cx: &mut Ctx) {
        if let Some(r) = cx.resources.get_mut::<Send>().and_then(|s| s.0.take()) {
            cx.send(r);
        }
    }
    let mut t = editor();
    t.engine.insert_resource(Send(None));
    t.engine.add_system(Stage::Update, go).name("test_go").order(order::EARLY - 10);
    t.resource_mut::<Send>().0 = Some(EditorRequest::Play);
    t.step(4);
    assert_eq!(mode(&t), EditorMode::Play);
    t.resource_mut::<Send>().0 = Some(EditorRequest::Stop);
    t.step(4);
    assert_eq!(mode(&t), EditorMode::Edit);
}
