//! Play states (docs/IN_GAME_EDITOR.md, "Modes"): which systems an editor mode freezes and how
//! Play / Pause / Step / Stop move between them. Everything here runs inside `Engine::engine_task`
//! closures (between systems, with `&mut Engine`), so snapshots and system flags change at one
//! well defined point of the frame.

use efeengine_core::ecs::Snapshot;
use efeengine_core::prelude::*;

use super::{EditorSelection, EditorState};

/// What the editor is doing with the game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorMode {
    /// The game's systems are frozen; the world can be inspected and edited.
    #[default]
    Edit,
    /// The game runs. Entering it from Edit took a snapshot; Stop puts the world back.
    Play,
    /// The game's systems are frozen again but the world stays as it is; Step runs one frame.
    Paused,
}

/// A mode change request. Sent as an event (`cx.send(EditorRequest::Play)`) or by the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorRequest {
    /// Same as `Stop` while playing or paused; nothing in Edit.
    Edit,
    /// Edit: snapshot, then run the game. Paused: resume. Play: nothing.
    Play,
    /// Play: freeze the game systems but keep the world.
    Pause,
    /// Paused: run the game systems for one frame.
    Step,
    /// Play or Paused: back to Edit; the world is restored from the snapshot unless
    /// `EditorState::keep_changes` is set.
    Stop,
}

/// Stages whose systems the editor freezes. `Startup` and `Shutdown` are one-shot and
/// `RenderExtract` carries the engine's own render step.
const FROZEN_STAGES: [Stage; 4] = [Stage::PreUpdate, Stage::FixedUpdate, Stage::Update, Stage::PostUpdate];

/// Built-in systems that keep running in every mode: the transform hierarchy (pickers and gizmos
/// need `GlobalTransform`).
const ALWAYS: [&str; 2] = ["propagate_transforms", "render_extract"];

/// Private runtime of the mode machine (a resource).
#[derive(Default)]
pub(crate) struct ModeRuntime {
    /// Taken when Play started from Edit.
    pub snapshot: Option<Snapshot>,
    /// The game systems Edit froze (enabled before): Play enables exactly these again.
    pub edit_list: Vec<String>,
    /// The game systems enabled when Pause froze them.
    pub pause_list: Vec<String>,
    /// 0 idle; 1 a Step was queued; 2 the stepped frame is running.
    pub step: u8,
}

/// A system the editor may freeze: not global, not an engine or tool system.
fn is_game(e: &Engine, name: &str, owner: Option<&str>) -> bool {
    !e.is_global_system(name) && !ALWAYS.contains(&name) && owner.is_none_or(|o| !o.starts_with("efeengine"))
}

/// Disables every enabled game system and returns their names.
pub(crate) fn freeze(e: &mut Engine) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for st in FROZEN_STAGES {
        let owners = e.system_owners(st);
        for ((name, enabled), owner) in e.system_states(st).into_iter().zip(owners) {
            if enabled && is_game(e, name, owner) && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    set_all(e, &names, false);
    names
}

fn set_all(e: &mut Engine, names: &[String], on: bool) {
    for n in names {
        let _ = e.set_system_enabled(n, on);
    }
}

fn rt(e: &mut Engine) -> &mut ModeRuntime {
    e.resources_mut().get_mut::<ModeRuntime>().expect("ModeRuntime")
}

fn take_rt(e: &mut Engine) -> ModeRuntime {
    std::mem::take(rt(e))
}

fn put_rt(e: &mut Engine, r: ModeRuntime) {
    *rt(e) = r;
}

fn set_state(e: &mut Engine, mode: EditorMode, frozen: usize) {
    let differs = e.resources().get::<EditorState>().is_some_and(|s| s.mode != mode || s.frozen != frozen);
    if differs {
        if let Some(s) = e.resources_mut().get_mut::<EditorState>() {
            s.mode = mode;
            s.frozen = frozen;
        }
    }
    super::request_refresh(e);
}

/// Freezes the game and sets the mode to Edit (the plugin's first frame).
pub(crate) fn enter_edit(e: &mut Engine) {
    let list = freeze(e);
    let n = list.len();
    rt(e).edit_list = list;
    set_state(e, EditorMode::Edit, n);
}

/// The state the plugin starts in when it does not freeze anything (`start_in(Play)`).
pub(crate) fn enter_play_unrecorded(e: &mut Engine) {
    set_state(e, EditorMode::Play, 0);
}

/// Runs one request. Requests that make no sense in the current mode are ignored.
pub(crate) fn apply(e: &mut Engine, req: EditorRequest) {
    let (mode, keep) = match e.resources().get::<EditorState>() {
        Some(s) => (s.mode, s.keep_changes),
        None => return,
    };
    let mut r = take_rt(e);
    match (req, mode) {
        (EditorRequest::Play, EditorMode::Edit) => {
            // The snapshot is taken with the game still frozen, so Stop brings the frozen flags back.
            r.snapshot = Some(e.snapshot());
            set_all(e, &r.edit_list, true);
            set_state(e, EditorMode::Play, 0);
        }
        (EditorRequest::Play, EditorMode::Paused) => {
            set_all(e, &r.pause_list, true);
            r.step = 0;
            set_state(e, EditorMode::Play, 0);
        }
        (EditorRequest::Pause, EditorMode::Play) => {
            r.pause_list = freeze(e);
            set_state(e, EditorMode::Paused, r.pause_list.len());
        }
        (EditorRequest::Step, EditorMode::Paused) => {
            if r.step == 0 {
                set_all(e, &r.pause_list, true);
                r.step = 1;
            }
        }
        (EditorRequest::Stop | EditorRequest::Edit, EditorMode::Play | EditorMode::Paused) => {
            let frozen_now = mode == EditorMode::Paused;
            // `Paused` is frozen already; a Play frame in progress is the game's last.
            let snap = r.snapshot.take();
            if r.step > 0 {
                set_all(e, &r.pause_list, false);
            }
            r.step = 0;
            match snap {
                Some(snap) if !keep => match e.restore(&snap) {
                    Ok(()) => {
                        // The restored flags are the ones the snapshot saw: Edit's frozen state.
                        super::edit::reset(e);
                        let sel = e.resources().get::<EditorSelection>().map(|s| s.editor_id);
                        if let Some(id) = sel {
                            let entity = id.and_then(|id| e.entity_by_editor_id(id));
                            if let Some(s) = e.resources_mut().get_mut::<EditorSelection>() {
                                s.entity = entity;
                                if entity.is_none() {
                                    s.editor_id = None;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        global_warn(&format!("editor: restoring the world failed ({err}); keeping the current state"));
                        r.edit_list = if frozen_now { r.pause_list.clone() } else { freeze(e) };
                    }
                },
                _ => {
                    // No snapshot (started in Play) or keep changes: freeze whatever runs now.
                    r.edit_list = if frozen_now { r.pause_list.clone() } else { freeze(e) };
                }
            }
            r.pause_list.clear();
            set_state(e, EditorMode::Edit, r.edit_list.len());
        }
        _ => {}
    }
    put_rt(e, r);
}

/// End of a Step: after one full frame of the game, freeze it again.
pub(crate) fn step_end(e: &mut Engine) {
    let r = rt(e);
    match r.step {
        1 => r.step = 2,
        2 => {
            r.step = 0;
            let list = r.pause_list.clone();
            set_all(e, &list, false);
        }
        _ => {}
    }
}

/// Whether a step is queued or running (the end-of-step system polls it cheaply).
pub(crate) fn step_active(cx: &Ctx) -> bool {
    cx.resources.get::<ModeRuntime>().is_some_and(|r| r.step > 0)
}
