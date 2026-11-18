//! Scene files (docs/SCENES.md, phase S1): a versioned RON file that lists the mods and
//! plugins a scene needs, the systems it enables and its entities with their components.
//!
//! Components are read and written by registry name through the [`TypeRegistry`](super::types::TypeRegistry)
//! (field layout and bytes), so Rust components marked `#[script_component]` and mod components
//! (`mod:<mod>/<Name>`) use the same codec. A field is a number or `true`/`false`; a field with
//! more than one value (`Vec2`, arrays) is a list. Entity fields are stored as raw bits. A scene
//! entity may carry an `id` (the raw bits it had when captured; snapshots set it): on load every
//! entity field whose value is one of the file's ids is remapped to the newly spawned entity, so
//! references between the scene's entities survive. Plain scene files have no ids and need no
//! remapping.
//!
//! ```text
//! Scene(
//!     format: 1,
//!     name: "level_1",
//!     requires: [],
//!     systems: ["plugin:coin_dash"],
//!     entities: [
//!         (components: { "Transform": { "position": [0.0, 1.0], "rotation": 0.0, "scale": [1.0, 1.0] } }),
//!     ],
//! )
//! ```
//!
//! Loading validates the whole file before it spawns anything: a bad file changes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::any::{Any, TypeId};
use std::sync::Arc;
use std::fmt;
use std::path::Path;

use efevm::Prim;
use hecs::Entity;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Engine;
use super::resources::Resources;
use super::schedule::Stage;
use super::types::TypeEntry;

/// The scene file format this engine reads and writes.
pub const SCENE_FORMAT: u32 = 1;

/// One field value: a scalar, or a list for fields with more than one value.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneValue {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    List(Vec<SceneValue>),
}

