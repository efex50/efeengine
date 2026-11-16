//! System schedule: stages and the per-system context.

use hecs::{DynamicBundle, Entity, World};

use crate::engine::{
    WindowState,
    events::{EventRegistry, Events},
    input::{GamepadState, InputState},
    resources::Resources,
    time::Time,
    scripting::Scripting,
    types::TypeRegistry,
};
pub use crate::engine::commands::{Commands, EngineCommand, EngineCommands};

/// When a system runs. See the frame order in `docs/design-io-and-loop.md` section 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Once, at the start of the first frame.
    Startup,
    PreUpdate,
    /// Zero or more times per frame from the fixed-step accumulator.
    FixedUpdate,
    Update,
    PostUpdate,
    RenderExtract,
    /// Once, when the engine is killed.
    Shutdown,
}

impl Stage {
    pub(crate) const COUNT: usize = 7;

    fn index(self) -> usize {
        self as usize
    }
}

/// Borrowed view of the engine handed to every system.
pub struct Ctx<'a> {
    /// The game world. Query it freely; queue structural changes through `commands`
    /// (or [`Ctx::spawn`]) so they are applied after the stage.
    pub world: &'a mut World,
    /// Registered script-visible types (components and structs).
    pub types: &'a TypeRegistry,
    pub resources: &'a mut Resources,
    pub time: &'a Time,
    pub input: &'a InputState,
    pub gamepads: &'a GamepadState,
    pub window: &'a WindowState,
    pub commands: &'a mut Commands,
    /// The scripting runtime (host registry, loaded modules). Systems may register host
    /// functions or load modules; the script system itself runs first in `Stage::Update`.
    pub scripting: &'a mut Scripting,
    events: &'a mut EventRegistry,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(
        world: &'a mut World,
        types: &'a TypeRegistry,
        resources: &'a mut Resources,
        time: &'a Time,
        input: &'a InputState,
        gamepads: &'a GamepadState,
        window: &'a WindowState,
        commands: &'a mut Commands,
        scripting: &'a mut Scripting,
        events: &'a mut EventRegistry,
    ) -> Self {
        Self { world, types, resources, time, input, gamepads, window, commands, scripting, events }
    }

    /// Events of type `T` readable this frame (empty if none were ever sent).
    pub fn events<T: 'static>(&self) -> &Events<T> {
        self.events.get_or_empty::<T>()
    }

    /// Spawn an entity with `bundle`: the entity is reserved now (the handle is valid for
    /// further commands at once) and gets its components when the stage's commands apply.
    pub fn spawn(&mut self, bundle: impl DynamicBundle) -> Entity {
        self.commands.spawn(self.world, bundle)
    }

    /// Send an event; readable by every system during the next frame.
    pub fn send<T: 'static>(&mut self, ev: T) {
        self.events.send(ev);
    }

    /// Send the registered script event `event` (`namespace.name`) with `payload` (exactly the
    /// size of its payload struct; empty for an event without payload). It is refused, and not
    /// queued, if the event is not registered or the size is wrong
    /// ([`ScriptEventError`](crate::engine::scripting::ScriptEventError)). Readable by scripts
    /// (`engine.events.read`) and by Rust systems ([`script_events`](Self::script_events))
    /// during the next frame. Rust sends do not count against the scripts' 4096 per frame cap.
    pub fn send_script_event(&mut self, event: &str, payload: &[u8]) -> Result<(), crate::engine::scripting::ScriptEventError> {
        self.scripting.check_event(event, payload.len())?;
        self.events.send(crate::engine::scripting::ScriptEvent { event: event.to_string(), payload: payload.to_vec() });
        Ok(())
    }

    /// The payloads of the script events `event` (`namespace.name`) readable this frame, in
    /// send order (sent by scripts or by Rust last frame). Entries whose size is not the
    /// registered payload size are skipped.
    pub fn script_events<'b>(&'b self, event: &'b str) -> impl Iterator<Item = &'b [u8]> + 'b {
        let size = self.scripting.event_payload_size(event);
        self.events::<crate::engine::scripting::ScriptEvent>()
            .iter()
            .filter(move |e| e.event == event && Some(e.payload.len()) == size)
            .map(|e| e.payload.as_slice())
    }
}

/// A boxed system function.
pub type System = Box<dyn FnMut(&mut Ctx<'_>)>;

/// Systems per stage, in insertion order.
pub struct Schedule {
    stages: [Vec<System>; Stage::COUNT],
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    pub fn new() -> Self {
        Self { stages: std::array::from_fn(|_| Vec::new()) }
    }

    pub fn add(&mut self, stage: Stage, system: System) {
        self.stages[stage.index()].push(system);
    }

    pub fn len(&self, stage: Stage) -> usize {
        self.stages[stage.index()].len()
    }

    pub(crate) fn run(&mut self, stage: Stage, cx: &mut Ctx<'_>) {
        for system in &mut self.stages[stage.index()] {
            system(cx);
        }
    }
}
