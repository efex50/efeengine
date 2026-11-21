//! The in-game scene editor (docs/IN_GAME_EDITOR.md): a plugin on top of the Debug menu. Steps 1
//! to 3: play modes, the hierarchy and an editable inspector in an "Editor" tab, entity actions,
//! undo / redo, scene files, and the viewport: an editor camera (pan, zoom, frame), picking,
//! move / rotate / scale gizmos with snapping and a grid, reparenting, and an optional separate
//! editor window. Build with feature `editor`; add it after `UiPlugin` and `DebugMenuPlugin`.
//!
//! ```ignore
//! engine.add_plugin(UiPlugin::new())?;
//! engine.add_plugin(DebugMenuPlugin)?;
//! engine.add_plugin(EditorPlugin::new())?; // F5, tab "Editor"
//! ```
//!
//! - **Modes** ([`EditorMode`], resource [`EditorState`]): Edit freezes every game system (not
//!   global, not a built-in or `efeengine*` plugin system; mark more with
//!   `Engine::set_global_system`), Play snapshots the world and runs, Pause freezes again, Step
//!   runs one frame, Stop restores the snapshot (or keeps the changes, `keep_changes`). Requests:
//!   [`EditorRequest`] events or the toolbar. Only `#[script_component]` components and resources
//!   registered with `register_snapshot_resource` survive Stop (`Name` too: it is in the snapshot).
//! - **Hierarchy and inspector**: `Engine::editable_entities` / `inspect_entity`, copied into
//!   [`EditorPanel`] at most every [`REFRESH`] seconds while the tab shows, and at once after a
//!   selection or mode change. The panel resource is written only when its content changed, so
//!   the debug menu view stays asleep otherwise.
//! - **Editing** (`edit.rs`): inspector fields (text, checkbox, one input per list element) write
//!   through `Engine::set_component_field` on Enter or focus loss, a rejected text keeps the
//!   value and shows an error; add / remove component, New / Duplicate / Delete / Rename entity.
//!   Allowed in every mode (in Play Stop undoes them unless "Keep changes"). Every edit is logged
//!   in [`EditorHistory`] with its inverse, entities named by `EditorId`; Ctrl+Z, Ctrl+Shift+Z /
//!   Ctrl+Y (not while a text input has focus) or the toolbar undo and redo.
//! - **Scene files** (`files.rs`): Save, Save As, Load, New and the `scenes/*.ron` list; the dirty
//!   marker `*` shows unsaved edits. Edit mode only.
//! - **Viewport** (`viewport.rs`): [`EditorCamera`] (an `EditorOnly` camera, never saved) renders
//!   the main window in Edit / Paused (layout A) or its own window (layout B,
//!   [`EditorPlugin::separate_window`]); middle drag or Space + left drag pans, the wheel zooms
//!   at the cursor, F frames the selection, Home resets; a left click picks, W / E / R choose
//!   the gizmo ([`GizmoTool`]), G toggles the grid, Ctrl snaps; a drag is one undo entry.
//! - **Selection** ([`EditorSelection`]): the entity and its `EditorId` (entity ids change on
//!   Stop; the id is how the selection survives). A rectangle is drawn around a selected sprite
//!   with `DebugDraw2D`.

mod edit;
mod files;
mod mode;
mod panel;
pub mod viewport;

pub use edit::{EditorHistory, HISTORY_LIMIT, parse_scalar, removable, with_element};
pub use files::{SCENES_DIR, list_scenes};
pub use mode::{EditorMode, EditorRequest};
pub use viewport::EditorCamera;

use std::collections::BTreeMap;

use efeengine_core::engine::inspect::{EntityRow, InspectedComponent};
use efeengine_core::prelude::*;

use crate::debug_menu::{DebugExtraTabs, DebugMsg, DebugState, DebugTab};
use crate::view::{RunCondition, UiView, UiViews};

/// The id of the tab in [`DebugExtraTabs`] (view keys live under `debug_menu/editor`).
pub const TAB: &str = "editor";

/// How often (seconds) the hierarchy and the inspector read the world while the tab shows.
pub const REFRESH: f32 = 0.25;

/// The name of the panel view that lives in the editor window (layout B).
pub(crate) const PANEL_VIEW: &str = "editor_panel";