impl Serialize for SceneValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            SceneValue::Bool(v) => s.serialize_bool(*v),
            SceneValue::Int(v) => s.serialize_i64(*v),
            SceneValue::UInt(v) => s.serialize_u64(*v),
            SceneValue::Float(v) => s.serialize_f64(*v),
            SceneValue::List(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for SceneValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = SceneValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a number, a bool or a list of them")
            }
            fn visit_bool<E>(self, v: bool) -> Result<SceneValue, E> {
                Ok(SceneValue::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<SceneValue, E> {
                Ok(SceneValue::Int(v))
            }
            fn visit_u64<E>(self, v: u64) -> Result<SceneValue, E> {
                Ok(SceneValue::UInt(v))
            }
            fn visit_f64<E>(self, v: f64) -> Result<SceneValue, E> {
                Ok(SceneValue::Float(v))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut a: A) -> Result<SceneValue, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element()? {
                    out.push(v);
                }
                Ok(SceneValue::List(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// One entity of a scene: components by registry name, fields by name. Sorted maps keep the
/// written file stable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneEntity {
    /// Stable id: the entity's raw bits when it was captured. Entity-typed fields of the scene
    /// that hold this value are remapped to the new entity on load. Optional; scene files
    /// written by [`Engine::save_scene`] have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub components: BTreeMap<String, BTreeMap<String, SceneValue>>,
}

/// A scene file in memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// File format version (required); this engine reads [`SCENE_FORMAT`].
    pub format: u32,
    pub name: String,
    /// Plugins or mods that must be loaded.
    #[serde(default)]
    pub requires: Vec<String>,
    /// System names, or `plugin:<name>` for all systems of a plugin.
    #[serde(default)]
    pub systems: Vec<String>,
    #[serde(default)]
    pub entities: Vec<SceneEntity>,
}

/// Why a scene could not be read, checked or written. `scene` is the scene's name (the file
/// path or `<unnamed>` while it could not be parsed).
#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("scene `{scene}`: cannot parse: {msg}")]
    Parse { scene: String, msg: String },
    #[error("scene `{scene}`: format {found} is not supported (this engine reads format {SCENE_FORMAT})")]
    Format { scene: String, found: u32 },
    #[error("scene `{scene}`: requires `{name}`, which is not loaded (neither a plugin nor a mod)")]
    Requires { scene: String, name: String },
    #[error("scene `{scene}`: unknown system `{name}`")]
    System { scene: String, name: String },
    #[error("scene `{scene}`: entity {entity}: unknown component `{component}`")]
    UnknownComponent { scene: String, entity: usize, component: String },
    #[error("scene `{scene}`: entity {entity}: `{component}` is not a component")]
    NotComponent { scene: String, entity: usize, component: String },
    #[error("scene `{scene}`: entity {entity}: `{component}`: field `{field}`: {msg}")]
    Field { scene: String, entity: usize, component: String, field: String, msg: String },
    #[error("scene `{scene}`: entity {entity}: `{component}`: {msg}")]
    Insert { scene: String, entity: usize, component: String, msg: String },
    #[error("scene `{scene}`: entity {entity}: id {id} is used by an earlier entity")]
    DuplicateId { scene: String, entity: usize, id: u64 },
    #[error("scene `{scene}`: entity {entity}: EditorId {id} is used by an earlier entity")]
    DuplicateEditorId { scene: String, entity: usize, id: u64 },
    #[error("snapshot is inconsistent: {0}")]
    Snapshot(String),
    #[error("scene file `{path}`: {msg}")]
    Io { path: String, msg: String },
}

fn f32_value(v: f32) -> f64 {
    // The shortest decimal of the f32, so `0.1` is written `0.1`, not `0.10000000149011612`.
    format!("{v}").parse().unwrap_or(f64::from(v))
}

fn read_prim(p: Prim, b: &[u8]) -> SceneValue {
    match p {
        Prim::U8 => SceneValue::UInt(u64::from(b[0])),
        Prim::I8 => SceneValue::Int(i64::from(b[0] as i8)),
        Prim::U16 => SceneValue::UInt(u64::from(u16::from_le_bytes([b[0], b[1]]))),
        Prim::I16 => SceneValue::Int(i64::from(i16::from_le_bytes([b[0], b[1]]))),
        Prim::U32 => SceneValue::UInt(u64::from(u32::from_le_bytes(b[..4].try_into().unwrap()))),
        Prim::I32 => SceneValue::Int(i64::from(i32::from_le_bytes(b[..4].try_into().unwrap()))),
        Prim::U64 | Prim::Entity => SceneValue::UInt(u64::from_le_bytes(b[..8].try_into().unwrap())),
        Prim::I64 => SceneValue::Int(i64::from_le_bytes(b[..8].try_into().unwrap())),
        Prim::F32 => SceneValue::Float(f32_value(f32::from_le_bytes(b[..4].try_into().unwrap()))),
        Prim::F64 => SceneValue::Float(f64::from_le_bytes(b[..8].try_into().unwrap())),
        Prim::Bool => SceneValue::Bool(b[0] != 0),
    }
}

fn prim_size(p: Prim) -> usize {
    match p {
        Prim::U8 | Prim::I8 | Prim::Bool => 1,
        Prim::U16 | Prim::I16 => 2,
        Prim::U32 | Prim::I32 | Prim::F32 => 4,
        Prim::U64 | Prim::I64 | Prim::F64 | Prim::Entity => 8,
    }
}

fn int_of(v: &SceneValue) -> Option<i128> {
    match v {
        SceneValue::Int(i) => Some(i128::from(*i)),
        SceneValue::UInt(u) => Some(i128::from(*u)),
        _ => None,
    }
}

fn write_prim(p: Prim, v: &SceneValue, out: &mut [u8]) -> Result<(), String> {
    macro_rules! int {
        ($t:ty) => {{
            let i = int_of(v).ok_or_else(|| format!("expected an integer, got {v:?}"))?;
            let x = <$t>::try_from(i).map_err(|_| format!("{i} does not fit {}", stringify!($t)))?;
            out.copy_from_slice(&x.to_le_bytes());
        }};
    }
    match p {
        Prim::U8 => int!(u8),
        Prim::I8 => int!(i8),
        Prim::U16 => int!(u16),
        Prim::I16 => int!(i16),
        Prim::U32 => int!(u32),
        Prim::I32 => int!(i32),
        Prim::U64 | Prim::Entity => int!(u64),
        Prim::I64 => int!(i64),
        Prim::F32 | Prim::F64 => {
            let f = match v {
                SceneValue::Float(f) => *f,
                SceneValue::Int(i) => *i as f64,
                SceneValue::UInt(u) => *u as f64,
                _ => return Err(format!("expected a number, got {v:?}")),
            };
            if p == Prim::F32 {
                out.copy_from_slice(&(f as f32).to_le_bytes());
            } else {
                out.copy_from_slice(&f.to_le_bytes());
            }
        }
        Prim::Bool => match v {
            SceneValue::Bool(b) => out[0] = u8::from(*b),
            _ => match int_of(v) {
                Some(i @ (0 | 1)) => out[0] = i as u8,
                _ => return Err(format!("expected true, false, 0 or 1, got {v:?}")),
            },
        },
    }
    Ok(())
}

/// Component bytes to named fields (the codec's write side).
fn encode_component(entry: &TypeEntry, bytes: &[u8]) -> BTreeMap<String, SceneValue> {
    let mut fields = BTreeMap::new();
    for f in &entry.desc().fields {
        let sz = prim_size(f.ty);
        let at = |i: usize| read_prim(f.ty, &bytes[f.offset as usize + i * sz..]);
        let v = if f.count == 1 { at(0) } else { SceneValue::List((0..f.count as usize).map(at).collect()) };
        fields.insert(f.name.clone(), v);
    }
    fields
}

/// Named fields to component bytes (the codec's read side). A missing field is zero; an
/// unknown field, a wrong type or an out-of-range number is an error naming the field.
fn decode_component(entry: &TypeEntry, fields: &BTreeMap<String, SceneValue>) -> Result<Vec<u8>, (String, String)> {
    let desc = entry.desc();
    if let Some(unknown) = fields.keys().find(|k| !desc.fields.iter().any(|f| &f.name == *k)) {
        return Err((unknown.clone(), "no such field".into()));
    }
    let mut bytes = vec![0u8; desc.size as usize];
    for f in &desc.fields {
        let Some(v) = fields.get(&f.name) else { continue };
        let sz = prim_size(f.ty);
        let at = f.offset as usize;
        let fail = |m: String| (f.name.clone(), m);
        if f.count == 1 {
            write_prim(f.ty, v, &mut bytes[at..at + sz]).map_err(fail)?;
        } else {
            let SceneValue::List(items) = v else {
                return Err(fail(format!("expected a list of {} values", f.count)));
            };
            if items.len() != f.count as usize {
                return Err(fail(format!("expected {} values, got {}", f.count, items.len())));
            }
            for (i, item) in items.iter().enumerate() {
                let o = at + i * sz;
                write_prim(f.ty, item, &mut bytes[o..o + sz]).map_err(|m| (f.name.clone(), format!("[{i}]: {m}")))?;
            }
        }
    }
    Ok(bytes)
}

/// The restart restore's decoder (E6.2): never fails. A field the type no longer has is dropped
/// with a warning, a field the file lacks is zero (the registry holds no per-field defaults, so
/// zero is the default of a new field) with a warning, and a field whose value does not fit the
/// type (a changed type) is zero with a warning. `what` prefixes the warnings.
fn decode_component_lenient(entry: &TypeEntry, fields: &BTreeMap<String, SceneValue>, what: &str, warnings: &mut Vec<String>) -> Vec<u8> {
    let desc = entry.desc();
    for k in fields.keys().filter(|k| !desc.fields.iter().any(|f| &f.name == *k)) {
        warnings.push(format!("{what}: field `{k}` no longer exists, dropped"));
    }
    let mut bytes = vec![0u8; desc.size as usize];
    for f in &desc.fields {
        let Some(v) = fields.get(&f.name) else {
            warnings.push(format!("{what}: new field `{}` not in the snapshot, set to zero", f.name));
            continue;
        };
        let sz = prim_size(f.ty);
        let at = f.offset as usize;
        let mut one = |v: &SceneValue, o: usize| write_prim(f.ty, v, &mut bytes[o..o + sz]);
        let result = if f.count == 1 {
            one(v, at)
        } else {
            match v {
                SceneValue::List(items) if items.len() == f.count as usize => {
                    items.iter().enumerate().try_for_each(|(i, item)| one(item, at + i * sz))
                }
                _ => Err(format!("expected a list of {} values", f.count)),
            }
        };
        if let Err(m) = result {
            // A partly written list is cleared so the field is wholly zero.
            bytes[at..at + sz * f.count as usize].fill(0);
            warnings.push(format!("{what}: field `{}` does not fit its new type ({m}), set to zero", f.name));
        }
    }
    bytes
}

/// Replace Entity-typed field values found in `map` (old raw bits to new raw bits).
fn remap_entities(entry: &TypeEntry, bytes: &mut [u8], map: &BTreeMap<u64, u64>) {
    for f in &entry.desc().fields {
        if f.ty != Prim::Entity {
            continue;
        }
        for i in 0..f.count as usize {
            let at = f.offset as usize + i * 8;
            let old = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
            if let Some(new) = map.get(&old) {
                bytes[at..at + 8].copy_from_slice(&new.to_le_bytes());
            }
        }
    }
}

/// One entity ready to spawn: `(type id, component bytes)` pairs.
type DecodedEntity = Vec<(u32, Vec<u8>)>;

/// Marks an entity spawned by a scene (the same pattern as `SpawnedBy` for mods). Engine
/// bookkeeping: not in the `TypeRegistry`, so scripts never see it and scene files never
/// contain it. Switching scenes despawns the entities of the old scene.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneOwned {
    id: u32,
    name: Arc<str>,
}

