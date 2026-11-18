//! Applies a read-only [`Request`] to the engine (runs inside the per-frame drain system).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use efeengine_core::efevm::Prim;
use efeengine_core::engine::scene::Snapshot;
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

fn field_value(f: &efeengine_core::efevm::FieldDesc, bytes: &[u8]) -> Value {
    let sz = f.ty.size() as usize;
    let at = f.offset as usize;
    let vals: Vec<Value> = (0..f.count as usize)
        .map(|i| bytes.get(at + i * sz..at + (i + 1) * sz).map(|s| prim_value(f.ty, s)).unwrap_or(Value::Null))
        .collect();
    if f.count == 1 { vals.into_iter().next().unwrap_or(Value::Null) } else { Value::Array(vals) }
}

fn component_info(entry: &TypeEntry, world: &World, e: Entity) -> Option<ComponentInfo> {
    let d = entry.desc();
    let mut bytes = vec![0u8; d.size as usize];
    entry.read(world, e, &mut bytes).ok()?;
    let mut fields = Map::new();
    for f in &d.fields {
        fields.insert(f.name.clone(), field_value(f, &bytes));
    }
    Some(ComponentInfo { name: d.name.clone(), id: entry.id(), fields })
}

// ---- E4: edits ----

fn bad_value(msg: impl Into<String>) -> Reply {
    Reply::error("bad_value", msg)
}

fn editor_id_entity(world: &World, id: u64) -> Option<Entity> {
    world.query::<(Entity, &EditorId)>().iter().find(|(_, e)| e.id == id).map(|(e, _)| e)
}

/// The entity a request names, by bits or by editor id.
fn resolve_target(world: &World, entity: Option<u64>, editor_id: Option<&str>) -> Result<Entity, Reply> {
    let bits = match (entity, editor_id) {
        (Some(b), _) => b,
        (None, Some(id)) => {
            let n = id.parse::<u64>().map_err(|_| Reply::error("unknown_editor_id", format!("editor id `{id}` is not a number")))?;
            match editor_id_entity(world, n) {
                Some(e) => entity_to_bits(e),
                None => return Err(Reply::error("unknown_editor_id", format!("no entity with editor id `{id}`"))),
            }
        }
        (None, None) => return Err(Reply::error("bad_request", "needs `entity` or `editor_id`")),
    };
    entity_from_bits(bits)
        .filter(|e| world.contains(*e))
        .ok_or_else(|| Reply::error("unknown_entity", format!("no live entity with bits {bits}")))
}

/// Little-endian bytes of one JSON value as primitive `ty`; wrong types and out-of-range
/// values are errors (never a panic, never a silent cast).
fn prim_bytes(ty: Prim, v: &Value, world: &World) -> Result<Vec<u8>, Reply> {
    if let Value::String(s) = v {
        if ty != Prim::Entity {
            return Err(Reply::error("unsupported_type", format!("a string cannot be stored in a `{ty}` field (the registry has no string fields)")));
        }
        let n = s.parse::<u64>().map_err(|_| bad_value(format!("`{s}` is not an editor id")))?;
        let e = editor_id_entity(world, n).ok_or_else(|| Reply::error("unknown_editor_id", format!("no entity with editor id `{s}`")))?;
        return Ok(entity_to_bits(e).to_le_bytes().to_vec());
    }
    macro_rules! int {
        ($t:ty, $get:ident) => {{
            let x = v.$get().ok_or_else(|| bad_value(format!("expected an integer for `{ty}`, got {v}")))?;
            let x = <$t>::try_from(x).map_err(|_| bad_value(format!("{v} is out of range for `{ty}`")))?;
            Ok(x.to_le_bytes().to_vec())
        }};
    }
    match ty {
        Prim::U8 => int!(u8, as_u64),
        Prim::U16 => int!(u16, as_u64),
        Prim::U32 => int!(u32, as_u64),
        Prim::U64 => int!(u64, as_u64),
        Prim::I8 => int!(i8, as_i64),
        Prim::I16 => int!(i16, as_i64),
        Prim::I32 => int!(i32, as_i64),
        Prim::I64 => int!(i64, as_i64),
        Prim::F32 => {
            let x = v.as_f64().ok_or_else(|| bad_value(format!("expected a number for `f32`, got {v}")))?;
            let y = x as f32;
            if !y.is_finite() {
                return Err(bad_value(format!("{v} is out of range for `f32`")));
            }
            Ok(y.to_le_bytes().to_vec())
        }
        Prim::F64 => {
            let x = v.as_f64().ok_or_else(|| bad_value(format!("expected a number for `f64`, got {v}")))?;
            Ok(x.to_le_bytes().to_vec())
        }
        Prim::Bool => match v {
            Value::Bool(b) => Ok(vec![*b as u8]),
            Value::Number(n) if n.as_u64() == Some(0) || n.as_u64() == Some(1) => Ok(vec![n.as_u64().unwrap() as u8]),
            _ => Err(bad_value(format!("{v} is not a bool (use true, false, 0 or 1)"))),
        },
        Prim::Entity => {
            let bits = v.as_u64().ok_or_else(|| bad_value(format!("expected entity bits or an editor id string, got {v}")))?;
            if bits != 0 && !entity_from_bits(bits).is_some_and(|e| world.contains(e)) {
                return Err(Reply::error("unknown_entity", format!("no live entity with bits {bits}")));
            }
            Ok(bits.to_le_bytes().to_vec())
        }
    }
}

