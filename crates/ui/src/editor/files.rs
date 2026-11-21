//! Scene files (docs/IN_GAME_EDITOR.md, step 2): Save, Save As, Load, New and the scene list.
//!
//! A saved scene is `Engine::save_world_scene`: every entity the editor lists, with ids (so
//! `Parent` survives) and its `Name` (`SceneEntity::name`). Paths are relative to the current
//! directory (the project dir when the game runs from it); the list shows the `*.ron` files of
//! the plugin's scenes directory ([`EditorPlugin::scenes_dir`](super::EditorPlugin::scenes_dir)).

use std::path::Path;

use efeengine_core::ecs::Scene;
use efeengine_core::prelude::*;

use super::edit::{reset, say, select, with_ui};
use super::{EditorConfig, EditorMode, EditorState};

/// The default directory whose `*.ron` files the Load list shows.
pub const SCENES_DIR: &str = "scenes";

/// The `.ron` files of `dir`, sorted, as path text (`scenes/demo.ron`).
pub fn list_scenes(dir: impl AsRef<Path>) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir.as_ref())
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

/// Saves the editable world to `path`, creating missing directories.
fn save_to(e: &Engine, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|x| format!("{}: {x}", dir.display()))?;
    }
    e.save_world_scene(path).map_err(|x| x.to_string())
}

/// Reads, validates and loads the scene at `path` (nothing changes when it is invalid), replacing
/// the editable world and the current scene. Returns the number of entities.
fn load_replacing(e: &mut Engine, path: &Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path).map_err(|x| format!("{}: {x}", path.display()))?;
    let scene = Scene::from_ron(&text).map_err(|x| x.to_string())?;
    e.validate_scene(&scene).map_err(|x| x.to_string())?;
    e.clear_editable_world();
    e.load_scene(&scene).map(|v| v.len()).map_err(|x| x.to_string())
}

fn in_edit(e: &Engine) -> bool {
    e.resources().get::<EditorState>().is_some_and(|s| s.mode == EditorMode::Edit)
}

pub(crate) const NOT_IN_EDIT: &str = "Scene files can only be changed in Edit mode (press Stop first)";

/// Refreshes the list of `scenes/*.ron`.
pub(crate) fn scan(e: &mut Engine) {
    let dir = e.resources().get::<EditorConfig>().map(|c| c.scenes_dir.clone()).unwrap_or_else(|| SCENES_DIR.to_string());
    let files = list_scenes(dir);
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
    match save_to(e, Path::new(&path)) {
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
    match load_replacing(e, Path::new(&path)) {
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