impl SceneOwned {
    /// The id the engine gave the scene when it was loaded (unique per load).
    pub fn scene_id(&self) -> u32 {
        self.id
    }

    /// The scene's name.
    pub fn scene_name(&self) -> &str {
        &self.name
    }
}

/// Sent when a scene finished loading (after its entities are spawned); readable next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneLoaded {
    /// Position in the engine's scene event order, shared with [`SceneUnloaded`]: a larger
    /// `seq` was sent later (events of two types are separate queues, so this is how to tell).
    pub seq: u64,
    pub id: u32,
    pub name: String,
}

/// Sent when a scene is unloaded (its entities are gone), before the next `SceneLoaded` of a
/// switch; readable next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneUnloaded {
    /// See [`SceneLoaded::seq`].
    pub seq: u64,
    pub id: u32,
    pub name: String,
}

/// The scene that is active and the systems it switched on.
#[derive(Debug, Clone)]
pub(crate) struct ActiveScene {
    id: u32,
    name: Arc<str>,
    systems: BTreeSet<String>,
}

/// A resource the game opted in to snapshots (E0.4). Resources have no byte layout, so they
/// cannot go through the type registry like components. Instead the game registers a resource
/// type `T: Clone` with [`Engine::register_snapshot_resource`]; a snapshot then keeps a clone
/// of the value in memory (never in scene files) and `restore` puts the clone back, replacing
/// the current value. The cost of a snapshot is therefore visible and opt-in per resource.
/// Resources not registered are untouched by restore and listed by [`Engine::snapshot_gaps`].
pub(crate) struct SnapResource {
    id: TypeId,
    name: &'static str,
    capture: fn(&Resources) -> Option<ResourceCopy>,
    put_back: fn(&mut Resources, &ResourceCopy),
}

impl fmt::Debug for SnapResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// One cloned resource value inside a [`Snapshot`].
#[derive(Clone)]
pub struct ResourceCopy {
    id: TypeId,
    value: Arc<dyn Any + Send + Sync>,
}

impl fmt::Debug for ResourceCopy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResourceCopy")
    }
}

#[derive(Debug, Default)]
pub(crate) struct SceneState {
    snap_resources: Vec<SnapResource>,
    active: Option<ActiveScene>,
    next_id: u32,
    global: BTreeSet<String>,
    event_seq: u64,
}

impl SceneState {
    fn next_seq(&mut self) -> u64 {
        self.event_seq += 1;
        self.event_seq
    }
}

const ALL_STAGES: [Stage; 7] =
    [Stage::Startup, Stage::PreUpdate, Stage::FixedUpdate, Stage::Update, Stage::PostUpdate, Stage::RenderExtract, Stage::Shutdown];

impl Scene {
    /// Parse a scene from RON text (the format version is checked by [`Engine::load_scene_str`]).
    pub fn from_ron(text: &str) -> Result<Scene, SceneError> {
        ron::from_str(text).map_err(|e| SceneError::Parse { scene: "<unnamed>".into(), msg: e.to_string() })
    }