/// The plugin name: systems of plugins whose name starts with `efeengine` are never frozen.
const PLUGIN_NAME: &str = "efeengine-editor";

/// Where the plugin starts: Edit (the default: the game starts frozen) or Play (nothing is frozen
/// until the first Pause; Stop then freezes without restoring, there is no snapshot).
pub struct EditorPlugin {
    start: EditorMode,
    separate_window: bool,
    scenes_dir: String,
}

impl EditorPlugin {
    pub fn new() -> Self {
        EditorPlugin { start: EditorMode::Edit, separate_window: false, scenes_dir: SCENES_DIR.to_string() }
    }
    /// Layout B from the first frame: the editor camera and the editor panels get their own
    /// window (see [`EditorView`]). Without a second window (SDL mode, a custom renderer, a
    /// backend that cannot) a warning is logged and the layout stays A. The toolbar's Detach /
    /// Attach button switches at runtime.
    pub fn separate_window(mut self, on: bool) -> Self {
        self.separate_window = on;
        self
    }
    /// The directory whose `*.ron` files the Load list shows (default `scenes`, relative to the
    /// current directory).
    pub fn scenes_dir(mut self, dir: impl Into<String>) -> Self {
        self.scenes_dir = dir.into();
        self
    }
    /// `Edit` or `Play` (`Paused` is `Edit`).
    pub fn start_in(mut self, mode: EditorMode) -> Self {
        self.start = if mode == EditorMode::Play { EditorMode::Play } else { EditorMode::Edit };
        self
    }
}

impl Default for EditorPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// The mode and what it froze. A resource; written only on a real change.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorState {
    pub mode: EditorMode,
    /// Stop keeps the world as it is instead of restoring the Play snapshot.
    pub keep_changes: bool,
    /// How many game systems the current mode keeps frozen.
    pub frozen: usize,
}

/// Plugin settings, readable by the systems. A resource.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditorConfig {
    pub scenes_dir: String,
    pub separate_window: bool,
}

/// The gizmo the viewport shows for the selection (keys W / E / R).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GizmoTool {
    #[default]
    Move,
    Rotate,
    Scale,
}

/// Viewport settings and layout. A resource; written only on a real change (the toolbar shows it).
#[derive(Clone, Debug, PartialEq)]
pub struct EditorView {
    pub tool: GizmoTool,
    /// Draw the grid (key G).
    pub grid: bool,
    /// World units between grid lines, and the snap step while Ctrl is held.
    pub grid_step: f32,
    /// Layout B: the editor camera and the panels are in their own window.
    pub detached: bool,
    /// That window (`Some` exactly while `detached`).
    pub window: Option<u32>,
}

impl Default for EditorView {
    fn default() -> Self {
        EditorView { tool: GizmoTool::Move, grid: false, grid_step: 16.0, detached: false, window: None }
    }
}

/// The selected entity. A resource.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorSelection {
    pub entity: Option<Entity>,
    /// The entity's `EditorId` when it has one: re-selects it after Stop.
    pub editor_id: Option<u64>,
}

/// What the Editor tab shows: a copy of the world's entity list and the selected entity's
/// components (refreshed throttled), and the filter text. A resource.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorPanel {
    pub rows: Vec<EntityRow>,
    pub inspected: Option<Vec<InspectedComponent>>,
    /// Case-insensitive substring filter of the hierarchy.
    pub filter: String,
    /// The selected entity's `Name` component (`None`: it has none).
    pub selected_name: Option<String>,
    /// Components the selected entity lacks and the picker offers.
    pub addable: Vec<String>,
    /// The engine's current scene (`Engine::current_scene`).
    pub scene: Option<String>,
}

/// One input of the inspector: component, field, element of a list field (0 for a scalar). The
/// entity name input is [`FieldKey::name`].
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct FieldKey {
    pub component: String,
    pub field: String,
    pub index: usize,
}

impl FieldKey {
    /// The key of the grid step input of the toolbar.
    pub fn grid() -> Self {
        FieldKey::new("@grid", "", 0)
    }
    pub fn is_grid(&self) -> bool {
        self.component == "@grid"
    }
    pub fn new(component: &str, field: &str, index: usize) -> Self {
        FieldKey { component: component.to_string(), field: field.to_string(), index }
    }
    /// The key of the name input at the top of the inspector.
    pub fn name() -> Self {
        FieldKey::new("@name", "", 0)
    }
    pub fn is_name(&self) -> bool {
        self.component == "@name"
    }
    /// A string that identifies the input (for the focus tracker).
    pub fn tag(&self) -> String {
        format!("{}.{}.{}", self.component, self.field, self.index)
    }
}

