//! Applies a read-only [`Request`] to the engine (runs inside the per-frame drain system).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use efeengine_core::efevm::Prim;
use efeengine_core::engine::components::{EditorId, Name};
use efeengine_core::engine::types::{TypeEntry, entity_from_bits, entity_to_bits};
use efeengine_core::log::{LogMsg, Logger};
use efeengine_core::prelude::*;
use serde_json::{Map, Value, json};

use crate::protocol::{ComponentInfo, EntityInfo, LogLine, Mode, Reply, Request, StageInfo, SystemInfo};

const LOG_CAP: usize = 1000;

fn ring() -> &'static Mutex<VecDeque<LogLine>> {
    static RING: OnceLock<Mutex<VecDeque<LogLine>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn push_line(level: &str, text: &str) {
    if let Ok(mut r) = ring().lock() {
        if r.len() >= LOG_CAP {
            r.pop_front();
        }
        r.push_back(LogLine { level: level.to_string(), text: text.to_string() });
    }
}

/// The last `count` recorded log lines, oldest first.
pub fn recent_logs(count: usize) -> Vec<LogLine> {
    let Ok(r) = ring().lock() else { return Vec::new() };
    r.iter().skip(r.len().saturating_sub(count)).cloned().collect()
}

/// A logger that records every line in a bounded ring buffer and forwards it to `inner`.
pub struct RingLogger {
    pub inner: Box<dyn Logger>,
}

impl Logger for RingLogger {
    fn logger_name(&self) -> &str {
        "editor-link ring logger"
    }
    fn info(&mut self, log: &str) {
        push_line("info", log);
        self.inner.info(log);
    }
    fn warn(&mut self, log: &str) {
        push_line("warn", log);
        self.inner.warn(log);
    }
    fn error(&mut self, log: &str) {
        push_line("error", log);
        self.inner.error(log);
    }
    fn alert(&mut self, log: &str) {
        push_line("alert", log);
        self.inner.alert(log);
    }
    fn log(&mut self, log: &LogMsg) {
        let (level, text) = match log {
            LogMsg::Info(m) => ("info", m),
            LogMsg::Warn(m) => ("warn", m),
            LogMsg::Alert(m) => ("alert", m),
            LogMsg::Error(m) => ("error", m),
        };
        push_line(level, text);
        self.inner.log(log);
    }
}

fn type_json(t: &TypeEntry) -> Value {
    let d = t.desc();
    let fields: Vec<Value> = d
        .fields
        .iter()
        .map(|f| json!({
            "name": f.name, "type": f.ty.name(), "count": f.count,
            "offset": f.offset, "size": f.size(), "doc": f.doc,
        }))
        .collect();
    json!({
        "name": d.name, "id": t.id(), "owner": d.owner, "size": d.size, "align": d.align,
        "readonly": d.readonly, "dynamic": t.is_dynamic(), "doc": d.doc, "fields": fields,
    })
}

/// The `efe-schema` document built from the registry (same structure as `Engine::schema`).
fn schema(ctx: &Ctx<'_>) -> Value {
    let (mut components, mut structs) = (Vec::new(), Vec::new());
    for t in ctx.types.entries() {
        if t.is_component() { components.push(type_json(t)) } else { structs.push(type_json(t)) }
    }
    json!({
        "format": "efe-schema", "version": 1,
        "game": ctx.scripting.registry().index().game,
        "components": components, "structs": structs,
    })
}

fn prim_value(ty: Prim, b: &[u8]) -> Value {
    fn num(f: f64) -> Value {
        serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
    }
    match ty {
        Prim::U8 => json!(b[0]),
        Prim::I8 => json!(b[0] as i8),
        Prim::U16 => json!(u16::from_le_bytes([b[0], b[1]])),
        Prim::I16 => json!(i16::from_le_bytes([b[0], b[1]])),
        Prim::U32 => json!(u32::from_le_bytes(b.try_into().unwrap())),
        Prim::I32 => json!(i32::from_le_bytes(b.try_into().unwrap())),
        Prim::U64 | Prim::Entity => json!(u64::from_le_bytes(b.try_into().unwrap())),
        Prim::I64 => json!(i64::from_le_bytes(b.try_into().unwrap())),
        Prim::F32 => num(f32::from_le_bytes(b.try_into().unwrap()) as f64),
        Prim::F64 => num(f64::from_le_bytes(b.try_into().unwrap())),
        // Same rule as ScriptBool: any non-zero byte is true.
        Prim::Bool => json!(b[0] != 0),
    }
}