    /// Write the scene as RON. The output is stable: the same scene gives the same text.
    pub fn to_ron(&self) -> String {
        let cfg = ron::ser::PrettyConfig::new().struct_names(true);
        ron::ser::to_string_pretty(self, cfg).expect("a scene always serialises") + "\n"
    }
}

impl Engine {
    /// Entities a scene or snapshot covers: those owned by a scene when `owned_only`, else also
    /// every entity with at least one registered, writable component.
    fn captured_entities(&self, owned_only: bool) -> Vec<Entity> {
        let world = self.world();
        let types = self.types();
        world
            .iter()
            .map(|r| r.entity())
            .filter(|&e| {
                world.get::<&SceneOwned>(e).is_ok()
                    || (!owned_only && types.entries().iter().any(|t| t.is_component() && !t.readonly() && t.has(world, e)))
            })
            .collect()
    }

    fn capture_entity(&self, e: Entity) -> SceneEntity {
        let world = self.world();
        let mut ent = SceneEntity::default();
        for entry in self.types().entries() {
            if !entry.is_component() || entry.readonly() || !entry.has(world, e) {
                continue;
            }
            let mut buf = vec![0u8; entry.size() as usize];
            if entry.read(world, e, &mut buf).is_ok() {
                ent.components.insert(entry.name().to_string(), encode_component(entry, &buf));
            }
        }
        ent
    }

    /// Capture the scene-owned entities (those with [`SceneOwned`], that is, what
    /// [`load_scene`](Self::load_scene) spawned) as a scene named `name` (no requires, no
    /// systems; set those on the result). Entities spawned by other means are not part of the
    /// scene. Use [`snapshot`](Self::snapshot) to capture the whole world.
    ///
    /// Read-only components (derived data such as `GlobalTransform`) and Rust components
    /// without a registered layout are not written, and an owned entity left with no written
    /// component is skipped. Entities are sorted by their written content, components and
    /// fields by name, so the same world always gives the same file, whatever the entity ids
    /// are. The file carries no entity ids, so Entity-typed fields are written as the raw bits
    /// they have now and are not remapped on load; snapshots keep references intact.
    pub fn capture_scene(&self, name: &str) -> Scene {
        let mut ents: Vec<(String, SceneEntity)> = Vec::new();
        for e in self.captured_entities(true) {
            let ent = self.capture_entity(e);
            if !ent.components.is_empty() {
                let key = ron::to_string(&ent).unwrap_or_default();
                ents.push((key, ent));
            }
        }
        ents.sort_by(|a, b| a.0.cmp(&b.0));
        Scene {
            format: SCENE_FORMAT,
            name: name.to_string(),
            requires: Vec::new(),
            systems: Vec::new(),
            entities: ents.into_iter().map(|(_, e)| e).collect(),
        }
    }

    /// Write the current scene's [`SceneOwned`] entities as a scene file (see
    /// [`capture_scene`](Self::capture_scene)); the scene is named after the file stem.
    pub fn save_scene(&self, path: impl AsRef<Path>) -> Result<(), SceneError> {
        let path = path.as_ref();
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
        let text = self.capture_scene(name).to_ron();
        std::fs::write(path, text).map_err(|e| SceneError::Io { path: path.display().to_string(), msg: e.to_string() })
    }

    /// Parse, validate and spawn a scene from RON text; returns the spawned entities in file
    /// order. Nothing is spawned unless the whole scene is valid. The scene's `systems` are
    /// checked but not toggled (switching is phase S2).
    pub fn load_scene_str(&mut self, text: &str) -> Result<Vec<Entity>, SceneError> {
        let scene = Scene::from_ron(text)?;
        self.load_scene(&scene)
    }