/// What the user is doing in the tab: half-typed values, errors, status line, scene file state.
/// A resource; written only on a real change (a write wakes the view).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorUi {
    /// Typed text not committed yet (shown instead of the world's value).
    pub drafts: BTreeMap<FieldKey, String>,
    /// Inputs whose text was rejected, with the reason.
    pub errors: BTreeMap<FieldKey, String>,
    /// The editor text input that has keyboard focus (a tag; `None`: none). Shortcuts wait for it.
    pub focus: Option<String>,
    pub status: String,
    pub status_error: bool,
    /// The "Add component" list is open.
    pub picker_open: bool,
    /// The file the scene came from or was last saved to; Save writes there.
    pub path: Option<String>,
    /// The text in the path field.
    pub path_text: String,
    /// `*.ron` files of `scenes/`.
    pub scene_files: Vec<String>,
    /// Edits since the last save, load or New.
    pub dirty: bool,
    pub undo_len: usize,
    pub redo_len: usize,
    /// The "Parent" list of the inspector is open.
    pub parent_picker: bool,
}

/// What the tab's widgets send (inside `DebugMsg::Ext`) and what other code may send as an event.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorMsg {
    Request(EditorRequest),
    Select(Option<Entity>),
    Filter(String),
    KeepChanges(bool),
    /// Typing in an inspector input (the whole text so far).
    Draft(FieldKey, String),
    /// Enter pressed in an inspector input (or its focus was lost with a draft).
    Commit(FieldKey, String),
    /// A checkbox of a `Bool` field.
    Toggle(FieldKey, bool),
    /// An editor text input gained (`true`) or lost focus (the tag names it).
    Focus(String, bool),
    AddComponent(String),
    RemoveComponent(String),
    /// Opens or closes the "Add component" list.
    Picker(bool),
    NewEntity,
    Duplicate,
    Delete,
    Undo,
    Redo,
    /// Typing in the scene path field.
    PathText(String),
    Save,
    /// Save to the path in the path field.
    SaveAs,
    /// Load this file (replaces the editable world).
    Load(String),
    /// Unload the scene and empty the world.
    NewScene,
    /// The toolbar's tool buttons.
    Tool(GizmoTool),
    /// Grid on / off.
    Grid(bool),
    /// Open (`true`) or close (`false`) the separate editor window.
    Detach(bool),
    /// Make the selected entity a child of this one (`None`: unparent), keeping its world position.
    Reparent(Option<Entity>),
    /// Opens or closes the "Parent" list of the inspector.
    ParentPicker(bool),
}

/// Seconds since the last refresh and a "refresh now" flag. Private: writing it never wakes the view.
struct EditorClock {
    since: f32,
    force: bool,
}

/// The flag lives in a resource; tasks and systems set it through this.
pub(crate) fn request_refresh(e: &mut Engine) {
    if let Some(c) = e.resources_mut().get_mut::<EditorClock>() {
        c.force = true;
    }
}

fn ext(m: EditorMsg) -> DebugMsg {
    DebugMsg::Ext(crate::debug_menu::ExtMsg::new(TAB, m))
}

/// The editor tab is the one the open window shows.
fn tab_shown(cx: &Ctx) -> bool {
    cx.resources.get::<UiViews>().is_some_and(|v| v.is_enabled("debug_menu")) && cx.resources.get::<DebugState>().is_some_and(|s| s.tab == DebugTab::Extra(TAB))
}