/// The bytes of a whole field (`count` values) from JSON: a scalar, or an array of `count`.
fn field_bytes(f: &efeengine_core::efevm::FieldDesc, v: &Value, world: &World) -> Result<Vec<u8>, Reply> {
    if f.count == 1 {
        return prim_bytes(f.ty, v, world);
    }
    let Value::Array(items) = v else {
        return Err(bad_value(format!("field `{}` holds {} x `{}`: expected an array, got {v}", f.name, f.count, f.ty)));
    };
    if items.len() != f.count as usize {
        return Err(bad_value(format!("field `{}` needs {} values, got {}", f.name, f.count, items.len())));
    }
    let mut out = Vec::with_capacity(f.size() as usize);
    for i in items {
        out.extend(prim_bytes(f.ty, i, world)?);
    }
    Ok(out)
}

fn writable<'a>(types: &'a efeengine_core::engine::types::TypeRegistry, name: &str) -> Result<&'a TypeEntry, Reply> {
    let entry = types
        .by_name(name)
        .filter(|t| t.is_component())
        .ok_or_else(|| Reply::error("unknown_component", format!("no component `{name}` in the schema")))?;
    if entry.readonly() {
        return Err(Reply::error("readonly", format!("component `{name}` is read-only")));
    }
    Ok(entry)
}

fn set_field(e: &mut Engine, entity: Option<u64>, editor_id: Option<&str>, component: &str, field: &str, value: &Value) -> Reply {
    let (types, world) = e.types_and_world_mut();
    let ent = match resolve_target(world, entity, editor_id) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let entry = match writable(types, component) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let d = entry.desc();
    let Some(f) = d.fields.iter().find(|f| f.name == field) else {
        return Reply::error("unknown_field", format!("component `{component}` has no field `{field}`"));
    };
    if !entry.has(world, ent) {
        return Reply::error("missing_component", format!("the entity has no `{component}`"));
    }
    let new = match field_bytes(f, value, world) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if component == "EditorId" && field == "id" {
        let n = u64::from_le_bytes(new[..8].try_into().unwrap());
        if editor_id_entity(world, n).is_some_and(|o| o != ent) {
            return Reply::error("duplicate_editor_id", format!("editor id {n} is already used"));
        }
    }
    let mut buf = vec![0u8; d.size as usize];
    if let Err(err) = entry.read(world, ent, &mut buf) {
        return Reply::error("bad_request", err.to_string());
    }
    let old = field_value(f, &buf);
    let at = f.offset as usize;
    buf[at..at + new.len()].copy_from_slice(&new);
    match entry.write(world, ent, &buf) {
        Ok(()) => Reply::FieldSet { entity: entity_to_bits(ent), old },
        Err(err) => Reply::error("bad_request", err.to_string()),
    }
}

fn spawn(e: &mut Engine, components: &Map<String, Value>, editor_id: Option<&str>) -> Reply {
    // Validate everything first so a bad request spawns nothing.
    let mut parts: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut from_components: Option<u64> = None;
    let id = {
        let (types, world) = e.types_and_world_mut();
        for (name, fields) in components {
            let entry = match writable(types, name) {
                Ok(x) => x,
                Err(r) => return r,
            };
            let Value::Object(fields) = fields else {
                return bad_value(format!("`{name}` must be an object of field values"));
            };
            let d = entry.desc();
            let mut buf = vec![0u8; d.size as usize];
            for (fname, v) in fields {
                let Some(f) = d.fields.iter().find(|f| &f.name == fname) else {
                    return Reply::error("unknown_field", format!("component `{name}` has no field `{fname}`"));
                };
                match field_bytes(f, v, world) {
                    Ok(b) => buf[f.offset as usize..f.offset as usize + b.len()].copy_from_slice(&b),
                    Err(r) => return r,
                }
            }
            if name == "EditorId" {
                from_components = Some(u64::from_le_bytes(buf[..8].try_into().unwrap()));
            } else {
                parts.push((entry.id(), buf));
            }
        }
        let id = match editor_id {
            Some(s) => match s.parse::<u64>() {
                Ok(n) => n,
                Err(_) => return bad_value(format!("editor id `{s}` is not a number")),
            },
            None => from_components
                .unwrap_or_else(|| world.query::<&EditorId>().iter().map(|i| i.id).max().map_or(1, |m| m.saturating_add(1))),
        };
        if editor_id_entity(world, id).is_some() {
            return Reply::error("duplicate_editor_id", format!("editor id {id} is already used"));
        }
        id
    };
    let ent = e.spawn_scene_owned();
    let (types, world) = e.types_and_world_mut();
    for (tid, bytes) in &parts {
        let res = types.get(*tid).ok_or_else(|| "type vanished".to_string()).and_then(|t| t.insert(world, ent, bytes).map_err(|x| x.to_string()));
        if let Err(msg) = res {
            let _ = world.despawn(ent);
            return Reply::error("bad_request", msg);
        }
    }
    let _ = world.insert_one(ent, EditorId { id });
    Reply::Spawned { entity: entity_to_bits(ent), editor_id: id.to_string() }
}