    /// [`load_scene_str`](Self::load_scene_str) for a file.
    pub fn load_scene_file(&mut self, path: impl AsRef<Path>) -> Result<Vec<Entity>, SceneError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| SceneError::Io { path: path.display().to_string(), msg: e.to_string() })?;
        self.load_scene_str(&text)
    }

    /// Check `scene` against the engine without changing anything: format, `requires`,
    /// `systems` and every component and field.
    pub fn validate_scene(&self, scene: &Scene) -> Result<(), SceneError> {
        self.decode_scene(scene).map(|_| ())
    }

    /// Mark `name` as a global system: scene switches never enable or disable it, even when a
    /// scene lists it. (Stands in for the `global` list of `project.toml`, roadmap 7.1.)
    /// Errors if no system has that name.
    pub fn set_global_system(&mut self, name: &str) -> Result<(), super::schedule::ScheduleError> {
        if self.is_system_enabled(name).is_none() {
            return Err(super::schedule::ScheduleError::UnknownSystem(name.to_string()));
        }
        self.scene_state.global.insert(name.to_string());
        Ok(())
    }

    /// Whether `name` was marked with [`set_global_system`](Self::set_global_system).
    pub fn is_global_system(&self, name: &str) -> bool {
        self.scene_state.global.contains(name)
    }

    /// The type registry and the world (mutable) together, for byte-level edits that cannot
    /// borrow both through `types()` and `world_mut()`.
    pub fn types_and_world_mut(&mut self) -> (&super::types::TypeRegistry, &mut hecs::World) {
        (&self.state.types, &mut self.state.world)
    }

    /// Spawn an empty entity tagged [`SceneOwned`] by the active scene (so
    /// [`capture_scene`](Self::capture_scene) and [`save_scene`](Self::save_scene) include it
    /// and a scene switch despawns it). Without an active scene the tag has scene id 0 and an
    /// empty name. Used by the editor link; add components with the registry afterwards.
    pub fn spawn_scene_owned(&mut self) -> Entity {
        let owned = match &self.scene_state.active {
            Some(a) => SceneOwned { id: a.id, name: a.name.clone() },
            None => SceneOwned { id: 0, name: Arc::from("") },
        };
        self.state.world.spawn((owned,))
    }

    /// `true` if `e` is alive and tagged [`SceneOwned`].
    pub fn is_scene_owned(&self, e: Entity) -> bool {
        self.state.world.get::<&SceneOwned>(e).is_ok()
    }

    /// Name of the active scene, if one is loaded.
    pub fn current_scene(&self) -> Option<&str> {
        self.scene_state.active.as_ref().map(|a| &*a.name)
    }

    /// The system names the active scene lists (`plugin:` entries expanded), sorted; empty
    /// without an active scene.
    pub fn scene_systems(&self) -> Vec<String> {
        self.scene_state.active.as_ref().map(|a| a.systems.iter().cloned().collect()).unwrap_or_default()
    }

    /// Switch to `scene` (docs/SCENES.md, "Switching rules"):
    ///
    /// 1. the scene is validated completely and its entities are spawned (tagged
    ///    [`SceneOwned`]); on any error everything spawned is removed again and the current
    ///    scene, its entities and its systems are untouched;
    /// 2. the entities of the current scene are despawned;
    /// 3. the systems the current scene listed and the new one does not are disabled, then
    ///    the new scene's systems are enabled (global systems are never touched);
    /// 4. [`SceneUnloaded`] (if there was a scene) and [`SceneLoaded`] are sent.
    ///
    /// The new entities are spawned before the old ones are despawned only so that a failure
    /// can still roll back; nothing can observe the difference. Call it from outside the
    /// frame loop (before it starts, or between frames). From a system, queue
    /// [`Commands::load_scene`](super::commands::Commands::load_scene) instead: it runs this
    /// at the end of the stage. Returns the spawned entities in file order.
    pub fn load_scene(&mut self, scene: &Scene) -> Result<Vec<Entity>, SceneError> {
        let (decoded, systems) = self.decode_scene_full(scene)?;
        let id = self.scene_state.next_id + 1;
        let name: Arc<str> = Arc::from(scene.name.as_str());
        let owned = SceneOwned { id, name: name.clone() };
        let owners = vec![Some(owned); decoded.len()];
        let spawned = self.spawn_decoded(scene, &decoded, &owners)?;
        self.scene_state.next_id = id;
        // The point of no return: the new scene is fully built.
        let old = self.scene_state.active.take();
        if let Some(old) = &old {
            self.despawn_owned(old.id);
            for n in old.systems.difference(&systems) {
                if !self.scene_state.global.contains(n) {
                    self.schedule.set_enabled(n, false);
                }
            }
            let seq = self.scene_state.next_seq();
            self.events.get_mut::<SceneUnloaded>().send(SceneUnloaded { seq, id: old.id, name: old.name.to_string() });
        }
        for n in &systems {
            if !self.scene_state.global.contains(n) {
                self.schedule.set_enabled(n, true);
            }
        }
        self.scene_state.active = Some(ActiveScene { id, name, systems });
        let seq = self.scene_state.next_seq();
        self.events.get_mut::<SceneLoaded>().send(SceneLoaded { seq, id, name: scene.name.clone() });
        Ok(spawned)
    }

    /// Unload the active scene: despawn its entities, disable the systems it listed (except
    /// global ones) and send [`SceneUnloaded`]. Does nothing without an active scene.
    pub fn unload_scene(&mut self) {
        let Some(old) = self.scene_state.active.take() else { return };
        self.despawn_owned(old.id);
        for n in &old.systems {
            if !self.scene_state.global.contains(n) {
                self.schedule.set_enabled(n, false);
            }
        }
        let seq = self.scene_state.next_seq();
            self.events.get_mut::<SceneUnloaded>().send(SceneUnloaded { seq, id: old.id, name: old.name.to_string() });
    }

    fn despawn_owned(&mut self, id: u32) {
        let victims: Vec<Entity> =
            self.state.world.query::<(Entity, &SceneOwned)>().iter().filter(|(_, o)| o.id == id).map(|(e, _)| e).collect();
        for e in victims {
            let _ = self.state.world.despawn(e);
        }
    }

    /// Spawn the decoded entities, tagged with their `owners` entry; on failure despawn them
    /// all. Entities are spawned first, then their components are inserted, so an Entity-typed
    /// field holding the `id` of a scene entity can be remapped to the entity it became (the
    /// stable-id scheme of snapshots, docs/SCENES.md S3.2). Values that are no scene id are left
    /// as they are (they may name an entity outside the scene).
    fn spawn_decoded(&mut self, scene: &Scene, decoded: &[DecodedEntity], owners: &[Option<SceneOwned>]) -> Result<Vec<Entity>, SceneError> {
        let mut spawned = Vec::with_capacity(decoded.len());
        let mut map: BTreeMap<u64, u64> = BTreeMap::new();
        for (i, owner) in owners.iter().enumerate() {
            let e = match owner {
                Some(o) => self.state.world.spawn((o.clone(),)),
                None => self.state.world.spawn(()),
            };
            spawned.push(e);
            if let Some(id) = scene.entities[i].id {
                map.insert(id, super::types::entity_to_bits(e));
            }
        }
        for (i, comps) in decoded.iter().enumerate() {
            let e = spawned[i];
            for (id, bytes) in comps {
                let entry = self.state.types.get(*id).expect("validated");
                let mut bytes = bytes.clone();
                if !map.is_empty() {
                    remap_entities(entry, &mut bytes, &map);
                }
                if let Err(err) = entry.insert(&mut self.state.world, e, &bytes) {
                    let component = entry.name().to_string();
                    for e in spawned {
                        let _ = self.state.world.despawn(e);
                    }
                    return Err(SceneError::Insert { scene: scene.name.clone(), entity: i, component, msg: err.to_string() });
                }
            }
        }
        Ok(spawned)
    }

    fn decode_scene(&self, scene: &Scene) -> Result<Vec<DecodedEntity>, SceneError> {
        self.decode_scene_full(scene).map(|(d, _)| d)
    }

    /// Everything that can fail, before the world is touched: per entity, `(type id, bytes)`,
    /// and the system names the scene lists (`plugin:` entries expanded).
    fn decode_scene_full(&self, scene: &Scene) -> Result<(Vec<DecodedEntity>, BTreeSet<String>), SceneError> {
        let name = &scene.name;
        if scene.format != SCENE_FORMAT {
            return Err(SceneError::Format { scene: name.clone(), found: scene.format });
        }
        for r in &scene.requires {
            if !self.has_plugin(r) && self.scripting().module_id(r).is_none() {
                return Err(SceneError::Requires { scene: name.clone(), name: r.clone() });
            }
        }
        let mut systems = BTreeSet::new();
        for s in &scene.systems {
            match s.strip_prefix("plugin:") {
                Some(p) if self.has_plugin(p) => {
                    for st in ALL_STAGES {
                        for (n, o) in self.system_names(st).into_iter().zip(self.system_owners(st)) {
                            if o == Some(p) {
                                systems.insert(n.to_string());
                            }
                        }
                    }
                }
                Some(_) => return Err(SceneError::System { scene: name.clone(), name: s.clone() }),
                None if ALL_STAGES.iter().any(|st| self.system_names(*st).contains(&s.as_str())) => {
                    systems.insert(s.clone());
                }
                None => return Err(SceneError::System { scene: name.clone(), name: s.clone() }),
            }
        }
        let mut out = Vec::with_capacity(scene.entities.len());
        let mut ids = BTreeSet::new();
        let mut editor_ids = BTreeSet::new();
        for (i, ent) in scene.entities.iter().enumerate() {
            let editor_id = ent.components.get("EditorId").and_then(|f| f.get("id")).and_then(int_of);
            if let Some(id) = editor_id.filter(|id| !editor_ids.insert(*id)) {
                return Err(SceneError::DuplicateEditorId { scene: name.clone(), entity: i, id: id as u64 });
            }
            if let Some(id) = ent.id.filter(|id| !ids.insert(*id)) {
                return Err(SceneError::DuplicateId { scene: name.clone(), entity: i, id });
            }
            let mut comps = Vec::with_capacity(ent.components.len());
            for (cname, fields) in &ent.components {
                let entry = self
                    .types()
                    .by_name(cname)
                    .ok_or_else(|| SceneError::UnknownComponent { scene: name.clone(), entity: i, component: cname.clone() })?;
                if !entry.is_component() {
                    return Err(SceneError::NotComponent { scene: name.clone(), entity: i, component: cname.clone() });
                }
                let bytes = decode_component(entry, fields).map_err(|(field, msg)| SceneError::Field {
                    scene: name.clone(),
                    entity: i,
                    component: cname.clone(),
                    field,
                    msg,
                })?;
                comps.push((entry.id(), bytes));
            }
            out.push(comps);
        }
        Ok((out, systems))
    }
}

