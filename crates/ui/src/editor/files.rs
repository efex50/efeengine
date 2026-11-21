//! Scene files (docs/IN_GAME_EDITOR.md, step 2): Save, Save As, Load, New and the `scenes/` list.
//!
//! A saved scene is `Engine::save_world_scene` (every entity the editor lists, with ids, so
//! `Parent` survives) plus a sidecar `<file>.names` with the `Name` of every entity that has one
//! and an `EditorId` (`Name` is Rust-only, scene files cannot hold it). Paths are relative to the
//! current directory (the project dir when the game runs from it).

use std::path::{Path, PathBuf};

use efeengine_core::ecs::Scene;
use efeengine_core::prelude::*;

use super::edit::{reset, say, select, with_ui};
use super::{EditorMode, EditorState};

/// The directory whose `*.ron` files the Load list shows.
pub const SCENES_DIR: &str = "scenes";

fn names_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".names");
    PathBuf::from(s)
}

/// The `.ron` files of `scenes/`, sorted, as paths text (`scenes/demo.ron`).
pub fn list_scenes() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(SCENES_DIR)
        .into_iter()
        .flatten()
        .filter_map(|d| d.ok())
        .map(|d| d.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ron"))
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    out.sort();
    out
}

/// Saves the editable world to `path` (and the names next to it). Creates missing directories.
pub fn save_scene_with_names(e: &Engine, path: impl AsRef<Path>) -> Result<(), String> {
    let path = path.as_ref();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|x| format!("{}: {x}", dir.display()))?;
    }
    e.save_world_scene(path).map_err(|x| x.to_string())?;
    let mut lines: Vec<(u64, String)> =
        e.world().query::<(&EditorId, &Name)>().iter().filter(|(_, n)| !n.0.is_empty()).map(|(i, n)| (i.id, n.0.replace(['\n', '\r', '\t'], " "))).collect();
    lines.sort();
    let side = names_path(path);
    if lines.is_empty() {
        let _ = std::fs::remove_file(&side);
    } else {
        let text: String = lines.iter().map(|(id, n)| format!("{id}\t{n}\n")).collect();
        std::fs::write(&side, text).map_err(|x| format!("{}: {x}", side.display()))?;
    }
    Ok(())
}

/// Reads, validates and loads the scene at `path` (nothing changes when it is invalid), then puts
/// the names back. `replace` first empties the editable world and unloads the current scene
/// (what the editor's Load does); without it the scene is loaded next to what is there (what a
/// game does at startup). Returns the number of entities.
pub fn load_scene_with_names(e: &mut Engine, path: impl AsRef<Path>, replace: bool) -> Result<usize, String> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|x| format!("{}: {x}", path.display()))?;
    let scene = Scene::from_ron(&text).map_err(|x| x.to_string())?;
    e.validate_scene(&scene).map_err(|x| x.to_string())?;
    if replace {
        e.clear_editable_world();
    }
    let spawned = e.load_scene(&scene).map_err(|x| x.to_string())?;
    if let Ok(names) = std::fs::read_to_string(names_path(path)) {
        for line in names.lines() {
            let Some((id, name)) = line.split_once('\t') else { continue };
            if let Some(ent) = id.parse::<u64>().ok().and_then(|id| e.entity_by_editor_id(id)) {
                e.set_entity_name(ent, name);
            }
        }
    }
    Ok(spawned.len())
}

fn in_edit(e: &Engine) -> bool {
    e.resources().get::<EditorState>().is_some_and(|s| s.mode == EditorMode::Edit)
}

pub(crate) const NOT_IN_EDIT: &str = "Scene files can only be changed in Edit mode (press Stop first)";

/// Refreshes the list of `scenes/*.ron`.
pub(crate) fn scan(e: &mut Engine) {
    let files = list_scenes();
    with_ui(e, |ui| ui.scene_files = files);
}

pub(crate) fn save(e: &mut Engine) {
    if !in_edit(e) {
        return say(e, NOT_IN_EDIT, true);
    }
    let Some(path) = e.resources().get::<super::EditorUi>().and_then(|u| u.path.clone()) else {
        return say(e, "This scene has no file yet: type a path and use Save As", true);
    };
    write(e, path);
}

pub(crate) fn save_as(e: &mut Engine, text: String) {
    if !in_edit(e) {
        return say(e, NOT_IN_EDIT, true);
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return say(e, "Type a file path first (for example scenes/level1.ron)", true);
    }
    let path = if Path::new(&text).extension().is_some() { text } else { format!("{text}.ron") };
    write(e, path);
}

fn write(e: &mut Engine, path: String) {
    match save_scene_with_names(e, &path) {
        Ok(()) => {
            global_info(&format!("[editor] saved {path}"));
            with_ui(e, |ui| {
                ui.path = Some(path.clone());
                ui.path_text = path.clone();
                ui.dirty = false;
                ui.status = format!("Saved {path}");
                ui.status_error = false;
            });
            scan(e);
        }
        Err(msg) => say(e, format!("Save failed: {msg}"), true),
    }
}

pub(crate) fn load(e: &mut Engine, text: String) {
    if !in_edit(e) {
        return say(e, NOT_IN_EDIT, true);
    }
    let path = text.trim().to_string();
    if path.is_empty() {
        return say(e, "Type a file path first, or pick one from the list", true);
    }
    match load_scene_with_names(e, &path, true) {
        Ok(n) => {
            global_info(&format!("[editor] loaded {path} ({n} entities)"));
            reset(e);
            select(e, None);
            with_ui(e, |ui| {
                ui.path = Some(path.clone());
                ui.path_text = path.clone();
                ui.dirty = false;
                ui.status = format!("Loaded {path} ({n} entities)");
                ui.status_error = false;
            });
            scan(e);
        }
        Err(msg) => say(e, format!("Load failed: {msg}"), true),
    }
}

pub(crate) fn new_scene(e: &mut Engine) {
    if !in_edit(e) {
        return say(e, NOT_IN_EDIT, true);
    }
    e.clear_editable_world();
    reset(e);
    select(e, None);
    with_ui(e, |ui| {
        ui.path = None;
        ui.dirty = false;
        ui.status = "New empty scene".to_string();
        ui.status_error = false;
    });
}
