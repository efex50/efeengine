//! Step 4 tests: prefabs (save as prefab, place, overrides and revert, apply, unpack, undo), the
//! scene system list, the handles toggle and the layout-B panel size. Headless (`UiTest`).

use super::tests::{Drawn, editor, editor_with, entity, pick, ui_state};
use super::viewport;
use super::*;
use crate::testing::UiTest;
use efeengine_core::ecs::Scene;
use efeengine_core::engine::prefab::Prefab;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("efe_editor_s4_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text_path(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

const SCENE: &str = "debug_menu/editor/scene";
const INSP: &str = "debug_menu/editor/inspector";

fn press(t: &mut UiTest, key: &str) {
    t.click(&format!("debug_menu/editor/{key}"));
    t.step(4);
}

fn undo(t: &mut UiTest) {
    press(t, "toolbar/undo");
}

fn redo(t: &mut UiTest) {
    press(t, "toolbar/redo");
}

fn set_path(t: &mut UiTest, p: &str) {
    t.set_text(&format!("{SCENE}/path_row/path"), p);
    t.step(3);
}

fn load(t: &mut UiTest, path: &str) {
    set_path(t, path);
    press(t, "scene/bar/load");
    assert!(ui_state(t).status.starts_with("Loaded"), "{}", ui_state(t).status);
}

fn tab(t: &mut UiTest, name: &str) {
    press(t, &format!("left/{name}"));
}

fn editor_in(dir: &std::path::Path) -> UiTest {
    let mut t = editor_with(EditorPlugin::new().prefabs_dir(text_path(dir)));
    t.click("debug_menu/tab_editor");
    t.step(3);
    t
}

fn pos(t: &UiTest, e: Entity) -> Vec2 {
    t.engine.world().get::<&Transform>(e).unwrap().position
}

/// `inspector/c<N>/f<M>` of `comp.field`.
fn field_row(t: &UiTest, comp: &str, field: &str) -> String {
    for i in 0.. {
        let base = format!("{INSP}/c{i}");
        if t.ui().find(&base).is_none() {
            break;
        }
        let head = t.text(&format!("{base}/bar/head"));
        if head == comp {
            for j in 0.. {
                let f = format!("{base}/f{j}");
                if t.ui().find(&f).is_none() {
                    break;
                }
                if t.text(&format!("{f}/name")).trim_end_matches(" *") == field {
                    return f;
                }
            }
        }
    }
    panic!("`{comp}.{field}` is not shown");
}

fn type_into(t: &mut UiTest, key: &str, text: &str) {
    t.set_text(key, text);
    t.step(1);
    t.submit(key);
    t.step(3);
}

fn instances(t: &UiTest, name: &str) -> Vec<Entity> {
    t.engine.world().query::<(Entity, &efeengine_core::engine::prefab::PrefabInstance)>().iter().filter(|(_, p)| p.prefab() == name).map(|(e, _)| e).collect()
}

// ----- scene systems ---------------------------------------------------------------------------

fn system_check(t: &UiTest, name: &str) -> String {
    for i in 0.. {
        let k = format!("debug_menu/editor/systems/list/s{i}/check");
        if t.ui().find(&k).is_none() {
            break;
        }
        if t.text(&k) == name {
            return k;
        }
    }
    panic!("system `{name}` is not listed");
}

fn checked(t: &UiTest, key: &str) -> bool {
    matches!(t.value(key), Some(crate::UiValue::Bool(true)))
}

#[test]
fn the_systems_panel_edits_the_scene_list_with_undo_and_saves_it() {
    let dir = tmp("systems");
    let scene = dir.join("lv.ron");
    std::fs::write(&scene, r#"Scene(format: 1, name: "lv", systems: ["game_tick"], entities: [])"#).unwrap();
    let mut t = editor_in(&dir);
    tab(&mut t, "systems");
    assert!(t.text("debug_menu/editor/systems/title").contains("No scene"), "{}", t.text("debug_menu/editor/systems/title"));
    assert!(!t.ui().is_enabled(t.id(&system_check(&t, "game_tick"))), "no scene: nothing to edit");

    load(&mut t, &text_path(&scene));
    tab(&mut t, "systems");
    assert!(t.text("debug_menu/editor/systems/title").contains("lv"));
    let tick = system_check(&t, "game_tick");
    let draw = system_check(&t, "watch_draw");
    assert!(checked(&t, &tick) && !checked(&t, &draw), "the scene lists game_tick only");
    // global and engine systems are shown, read-only
    let global = system_check(&t, "keep_running");
    assert!(!t.ui().is_enabled(t.id(&global)), "global systems are read-only");
    assert!(!t.ui().is_enabled(t.id(&system_check(&t, "propagate_transforms"))));
    assert!(t.text(&global.replace("check", "info")).contains("global"));
    assert!(t.ui().is_enabled(t.id(&draw)));
    assert!(!ui_state(&t).dirty);

    // list a system: only the list changes while the game is frozen in Edit
    let enabled_before = t.engine.is_system_enabled("watch_draw");
    t.set_checked(&draw, true);
    t.step(4);
    assert_eq!(t.engine.scene_system_entries(), vec!["game_tick".to_string(), "watch_draw".to_string()]);
    assert!(checked(&t, &system_check(&t, "watch_draw")));
    assert!(ui_state(&t).dirty, "marks the scene dirty");
    assert_eq!(t.engine.is_system_enabled("watch_draw"), enabled_before, "Edit keeps the flags");
    // undo and redo
    undo(&mut t);
    assert_eq!(t.engine.scene_system_entries(), vec!["game_tick".to_string()]);
    assert!(!checked(&t, &system_check(&t, "watch_draw")));
    redo(&mut t);
    assert_eq!(t.engine.scene_system_entries().len(), 2);
    // unlist game_tick, then save: the file has the list
    t.set_checked(&system_check(&t, "game_tick"), false);
    t.step(4);
    let out = dir.join("out.ron");
    set_path(&mut t, &text_path(&out));
    press(&mut t, "scene/bar/save_as");
    let saved = Scene::from_ron(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(saved.systems, vec!["watch_draw".to_string()], "written to the scene file");
    // and it loads again with that list
    load(&mut t, &text_path(&out));
    assert_eq!(t.engine.scene_systems(), vec!["watch_draw".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- prefabs ---------------------------------------------------------------------------------

#[test]
fn save_as_prefab_place_override_revert_apply_and_unpack_with_undo() {
    let dir = tmp("prefabs");
    let pdir = dir.join("prefabs");
    let mut t = editor_with(EditorPlugin::new().prefabs_dir(text_path(&pdir)));
    t.click("debug_menu/tab_editor");
    t.step(3);
    // nothing selected: Save as prefab is disabled
    tab(&mut t, "prefabs");
    t.set_text("debug_menu/editor/prefabs/name", "Hero");
    t.step(3);
    assert!(!t.ui().is_enabled(t.id("debug_menu/editor/prefabs/actions/save")));
    assert!(t.ui().find("debug_menu/editor/prefabs/list/empty").is_some());

    // select hero (with its child pet) and save it
    tab(&mut t, "hierarchy");
    pick(&mut t, "hero");
    tab(&mut t, "prefabs");
    press(&mut t, "prefabs/actions/save");
    let file = pdir.join("Hero.ron");
    assert!(file.exists(), "{}", ui_state(&t).status);
    let prefab = Prefab::from_ron(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(prefab.entities.len(), 2, "the entity and its child");
    assert_eq!(prefab.entities[0].name.as_deref(), Some("hero"));
    assert!(prefab.entities.iter().all(|e| !e.components.contains_key("EditorId")));
    assert_eq!(prefab.entities[0].components["Transform"]["position"], efeengine_core::ecs::SceneValue::List(vec![efeengine_core::ecs::SceneValue::Float(0.0), efeengine_core::ecs::SceneValue::Float(0.0)]), "a new prefab starts at the origin");
    let hero = entity(&t, "hero");
    assert!(t.engine.prefab_instance(hero).unwrap().is_root(), "the entity is an instance now");
    assert!(t.engine.prefab_instance(entity(&t, "pet")).is_some());
    assert_eq!(ui_state(&t).prefab_files, vec!["Hero".to_string()]);
    assert!(ui_state(&t).status.starts_with("Saved prefab Hero"), "{}", ui_state(&t).status);
    assert!(t.ui().find("debug_menu/editor/prefabs/list/p0").is_some());
    assert_eq!(t.text("debug_menu/editor/prefabs/list/p0"), "Place Hero");
    // the hero's own position (1.5, 2.0) is an override of the prefab's origin
    assert_eq!(t.engine.prefab_overrides(hero), vec![("Transform".to_string(), "position".to_string())]);
    tab(&mut t, "hierarchy");
    assert!(t.text(&super::tests::row_key(&t, "hero")).contains("[Hero]"), "{}", t.text(&super::tests::row_key(&t, "hero")));
    // undo takes the file and the link back, redo writes them again
    undo(&mut t);
    assert!(!file.exists(), "undo removes a new prefab file");
    assert!(t.engine.prefab_instance(hero).is_none());
    assert!(ui_state(&t).prefab_files.is_empty());
    redo(&mut t);
    assert!(file.exists() && t.engine.prefab_instance(hero).is_some());

    // place a copy: at the camera centre, selected, two entities, undoable
    tab(&mut t, "prefabs");
    let rows_before = t.resource::<EditorPanel>().rows.len();
    press(&mut t, "prefabs/list/p0");
    assert_eq!(t.resource::<EditorPanel>().rows.len(), rows_before + 2);
    let copy = t.resource::<EditorSelection>().entity.expect("the new root is selected");
    assert!(copy != hero && t.engine.prefab_instance(copy).unwrap().is_root());
    assert_eq!(pos(&t, copy), Vec2::ZERO, "the editor camera starts at the origin");
    assert_eq!(instances(&t, "Hero").len(), 4);
    assert!(t.engine.prefab_overrides(copy).is_empty());
    undo(&mut t);
    assert_eq!(instances(&t, "Hero").len(), 2);
    assert_eq!(t.resource::<EditorPanel>().rows.len(), rows_before);
    redo(&mut t);
    let copy = t.resource::<EditorSelection>().entity.expect("redo selects the copy");
    assert_eq!(instances(&t, "Hero").len(), 4);
    assert!(t.engine.prefab_instance(copy).is_some(), "the link survives undo and redo");

    // an overridden field is marked and can be reverted
    tab(&mut t, "hierarchy");
    let row = field_row(&t, "Transform", "position");
    assert!(!t.text(&format!("{row}/name")).ends_with('*') && t.ui().find(&format!("{row}/revert")).is_none());
    type_into(&mut t, &format!("{row}/value/e0"), "5");
    assert_eq!(pos(&t, copy).x, 5.0);
    let row = field_row(&t, "Transform", "position");
    assert!(t.text(&format!("{row}/name")).ends_with('*'), "the label is marked: {}", t.text(&format!("{row}/name")));
    t.click(&format!("{row}/revert"));
    t.step(4);
    assert_eq!(pos(&t, copy), Vec2::ZERO, "revert puts the prefab value back");
    assert!(t.engine.prefab_overrides(copy).is_empty());
    assert!(t.ui().find(&format!("{}/revert", field_row(&t, "Transform", "position"))).is_none());
    undo(&mut t);
    assert_eq!(pos(&t, copy).x, 5.0, "revert is one undo step");
    undo(&mut t);
    assert_eq!(pos(&t, copy).x, 0.0);

    // edit the copy's child, apply: the other instance follows, the file changes
    let pet_copy = t.engine.prefab_instance_entities(copy)[1];
    let hero_pet = entity(&t, "pet");
    assert!(pet_copy != hero_pet);
    t.click(&format!("debug_menu/editor/hierarchy/list/e{}/row", efeengine_core::ecs::entity_to_bits(pet_copy)));
    t.step(3);
    let row = field_row(&t, "Transform", "position");
    type_into(&mut t, &format!("{row}/value/e0"), "3");
    assert_eq!(pos(&t, pet_copy).x, 3.0);
    let p_before = t.engine.prefab_overrides(pet_copy);
    assert_eq!(p_before, vec![("Transform".to_string(), "position".to_string())]);
    let apply = format!("{INSP}/prefab/actions/apply");
    assert!(t.ui().is_enabled(t.id(&apply)));
    let file_before = std::fs::read_to_string(&file).unwrap();
    t.click(&apply);
    t.step(4);
    assert_ne!(std::fs::read_to_string(&file).unwrap(), file_before, "the prefab file was written");
    assert!(t.engine.prefab_overrides(pet_copy).is_empty(), "no overrides left on the source");
    assert_eq!(pos(&t, hero_pet).x, 3.0, "another instance follows the prefab");
    assert_eq!(t.engine.load_prefab("Hero").unwrap().entities[1].components["Transform"]["position"], efeengine_core::ecs::SceneValue::List(vec![efeengine_core::ecs::SceneValue::Float(3.0), efeengine_core::ecs::SceneValue::Float(0.0)]));
    // undo: file, links and the followed value are back
    undo(&mut t);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), file_before);
    assert_eq!(pos(&t, hero_pet).x, 0.0);
    assert_eq!(t.engine.prefab_overrides(pet_copy), p_before);
    redo(&mut t);
    assert!(t.engine.prefab_overrides(pet_copy).is_empty());
    assert_eq!(pos(&t, hero_pet).x, 3.0);

    // the scene file: references and overrides only
    let out = dir.join("level.ron");
    set_path(&mut t, &text_path(&out));
    press(&mut t, "scene/bar/save_as");
    let text = std::fs::read_to_string(&out).unwrap();
    let saved = Scene::from_ron(&text).unwrap();
    let placed: Vec<_> = saved.entities.iter().filter(|e| e.prefab.is_some()).collect();
    assert_eq!(placed.len(), 2, "{text}");
    assert!(placed.iter().all(|e| e.components.is_empty() && e.prefab.as_deref() == Some("Hero")));
    assert_eq!(saved.entities.len(), 4, "two placements, the rock and the camera: {text}");
    // the hero keeps its own position as the only override
    assert!(placed.iter().any(|e| e.overrides.get("Transform").is_some_and(|f| f.contains_key("position"))));

    // unpack the copy: the link is gone for the whole instance, undo brings it back
    t.click(&format!("debug_menu/editor/hierarchy/list/e{}/row", efeengine_core::ecs::entity_to_bits(copy)));
    t.step(3);
    t.click(&format!("{INSP}/prefab/actions/unpack"));
    t.step(4);
    assert!(t.engine.prefab_instance(copy).is_none() && t.engine.prefab_instance(pet_copy).is_none());
    assert_eq!(instances(&t, "Hero").len(), 2);
    assert!(t.ui().find(&format!("{INSP}/prefab")).is_none(), "no prefab section any more");
    undo(&mut t);
    assert_eq!(instances(&t, "Hero").len(), 4);
    redo(&mut t);
    assert_eq!(instances(&t, "Hero").len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_prefab_name_or_play_mode_is_refused_with_a_message() {
    let dir = tmp("badname");
    let mut t = editor_in(&dir.join("p"));
    pick(&mut t, "hero");
    tab(&mut t, "prefabs");
    t.set_text("debug_menu/editor/prefabs/name", "bad/name");
    t.step(3);
    press(&mut t, "prefabs/actions/save");
    assert!(ui_state(&t).status.contains("not a prefab name"), "{}", ui_state(&t).status);
    assert!(!dir.join("p").exists());
    t.set_text("debug_menu/editor/prefabs/name", "Fine");
    t.step(3);
    press(&mut t, "toolbar/play");
    press(&mut t, "prefabs/actions/save");
    assert!(!dir.join("p").join("Fine.ron").exists());
    assert!(ui_state(&t).status.contains("Edit mode") || !t.ui().is_enabled(t.id("debug_menu/editor/prefabs/actions/save")));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- viewport polish ---------------------------------------------------------------------------

#[test]
fn the_handles_toggle_hides_the_handles_of_entities_without_a_sprite() {
    let mut t = editor();
    t.step(20);
    let rock = entity(&t, "rock");
    let with = t.resource::<Drawn>().0;
    assert!(with >= 2, "handles are drawn: {with}");
    assert_eq!(viewport::pick(&t.engine, Vec2::new(40.0, 40.0), 1.0), None, "empty space");
    t.set_checked("debug_menu/editor/options/handles", false);
    t.step(20);
    assert!(!t.resource::<EditorView>().handles);
    let without = t.resource::<Drawn>().0;
    assert!(without < with, "{without} vs {with}");
    assert_ne!(viewport::pick(&t.engine, Vec2::ZERO, 1.0), Some(rock), "a hidden handle cannot be picked");
    t.set_checked("debug_menu/editor/options/handles", true);
    t.step(20);
    assert_eq!(t.resource::<Drawn>().0, with);
}

#[test]
fn the_editor_window_panel_follows_the_window_size() {
    assert_eq!(super::panel::panel_size((0, 0)), (540.0, 660.0));
    assert_eq!(super::panel::panel_size((1100, 720)), (495.0, 720.0));
    assert_eq!(super::panel::panel_size((3000, 2000)), (760.0, 2000.0), "capped width, full height");
    assert_eq!(super::panel::panel_size((600, 400)), (420.0, 400.0));
    assert_eq!(super::panel::panel_size((300, 400)), (300.0, 400.0), "never wider than the window");
}