/// The world handed from one run of the game to the next on a seamless restart (docs/EDITOR.md
/// E6.2): the same entity list as a [`Snapshot`] (with ids, so references survive), plus
/// ownership, as RON text. [`Engine::restore_lenient`] reads it with a tolerant decoder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestartSnapshot {
    pub scene: Scene,
    /// Per entity of `scene`: the `(scene id, scene name)` of its `SceneOwned`, if any.
    pub owners: Vec<Option<(u32, String)>>,
}

impl RestartSnapshot {
    pub fn to_ron(&self) -> String {
        let cfg = ron::ser::PrettyConfig::new().struct_names(true);
        ron::ser::to_string_pretty(self, cfg).expect("a restart snapshot always serialises") + "\n"
    }

    pub fn from_ron(text: &str) -> Result<Self, SceneError> {
        ron::from_str(text).map_err(|e| SceneError::Parse { scene: "<restart snapshot>".into(), msg: e.to_string() })
    }
}

/// What [`Engine::restore_lenient`] did: how many entities it spawned and everything it had to
/// drop, default or skip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoreReport {
    pub entities: usize,
    pub warnings: Vec<String>,
}

/// An in-memory copy of the world's scene state (docs/SCENES.md, phase S3): the same data as a
/// scene file, plus ownership, the system enabled flags, the world's tick and the current-scene
/// bookkeeping. Made by [`Engine::snapshot`], applied by [`Engine::restore`].
///
/// Identity: every captured entity carries its raw entity bits as `scene.entities[i].id`. On
/// restore all entities are respawned first, which gives a map old bits to new entity, and every
/// Entity-typed field that holds an old id is rewritten to the new entity. Two entities that
/// referred to each other still do. Entity values that are not ids of the snapshot (a reference
/// to a despawned entity, or to a live entity the snapshot does not cover) are left as they are.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The captured entities (with ids) and the names of the systems the current scene listed.
    pub scene: Scene,
    /// Per entity of `scene`: the `(scene id, scene name)` of its `SceneOwned`, if it has one.
    pub owners: Vec<Option<(u32, String)>>,
    /// `(stage, name, enabled)` of every system, disabled ones included.
    pub systems: Vec<(Stage, String, bool)>,
    /// The world's change tick when the snapshot was taken (informational: change ticks only
    /// move forward, so restore does not rewind them).
    pub tick: u32,
    /// Clones of the resources registered with [`Engine::register_snapshot_resource`] that were
    /// present at snapshot time. In memory only.
    pub resources: Vec<ResourceCopy>,
    /// Type names of resources that were present but not registered: restore leaves them as
    /// they are, so they are not part of the play-mode fidelity (docs/EDITOR.md).
    pub gaps: Vec<String>,
    active: Option<(u32, String, BTreeSet<String>)>,
    next_id: u32,
}

