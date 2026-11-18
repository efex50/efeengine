//! Wire protocol (docs/EDITOR.md "Protocol"): length-prefixed JSON messages.
//!
//! A frame is a `u32` big-endian length followed by that many bytes of UTF-8 JSON.
//!
//! ```text
//! client -> {"hello":{"protocol":1,"client":"efe-editor"}}
//! server -> {"hello":{"protocol":1,"game":"my_game"}}        (or {"error":{"code":"protocol_mismatch",..}} and close)
//! client -> {"request":{"id":7,"request":"stats"}}
//! server -> {"reply":{"id":7,"reply":{"stats":{"frame":10,"frame_time":0.016,"entity_count":3}}}}
//! ```
//!
//! Requests: `"stats"`, `"schema"`, `"entities"`, `{"components":{"entity":bits}}` (or
//! `{"components":{"editor_id":"..."}}`; an editor id is the decimal `EditorId.id`),
//! `{"logs":{"count":50}}`, `"systems"`, `"scene"`, `{"set_mode":{"mode":"edit"|"play","keep_changes":false}}`, `{"stop":{"keep_changes":false}}`, `"fidelity"`,
//! `{"save_scene":{"path":"levels/a.ron"}}`, `{"load_scene":{"path":"levels/a.ron"}}`.
//! Edits (E4): `{"set_field":{"entity"|"editor_id":..,"component":"Door","field":"speed","value":2.5}}`,
//! `{"spawn":{"components":{"Door":{"speed":1.0}},"editor_id":"7"}}` and
//! `{"despawn":{"entity"|"editor_id":..}}`. They run as engine tasks (end of the stage), in edit
//! and play mode. Error codes: `unknown_entity`, `unknown_editor_id`, `unknown_component`,
//! `unknown_field`, `missing_component`, `readonly`, `bad_value`, `unsupported_type` (a string
//! for a non-entity field: the registry has no string fields) and `duplicate_editor_id`.
//! Failures are replies: `{"error":{"code":"...","message":"..."}}`.
//!
//! # Modes
//!
//! The link starts in `play`. `set_mode edit` records the enabled flag of every system, then
//! disables every system that is not global (`Engine::set_global_system`; the link's own
//! `editor_link_drain` is global, and so are the systems the game marked). It runs at the end of
//! the frame's `PostUpdate`, so the frame that carried the request still finishes. While in
//! edit mode, a `load_scene` re-disables every non-global system after the switch (a scene
//! switch would otherwise enable its systems). `set_mode play` restores the recorded flags
//! (adjusted for scene switches made in edit mode) and enables the systems the active scene
//! lists, as `load_scene` does (global systems are never touched). Setting the mode already
//! active is a no-op that still replies.
//!
//! Entering play (from edit) takes `Engine::snapshot()` before any system is enabled. Leaving
//! play calls `Engine::restore` on it unless `keep_changes` is true (then it is dropped); a
//! failed restore replies `scene_error` and stays in play with the snapshot kept. `stop` is
//! `set_mode edit` that replies `no_snapshot` when no play session was started by `play`
//! (`set_mode edit` from the initial play state just enters edit mode). `fidelity` lists
//! `Engine::snapshot_gaps` and the fixed list of state a snapshot never covers. Edit mode never mutates entities.
//!
//! # Scene files
//!
//! `save_scene` and `load_scene` paths are relative to the link's project directory
//! (`EditorLink::with_project_dir`, else `EFE_EDITOR_PROJECT`, else the working directory).
//! Absolute paths, `..`, empty paths and drive prefixes are rejected with `bad_path`. Save writes
//! `Engine::save_scene` (scene-owned entities); load is `Engine::load_scene_file`, so a failed
//! load leaves the current scene running. Errors reply `scene_error`.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version; bumped on every incompatible change.
pub const PROTOCOL_VERSION: u32 = 1;
/// The largest frame either side accepts (1 MiB); a bigger length closes the connection.
pub const MAX_FRAME: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientMessage {
    Hello { protocol: u32, client: String },
    Request { id: u64, request: Request },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerMessage {
    Hello { protocol: u32, game: String },
    Reply { id: u64, reply: Reply },
    /// A connection-level failure (handshake); the server closes after sending it.
    Error { code: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    Entities,
    /// Components of one entity, by raw entity bits or by editor id.
    Components {
        #[serde(default)]
        entity: Option<u64>,
        #[serde(default)]
        editor_id: Option<String>,
    },
    Schema,
    Stats,
    Logs { count: usize },
    /// Every stage's systems with enabled and global flags.
    Systems,
    /// The active scene, the last saved or loaded path and the mode.
    Scene,
    /// Switch mode. `play` takes a snapshot; `edit` restores it unless `keep_changes`.
    SetMode {
        mode: Mode,
        #[serde(default)]
        keep_changes: bool,
    },
    /// Leave play mode (like `set_mode edit`), but an error (`no_snapshot`) when no play
    /// session is running.
    Stop {
        #[serde(default)]
        keep_changes: bool,
    },
    /// What a snapshot does not cover in this game.
    Fidelity,
    /// Save the scene-owned entities to a file under the project directory.
    SaveScene { path: String },
    /// Load a scene file from the project directory (switches the scene).
    LoadScene { path: String },
    /// Set one field of one component (edit and play mode). `value` is JSON converted by the
    /// field's schema type: a number, a bool, an array for vectors, entity bits or an editor
    /// id string for `entity`. Reply: `field_set` with the old value.
    SetField {
        #[serde(default)]
        entity: Option<u64>,
        #[serde(default)]
        editor_id: Option<String>,
        component: String,
        field: String,
        value: Value,
    },
    /// Spawn a scene-owned entity. `components` maps a component name to `{field: value}`
    /// (missing fields are zero). An `EditorId` is always set: `editor_id`, else the one in
    /// `components`, else the largest existing plus one.
    Spawn {
        #[serde(default)]
        components: serde_json::Map<String, Value>,
        #[serde(default)]
        editor_id: Option<String>,
    },
    /// Despawn an entity. The reply carries what it had, so the editor can respawn it.
    Despawn {
        #[serde(default)]
        entity: Option<u64>,
        #[serde(default)]
        editor_id: Option<String>,
    },
}

/// Edit mode runs only global systems; play mode runs what the scene activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Edit,
    Play,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemInfo {
    pub name: String,
    pub enabled: bool,
    pub global: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageInfo {
    pub stage: String,
    /// In execution order.
    pub systems: Vec<SystemInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityInfo {
    /// The persistent editor id, when the entity has one.
    pub editor_id: Option<String>,
    /// `entity_to_bits`.
    pub entity: u64,
    /// The `Name` component, when present.
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentInfo {
    pub name: String,
    pub id: u32,
    /// Field name to value: a scalar, or an array for `count > 1`; `bool` is a JSON bool and
    /// `entity` is the raw bits.
    pub fields: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    /// `info`, `warn`, `alert` or `error`.
    pub level: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Entities { entities: Vec<EntityInfo> },
    Components { entity: u64, components: Vec<ComponentInfo> },
    /// The `efe-schema` document (same structure as the exported `.efschema.json`).
    Schema { schema: Value },
    Stats { frame: u64, frame_time: f64, entity_count: u32 },
    Logs { lines: Vec<LogLine> },
    Systems { stages: Vec<StageInfo> },
    Scene { name: Option<String>, path: Option<String>, mode: Mode, project_dir: String },
    Mode { mode: Mode },
    /// `gaps`: type names of resources not registered for snapshots (engine ones included);
    /// `not_covered`: kinds of state no snapshot covers; `snapshot`: a play snapshot is held.
    Fidelity { gaps: Vec<String>, not_covered: Vec<String>, snapshot: bool },
    Saved { path: String },
    Loaded { path: String, entities: usize },
    /// `old`: the field's value before the change (same JSON shape as `components`).
    FieldSet { entity: u64, old: Value },
    Spawned { entity: u64, editor_id: String },
    /// `owned`: the entity carried `SceneOwned` (saved with the scene). `components` are its
    /// writable registered components, ready to feed back into `spawn`.
    Despawned { owned: bool, editor_id: Option<String>, components: Vec<ComponentInfo> },
    Error { code: String, message: String },
}

impl Reply {
    pub fn error(code: &str, message: impl Into<String>) -> Reply {
        Reply::Error { code: code.to_string(), message: message.into() }
    }
}

/// Write one frame.
pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.len() > u32::MAX as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large"));
    }
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)?;
    w.flush()
}

/// Serialize `msg` and write it as one frame.
pub fn send<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write_frame(w, &bytes)
}

/// Read one frame, blocking (client side and tests).
pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Read one frame and decode it.
pub fn recv<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> io::Result<T> {
    let bytes = read_frame(r)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Incremental frame reader for sockets with a read timeout: partial frames survive a timeout.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Do at most one `read`, then return a complete frame if one is buffered. `Ok(None)` means
    /// "no full frame yet" (including a timeout); EOF is `UnexpectedEof`.
    pub fn poll(&mut self, r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
        if let Some(f) = self.take()? {
            return Ok(Some(f));
        }
        let mut tmp = [0u8; 4096];
        match r.read(&mut tmp) {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                self.buf.extend_from_slice(&tmp[..n]);
                self.take()
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn take(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let n = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if n > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
        }
        if self.buf.len() < 4 + n {
            return Ok(None);
        }
        let frame = self.buf[4..4 + n].to_vec();
        self.buf.drain(..4 + n);
        Ok(Some(frame))
    }
}