/// Reads the toolbar and event messages and applies them.
fn editor_update(cx: &mut Ctx) {
    let mut msgs: Vec<EditorMsg> = cx.events::<EditorMsg>().iter().cloned().collect();
    msgs.extend(cx.events::<EditorRequest>().iter().map(|r| EditorMsg::Request(*r)));
    msgs.extend(cx.events::<DebugMsg>().iter().filter_map(|m| match m {
        DebugMsg::Ext(x) if x.tag == TAB => x.payload::<EditorMsg>().cloned(),
        _ => None,
    }));
    // The inspector's state is edited on a copy and written back only when it differs: a write
    // wakes the view.
    let Some(before) = cx.resources.get::<EditorUi>().map(|u| (*u).clone()) else { return };
    let mut ui = before.clone();
    // Escape in a focused input drops the typing (the blur that follows then commits nothing).
    if ui.focus.is_some() && cx.input.just_pressed(KeyCode::Escape) {
        ui.drafts.clear();
        ui.errors.clear();
    }
    for m in msgs {
        match m {
            EditorMsg::Request(r) => cx.commands.engine_task(move |e| mode::apply(e, r)),
            EditorMsg::KeepChanges(on) => {
                if cx.resources.get::<EditorState>().is_some_and(|s| s.keep_changes != on) {
                    if let Some(s) = cx.resources.get_mut::<EditorState>() {
                        s.keep_changes = on;
                    }
                }
            }
            EditorMsg::Filter(f) => {
                if cx.resources.get::<EditorPanel>().is_some_and(|p| p.filter != f) {
                    if let Some(p) = cx.resources.get_mut::<EditorPanel>() {
                        p.filter = f;
                    }
                }
            }
            EditorMsg::Select(entity) => {
                let editor_id = entity.and_then(|e| cx.world.get::<&EditorId>(e).ok().map(|i| i.id));
                let new = EditorSelection { entity, editor_id };
                if cx.resources.get::<EditorSelection>().is_some_and(|s| *s != new) {
                    if let Some(s) = cx.resources.get_mut::<EditorSelection>() {
                        *s = new;
                    }
                    if let Some(c) = cx.resources.get_mut::<EditorClock>() {
                        c.force = true;
                    }
                    ui.drafts.clear();
                    ui.errors.clear();
                    ui.picker_open = false;
                    ui.parent_picker = false;
                    ui.status.clear();
                    ui.status_error = false;
                }
            }
            EditorMsg::Draft(key, text) => {
                ui.errors.remove(&key);
                ui.drafts.insert(key, text);
            }
            EditorMsg::Commit(key, text) if key.is_grid() => match text.trim().parse::<f32>() {
                Ok(v) if v.is_finite() && v > 0.0 => {
                    ui.drafts.remove(&key);
                    ui.errors.remove(&key);
                    if cx.resources.get::<EditorView>().is_some_and(|x| x.grid_step != v) {
                        if let Some(x) = cx.resources.get_mut::<EditorView>() {
                            x.grid_step = v;
                        }
                    }
                }
                _ => {
                    ui.errors.insert(key.clone(), "not a positive number".to_string());
                    ui.drafts.insert(key, text);
                    ui.status = "grid step: type a number above 0".to_string();
                    ui.status_error = true;
                }
            },
            EditorMsg::Commit(key, text) => {
                ui.drafts.insert(key.clone(), text.clone());
                cx.commands.engine_task(move |e| edit::commit_field(e, key, text));
            }
            EditorMsg::Toggle(key, on) => cx.commands.engine_task(move |e| edit::toggle_field(e, key, on)),
            EditorMsg::Focus(tag, on) => {
                if on {
                    ui.focus = Some(tag);
                } else {
                    if ui.focus.as_deref() == Some(tag.as_str()) {
                        ui.focus = None;
                    }
                    // Focus lost with typed text that was never submitted: commit it (a rejected
                    // text stays as it is, with its error).
                    let pending = ui.drafts.iter().find(|(k, _)| k.tag() == tag && !ui.errors.contains_key(*k)).map(|(k, t)| (k.clone(), t.clone()));
                    if let Some((key, text)) = pending {
                        cx.commands.engine_task(move |e| edit::commit_field(e, key, text));
                    }
                }
            }
            EditorMsg::AddComponent(name) => cx.commands.engine_task(move |e| edit::add_component(e, name)),
            EditorMsg::RemoveComponent(name) => cx.commands.engine_task(move |e| edit::remove_component(e, name)),
            EditorMsg::Picker(open) => ui.picker_open = open,
            EditorMsg::NewEntity => cx.commands.engine_task(edit::new_entity),
            EditorMsg::Duplicate => cx.commands.engine_task(edit::duplicate),
            EditorMsg::Delete => cx.commands.engine_task(edit::delete),
            EditorMsg::Undo => cx.commands.engine_task(edit::undo),
            EditorMsg::Redo => cx.commands.engine_task(edit::redo),
            EditorMsg::PathText(t) => ui.path_text = t,
            EditorMsg::Save => cx.commands.engine_task(files::save),
            EditorMsg::SaveAs => {
                let text = ui.path_text.clone();
                cx.commands.engine_task(move |e| files::save_as(e, text));
            }
            EditorMsg::Load(path) => cx.commands.engine_task(move |e| files::load(e, path)),
            EditorMsg::NewScene => cx.commands.engine_task(files::new_scene),
            EditorMsg::Tool(t) => {
                if cx.resources.get::<EditorView>().is_some_and(|v| v.tool != t) {
                    if let Some(v) = cx.resources.get_mut::<EditorView>() {
                        v.tool = t;
                    }
                }
            }
            EditorMsg::Grid(on) => {
                if cx.resources.get::<EditorView>().is_some_and(|v| v.grid != on) {
                    if let Some(v) = cx.resources.get_mut::<EditorView>() {
                        v.grid = on;
                    }
                }
            }
            EditorMsg::Detach(on) => cx.commands.engine_task(move |e| viewport::set_detached(e, on)),
            EditorMsg::Reparent(p) => {
                ui.parent_picker = false;
                cx.commands.engine_task(move |e| edit::reparent(e, p));
            }
            EditorMsg::ParentPicker(open) => ui.parent_picker = open,
        }
    }
    if ui != before {
        if let Some(u) = cx.resources.get_mut::<EditorUi>() {
            *u = ui;
        }
    }
}