fn component_info(entry: &TypeEntry, world: &World, e: Entity) -> Option<ComponentInfo> {
    let d = entry.desc();
    let mut bytes = vec![0u8; d.size as usize];
    entry.read(world, e, &mut bytes).ok()?;
    let mut fields = Map::new();
    for f in &d.fields {
        let sz = f.ty.size() as usize;
        let at = f.offset as usize;
        let vals: Vec<Value> = (0..f.count as usize)
            .map(|i| bytes.get(at + i * sz..at + (i + 1) * sz).map(|s| prim_value(f.ty, s)).unwrap_or(Value::Null))
            .collect();
        let v = if f.count == 1 { vals.into_iter().next().unwrap_or(Value::Null) } else { Value::Array(vals) };
        fields.insert(f.name.clone(), v);
    }
    Some(ComponentInfo { name: d.name.clone(), id: entry.id(), fields })
}

const ALL_STAGES: [Stage; 7] =
    [Stage::Startup, Stage::PreUpdate, Stage::FixedUpdate, Stage::Update, Stage::PostUpdate, Stage::RenderExtract, Stage::Shutdown];

/// Link state shared by the drain system and its deferred tasks.
pub struct LinkState {
    pub project_dir: PathBuf,
    pub mode: Mode,
    /// Enabled flags (by system name) recorded when edit mode was entered.
    saved: BTreeMap<String, bool>,
    /// The last path saved or loaded, as the client sent it.
    scene_path: Option<String>,
}

impl LinkState {
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir, mode: Mode::Play, saved: BTreeMap::new(), scene_path: None }
    }
}

pub type Shared = Arc<Mutex<LinkState>>;
/// Sends the reply of one request (run immediately, or from an engine task at the end of the stage).
pub type Responder = Box<dyn FnOnce(Reply) + Send>;

/// Resolve a client-sent scene path under `project_dir`: it must be relative, non-empty and
/// made only of normal components (no `..`, root or drive prefix).
pub fn resolve_project_path(project_dir: &Path, path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("path is empty".into());
    }
    if path.contains('\0') {
        return Err("path contains a NUL byte".into());
    }
    let p = Path::new(path);
    if p.is_absolute() || path.starts_with('/') || path.starts_with('\\') {
        return Err("absolute paths are not allowed".into());
    }
    let mut out = project_dir.to_path_buf();
    let mut any = false;
    for c in p.components() {
        match c {
            Component::Normal(n) => {
                out.push(n);
                any = true;
            }
            Component::CurDir => {}
            Component::ParentDir => return Err("`..` is not allowed".into()),
            Component::RootDir | Component::Prefix(_) => return Err("absolute paths are not allowed".into()),
        }
    }
    if !any {
        return Err("path names no file".into());
    }
    Ok(out)
}

fn all_systems(e: &Engine) -> Vec<(String, bool)> {
    ALL_STAGES.iter().flat_map(|s| e.system_states(*s).into_iter().map(|(n, en)| (n.to_string(), en))).collect()
}

fn disable_non_global(e: &mut Engine) {
    for (n, _) in all_systems(e) {
        if !e.is_global_system(&n) {
            let _ = e.set_system_enabled(&n, false);
        }
    }
}

fn set_mode(e: &mut Engine, st: &mut LinkState, mode: Mode) {
    if st.mode == mode {
        return;
    }
    match mode {
        Mode::Edit => {
            st.saved.clear();
            for (n, en) in all_systems(e) {
                *st.saved.entry(n).or_insert(false) |= en;
            }
            disable_non_global(e);
        }
        Mode::Play => {
            for (n, en) in std::mem::take(&mut st.saved) {
                if !e.is_global_system(&n) {
                    let _ = e.set_system_enabled(&n, en);
                }
            }
            for n in e.scene_systems() {
                if !e.is_global_system(&n) {
                    let _ = e.set_system_enabled(&n, true);
                }
            }
        }
    }
    st.mode = mode;
}

fn systems_reply(e: &Engine) -> Reply {
    let stages = ALL_STAGES
        .iter()
        .map(|s| StageInfo {
            stage: format!("{s:?}"),
            systems: e
                .system_states(*s)
                .into_iter()
                .map(|(n, en)| SystemInfo { name: n.to_string(), enabled: en, global: e.is_global_system(n) })
                .collect(),
        })
        .collect();
    Reply::Systems { stages }
}

fn lock(st: &Shared) -> std::sync::MutexGuard<'_, LinkState> {
    st.lock().unwrap_or_else(|p| p.into_inner())
}

