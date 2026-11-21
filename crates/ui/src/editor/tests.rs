use super::panel::format_value;
use super::*;
use crate::{NodeKind, UiViews};
use crate::debug_menu::{DebugMenuPlugin, VIEW_RUNS};
use crate::testing::UiTest;
use efeengine_core::ecs::{SceneValue, entity_to_bits};

#[script_component(name = "Knob")]
#[derive(Debug, Default, PartialEq)]
struct Knob {
    on: ScriptBool,
    b: ScriptBool,
    c: ScriptBool,
    d: ScriptBool,
    level: i32,
}

struct Ticks(u32);
struct Kept(u32);
/// `DebugDraw2D::len()` as seen just before the render step empties it.
pub(super) struct Drawn(pub usize);

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
    e.register::<Knob>().expect("Knob");
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
    w.spawn((Name::new("rock"), EditorId { id: 3 }, Transform::default(), Knob { level: 4, ..Default::default() }));
}

pub(super) fn editor_with(plugin: EditorPlugin) -> UiTest {
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

pub(super) fn editor() -> UiTest {
    let mut t = editor_with(EditorPlugin::new());
    t.click("debug_menu/tab_editor");
    t.step(3);
    t
}

pub(super) fn press(t: &mut UiTest, key: &str) {
    t.click(&format!("debug_menu/editor/toolbar/{key}"));
    t.step(2);
}

fn ticks(t: &UiTest) -> u32 {
    t.resource::<Ticks>().0
}

pub(super) fn entity(t: &UiTest, name: &str) -> Entity {
    t.engine.world().query::<(Entity, &Name)>().iter().find(|(_, n)| n.0 == name).map(|(e, _)| e).unwrap_or_else(|| panic!("no entity `{name}`"))
}

pub(super) fn x_of(t: &UiTest, name: &str) -> f32 {
    t.engine.world().get::<&Transform>(entity(t, name)).unwrap().position.x
}

pub(super) fn row_key(t: &UiTest, name: &str) -> String {
    format!("debug_menu/editor/hierarchy/list/e{}/row", entity_to_bits(entity(t, name)))
}

pub(super) fn mode(t: &UiTest) -> EditorMode {
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

/// The text of a field's widget: a scalar input / text, or the elements of a list as `[a, b]`.
fn value_text(t: &UiTest, f: &str) -> String {
    let v = format!("{f}/value");
    if t.ui().find(&format!("{v}/e0")).is_some() {
        let parts: Vec<String> = (0..).map(|k| format!("{v}/e{k}")).take_while(|k| t.ui().find(k).is_some()).map(|k| t.text(&k)).collect();
        format!("[{}]", parts.join(", "))
    } else {
        t.text(&v)
    }
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
            fields.push((t.text(&format!("{f}/name")), value_text(t, &f)));
        }
        out.push((t.text(&format!("{base}/bar/head")), fields));
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
fn the_selected_sprite_gets_an_outline_and_a_gizmo() {
    let mut t = editor();
    t.step(3);
    // Entities without a sprite (the game's camera, pet, rock) show a handle each.
    let base = t.resource::<Drawn>().0;
    assert_eq!(base, 3, "three handles, nothing selected");
    t.click(&row_key(&t, "hero"));
    t.step(3);
    assert!(t.resource::<Drawn>().0 >= base + 4 + 3, "four edges and the move gizmo around the sprite: {}", t.resource::<Drawn>().0);
    t.click(&row_key(&t, "rock"));
    t.step(3);
    let n = t.resource::<Drawn>().0;
    assert!(n > base && n < base + 4 + 3 + 1, "rock has no sprite: no outline, but a gizmo: {n}");
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

// ----- step 2: editing -----------------------------------------------------------------------

use super::panel::edit_text as panel_edit_text;
use efeengine_core::engine::inspect::InspectedComponent;
use efeengine_core::input::{ElementState, InputEvent, InputEventKind, Key, Modifiers, WindowId};

const INSP: &str = "debug_menu/editor/inspector";
const HIER: &str = "debug_menu/editor/hierarchy";
const SCENE: &str = "debug_menu/editor/scene";

pub(super) fn pick(t: &mut UiTest, name: &str) {
    t.click(&row_key(t, name));
    t.step(3);
}

fn comp_key(t: &UiTest, comp: &str) -> String {
    for i in 0.. {
        let base = format!("{INSP}/c{i}");
        if t.ui().find(&base).is_none() {
            break;
        }
        let head = t.text(&format!("{base}/bar/head"));
        if head == comp || head == format!("{comp} (read-only)") {
            return base;
        }
    }
    panic!("`{comp}` is not shown");
}

fn has_comp(t: &UiTest, comp: &str) -> bool {
    shown(t).iter().any(|(n, _)| n == comp || n == &format!("{comp} (read-only)"))
}

fn field_key(t: &UiTest, comp: &str, field: &str) -> String {
    let base = comp_key(t, comp);
    for j in 0.. {
        let f = format!("{base}/f{j}");
        if t.ui().find(&f).is_none() {
            break;
        }
        if t.text(&format!("{f}/name")) == field {
            return f;
        }
    }
    panic!("`{comp}.{field}` is not shown");
}

fn val_key(t: &UiTest, comp: &str, field: &str) -> String {
    format!("{}/value", field_key(t, comp, field))
}

fn elem_key(t: &UiTest, comp: &str, field: &str, k: usize) -> String {
    format!("{}/value/e{k}", field_key(t, comp, field))
}

/// Types `text` into an inspector input and presses Enter.
fn type_into(t: &mut UiTest, key: &str, text: &str) {
    t.set_text(key, text);
    t.step(1);
    t.submit(key);
    t.step(3);
}

pub(super) fn position(t: &UiTest, name: &str) -> Vec2 {
    t.engine.world().get::<&Transform>(entity(t, name)).unwrap().position
}

fn knob(t: &UiTest) -> (bool, i32) {
    let k = t.engine.world().get::<&Knob>(entity(t, "rock")).unwrap();
    (k.on.get(), k.level)
}

pub(super) fn ui_state(t: &UiTest) -> EditorUi {
    (*t.resource::<EditorUi>()).clone()
}

pub(super) fn by_id(t: &UiTest, id: u64) -> Entity {
    t.engine.entity_by_editor_id(id).unwrap_or_else(|| panic!("no entity #{id}"))
}

fn insp(t: &UiTest, id: u64) -> Vec<InspectedComponent> {
    t.engine.inspect_entity(by_id(t, id)).unwrap()
}

pub(super) type Dump = Vec<(Option<u64>, String, Vec<InspectedComponent>)>;

/// The whole editable world as a comparable value: `(EditorId, name, components)` sorted by id.
/// `Parent` is left out (an undone delete makes new entities, so the stored bits differ).
pub(super) fn dump(t: &UiTest) -> Dump {
    let mut out: Dump = t
        .engine
        .editable_entities()
        .into_iter()
        .map(|r| {
            let mut comps = t.engine.inspect_entity(r.entity).unwrap();
            comps.retain(|c| c.name != "Parent");
            (r.editor_id, r.name, comps)
        })
        .collect();
    out.sort_by_key(|(id, n, _)| (*id, n.clone()));
    out
}

pub(super) fn ctrl_key(t: &mut UiTest, code: KeyCode, ch: &str, shift: bool) {
    let mods = if shift { Modifiers(Modifiers::LCTRL.0 | Modifiers::LSHIFT.0) } else { Modifiers::LCTRL };
    for state in [ElementState::Pressed, ElementState::Released] {
        t.engine.inject(InputEvent::with_time(InputEventKind::key(code, Key::character(ch), state, false, mods), 0, WindowId::MAIN));
    }
    t.step(4);
}

pub(super) fn click_button(t: &mut UiTest, key: &str) {
    t.click(&format!("debug_menu/editor/{key}"));
    t.step(4);
}

pub(super) fn msg(t: &UiTest) -> String {
    t.ui().find("debug_menu/editor/message").map(|_| t.text("debug_menu/editor/message")).unwrap_or_default()
}

#[test]
fn edit_text_is_the_exact_short_form() {
    assert_eq!(panel_edit_text(&SceneValue::Float(0.1)), "0.1");
    assert_eq!(panel_edit_text(&SceneValue::Float(2.0)), "2");
    assert_eq!(panel_edit_text(&SceneValue::Float(0.33333)), "0.33333", "not rounded like format_value");
    assert_eq!(panel_edit_text(&SceneValue::UInt(7)), "7");
}

#[test]
fn parsing_follows_the_shape_of_the_current_value() {
    assert_eq!(parse_scalar(&SceneValue::Float(0.0), " 1.5 "), Ok(SceneValue::Float(1.5)));
    assert_eq!(parse_scalar(&SceneValue::Int(0), "-3"), Ok(SceneValue::Int(-3)));
    assert!(parse_scalar(&SceneValue::Int(0), "1.5").is_err());
    assert!(parse_scalar(&SceneValue::UInt(0), "-1").is_err());
    assert!(parse_scalar(&SceneValue::Float(0.0), "NaN").is_err());
    assert!(parse_scalar(&SceneValue::Float(0.0), "").is_err());
    assert_eq!(parse_scalar(&SceneValue::Bool(false), "TRUE"), Ok(SceneValue::Bool(true)));
    assert!(parse_scalar(&SceneValue::Bool(false), "maybe").is_err());
    let v = SceneValue::List(vec![SceneValue::Float(1.0), SceneValue::Float(2.0)]);
    assert_eq!(with_element(&v, 1, "9"), Ok(SceneValue::List(vec![SceneValue::Float(1.0), SceneValue::Float(9.0)])));
    assert!(with_element(&v, 2, "9").is_err());
    assert!(with_element(&v, 0, "x").is_err());
}

#[test]
fn a_field_is_edited_through_its_input_and_the_world_changes() {
    let mut t = editor();
    pick(&mut t, "hero");
    let x = elem_key(&t, "Transform", "position", 0);
    assert_eq!(t.text(&x), "1.5");
    assert!(matches!(t.ui().kind(t.id(&x)), Some(NodeKind::TextInput { read_only: false, .. })));
    type_into(&mut t, &x, "42.25");
    assert_eq!(position(&t, "hero"), Vec2::new(42.25, 2.0), "only x changed");
    assert_eq!(t.text(&elem_key(&t, "Transform", "position", 0)), "42.25");
    // a scalar field and a list element further in
    let r = val_key(&t, "Transform", "rotation");
    type_into(&mut t, &r, "0.5");
    assert_eq!(t.engine.world().get::<&Transform>(entity(&t, "hero")).unwrap().rotation, 0.5);
    let sy = elem_key(&t, "Transform", "scale", 1);
    type_into(&mut t, &sy, "3");
    assert_eq!(t.engine.world().get::<&Transform>(entity(&t, "hero")).unwrap().scale, Vec2::new(1.0, 3.0));
    let ui = ui_state(&t);
    assert!(ui.drafts.is_empty() && ui.errors.is_empty());
    assert!(ui.dirty);
    assert_eq!(ui.undo_len, 3);
    // a color list of four
    let c = elem_key(&t, "Sprite", "color", 2);
    type_into(&mut t, &c, "0.25");
    assert_eq!(t.engine.world().get::<&Sprite>(entity(&t, "hero")).unwrap().color, [1.0, 1.0, 0.25, 1.0]);
}

#[test]
fn bad_input_shows_an_error_and_leaves_the_value() {
    let mut t = editor();
    pick(&mut t, "hero");
    let x = elem_key(&t, "Transform", "position", 0);
    type_into(&mut t, &x, "abc");
    assert_eq!(position(&t, "hero").x, 1.5, "unchanged");
    assert_eq!(t.text(&x), "abc", "the typed text stays so it can be fixed");
    assert!(t.ui().has_class(t.id(&x), "wire-error"));
    assert!(ui_state(&t).errors.contains_key(&FieldKey::new("Transform", "position", 0)));
    assert!(msg(&t).contains("not a number"), "{}", msg(&t));
    assert!(!ui_state(&t).dirty && ui_state(&t).undo_len == 0, "a rejected edit is not an edit");
    // typing again drops the error mark; a good value is applied
    t.set_text(&x, "7");
    t.step(3);
    assert!(!t.ui().has_class(t.id(&x), "wire-error"));
    t.submit(&x);
    t.step(3);
    assert_eq!(position(&t, "hero").x, 7.0);
    assert!(ui_state(&t).errors.is_empty());

    // integers: wrong shape and out of range both come back as errors
    pick(&mut t, "rock");
    let level = val_key(&t, "Knob", "level");
    assert_eq!(t.text(&level), "4");
    type_into(&mut t, &level, "1.5");
    assert_eq!(knob(&t).1, 4);
    assert!(t.ui().has_class(t.id(&level), "wire-error"));
    type_into(&mut t, &level, "99999999999");
    assert_eq!(knob(&t).1, 4);
    assert!(msg(&t).contains("Knob.level"), "{}", msg(&t));
    type_into(&mut t, &level, "-12");
    assert_eq!(knob(&t).1, -12);
    // switching the selection drops what was half typed
    t.set_text(&level, "zzz");
    t.step(2);
    pick(&mut t, "hero");
    assert!(ui_state(&t).drafts.is_empty());
}

#[test]
fn losing_focus_commits_typed_text() {
    let mut t = editor();
    pick(&mut t, "hero");
    let x = elem_key(&t, "Transform", "position", 0);
    t.focus(&x, true);
    t.step(2);
    t.set_text(&x, "9");
    t.step(2);
    assert_eq!(position(&t, "hero").x, 1.5, "typing alone does not write");
    assert_eq!(t.text(&x), "9");
    t.focus(&x, false);
    t.step(3);
    assert_eq!(position(&t, "hero").x, 9.0, "blur commits");
    // an invalid text on blur: error, value stays
    t.focus(&x, true);
    t.step(2);
    t.set_text(&x, "zz");
    t.step(2);
    t.focus(&x, false);
    t.step(3);
    assert_eq!(position(&t, "hero").x, 9.0);
    assert!(t.ui().has_class(t.id(&x), "wire-error"));
}

#[test]
fn escape_while_typing_drops_the_text() {
    let mut t = editor();
    pick(&mut t, "hero");
    let x = elem_key(&t, "Transform", "position", 0);
    t.focus(&x, true);
    t.step(2);
    t.set_text(&x, "55");
    t.step(2);
    t.engine.inject(InputEvent::with_time(InputEventKind::key(KeyCode::Escape, Key::Named(efeengine_core::input::NamedKey::Escape), ElementState::Pressed, false, Modifiers::NONE), 0, WindowId::MAIN));
    t.step(1);
    t.focus(&x, false);
    t.step(3);
    assert_eq!(position(&t, "hero").x, 1.5);
    assert_eq!(t.text(&elem_key(&t, "Transform", "position", 0)), "1.5");
}

#[test]
fn a_bool_field_is_a_checkbox() {
    let mut t = editor();
    pick(&mut t, "rock");
    let on = val_key(&t, "Knob", "on");
    assert!(matches!(t.ui().kind(t.id(&on)), Some(NodeKind::Checkbox { checked: false, .. })));
    t.set_checked(&on, true);
    t.step(3);
    assert!(knob(&t).0);
    assert!(matches!(t.ui().kind(t.id(&val_key(&t, "Knob", "on"))), Some(NodeKind::Checkbox { checked: true, .. })));
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert!(!knob(&t).0, "undone");
}

#[test]
fn read_only_and_managed_parts_have_no_editor_and_no_remove_button() {
    let mut t = editor();
    pick(&mut t, "pet");
    // read-only component
    let g = comp_key(&t, "GlobalTransform");
    assert!(t.ui().find(&format!("{g}/bar/remove")).is_none());
    assert!(t.ui().find(&format!("{g}/f0/value/e0")).is_none(), "no inputs in a read-only component");
    // the id is managed
    let id = comp_key(&t, "EditorId");
    assert!(t.ui().find(&format!("{id}/bar/remove")).is_none());
    assert!(matches!(t.ui().kind(t.id(&format!("{id}/f0/value"))), Some(NodeKind::Text { .. })));
    // an entity reference is text
    let p = val_key(&t, "Parent", "entity");
    assert!(matches!(t.ui().kind(t.id(&p)), Some(NodeKind::Text { .. })));
    // but Transform has both
    let tr = comp_key(&t, "Transform");
    assert!(t.ui().find(&format!("{tr}/bar/remove")).is_some());
    assert!(matches!(t.ui().kind(t.id(&elem_key(&t, "Transform", "position", 0))), Some(NodeKind::TextInput { .. })));
}

#[test]
fn components_are_added_from_the_picker_and_removed_with_the_x_button() {
    let mut t = editor();
    pick(&mut t, "rock");
    assert!(!has_comp(&t, "Sprite"));
    assert!(t.ui().find(&format!("{INSP}/add/picker")).is_none(), "closed at first");
    t.click(&format!("{INSP}/add/row/toggle"));
    t.step(3);
    let addable = t.resource::<EditorPanel>().addable.clone();
    assert!(addable.contains(&"Sprite".to_string()) && addable.contains(&"Camera".to_string()));
    for hidden in ["Transform", "Knob", "EditorId", "GlobalTransform", "Parent"] {
        assert!(!addable.contains(&hidden.to_string()), "`{hidden}` is not offered");
    }
    let i = addable.iter().position(|n| n == "Sprite").unwrap();
    t.click(&format!("{INSP}/add/picker/a{i}"));
    t.step(4);
    assert_eq!(t.engine.world().get::<&Sprite>(entity(&t, "rock")).unwrap().color, [1.0; 4]);
    assert!(has_comp(&t, "Sprite"));
    assert!(t.ui().find(&format!("{INSP}/add/picker")).is_none(), "the picker closes");
    assert!(ui_state(&t).dirty);

    // remove it again
    let sprite = comp_key(&t, "Sprite");
    t.click(&format!("{sprite}/bar/remove"));
    t.step(4);
    assert!(t.engine.world().get::<&Sprite>(entity(&t, "rock")).is_err());
    assert!(!has_comp(&t, "Sprite"));
    // and Transform too (nothing special about it)
    let tr = comp_key(&t, "Transform");
    t.click(&format!("{tr}/bar/remove"));
    t.step(4);
    assert!(t.engine.world().get::<&Transform>(entity(&t, "rock")).is_err());
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert!(has_comp(&t, "Transform"));
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert!(has_comp(&t, "Sprite"), "undo of the remove");
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert!(!has_comp(&t, "Sprite"), "undo of the add");
}

#[test]
fn removing_a_component_and_undoing_brings_its_values_back() {
    let mut t = editor();
    pick(&mut t, "hero");
    let before = insp(&t, 1);
    let sprite = comp_key(&t, "Sprite");
    t.click(&format!("{sprite}/bar/remove"));
    t.step(4);
    assert!(t.engine.world().get::<&Sprite>(entity(&t, "hero")).is_err());
    click_button(&mut t, "toolbar/undo");
    assert_eq!(insp(&t, 1), before, "the same size, color and everything");
}

#[test]
fn rename_sets_the_name_and_undo_restores_it() {
    let mut t = editor();
    pick(&mut t, "hero");
    let name = format!("{INSP}/entity/name");
    assert_eq!(t.text(&name), "hero");
    type_into(&mut t, &name, "champion");
    assert_eq!(t.engine.world().get::<&Name>(by_id(&t, 1)).unwrap().0, "champion");
    let row = format!("{HIER}/list/e{}/row", entity_to_bits(by_id(&t, 1)));
    assert!(t.text(&row).starts_with("champion"));
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(t.engine.world().get::<&Name>(by_id(&t, 1)).unwrap().0, "hero");
    assert_eq!(t.text(&name), "hero");
    ctrl_key(&mut t, KeyCode::Y, "y", false);
    assert_eq!(t.engine.world().get::<&Name>(by_id(&t, 1)).unwrap().0, "champion");
}

#[test]
fn a_new_entity_is_selected_and_undo_redo_keep_its_id() {
    let mut t = editor();
    click_button(&mut t, "hierarchy/actions/new");
    let n = by_id(&t, 4);
    assert_eq!(t.engine.world().get::<&Name>(n).unwrap().0, "entity 4");
    assert!(t.engine.world().get::<&Transform>(n).is_ok());
    assert_eq!(t.resource::<EditorSelection>().entity, Some(n));
    assert_eq!(t.resource::<EditorPanel>().rows.len(), 5);
    assert!(ui_state(&t).dirty);
    let made = insp(&t, 4);
    // the inspector shows it
    assert!(has_comp(&t, "Transform") && has_comp(&t, "EditorId"));

    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert!(t.engine.entity_by_editor_id(4).is_none());
    assert_eq!(t.resource::<EditorPanel>().rows.len(), 4);
    assert_eq!(t.resource::<EditorSelection>().entity, None);
    ctrl_key(&mut t, KeyCode::Z, "z", true);
    assert_eq!(insp(&t, 4), made, "the same id and content come back");
    assert_eq!(t.engine.world().get::<&Name>(by_id(&t, 4)).unwrap().0, "entity 4");
    assert_eq!(t.resource::<EditorSelection>().editor_id, Some(4));
}

#[test]
fn duplicate_copies_the_selection_with_a_new_id() {
    let mut t = editor();
    pick(&mut t, "hero");
    click_button(&mut t, "hierarchy/actions/duplicate");
    assert_eq!(t.engine.world().get::<&Name>(by_id(&t, 4)).unwrap().0, "hero copy");
    let (a, b) = (insp(&t, 1), insp(&t, 4));
    for (x, y) in a.iter().zip(&b) {
        if x.name != "EditorId" && x.name != "GlobalTransform" {
            assert_eq!(x, y, "{}", x.name);
        }
    }
    assert_eq!(t.resource::<EditorSelection>().editor_id, Some(4), "the copy is selected");
    click_button(&mut t, "toolbar/undo");
    assert!(t.engine.entity_by_editor_id(4).is_none());
    assert!(t.engine.entity_by_editor_id(1).is_some());
}

#[test]
fn delete_takes_the_children_and_undo_brings_everything_back() {
    let mut t = editor();
    let before = dump(&t);
    pick(&mut t, "hero");
    click_button(&mut t, "hierarchy/actions/delete");
    assert!(t.engine.entity_by_editor_id(1).is_none() && t.engine.entity_by_editor_id(2).is_none(), "hero and its child pet are gone");
    assert!(t.engine.entity_by_editor_id(3).is_some());
    assert_eq!(t.resource::<EditorSelection>().entity, None);
    assert_eq!(t.text(&format!("{INSP}/none")), "(select an entity)");
    assert_eq!(t.resource::<EditorPanel>().rows.len(), 2);

    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(dump(&t), before, "names, ids and every component are back");
    let pet = entity(&t, "pet");
    assert_eq!(t.engine.world().get::<&Parent>(pet).unwrap().entity(), Some(entity(&t, "hero")), "the child follows its parent");
    assert_eq!(t.resource::<EditorPanel>().rows[1].depth, 1);
    ctrl_key(&mut t, KeyCode::Y, "y", false);
    assert!(t.engine.entity_by_editor_id(1).is_none());
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(dump(&t), before);
}

#[test]
fn a_whole_session_undoes_and_redoes_to_the_exact_states() {
    let mut t = editor();
    let s0 = dump(&t);
    pick(&mut t, "hero");
    let x = elem_key(&t, "Transform", "position", 0);
    type_into(&mut t, &x, "8"); // 1 field
    pick(&mut t, "rock");
    let tr = comp_key(&t, "Transform");
    t.click(&format!("{tr}/bar/remove")); // 2 remove
    t.step(4);
    type_into(&mut t, &format!("{INSP}/entity/name"), "stone"); // 3 rename
    click_button(&mut t, "hierarchy/actions/duplicate"); // 4 duplicate
    pick(&mut t, "hero");
    click_button(&mut t, "hierarchy/actions/delete"); // 5 delete
    click_button(&mut t, "hierarchy/actions/new"); // 6 new
    let s6 = dump(&t);
    assert_eq!(ui_state(&t).undo_len, 6);
    for _ in 0..6 {
        click_button(&mut t, "toolbar/undo");
    }
    assert_eq!(dump(&t), s0, "six undos give the first state");
    assert_eq!(ui_state(&t).redo_len, 6);
    assert!(!t.ui().is_enabled(t.id("debug_menu/editor/toolbar/undo")), "nothing left to undo");
    for _ in 0..6 {
        click_button(&mut t, "toolbar/redo");
    }
    assert_eq!(dump(&t), s6, "six redos give the last state");
    assert!(!t.ui().is_enabled(t.id("debug_menu/editor/toolbar/redo")));
    // a new edit empties the redo side
    click_button(&mut t, "toolbar/undo");
    pick(&mut t, "stone");
    { let k = val_key(&t, "Knob", "level"); type_into(&mut t, &k, "77"); }
    assert_eq!(ui_state(&t).redo_len, 0);
}

#[test]
fn undo_shortcuts_and_the_buttons_agree() {
    let mut t = editor();
    assert!(!t.ui().is_enabled(t.id("debug_menu/editor/toolbar/undo")), "empty log: disabled");
    assert!(!t.ui().is_enabled(t.id("debug_menu/editor/toolbar/redo")));
    pick(&mut t, "hero");
    let x = elem_key(&t, "Transform", "position", 0);
    type_into(&mut t, &x, "5");
    type_into(&mut t, &x, "6");
    assert!(t.ui().is_enabled(t.id("debug_menu/editor/toolbar/undo")));
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(position(&t, "hero").x, 5.0);
    assert!(t.ui().is_enabled(t.id("debug_menu/editor/toolbar/redo")));
    ctrl_key(&mut t, KeyCode::Z, "z", true);
    assert_eq!(position(&t, "hero").x, 6.0, "Ctrl+Shift+Z redoes");
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    ctrl_key(&mut t, KeyCode::Y, "y", false);
    assert_eq!(position(&t, "hero").x, 6.0, "Ctrl+Y redoes");
    click_button(&mut t, "toolbar/undo");
    click_button(&mut t, "toolbar/undo");
    assert_eq!(position(&t, "hero").x, 1.5);
    click_button(&mut t, "toolbar/redo");
    assert_eq!(position(&t, "hero").x, 5.0);
    // the field shows the world again after an undo
    assert_eq!(t.text(&elem_key(&t, "Transform", "position", 0)), "5");
}

#[test]
fn shortcuts_are_ignored_while_a_text_input_has_focus() {
    let mut t = editor();
    pick(&mut t, "hero");
    let x = elem_key(&t, "Transform", "position", 0);
    type_into(&mut t, &x, "5");
    // the hierarchy filter has focus: Ctrl+Z belongs to the text field
    let filter = format!("{HIER}/filter");
    t.focus(&filter, true);
    t.step(3);
    assert_eq!(ui_state(&t).focus.as_deref(), Some("@filter"));
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(position(&t, "hero").x, 5.0, "ignored while typing");
    // an inspector input counts too
    t.focus(&filter, false);
    t.focus(&x, true);
    t.step(3);
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(position(&t, "hero").x, 5.0);
    t.focus(&x, false);
    t.step(3);
    assert_eq!(ui_state(&t).focus, None);
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(position(&t, "hero").x, 1.5, "and works again after");
}

#[test]
fn shortcuts_do_nothing_when_the_editor_tab_is_not_showing() {
    let mut t = editor();
    pick(&mut t, "hero");
    { let k = elem_key(&t, "Transform", "position", 0); type_into(&mut t, &k, "5"); }
    t.click("debug_menu/tab_logs");
    t.step(3);
    ctrl_key(&mut t, KeyCode::Z, "z", false);
    assert_eq!(position(&t, "hero").x, 5.0);
}

#[test]
fn edits_work_in_play_and_stop_takes_them_back() {
    let mut t = editor();
    pick(&mut t, "rock");
    press(&mut t, "play");
    { let k = val_key(&t, "Knob", "level"); type_into(&mut t, &k, "9"); }
    assert_eq!(knob(&t).1, 9, "a live edit");
    assert_eq!(ui_state(&t).undo_len, 1);
    press(&mut t, "stop");
    assert_eq!(knob(&t).1, 4, "Stop restored the snapshot");
    assert_eq!(ui_state(&t).undo_len, 0, "the log was cleared with the restore");
    assert_eq!(ui_state(&t).redo_len, 0);
    // with Keep changes the edit stays
    t.set_checked("debug_menu/editor/options/keep", true);
    t.step(2);
    press(&mut t, "play");
    { let k = val_key(&t, "Knob", "level"); type_into(&mut t, &k, "11"); }
    press(&mut t, "stop");
    assert_eq!(knob(&t).1, 11);
}

fn temp_file(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("efe_editor_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().replace('\\', "/")
}

fn scene_label(t: &UiTest) -> String {
    t.text(&format!("{SCENE}/bar/label"))
}

fn set_path(t: &mut UiTest, p: &str) {
    t.set_text(&format!("{SCENE}/path_row/path"), p);
    t.step(3);
}

#[test]
fn save_as_writes_a_file_that_loads_back_and_the_dirty_marker_follows() {
    let path = temp_file("roundtrip.ron");
    let _ = std::fs::remove_file(&path);
    let mut t = editor();
    assert_eq!(scene_label(&t), "Scene: (none)");
    assert!(!t.ui().is_enabled(t.id(&format!("{SCENE}/bar/save"))), "no file yet: Save waits for Save As");
    assert!(t.ui().is_enabled(t.id(&format!("{SCENE}/bar/save_as"))));

    pick(&mut t, "hero");
    { let k = elem_key(&t, "Transform", "position", 0); type_into(&mut t, &k, "12"); }
    assert_eq!(scene_label(&t), "Scene: (none) *", "an edit marks the scene dirty");
    // Save As needs a path
    click_button(&mut t, "scene/bar/save_as");
    assert!(msg(&t).contains("path"), "{}", msg(&t));
    set_path(&mut t, &path);
    click_button(&mut t, "scene/bar/save_as");
    assert!(std::path::Path::new(&path).exists(), "{}", msg(&t));
    assert_eq!(scene_label(&t), "Scene: roundtrip", "saved: no star");
    assert!(!ui_state(&t).dirty);
    assert!(t.ui().is_enabled(t.id(&format!("{SCENE}/bar/save"))), "Save has a file now");
    assert!(msg(&t).starts_with("Saved"), "{}", msg(&t));

    let text = std::fs::read_to_string(&path).unwrap();
    let scene = efeengine_core::ecs::Scene::from_ron(&text).expect("the file parses");
    assert_eq!(scene.entities.len(), 4);
    assert!(scene.entities.iter().all(|e| e.id.is_some()), "ids keep Parent working");
    assert!(text.contains("12"), "the edit is in the file");
    assert!(text.contains("name: Some(\"hero\")"), "names live in the scene file");
    assert!(!std::path::Path::new(&format!("{path}.names")).exists(), "no sidecar file any more");
    assert!(scene.entities.iter().any(|e| e.name.as_deref() == Some("pet")) && scene.entities.iter().any(|e| e.name.as_deref() == Some("rock")), "every name is in the file");

    // change the world, then Load: the file's world comes back, with names and the tree
    let saved = dump(&t);
    pick(&mut t, "rock");
    click_button(&mut t, "hierarchy/actions/delete");
    assert_eq!(t.resource::<EditorPanel>().rows.len(), 3);
    assert!(ui_state(&t).dirty);
    click_button(&mut t, "scene/bar/load");
    assert!(msg(&t).starts_with("Loaded"), "{}", msg(&t));
    let loaded = dump(&t);
    // the camera has no EditorId, so it lost its name; everything else matches
    assert_eq!(loaded.len(), saved.len());
    for (a, b) in saved.iter().zip(&loaded) {
        if a.0.is_some() {
            assert_eq!(a, b);
        }
    }
    assert_eq!(position(&t, "hero"), Vec2::new(12.0, 2.0));
    assert_eq!(t.engine.world().get::<&Parent>(entity(&t, "pet")).unwrap().entity(), Some(entity(&t, "hero")));
    assert_eq!(t.engine.current_scene(), Some("roundtrip"));
    assert!(t.engine.editable_entities().iter().all(|r| r.scene_owned), "a loaded scene owns its entities");
    assert_eq!(scene_label(&t), "Scene: roundtrip");
    assert_eq!((ui_state(&t).undo_len, ui_state(&t).redo_len), (0, 0), "the log is cleared by a load");
    assert_eq!(t.resource::<EditorSelection>().entity, None);

    // Save writes back to the same file
    pick(&mut t, "rock");
    { let k = val_key(&t, "Knob", "level"); type_into(&mut t, &k, "31"); }
    assert_eq!(scene_label(&t), "Scene: roundtrip *");
    click_button(&mut t, "scene/bar/save");
    assert_eq!(scene_label(&t), "Scene: roundtrip");
    click_button(&mut t, "scene/bar/new");
    assert!(t.resource::<EditorPanel>().rows.is_empty(), "New empties the world");
    assert_eq!(scene_label(&t), "Scene: (none)");
    assert_eq!(t.engine.current_scene(), None);
    assert!(!t.ui().is_enabled(t.id(&format!("{SCENE}/bar/save"))));
    set_path(&mut t, &path);
    click_button(&mut t, "scene/bar/load");
    assert_eq!(knob(&t).1, 31, "the second save is what loads");
    // a new entity in a loaded scene belongs to it
    click_button(&mut t, "hierarchy/actions/new");
    let fresh = t.engine.editable_entities().into_iter().find(|r| r.name.starts_with("entity")).unwrap();
    assert!(fresh.scene_owned);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_bad_file_is_an_error_in_the_status_line_and_changes_nothing() {
    let path = temp_file("broken.ron");
    std::fs::write(&path, "this is not a scene").unwrap();
    let mut t = editor();
    let before = dump(&t);
    set_path(&mut t, &path);
    click_button(&mut t, "scene/bar/load");
    assert!(msg(&t).starts_with("Load failed"), "{}", msg(&t));
    assert!(t.ui().has_class(t.id("debug_menu/editor/message"), "danger"));
    assert_eq!(dump(&t), before);
    set_path(&mut t, &temp_file("missing.ron"));
    click_button(&mut t, "scene/bar/load");
    assert!(msg(&t).starts_with("Load failed"));
    // a scene with an unknown component is refused as a whole
    std::fs::write(&path, "(format: 1, name: \"x\", entities: [(components: {\"Nope\": {}})])").unwrap();
    set_path(&mut t, &path);
    click_button(&mut t, "scene/bar/load");
    assert!(msg(&t).contains("Nope"), "{}", msg(&t));
    assert_eq!(dump(&t), before);
    assert_eq!(ui_state(&t).path, None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn scene_files_are_disabled_in_play_and_pause() {
    struct Probe(Option<EditorMsg>);
    fn send(cx: &mut Ctx) {
        if let Some(m) = cx.resources.get_mut::<Probe>().and_then(|p| p.0.take()) {
            cx.send(m);
        }
    }
    let path = temp_file("playmode.ron");
    let _ = std::fs::remove_file(&path);
    let mut t = editor();
    set_path(&mut t, &path);
    press(&mut t, "play");
    for b in ["save", "save_as", "load", "new"] {
        assert!(!t.ui().is_enabled(t.id(&format!("{SCENE}/bar/{b}"))), "{b} is disabled in Play");
    }
    assert!(msg(&t).contains("Edit mode"), "{}", msg(&t));
    t.click(&format!("{SCENE}/bar/save_as"));
    t.step(4);
    assert!(!std::path::Path::new(&path).exists(), "nothing was written");
    press(&mut t, "pause");
    assert!(!t.ui().is_enabled(t.id(&format!("{SCENE}/bar/save_as"))), "and in Pause");
    // the request itself (an event) is refused too
    t.engine.insert_resource(Probe(Some(EditorMsg::SaveAs)));
    t.engine.add_system(Stage::Update, send).name("probe_send").order(order::EARLY - 10);
    t.step(5);
    assert!(!std::path::Path::new(&path).exists(), "SaveAs in Pause is refused");
    assert!(msg(&t).contains("Edit mode"), "{}", msg(&t));
    press(&mut t, "stop");
    assert!(t.ui().is_enabled(t.id(&format!("{SCENE}/bar/save_as"))));
    click_button(&mut t, "scene/bar/save_as");
    assert!(std::path::Path::new(&path).exists());
    std::fs::remove_file(&path).ok();
}

#[test]
fn the_scenes_dir_is_listed_and_a_click_loads() {
    let dir = std::env::temp_dir().join(format!("efe_editor_scenes_{}", std::process::id()));
    let scenes = dir.join("scenes");
    std::fs::create_dir_all(&scenes).unwrap();
    std::fs::write(scenes.join("b.ron"), "(format: 1, name: \"b\", entities: [])").unwrap();
    std::fs::write(scenes.join("a.ron"), "(format: 1, name: \"a\", entities: [])").unwrap();
    std::fs::write(scenes.join("notes.txt"), "x").unwrap();
    let scenes_text = scenes.to_string_lossy().replace('\\', "/");
    let mut t = editor_with(EditorPlugin::new().scenes_dir(scenes_text.clone()));
    t.click("debug_menu/tab_editor");
    t.step(5);
    let files = ui_state(&t).scene_files;
    assert_eq!(files, [format!("{scenes_text}/a.ron"), format!("{scenes_text}/b.ron")], "sorted, .ron only");
    assert_eq!(t.text(&format!("{SCENE}/list/f0")), format!("{scenes_text}/a.ron"));
    t.click(&format!("{SCENE}/list/f1"));
    t.step(4);
    assert_eq!(t.engine.current_scene(), Some("b"));
    assert_eq!(ui_state(&t).path, Some(format!("{scenes_text}/b.ron")));
    assert!(t.resource::<EditorPanel>().rows.is_empty(), "b is an empty scene");
    std::fs::remove_dir_all(&dir).ok();
}
