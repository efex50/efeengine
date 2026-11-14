//! System schedule: stages, the per-system context and engine control commands.

use crate::engine::{
    WindowState,
    events::{EventRegistry, Events},
    input::{GamepadState, InputState},
    resources::Resources,
    time::Time,
    world::EngineWorld,
};

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
    pub world: &'a mut EngineWorld,
    pub resources: &'a mut Resources,
    pub time: &'a Time,
    pub input: &'a InputState,
    pub gamepads: &'a GamepadState,
    pub window: &'a WindowState,
    pub commands: &'a mut EngineCommands,
    events: &'a mut EventRegistry,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(
        world: &'a mut EngineWorld,
        resources: &'a mut Resources,
        time: &'a Time,
        input: &'a InputState,
        gamepads: &'a GamepadState,
        window: &'a WindowState,
        commands: &'a mut EngineCommands,
        events: &'a mut EventRegistry,
    ) -> Self {
        Self { world, resources, time, input, gamepads, window, commands, events }
    }

    /// Events of type `T` readable this frame (empty if none were ever sent).
    pub fn events<T: 'static>(&self) -> &Events<T> {
        self.events.get_or_empty::<T>()
    }

    /// Send an event; readable by every system during the next frame.
    pub fn send<T: 'static>(&mut self, ev: T) {
        self.events.send(ev);
    }
}

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

/// Buffer of [`EngineCommand`]s, applied by the engine after every stage.
#[derive(Debug, Default)]
pub struct EngineCommands {
    list: Vec<EngineCommand>,
}

impl EngineCommands {
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
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, EngineCommand> {
        self.list.drain(..)
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