/// Answer one request through `respond` (now, or at the end of the stage for requests that need
/// the whole engine). Never panics; failures are [`Reply::Error`].
pub fn handle(ctx: &mut Ctx<'_>, req: &Request, state: &Shared, respond: Responder) {
    match req {
        Request::Systems => ctx.commands.engine_task(move |e| respond(systems_reply(e))),
        Request::Scene => {
            let state = state.clone();
            ctx.commands.engine_task(move |e| {
                let st = lock(&state);
                respond(Reply::Scene {
                    name: e.current_scene().map(str::to_string),
                    path: st.scene_path.clone(),
                    mode: st.mode,
                    project_dir: st.project_dir.display().to_string(),
                });
            });
        }
        Request::SetMode { mode } => {
            let (state, mode) = (state.clone(), *mode);
            ctx.commands.engine_task(move |e| {
                set_mode(e, &mut lock(&state), mode);
                respond(Reply::Mode { mode });
            });
        }
        Request::SaveScene { path } => {
            let dir = lock(state).project_dir.clone();
            let full = match resolve_project_path(&dir, path) {
                Ok(f) => f,
                Err(m) => return respond(Reply::error("bad_path", m)),
            };
            let (state, path) = (state.clone(), path.clone());
            ctx.commands.engine_task(move |e| {
                respond(match e.save_scene(&full) {
                    Ok(()) => {
                        lock(&state).scene_path = Some(path.clone());
                        Reply::Saved { path }
                    }
                    Err(err) => Reply::error("scene_error", err.to_string()),
                });
            });
        }
        Request::LoadScene { path } => {
            let dir = lock(state).project_dir.clone();
            let full = match resolve_project_path(&dir, path) {
                Ok(f) => f,
                Err(m) => return respond(Reply::error("bad_path", m)),
            };
            let (state, path) = (state.clone(), path.clone());
            ctx.commands.engine_task(move |e| {
                let before: Vec<String> = e.scene_systems();
                respond(match e.load_scene_file(&full) {
                    Ok(spawned) => {
                        let mut st = lock(&state);
                        st.scene_path = Some(path.clone());
                        if st.mode == Mode::Edit {
                            // The switch enabled the new scene's systems: keep edit mode, but
                            // remember what play mode must restore.
                            let after = e.scene_systems();
                            for n in &before {
                                if !after.contains(n) {
                                    st.saved.insert(n.clone(), false);
                                }
                            }
                            for n in &after {
                                st.saved.insert(n.clone(), true);
                            }
                            disable_non_global(e);
                        }
                        Reply::Loaded { path, entities: spawned.len() }
                    }
                    Err(err) => Reply::error("scene_error", err.to_string()),
                });
            });
        }
        other => respond(handle_now(ctx, other)),
    }
}

fn handle_now(ctx: &mut Ctx<'_>, req: &Request) -> Reply {
    match req {
        Request::Stats => Reply::Stats {
            frame: ctx.time.frame(),
            frame_time: ctx.time.delta_f64(),
            entity_count: ctx.world.len(),
        },
        Request::Schema => Reply::Schema { schema: schema(ctx) },
        Request::Logs { count } => Reply::Logs { lines: recent_logs(*count) },
        Request::Entities => {
            let entities = ctx
                .world
                .iter()
                .map(|r| EntityInfo {
                    editor_id: r.get::<&EditorId>().map(|i| i.id.to_string()),
                    entity: entity_to_bits(r.entity()),
                    name: r.get::<&Name>().map(|n| n.0.clone()),
                })
                .collect();
            Reply::Entities { entities }
        }
        Request::Components { entity, editor_id } => {
            let bits = match (entity, editor_id) {
                (Some(b), _) => *b,
                (None, Some(id)) => {
                    let Ok(n) = id.parse::<u64>() else {
                        return Reply::error("unknown_editor_id", format!("editor id `{id}` is not a number"));
                    };
                    // Same lookup as `Engine::entity_by_editor_id` (a system has no `Engine`).
                    let found = ctx.world.query::<(Entity, &EditorId)>().iter().find(|(_, e)| e.id == n).map(|(e, _)| e);
                    match found {
                        Some(e) => entity_to_bits(e),
                        None => return Reply::error("unknown_editor_id", format!("no entity with editor id `{id}`")),
                    }
                }
                (None, None) => return Reply::error("bad_request", "components needs `entity` or `editor_id`"),
            };
            let Some(e) = entity_from_bits(bits).filter(|e| ctx.world.contains(*e)) else {
                return Reply::error("unknown_entity", format!("no live entity with bits {bits}"));
            };
            let components = ctx
                .types
                .entries()
                .iter()
                .filter(|t| t.is_component() && t.has(ctx.world, e))
                .filter_map(|t| component_info(t, ctx.world, e))
                .collect();
            Reply::Components { entity: bits, components }
        }
        _ => Reply::error("bad_request", "request not handled here"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_paths() {
        let d = Path::new("/proj");
        assert_eq!(resolve_project_path(d, "a.ron").unwrap(), Path::new("/proj/a.ron"));
        assert_eq!(resolve_project_path(d, "levels/./a.ron").unwrap(), Path::new("/proj/levels/a.ron"));
        for bad in ["", "/etc/passwd", "../x.ron", "a/../../x", "a/..", ".", "./", "\\x"] {
            assert!(resolve_project_path(d, bad).is_err(), "{bad:?}");
        }
    }
}