/// Ctrl+Z undoes, Ctrl+Shift+Z and Ctrl+Y redo. Only while the Editor tab shows, and never while
/// a text input has focus (the UI captures the keyboard then, and the editor tracks its own inputs).
fn editor_keys(cx: &mut Ctx) {
    let mods = cx.input.modifiers();
    if !mods.ctrl() || !(cx.input.just_pressed(KeyCode::Z) || cx.input.just_pressed(KeyCode::Y)) || !(tab_shown(cx) || viewport::detached_focused(cx)) {
        return;
    }
    let typing = cx.input.capture().keyboard || cx.resources.get::<EditorUi>().is_none_or(|u| u.focus.is_some());
    if typing {
        return;
    }
    let redo = cx.input.just_pressed(KeyCode::Y) || mods.shift();
    if redo {
        cx.commands.engine_task(edit::redo);
    } else {
        cx.commands.engine_task(edit::undo);
    }
}

/// Queues the read of the world into [`EditorPanel`] when it is due.
fn editor_refresh(cx: &mut Ctx) {
    let shown = tab_shown(cx);
    let dt = cx.time.delta();
    let (due, opened) = {
        let Some(c) = cx.resources.get_mut::<EditorClock>() else { return };
        let opened = shown && c.since.is_infinite();
        // Hidden: the next show reads at once.
        c.since = if shown { (c.since + dt).min(1.0e6) } else { f32::INFINITY };
        let due = shown && (c.force || c.since >= REFRESH);
        if due {
            c.since = 0.0;
            c.force = false;
        }
        if !shown {
            c.force = false;
        }
        (due, opened)
    };
    if opened {
        cx.commands.engine_task(files::scan);
    }
    if due {
        cx.commands.engine_task(refresh_panel);
    }
}

/// Copies the entity list and the selection's components into the panel (only when different).
fn refresh_panel(e: &mut Engine) {
    // The selection follows its `EditorId` when the entity is gone or was respawned.
    let sel = e.resources().get::<EditorSelection>().map(|s| (*s).clone()).unwrap_or_default();
    let mut entity = sel.entity.filter(|x| e.world().contains(*x));
    if entity.is_none() {
        entity = sel.editor_id.and_then(|id| e.entity_by_editor_id(id));
    }
    if entity != sel.entity {
        if let Some(s) = e.resources_mut().get_mut::<EditorSelection>() {
            s.entity = entity;
            if entity.is_none() {
                s.editor_id = None;
            }
        }
    }
    let rows = e.editable_entities();
    let inspected = entity.and_then(|x| e.inspect_entity(x));
    let selected_name = entity.and_then(|x| e.world().get::<&Name>(x).ok().map(|n| n.0.clone()));
    let addable = entity.map(|x| e.addable_components(x).into_iter().filter(|n| n != "Parent").collect()).unwrap_or_default();
    let scene = e.current_scene().map(str::to_string);
    let stale = e
        .resources()
        .get::<EditorPanel>()
        .is_none_or(|p| p.rows != rows || p.inspected != inspected || p.selected_name != selected_name || p.addable != addable || p.scene != scene);
    if stale {
        if let Some(p) = e.resources_mut().get_mut::<EditorPanel>() {
            p.rows = rows;
            p.inspected = inspected;
            p.selected_name = selected_name;
            p.addable = addable;
            p.scene = scene;
        }
    }
}

