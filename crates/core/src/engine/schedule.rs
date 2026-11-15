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
