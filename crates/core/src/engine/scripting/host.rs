//! `ScriptHost`: the host state every script call sees (`HostRegistry<ScriptHost>`), and the
//! table of loaded modules used for cross-module calls.

use std::fmt;

use efevm::{Exit, ExportKind, Instance, Module, SpawnError, Trap};
use hecs::World;

use super::{ModuleId, ScriptEvent, ScriptEventError};
use crate::engine::{commands::Commands, input::InputState, time::Time, types::TypeRegistry};

/// Severity of a script log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line a script (or the script system) logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

/// At most this many log lines are kept per frame; the rest are dropped.
pub(crate) const MAX_LOG_LINES: usize = 1000;
/// At most this many script events can be sent per frame (all events together).
pub(crate) const MAX_EVENTS: usize = 4096;

/// File watch of a hot-reloaded module (native only).
#[cfg(not(target_family = "wasm"))]
pub(crate) struct Watch {
    /// The file is a `.efmod` package (else a bare `.efvm` module).
    pub package: bool,
    /// `path` is a `.efa` source: the watch covers every `.efa` file in its directory tree, and
    /// a change recompiles it through the cache.
    pub source: bool,
    pub path: std::path::PathBuf,
    pub mtime: Option<std::time::SystemTime>,
}

/// A loaded (or unloaded: `loaded == false`) module. Slots are never reused, so a
/// [`ModuleId`] stays valid (and says "unloaded") forever.
pub(crate) struct ModuleSlot {
    pub name: String,
    pub module: Module,
    /// `None` while the module runs (re-entrance guard) and after unload.
    pub instance: Option<Instance<ScriptHost>>,
    pub loaded: bool,
    pub version: u32,
    /// Units per frame.
    pub budget: u64,
    /// `Limits::max_memory_pages` of the instance.
    pub memory_pages: u32,
    /// Units left this frame.
    pub remaining: u64,
    /// Import namespace prefixes this module may use; `None` = all (game modules).
    pub allowed: Option<Vec<String>>,
    pub pending_init: bool,
    /// Doc of the namespace the mod's provides live in (only used with section 13).
    pub doc: String,
    /// What a loaded `.efmod` package adds (manifest, grant, assets); `None` for game modules
    /// and bare `load_mod` mods.
    pub package: Option<super::packages::PackageInfo>,
    #[cfg(not(target_family = "wasm"))]
    pub watch: Option<Watch>,
}

/// Why a call into a module failed.
#[derive(Debug)]
pub(crate) enum RunError {
    NoModule,
    Unloaded(String),
    Busy(String),
    OutOfBudget,
    Yielded,
    Spawn(SpawnError),
    /// Trap text with function names from the names section.
    Trap(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::NoModule => f.write_str("no such module"),
            RunError::Unloaded(n) => write!(f, "module {n} unloaded"),
            RunError::Busy(n) => write!(f, "module {n} is already running (re-entrant call)"),
            RunError::OutOfBudget => f.write_str("out of budget"),
            RunError::Yielded => f.write_str("function yielded"),
            RunError::Spawn(e) => write!(f, "cannot call: {e}"),
            RunError::Trap(t) => f.write_str(t),
        }
    }
}

/// Name of function `func` (module index space) from the names section or the exports.
pub(crate) fn function_name(module: &Module, func: u32) -> String {
    let d = module.data();
    if let Some((_, n)) = d.names.funcs.iter().find(|(i, _)| *i == func) {
        return n.clone();
    }
    if let Some(e) = d.exports.iter().find(|e| e.kind == ExportKind::Func(func)) {
        return e.name.clone();
    }
    format!("func#{func}")
}

fn format_trap(module: &Module, trap: &Trap, trace: &[(u32, u32)]) -> String {
    let mut s = trap.to_string();
    for (func, pc) in trace {
        s.push_str(&format!("\n    at {} (instruction {pc})", function_name(module, *func)));
    }
    s
}

/// The state host functions see while scripts run.
///
/// The engine moves the world in (`std::mem::take`) before the script system runs and back
/// afterwards, so host functions have plain `&mut World` access without any unsafe code.
/// World changes that must be deferred go through `commands`, which the script system
/// appends to the stage's commands.
pub struct ScriptHost {
    pub(crate) world: World,
    pub(crate) types: TypeRegistry,
    pub(crate) commands: Commands,
    pub(crate) time: Time,
    pub(crate) input: InputState,
    /// Script events readable this frame.
    pub(crate) events_in: Vec<ScriptEvent>,
    /// Script events sent this frame (readable next frame).
    pub(crate) events_out: Vec<ScriptEvent>,
    /// Sends refused by the per-frame cap since the last `leave`.
    pub(crate) dropped_events: usize,
    /// Registered events and their payload sizes (qualified name), refreshed when scripts run.
    pub(crate) event_sizes: std::collections::BTreeMap<String, usize>,
    pub(crate) logs: Vec<LogLine>,
    pub(crate) modules: Vec<ModuleSlot>,
    pub(crate) current: Option<ModuleId>,
    /// State of `engine.random` (xorshift64*).
    pub(crate) rng: u64,
}