/// The first frame: freeze the game when starting in Edit.
fn editor_boot(cx: &mut Ctx) {
    let Some(start) = cx.resources.get_mut::<Boot>().and_then(|b| b.0.take()) else { return };
    let detach = cx.resources.get::<EditorConfig>().is_some_and(|c| c.separate_window);
    viewport::boot(cx, detach);
    match start {
        EditorMode::Edit => cx.commands.engine_task(mode::enter_edit),
        _ => cx.commands.engine_task(mode::enter_play_unrecorded),
    }
}

struct Boot(Option<EditorMode>);

fn editor_step_end(cx: &mut Ctx) {
    if mode::step_active(cx) {
        cx.commands.engine_task(mode::step_end);
    }
}

impl Plugin for EditorPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn dependencies(&self) -> &[&'static str] {
        &["efeengine-ui"]
    }

    fn build(&self, engine: &mut Engine) {
        engine.insert_resource(EditorState { mode: self.start, keep_changes: false, frozen: 0 });
        engine.insert_resource(EditorSelection::default());
        engine.insert_resource(EditorPanel::default());
        engine.insert_resource(EditorUi::default());
        engine.insert_resource(EditorHistory::default());
        engine.insert_resource(EditorView::default());
        engine.insert_resource(EditorConfig { scenes_dir: self.scenes_dir.clone(), separate_window: self.separate_window });
        engine.insert_resource(viewport::ViewportRt::default());
        viewport::spawn_camera(engine);
        engine.insert_resource(EditorClock { since: f32::INFINITY, force: false });
        engine.insert_resource(mode::ModeRuntime::default());
        engine.insert_resource(Boot(Some(self.start)));
        engine.add_system(Stage::PreUpdate, editor_boot).order(order::EARLY).name("editor_boot");
        engine.add_system(Stage::Update, editor_update).order(order::EARLY + 1).name("editor_update");
        engine.add_system(Stage::Update, editor_keys).order(order::EARLY + 2).name("editor_keys");
        engine.add_system(Stage::PostUpdate, editor_refresh).order(order::LATE).name("editor_refresh");
        engine.add_system(Stage::Update, viewport::input).order(order::EARLY + 3).name("editor_viewport_input");
        engine.add_system(Stage::Update, viewport::window_watch).order(order::EARLY + 4).name("editor_window_watch");
        engine.add_system(Stage::PostUpdate, viewport::camera_apply).order(order::LATE).name("editor_camera");
        engine.add_system(Stage::PostUpdate, viewport::overlay).order(order::TRANSFORM + 1).name("editor_overlay");
        engine.add_system(Stage::PostUpdate, viewport::place_panel).order(order::LATE + 12).name("editor_place_panel");
        engine
            .add_plugin(
                UiView::with(PANEL_VIEW, panel::window_view)
                    .layer(crate::Layer::top_left(40))
                    .enabled(false)
                    .when(RunCondition::custom(|cx| cx.res_changed::<EditorState>() || cx.res_changed::<EditorSelection>() || cx.res_changed::<EditorPanel>() || cx.res_changed::<EditorUi>() || cx.res_changed::<EditorView>())),
            )
            .expect("editor panel view");
        engine.add_system(Stage::RenderExtract, editor_step_end).order(order::EARLY).name("editor_step_end");
        let tabs = engine.resources_mut().get_mut::<DebugExtraTabs>().expect("EditorPlugin needs DebugMenuPlugin added first");
        tabs.add(
            TAB,
            "Editor",
            panel::view,
            |cx| cx.res_changed::<EditorState>() || cx.res_changed::<EditorSelection>() || cx.res_changed::<EditorPanel>() || cx.res_changed::<EditorUi>() || cx.res_changed::<EditorView>(),
        );
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_viewport;