impl Engine {
    /// Opt resource `T` in to snapshots: `snapshot()` clones it, `restore()` puts the clone
    /// back (replacing the current value, or inserting it if it was removed). Registering twice
    /// is a no-op. A resource that is absent at snapshot time is not recorded, and restore then
    /// leaves the current one alone.
    pub fn register_snapshot_resource<T: Clone + Send + Sync + 'static>(&mut self) -> &mut Self {
        let id = TypeId::of::<T>();
        if self.scene_state.snap_resources.iter().any(|r| r.id == id) {
            return self;
        }
        self.scene_state.snap_resources.push(SnapResource {
            id,
            name: std::any::type_name::<T>(),
            capture: |res| res.get::<T>().map(|v| ResourceCopy { id: TypeId::of::<T>(), value: Arc::new((*v).clone()) }),
            put_back: |res, copy| {
                if let Some(v) = copy.value.downcast_ref::<T>() {
                    res.insert(v.clone());
                }
            },
        });
        self
    }

    /// Type names of the resources present now that are not registered for snapshots: what
    /// `restore` will not rewind. Includes the engine's own resources. Sorted.
    pub fn snapshot_gaps(&self) -> Vec<String> {
        let mut gaps: Vec<String> = self
            .state
            .resources
            .type_names()
            .filter(|(id, _)| !self.scene_state.snap_resources.iter().any(|r| r.id == *id))
            .map(|(_, n)| n.to_string())
            .collect();
        gaps.sort();
        gaps
    }

    /// The entity whose [`EditorId`](super::components::EditorId) is `id`, if any. O(n) over
    /// the entities that have the component (editors call it on selection changes, not per
    /// frame). If a world holds duplicates (scene loading rejects them) the first found wins.
    pub fn entity_by_editor_id(&self, id: u64) -> Option<Entity> {
        self.world().query::<(Entity, &super::components::EditorId)>().iter().find(|(_, e)| e.id == id).map(|(e, _)| e)
    }

    /// Capture the scene state of the world in memory (see [`Snapshot`]): every entity with a
    /// registered, writable component or a [`SceneOwned`] tag, the enabled flag of every system,
    /// the world's tick and which scene is current.
    ///
    /// Resources are captured only when registered with
    /// [`register_snapshot_resource`](Self::register_snapshot_resource).
    ///
    /// Not captured: other resources, read-only components, Rust components without a registered
    /// layout, the global-system set and event queues.
    pub fn snapshot(&self) -> Snapshot {
        let mut list: Vec<(u64, Entity)> = self.captured_entities(false).into_iter().map(|e| (super::types::entity_to_bits(e), e)).collect();
        list.sort_by_key(|(bits, _)| *bits);
        let mut entities = Vec::with_capacity(list.len());
        let mut owners = Vec::with_capacity(list.len());
        for (bits, e) in list {
            let mut ent = self.capture_entity(e);
            ent.id = Some(bits);
            entities.push(ent);
            owners.push(self.world().get::<&SceneOwned>(e).ok().map(|o| (o.id, o.name.to_string())));
        }
        let active = self.scene_state.active.as_ref().map(|a| (a.id, a.name.to_string(), a.systems.clone()));
        let mut systems = Vec::new();
        for st in ALL_STAGES {
            for (n, en) in self.system_states(st) {
                systems.push((st, n.to_string(), en));
            }
        }
        Snapshot {
            scene: Scene {
                format: SCENE_FORMAT,
                name: active.as_ref().map(|a| a.1.clone()).unwrap_or_else(|| "<snapshot>".into()),
                requires: Vec::new(),
                systems: active.as_ref().map(|a| a.2.iter().cloned().collect()).unwrap_or_default(),
                entities,
            },
            owners,
            systems,
            tick: self.change_tick().get(),
            resources: self.scene_state.snap_resources.iter().filter_map(|r| (r.capture)(&self.state.resources)).collect(),
            gaps: self.snapshot_gaps(),
            active,
            next_id: self.scene_state.next_id,
        }
    }

    /// Put the world back as `snap` was: every entity [`snapshot`](Self::snapshot) would
    /// capture now is despawned, the snapshot's entities are respawned (references between
    /// them remapped, see [`Snapshot`]) with their ownership, the system enabled flags and the
    /// current scene are reinstated. Entities are new (new ids), their order is the snapshot's.
    /// No events are sent. Rust-side handles to captured entities are stale afterwards.
    ///
    /// The snapshot is validated completely first (format, systems, components, fields, ids,
    /// recorded systems still exist); on error the world is untouched. Systems added since
    /// the snapshot keep their flag.
    pub fn restore(&mut self, snap: &Snapshot) -> Result<(), SceneError> {
        let (decoded, _) = self.decode_scene_full(&snap.scene)?;
        if snap.owners.len() != snap.scene.entities.len() {
            return Err(SceneError::Snapshot(format!("{} owner entries for {} entities", snap.owners.len(), snap.scene.entities.len())));
        }
        for (st, n, _) in &snap.systems {
            if !self.system_names(*st).contains(&n.as_str()) {
                return Err(SceneError::System { scene: snap.scene.name.clone(), name: n.clone() });
            }
        }
        let owners: Vec<Option<SceneOwned>> =
            snap.owners.iter().map(|o| o.as_ref().map(|(id, name)| SceneOwned { id: *id, name: Arc::from(name.as_str()) })).collect();

        // From here on nothing can fail but an insert that validation already covered.
        for e in self.captured_entities(false) {
            let _ = self.state.world.despawn(e);
        }
        self.spawn_decoded(&snap.scene, &decoded, &owners)?;
        for (st, n, en) in &snap.systems {
            self.schedule.set_enabled_in(*st, n, *en);
        }
        let tick = self.state.world.increment_change_tick();
        self.state.resources.set_tick(tick);
        for copy in &snap.resources {
            if let Some(r) = self.scene_state.snap_resources.iter().find(|r| r.id == copy.id) {
                (r.put_back)(&mut self.state.resources, copy);
            }
        }
        self.scene_state.active = snap.active.as_ref().map(|(id, name, systems)| ActiveScene { id: *id, name: Arc::from(name.as_str()), systems: systems.clone() });
        self.scene_state.next_id = self.scene_state.next_id.max(snap.next_id);
        Ok(())
    }
}