impl ScriptHost {
    pub(crate) fn new() -> Self {
        ScriptHost {
            world: World::new(),
            types: TypeRegistry::new(),
            commands: Commands::new(),
            time: Time::new(1.0 / 60.0),
            input: InputState::new(),
            events_in: Vec::new(),
            events_out: Vec::new(),
            dropped_events: 0,
            event_sizes: Default::default(),
            logs: Vec::new(),
            modules: Vec::new(),
            current: None,
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    /// The next pseudo-random 64 bits (xorshift64*; never 0 internally).
    pub(crate) fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Reseed `engine.random`. Seed 0 is mapped to a fixed non-zero value.
    pub(crate) fn seed_random(&mut self, seed: u64) {
        self.rng = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
    }

    /// The world (only while scripts run; empty otherwise).
    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Deferred world changes; applied after the stage, in order.
    pub fn commands_mut(&mut self) -> &mut Commands {
        &mut self.commands
    }

    /// World and commands together (`commands.spawn(world, ..)` needs both).
    pub fn world_and_commands(&mut self) -> (&mut World, &mut Commands) {
        (&mut self.world, &mut self.commands)
    }

    pub fn types(&self) -> &TypeRegistry {
        &self.types
    }

    /// A copy of the engine's time for this frame.
    pub fn time(&self) -> &Time {
        &self.time
    }

    /// A copy of the engine's input state for this frame.
    pub fn input(&self) -> &InputState {
        &self.input
    }

    /// Script events readable this frame.
    pub fn events(&self) -> &[ScriptEvent] {
        &self.events_in
    }

    /// The payloads of the script events `event` (`namespace.name`) readable this frame, in
    /// send order. Entries whose length differs from the registered payload size are skipped.
    pub fn events_named<'a>(&'a self, event: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        let size = self.event_sizes.get(event).copied();
        self.events_in.iter().filter(move |e| e.event == event && Some(e.payload.len()) == size).map(|e| e.payload.as_slice())
    }

    /// Send the script event `event` (`namespace.name`, registered) with `payload` (exactly the
    /// registered payload struct's size; empty for an event without payload); readable next
    /// frame by scripts and Rust systems. An unregistered event or a payload of the wrong size
    /// is refused. Counts against the 4096 events per frame cap like a script's send: beyond
    /// it the event is dropped (and counted, `Scripting::dropped_events_last_frame`), which is
    /// not an error.
    pub fn send_event(&mut self, event: &str, payload: &[u8]) -> Result<(), ScriptEventError> {
        match self.event_sizes.get(event) {
            None => return Err(ScriptEventError::Unknown { event: event.to_string() }),
            Some(&n) if n != payload.len() => {
                return Err(ScriptEventError::Payload { event: event.to_string(), expected: n, got: payload.len() });
            }
            Some(_) => {}
        }
        self.queue_event(ScriptEvent { event: event.to_string(), payload: payload.to_vec() });
        Ok(())
    }

    /// Queue an already checked event, or count it as dropped over the cap.
    pub(crate) fn queue_event(&mut self, ev: ScriptEvent) {
        if self.events_out.len() < MAX_EVENTS {
            self.events_out.push(ev);
        } else {
            self.dropped_events += 1;
        }
    }

    /// The module whose code is running now, if any.
    pub fn current_module(&self) -> Option<ModuleId> {
        self.current
    }

    /// Name of module `id`.
    pub fn module_name(&self, id: ModuleId) -> Option<&str> {
        self.modules.get(id.0 as usize).map(|m| m.name.as_str())
    }

    /// Log a line (emitted through the engine logger after the stage).
    pub fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.logs.len() < MAX_LOG_LINES {
            self.logs.push(LogLine { level, text: text.into() });
        }
    }

    /// Call export `export` of module `id` with raw argument bits, from the module's
    /// remaining budget. The instance is taken out of its slot while it runs, so calling a
    /// module that is already running (directly or through another module) is a clean error.
    pub(crate) fn run_export(&mut self, id: ModuleId, export: &str, args: &[u64]) -> Result<Vec<u64>, RunError> {
        self.run_inner(id, Target::Export(export), args, 0, &[]).map(|(r, _)| r)
    }