fn despawn(e: &mut Engine, entity: Option<u64>, editor_id: Option<&str>) -> Reply {
    let ent = match resolve_target(e.world(), entity, editor_id) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let owned = e.is_scene_owned(ent);
    let (types, world) = e.types_and_world_mut();
    let components: Vec<ComponentInfo> = types
        .entries()
        .iter()
        .filter(|t| t.is_component() && !t.readonly() && t.has(world, ent))
        .filter_map(|t| component_info(t, world, ent))
        .collect();
    let eid = world.get::<&EditorId>(ent).ok().map(|i| i.id.to_string());
    let _ = world.despawn(ent);
    Reply::Despawned { owned, editor_id: eid, components }
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
    /// Taken when play starts; used (or dropped) when it stops.
    snapshot: Option<Snapshot>,
}

impl LinkState {
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir, mode: Mode::Play, saved: BTreeMap::new(), scene_path: None, snapshot: None }
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

/// What a snapshot never covers, whatever the game registered.
pub const NOT_COVERED: [&str; 4] = [
    "events in flight",
    "engine state outside the world (windows, audio, timers)",
    "read-only components and Rust components without a registered layout",
    "the global-system set",
];

/// Switch mode. `strict_stop` makes leaving play without a snapshot an error (the `stop`
/// request); `set_mode edit` from the initial play state (no snapshot) just enters edit mode.
fn set_mode(e: &mut Engine, st: &mut LinkState, mode: Mode, keep_changes: bool, strict_stop: bool) -> Result<(), Reply> {
    if strict_stop && st.snapshot.is_none() {
        return Err(Reply::error("no_snapshot", "stop needs a play session started with play (no snapshot taken)"));
    }
    if st.mode == mode {
        return Ok(());
    }
    match mode {
        Mode::Edit => {
            // Record the play-time flags first: restore rewinds them to the snapshot's.
            let mut flags = BTreeMap::new();
            for (n, en) in all_systems(e) {
                *flags.entry(n).or_insert(false) |= en;
            }
            if let (Some(snap), false) = (&st.snapshot, keep_changes) {
                e.restore(snap).map_err(|err| Reply::error("scene_error", err.to_string()))?;
            }
            st.snapshot = None;
            st.saved = flags;
            disable_non_global(e);
        }
        Mode::Play => {
            st.snapshot = Some(e.snapshot());
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
    Ok(())
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

fn switch(ctx: &mut Ctx<'_>, state: &Shared, mode: Mode, keep: bool, strict: bool, respond: Responder) {
    let state = state.clone();
    ctx.commands.engine_task(move |e| {
        respond(match set_mode(e, &mut lock(&state), mode, keep, strict) {
            Ok(()) => Reply::Mode { mode },
            Err(r) => r,
        });
    });
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
        Request::SetMode { mode, keep_changes } => switch(ctx, state, *mode, *keep_changes, false, respond),
        Request::Stop { keep_changes } => switch(ctx, state, Mode::Edit, *keep_changes, true, respond),
        Request::Fidelity => {
            let state = state.clone();
            ctx.commands.engine_task(move |e| {
                respond(Reply::Fidelity {
                    gaps: e.snapshot_gaps(),
                    not_covered: NOT_COVERED.iter().map(|s| s.to_string()).collect(),
                    snapshot: lock(&state).snapshot.is_some(),
                });
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
        Request::SetField { entity, editor_id, component, field, value } => {
            let (entity, editor_id, component, field, value) = (*entity, editor_id.clone(), component.clone(), field.clone(), value.clone());
            ctx.commands.engine_task(move |e| respond(set_field(e, entity, editor_id.as_deref(), &component, &field, &value)));
        }
        Request::Spawn { components, editor_id } => {
            let (components, editor_id) = (components.clone(), editor_id.clone());
            ctx.commands.engine_task(move |e| respond(spawn(e, &components, editor_id.as_deref())));
        }
        Request::Despawn { entity, editor_id } => {
            let (entity, editor_id) = (*entity, editor_id.clone());
            ctx.commands.engine_task(move |e| respond(despawn(e, entity, editor_id.as_deref())));
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
