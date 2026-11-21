//! The in-game scene editor (docs/IN_GAME_EDITOR.md): a plugin on top of the Debug menu. Step 1:
//! play modes, the hierarchy and a read-only inspector in an "Editor" tab, and a selection outline
//! in the world. Build with feature `editor`; add it after `UiPlugin` and `DebugMenuPlugin`.
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
//!   registered with `register_snapshot_resource` survive Stop; `Name` is put back by `EditorId`.
//! - **Hierarchy and inspector**: `Engine::editable_entities` / `inspect_entity`, copied into
//!   [`EditorPanel`] at most every [`REFRESH`] seconds while the tab shows, and at once after a
//!   selection or mode change. The panel resource is written only when its content changed, so
//!   the debug menu view stays asleep otherwise.
//! - **Selection** ([`EditorSelection`]): the entity and its `EditorId` (entity ids change on
//!   Stop; the id is how the selection survives). A rectangle is drawn around a selected sprite
//!   with `DebugDraw2D`.

mod mode;
mod panel;

pub use mode::{EditorMode, EditorRequest};

use efeengine_core::engine::inspect::{EntityRow, InspectedComponent};
use efeengine_core::prelude::*;

use crate::debug_menu::{DebugExtraTabs, DebugMsg, DebugState, DebugTab};
use crate::view::UiViews;

/// The id of the tab in [`DebugExtraTabs`] (view keys live under `debug_menu/editor`).
pub const TAB: &str = "editor";

/// How often (seconds) the hierarchy and the inspector read the world while the tab shows.
pub const REFRESH: f32 = 0.25;

/// The plugin name: systems of plugins whose name starts with `efeengine` are never frozen.
const PLUGIN_NAME: &str = "efeengine-editor";

/// Where the plugin starts: Edit (the default: the game starts frozen) or Play (nothing is frozen
/// until the first Pause; Stop then freezes without restoring, there is no snapshot).
pub struct EditorPlugin {
    start: EditorMode,
}

impl EditorPlugin {
    pub fn new() -> Self {
        EditorPlugin { start: EditorMode::Edit }
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
}

/// What the tab's widgets send (inside `DebugMsg::Ext`) and what other code may send as an event.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorMsg {
    Request(EditorRequest),
    Select(Option<Entity>),
    Filter(String),
    KeepChanges(bool),
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
                }
            }
        }
    }
}

/// Queues the read of the world into [`EditorPanel`] when it is due.
fn editor_refresh(cx: &mut Ctx) {
    let shown = tab_shown(cx);
    let dt = cx.time.delta();
    let due = {
        let Some(c) = cx.resources.get_mut::<EditorClock>() else { return };
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
        due
    };
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
    let stale = e.resources().get::<EditorPanel>().is_none_or(|p| p.rows != rows || p.inspected != inspected);
    if stale {
        if let Some(p) = e.resources_mut().get_mut::<EditorPanel>() {
            p.rows = rows;
            p.inspected = inspected;
        }
    }
}

/// The first frame: freeze the game when starting in Edit.
fn editor_boot(cx: &mut Ctx) {
    let Some(start) = cx.resources.get_mut::<Boot>().and_then(|b| b.0.take()) else { return };
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

/// A rectangle around the selected sprite.
fn editor_outline(cx: &mut Ctx) {
    let Some(entity) = cx.resources.get::<EditorSelection>().and_then(|s| s.entity) else { return };
    let mut q = cx.world.query_one::<(&Sprite, &GlobalTransform)>(entity);
    let Ok((s, g)) = q.get() else { return };
    let size = if s.auto_size != 0 {
        cx.resources.get::<Textures>().and_then(|t| t.size(s.texture)).map_or(Vec2::ONE, |(w, h)| Vec2::new(w as f32, h as f32))
    } else {
        s.size
    };
    let m = g.affine();
    let h = size * 0.5;
    let c = [m.transform_point2(Vec2::new(-h.x, -h.y)), m.transform_point2(Vec2::new(h.x, -h.y)), m.transform_point2(Vec2::new(h.x, h.y)), m.transform_point2(Vec2::new(-h.x, h.y))];
    drop(q);
    if let Some(d) = cx.resources.get_mut::<DebugDraw2D>() {
        let color = [1.0, 0.8, 0.1, 1.0];
        for i in 0..4 {
            d.line(c[i], c[(i + 1) % 4], color);
        }
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
        engine.insert_resource(EditorClock { since: f32::INFINITY, force: false });
        engine.insert_resource(mode::ModeRuntime::default());
        engine.insert_resource(Boot(Some(self.start)));
        engine.add_system(Stage::PreUpdate, editor_boot).order(order::EARLY).name("editor_boot");
        engine.add_system(Stage::Update, editor_update).order(order::EARLY + 1).name("editor_update");
        engine.add_system(Stage::PostUpdate, editor_refresh).order(order::LATE).name("editor_refresh");
        engine.add_system(Stage::PostUpdate, editor_outline).order(order::TRANSFORM + 1).name("editor_outline");
        engine.add_system(Stage::RenderExtract, editor_step_end).order(order::EARLY).name("editor_step_end");
        let tabs = engine.resources_mut().get_mut::<DebugExtraTabs>().expect("EditorPlugin needs DebugMenuPlugin added first");
        tabs.add(
            TAB,
            "Editor",
            panel::view,
            |cx| cx.res_changed::<EditorState>() || cx.res_changed::<EditorSelection>() || cx.res_changed::<EditorPanel>(),
        );
    }
}

#[cfg(test)]
mod tests;
