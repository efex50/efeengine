//! `ScriptHost`: the host state every script call sees (`HostRegistry<ScriptHost>`), and the
//! table of loaded modules used for cross-module calls.

use std::fmt;

use efevm::{Exit, ExportKind, Instance, Module, SpawnError, Trap};
use hecs::World;

use super::{ModuleId, ScriptEvent};
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
/// At most this many script events can be sent per frame.
pub(crate) const MAX_EVENTS: usize = 4096;

/// File watch of a hot-reloaded module (native only).
#[cfg(not(target_family = "wasm"))]
pub(crate) struct Watch {
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
    /// Units left this frame.
    pub remaining: u64,
    /// Import namespace prefixes this module may use; `None` = all (game modules).
    pub allowed: Option<Vec<String>>,
    pub pending_init: bool,
    /// `(function, doc)` requested with `engine.mods.export` during `init`.
    pub exports: Vec<(String, String)>,
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
    pub(crate) logs: Vec<LogLine>,
    pub(crate) modules: Vec<ModuleSlot>,
    pub(crate) current: Option<ModuleId>,
    pub(crate) in_init: bool,
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
            logs: Vec::new(),
            modules: Vec::new(),
            current: None,
            in_init: false,
        }
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

    /// Send a script event (readable next frame by scripts and Rust systems).
    pub fn send_event(&mut self, ev: ScriptEvent) {
        if self.events_out.len() < MAX_EVENTS {
            self.events_out.push(ev);
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

    /// `engine.mods.export`: remember `func` to be registered after `init` returns.
    /// 0 ok, -1 refused (not inside `init`, no such exported function, bad name or doc).
    pub(crate) fn request_export(&mut self, func: &str, doc: &str) -> i32 {
        let (Some(id), true) = (self.current, self.in_init) else { return -1 };
        if !efevm::api::is_ident(func) || doc.trim().is_empty() {
            return -1;
        }
        let Some(slot) = self.modules.get_mut(id.0 as usize) else { return -1 };
        if slot.module.export_func(func).is_none() {
            return -1;
        }
        match slot.exports.iter_mut().find(|(f, _)| f == func) {
            Some(e) => e.1 = doc.to_string(),
            None => slot.exports.push((func.to_string(), doc.to_string())),
        }
        0
    }

    /// Call export `export` of module `id` with raw argument bits, from the module's
    /// remaining budget. The instance is taken out of its slot while it runs, so calling a
    /// module that is already running (directly or through another module) is a clean error.
    pub(crate) fn run_export(&mut self, id: ModuleId, export: &str, args: &[u64]) -> Result<Vec<u64>, RunError> {
        let idx = id.0 as usize;
        let slot = self.modules.get_mut(idx).ok_or(RunError::NoModule)?;
        if !slot.loaded {
            return Err(RunError::Unloaded(slot.name.clone()));
        }
        let Some(mut inst) = slot.instance.take() else {
            return Err(RunError::Busy(slot.name.clone()));
        };
        let budget = slot.remaining;
        let prev = self.current.replace(id);
        let (result, units) = drive(&mut inst, export, args, self, budget);
        self.current = prev;
        let slot = &mut self.modules[idx];
        slot.remaining = slot.remaining.saturating_sub(units);
        if matches!(result, Err(RunError::OutOfBudget)) {
            slot.remaining = 0;
        }
        slot.instance = Some(inst);
        result
    }
}

fn drive(
    inst: &mut Instance<ScriptHost>,
    export: &str,
    args: &[u64],
    host: &mut ScriptHost,
    budget: u64,
) -> (Result<Vec<u64>, RunError>, u64) {
    let fiber = match inst.spawn(export, args) {
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