    /// Call function `func` (module index space) of module `id`: the same as
    /// [`run_export`](Self::run_export) but by index, for provides, which need not be exports.
    #[allow(dead_code)]
    pub(crate) fn run_func(&mut self, id: ModuleId, func: u32, args: &[u64]) -> Result<Vec<u64>, RunError> {
        self.run_inner(id, Target::Func(func), args, 0, &[]).map(|(r, _)| r)
    }

    /// Run a provide for a call from another module (the copy ABI, e8-design section 7):
    /// charge `call.units` from the module's remaining budget, write the copied arguments into
    /// the module's memory, run `call.func` and, only after a normal return, read the ranges
    /// marked `back` out again (in order).
    pub(crate) fn run_provide(&mut self, id: ModuleId, call: &ProvideCall) -> Result<(Vec<u64>, Vec<Vec<u8>>), RunError> {
        self.run_inner(id, Target::Func(call.func), &call.args, call.units, &call.copies)
    }

    fn run_inner(
        &mut self,
        id: ModuleId,
        target: Target<'_>,
        args: &[u64],
        units: u64,
        copies: &[CopyIn],
    ) -> Result<(Vec<u64>, Vec<Vec<u8>>), RunError> {
        let idx = id.0 as usize;
        let slot = self.modules.get_mut(idx).ok_or(RunError::NoModule)?;
        if !slot.loaded {
            return Err(RunError::Unloaded(slot.name.clone()));
        }
        if slot.instance.is_none() {
            return Err(RunError::Busy(slot.name.clone()));
        }
        if slot.remaining < units {
            return Err(RunError::OutOfBudget);
        }
        slot.remaining -= units;
        let mut inst = slot.instance.take().expect("checked above");
        let budget = slot.remaining;
        let mut written = Ok(());
        for c in copies {
            if let Err(t) = inst.memory_mut().write(c.dst, &c.data) {
                written = Err(RunError::Trap(format!("{t} (copying an argument into the scratch region of module {})", slot.name)));
                break;
            }
        }
        let (result, spent) = match written {
            Ok(()) => {
                let prev = self.current.replace(id);
                let r = drive(&mut inst, target, args, self, budget);
                self.current = prev;
                r
            }
            Err(e) => (Err(e), 0),
        };
        let mut back = Vec::new();
        let mut result = result;
        if result.is_ok() {
            for c in copies.iter().filter(|c| c.back) {
                let mut buf = vec![0u8; c.data.len()];
                match inst.memory().read(c.dst, &mut buf) {
                    Ok(()) => back.push(buf),
                    Err(t) => {
                        result = Err(RunError::Trap(t.to_string()));
                        break;
                    }
                }
            }
        }
        let slot = &mut self.modules[idx];
        slot.remaining = slot.remaining.saturating_sub(spent);
        if matches!(result, Err(RunError::OutOfBudget)) {
            slot.remaining = 0;
        }
        slot.instance = Some(inst);
        result.map(|r| (r, back))
    }
}

/// What to start in a module.
#[derive(Clone, Copy)]
enum Target<'a> {
    Export(&'a str),
    Func(u32),
}

/// One argument range copied into a provider's scratch region.
pub(crate) struct CopyIn {
    /// Address in the provider's memory.
    pub dst: u64,
    pub data: Vec<u8>,
    /// Copy the range back to the caller after a normal return.
    pub back: bool,
}

/// Everything [`ScriptHost::run_provide`] needs for one call.
pub(crate) struct ProvideCall {
    pub func: u32,
    /// Raw argument slots as the provider's function takes them (pointers already rewritten).
    pub args: Vec<u64>,
    pub copies: Vec<CopyIn>,
    /// Budget units to charge before the run.
    pub units: u64,
}

fn drive(
    inst: &mut Instance<ScriptHost>,
    target: Target<'_>,
    args: &[u64],
    host: &mut ScriptHost,
    budget: u64,
) -> (Result<Vec<u64>, RunError>, u64) {
    let spawned = match target {
        Target::Export(name) => inst.spawn(name, args),
        Target::Func(f) => inst.spawn_func(f, args),
    };
    let fiber = match spawned {
        Ok(f) => f,
        Err(e) => return (Err(RunError::Spawn(e)), 0),
    };
    let (exit, units) = inst.resume_units(fiber, host, budget);
    let result = match exit {
        Exit::Done => Ok(inst.results(fiber).map(<[u64]>::to_vec).unwrap_or_default()),
        Exit::Trap(trap) => {
            let trace = inst.backtrace(fiber).map(<[(u32, u32)]>::to_vec).unwrap_or_default();
            Err(RunError::Trap(format_trap(inst.module(), &trap, &trace)))
        }
        Exit::OutOfBudget => Err(RunError::OutOfBudget),
        Exit::Yield => Err(RunError::Yielded),
    };
    inst.remove_fiber(fiber);
    (result, units)
}