impl Engine {
    /// Capture what a restart must carry over: [`snapshot`](Self::snapshot)'s entities and
    /// ownership, as a serialisable value.
    pub fn restart_snapshot(&self) -> RestartSnapshot {
        let snap = self.snapshot();
        RestartSnapshot { scene: snap.scene, owners: snap.owners }
    }

    /// Replace the world's captured entities with those of `snap`, tolerating a changed
    /// layout (see `decode_component_lenient`): unknown fields are dropped, new fields are
    /// zero, a component the game no longer registers is skipped (all with a warning in the
    /// report), and the entity is kept with its other components. References between the
    /// restored entities are remapped as in [`restore`](Self::restore).
    ///
    /// Ownership: entities that belonged to the scene now active (same name, usually the one
    /// the game's setup just loaded) are owned by it again; other scene owners get a fresh
    /// scene id. System flags and resources are not restored: the new run's setup decides them.
    ///
    /// Fails (world untouched) only for a wrong format version or a malformed snapshot.
    pub fn restore_lenient(&mut self, snap: &RestartSnapshot) -> Result<RestoreReport, SceneError> {
        let name = snap.scene.name.clone();
        if snap.scene.format != SCENE_FORMAT {
            return Err(SceneError::Format { scene: name, found: snap.scene.format });
        }
        if snap.owners.len() != snap.scene.entities.len() {
            return Err(SceneError::Snapshot(format!("{} owner entries for {} entities", snap.owners.len(), snap.scene.entities.len())));
        }
        let mut ids = BTreeSet::new();
        for (i, ent) in snap.scene.entities.iter().enumerate() {
            if let Some(id) = ent.id.filter(|id| !ids.insert(*id)) {
                return Err(SceneError::DuplicateId { scene: name, entity: i, id });
            }
        }
        let mut warnings = Vec::new();
        let mut decoded: Vec<DecodedEntity> = Vec::with_capacity(snap.scene.entities.len());
        let mut editor_ids = BTreeSet::new();
        for (i, ent) in snap.scene.entities.iter().enumerate() {
            let mut comps = Vec::new();
            for (cname, fields) in &ent.components {
                let what = format!("entity {i} component `{cname}`");
                match self.types().by_name(cname) {
                    None => warnings.push(format!("{what}: no such component any more, skipped")),
                    Some(entry) if !entry.is_component() => warnings.push(format!("{what}: is no longer a component, skipped")),
                    Some(entry) => {
                        let bytes = decode_component_lenient(entry, fields, &what, &mut warnings);
                        if cname == "EditorId" {
                            // Duplicate editor ids would break the editor's lookups.
                            let id = fields.get("id").and_then(int_of);
                            if id.is_some_and(|id| !editor_ids.insert(id)) {
                                warnings.push(format!("{what}: duplicate editor id, skipped"));
                                continue;
                            }
                        }
                        comps.push((entry.id(), bytes));
                    }
                }
            }
            decoded.push(comps);
        }
        // Scene ids: the active scene keeps its id, any other old scene gets a new one.
        let active = self.scene_state.active.as_ref().map(|a| (a.id, a.name.to_string()));
        let mut next = self.scene_state.next_id;
        let mut fresh: BTreeMap<u32, u32> = BTreeMap::new();
        let owners: Vec<Option<SceneOwned>> = snap
            .owners
            .iter()
            .map(|o| {
                o.as_ref().map(|(old, n)| match &active {
                    Some((id, an)) if an == n => SceneOwned { id: *id, name: Arc::from(n.as_str()) },
                    _ => {
                        let id = *fresh.entry(*old).or_insert_with(|| {
                            next += 1;
                            next
                        });
                        SceneOwned { id, name: Arc::from(n.as_str()) }
                    }
                })
            })
            .collect();
        for e in self.captured_entities(false) {
            let _ = self.state.world.despawn(e);
        }
        let spawned = self.spawn_decoded(&snap.scene, &decoded, &owners)?;
        self.scene_state.next_id = next;
        Ok(RestoreReport { entities: spawned.len(), warnings })
    }
}
