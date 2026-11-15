//! `Commands`: engine control requests and deferred world changes (ROADMAP 3.3).
//!
//! Systems (and later scripts) never change the world's structure directly while they run:
//! spawn, despawn, insert and remove are queued here and applied in order after every stage,
//! so despawning inside a query is safe. An operation on an entity that is dead by then is
//! ignored (logged at debug level), never a panic.

use hecs::{Component, DynamicBundle, Entity, EntityBuilder, World};

use crate::engine::types::TypeRegistry;

/// A control request a system makes of the engine; applied after the stage ends.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineCommand {
    Quit,
    SetTitle(String),
    SetFrameRate(f32),
    SetCursorVisible(bool),
    SetCursorGrab(bool),
    SetTextInput(bool),
}

/// A queued structural change of the world.
enum WorldOp {
    /// Insert a bundle into an entity reserved by [`Commands::spawn`].
    Spawn(Entity, EntityBuilder),
    Despawn(Entity),
    Insert(Entity, EntityBuilder),
    Remove(Entity, fn(&mut World, Entity) -> bool, &'static str),
    InsertRaw(Entity, u32, Vec<u8>),
    RemoveRaw(Entity, u32),
}

impl std::fmt::Debug for WorldOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorldOp::Spawn(e, _) => write!(f, "Spawn({e:?})"),
            WorldOp::Despawn(e) => write!(f, "Despawn({e:?})"),
            WorldOp::Insert(e, _) => write!(f, "Insert({e:?})"),
            WorldOp::Remove(e, _, t) => write!(f, "Remove({e:?}, {t})"),
            WorldOp::InsertRaw(e, id, b) => write!(f, "InsertRaw({e:?}, {id:#010x}, {} bytes)", b.len()),
            WorldOp::RemoveRaw(e, id) => write!(f, "RemoveRaw({e:?}, {id:#010x})"),
        }
    }
}

fn remove_fn<T: Component>(w: &mut World, e: Entity) -> bool {
    w.remove_one::<T>(e).is_ok()
}

/// Buffer of control requests and world changes, applied by the engine after every stage.
#[derive(Debug, Default)]
pub struct Commands {
    list: Vec<EngineCommand>,
    ops: Vec<WorldOp>,
}

/// The old name of [`Commands`].
pub type EngineCommands = Commands;

impl Commands {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- engine control ----

    pub fn quit(&mut self) {
        self.list.push(EngineCommand::Quit);
    }
    pub fn set_title(&mut self, t: impl Into<String>) {
        self.list.push(EngineCommand::SetTitle(t.into()));
    }
    /// `fps <= 0` (or non-finite) removes the frame-rate cap.
    pub fn set_frame_rate(&mut self, fps: f32) {
        self.list.push(EngineCommand::SetFrameRate(fps));
    }
    pub fn set_cursor_visible(&mut self, v: bool) {
        self.list.push(EngineCommand::SetCursorVisible(v));
    }
    pub fn set_cursor_grab(&mut self, g: bool) {
        self.list.push(EngineCommand::SetCursorGrab(g));
    }
    pub fn set_text_input(&mut self, on: bool) {
        self.list.push(EngineCommand::SetTextInput(on));
    }

    // ---- world ----

    /// Reserve an entity in `world` now and queue inserting `bundle` into it. The returned
    /// handle can be used in later commands right away; it is alive (and has the bundle)
    /// once the commands are applied. If the commands are dropped instead, the reserved
    /// entity still comes to life empty at the world's next structural change (hecs `flush`).
    pub fn spawn(&mut self, world: &World, bundle: impl DynamicBundle) -> Entity {
        let e = world.reserve_entity();
        let mut b = EntityBuilder::new();
        b.add_bundle(bundle);
        self.ops.push(WorldOp::Spawn(e, b));
        e
    }

    /// Reserve an entity with no components (the byte-level path: scripts add components
    /// with [`Commands::insert_raw`]).
    pub fn spawn_empty(&mut self, world: &World) -> Entity {
        self.spawn(world, ())
    }

    pub fn despawn(&mut self, e: Entity) {
        self.ops.push(WorldOp::Despawn(e));
    }

    /// Add (or replace) one component.
    pub fn insert(&mut self, e: Entity, component: impl Component) {
        self.insert_bundle(e, (component,));
    }

    /// Add (or replace) several components.
    pub fn insert_bundle(&mut self, e: Entity, bundle: impl DynamicBundle) {
        let mut b = EntityBuilder::new();
        b.add_bundle(bundle);
        self.ops.push(WorldOp::Insert(e, b));
    }

    /// Remove component `T` (a no-op if the entity does not have it).
    pub fn remove<T: Component>(&mut self, e: Entity) {
        self.ops.push(WorldOp::Remove(e, remove_fn::<T>, std::any::type_name::<T>()));
    }

    /// Add (or replace) the registered component `component_id` from its bytes; resolved
    /// through the [`TypeRegistry`] when applied (unknown id or wrong size: logged, skipped).
    pub fn insert_raw(&mut self, e: Entity, component_id: u32, bytes: Vec<u8>) {
        self.ops.push(WorldOp::InsertRaw(e, component_id, bytes));
    }

    /// Remove the registered component `component_id`.
    pub fn remove_raw(&mut self, e: Entity, component_id: u32) {
        self.ops.push(WorldOp::RemoveRaw(e, component_id));
    }

    // ---- buffer ----

    /// Move everything queued in `other` to the end of this buffer (order kept).
    pub fn append(&mut self, other: &mut Commands) {
        self.list.append(&mut other.list);
        self.ops.append(&mut other.ops);
    }

    /// Nothing queued (neither control requests nor world changes).
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.ops.is_empty()
    }

    /// Number of queued world changes.
    pub fn world_ops(&self) -> usize {
        self.ops.len()
    }

    /// Apply the queued world changes to `world` in order and clear them. Control requests
    /// stay queued (the engine applies those). Operations on dead entities are ignored.
    pub fn apply(&mut self, world: &mut World, types: &TypeRegistry) {
        for op in self.ops.drain(..) {
            match op {
                WorldOp::Spawn(e, mut b) | WorldOp::Insert(e, mut b) => {
                    if world.insert(e, b.build()).is_err() {
                        ::log::debug!("commands: insert into dead entity {e:?} ignored");
                    }
                }
                WorldOp::Despawn(e) => {
                    if world.despawn(e).is_err() {
                        ::log::debug!("commands: despawn of dead entity {e:?} ignored");
                    }
                }
                WorldOp::Remove(e, f, name) => {
                    if !f(world, e) {
                        ::log::debug!("commands: remove of {name} from {e:?} ignored (dead entity or no such component)");
                    }
                }
                WorldOp::InsertRaw(e, id, bytes) => {
                    let r = types.component(id).and_then(|t| t.insert(world, e, &bytes));
                    log_raw("insert_raw", e, r);
                }
                WorldOp::RemoveRaw(e, id) => {
                    let r = types.component(id).and_then(|t| t.remove(world, e));
                    log_raw("remove_raw", e, r);
                }
            }
        }
    }

    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, EngineCommand> {
        self.list.drain(..)
    }
}

fn log_raw(what: &str, e: Entity, r: Result<(), crate::engine::types::CompError>) {
    use crate::engine::types::CompError;
    match r {
        Ok(()) => {}
        Err(CompError::NoEntity | CompError::Missing(_)) => ::log::debug!("commands: {what} on {e:?} ignored: {}", r.unwrap_err()),
        Err(err) => ::log::warn!("commands: {what} on {e:?} failed: {err}"),
    }
}
