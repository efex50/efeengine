//! `ScriptHost`: the host state every script call sees (`HostRegistry<ScriptHost>`), and the
//! table of loaded modules used for cross-module calls.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;

use efevm::{Exit, ExportKind, Instance, Module, SpawnError, Trap};
use hecs::{Entity, World};

use super::{ModuleId, ScriptEvent, ScriptEventError};
use crate::engine::{commands::Commands, input::InputState, time::Time, types::{CompError, TypeRegistry}};

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
/// Pool of reserved spawn entities per module: initial and bounds (docs/PARALLEL_SPEC.md 7.5).
pub(crate) const MIN_POOL: usize = 64;
pub(crate) const MAX_POOL: usize = 65536;
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
    /// State of `engine.random` of this module (xorshift64*, never 0).
    pub rng: u64,
    /// Entities reserved for `engine.world.spawn` this frame.
    pub spawn_pool: VecDeque<Entity>,
    /// How many entities to reserve for the next frame.
    pub pool_size: usize,
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
    /// The module belongs to another lane (cannot happen: lanes are closed under imports).
    OtherLane(String),
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
            RunError::OtherLane(n) => write!(f, "module {n} runs in another lane"),
            RunError::OutOfBudget => f.write_str("out of budget"),
            RunError::Yielded => f.write_str("function yielded"),
            RunError::Spawn(e) => write!(f, "cannot call: {e}"),
            RunError::Trap(t) => f.write_str(t),
        }
    }
}

/// The start state of the `engine.random` stream of module `name`: a mix of the engine's seed
/// and the name (FNV-1a), so every module has its own stream and the result does not depend on
/// which thread runs it.
pub(crate) fn seed_for(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let s = h ^ 0x2545_F491_4F6C_DD1D;
    if s == 0 { 0x9E37_79B9_7F4A_7C15 } else { s }
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

/// The read-only snapshot of the engine every lane sees during the script phase (shared as an
/// `Arc<FrameView>`). The world is moved in (`mem::take`) before the phase and back afterwards,
/// so the scripting side needs no unsafe code.
pub(crate) struct FrameView {
    pub world: World,
    pub types: Arc<TypeRegistry>,
    pub time: Time,
    pub input: InputState,
    /// Script events readable this frame.
    pub events_in: Vec<ScriptEvent>,
    /// Registered events and their payload sizes (qualified name).
    pub event_sizes: BTreeMap<String, usize>,
    /// Name of every module slot, by id (for error messages about modules outside a lane).
    pub module_names: Vec<String>,
    /// `(module id, entity)` of every `Script`, sorted by module (stable, so entity order stays).
    pub jobs: Vec<(u32, Entity)>,
}

/// The state host functions see while a lane of scripts runs (the `H` of the VM).
///
/// Every lane has its own value: the world is the frozen snapshot ([`FrameView`]); `set` is
/// buffered (with read-your-own-writes) and applied at the merge, structural changes go through
/// `commands`, spawns come from a pre-reserved pool per module. Nothing here is shared between
/// lanes, so lanes can run on different threads without any ordering dependence.
pub struct ScriptHost {
    pub(crate) frame: Arc<FrameView>,
    /// The slots of this lane's modules, by module id; `None` for modules of other lanes.
    pub(crate) modules: Vec<Option<ModuleSlot>>,
    /// Buffered `engine.world.set` calls, in call order.
    pub(crate) writes: Vec<(Entity, u32, Vec<u8>)>,
    /// Latest entry of `writes` per `(entity, component)`, for read-your-own-writes.
    pub(crate) write_index: HashMap<(Entity, u32), usize>,
    pub(crate) commands: Commands,
    /// New `Script.state` values, applied at the merge.
    pub(crate) script_state: Vec<(Entity, i64)>,
    /// Script events sent this frame (readable next frame), capped per lane.
    pub(crate) events_out: Vec<ScriptEvent>,
    /// Sends refused by the cap in this lane.
    pub(crate) dropped_events: usize,
    pub(crate) logs: Vec<LogLine>,
    /// `(key, text)` warnings that are logged once per key (deduplicated at the merge).
    pub(crate) warns: Vec<(String, String)>,
    /// Spawns refused because the module's pool was empty, per module.
    pub(crate) overflow: BTreeMap<ModuleId, u64>,
    pub(crate) current: Option<ModuleId>,
}

impl ScriptHost {
    pub(crate) fn lane(frame: Arc<FrameView>, modules: Vec<Option<ModuleSlot>>) -> Self {
        ScriptHost {
            frame,
            modules,
            writes: Vec::new(),
            write_index: HashMap::new(),
            commands: Commands::new(),
            script_state: Vec::new(),
            events_out: Vec::new(),
            dropped_events: 0,
            logs: Vec::new(),
            warns: Vec::new(),
            overflow: BTreeMap::new(),
            current: None,
        }
    }

    /// The slot of module `id` if it belongs to this lane.
    pub(crate) fn slot(&self, id: ModuleId) -> Option<&ModuleSlot> {
        self.modules.get(id.0 as usize).and_then(Option::as_ref)
    }

    /// The slot of module `id`, or the reason it is not available.
    pub(crate) fn slot_checked(&self, id: ModuleId) -> Result<&ModuleSlot, RunError> {
        match self.modules.get(id.0 as usize) {
            None => Err(RunError::NoModule),
            Some(None) => Err(RunError::OtherLane(self.frame.module_names.get(id.0 as usize).cloned().unwrap_or_default())),
            Some(Some(s)) => Ok(s),
        }
    }

    /// The next pseudo-random 64 bits of the current module's stream (xorshift64*; never 0).
    pub(crate) fn next_random(&mut self) -> u64 {
        let Some(slot) = self.current.and_then(|id| self.modules.get_mut(id.0 as usize)).and_then(Option::as_mut) else {
            return 0;
        };
        let mut x = slot.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        slot.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Reseed the current module's `engine.random` stream. Seed 0 is mapped to a fixed non-zero value.
    pub(crate) fn seed_random(&mut self, seed: u64) {
        if let Some(slot) = self.current.and_then(|id| self.modules.get_mut(id.0 as usize)).and_then(Option::as_mut) {
            slot.rng = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
        }
    }

    /// The world snapshot (the state at the start of the script phase; empty outside it).
    pub fn world(&self) -> &World {
        &self.frame.world
    }

    /// Overwrite, in place, the existing component `id` of `e` with `bytes`. Buffered: the
    /// write is applied when the lanes merge, but this lane reads it back at once (reading your
    /// own writes); other lanes see the start-of-phase value. The checks run against the
    /// snapshot (structure is frozen during the phase, so they still hold at the merge).
    pub fn set_component(&mut self, e: Entity, id: u32, bytes: &[u8]) -> Result<(), CompError> {
        let frame = self.frame.clone();
        if !frame.world.contains(e) {
            return Err(CompError::NoEntity);
        }
        let entry = frame.types.component(id)?;
        if !entry.has(&frame.world, e) {
            return Err(CompError::Missing(entry.name().to_string()));
        }
        if bytes.len() != entry.size() as usize {
            return Err(CompError::WrongSize { name: entry.name().to_string(), expected: entry.size(), got: bytes.len() });
        }
        self.write_index.insert((e, id), self.writes.len());
        self.writes.push((e, id, bytes.to_vec()));
        Ok(())
    }

    /// The bytes of component `id` of `e`: this lane's latest buffered write, else the snapshot.
    pub(crate) fn read_component(&self, e: Entity, id: u32) -> Result<Vec<u8>, CompError> {
        let entry = self.frame.types.component(id)?;
        if !self.frame.world.contains(e) {
            return Err(CompError::NoEntity);
        }
        if let Some(&i) = self.write_index.get(&(e, id)) {
            return Ok(self.writes[i].2.clone());
        }
        let mut buf = vec![0u8; entry.size() as usize];
        entry.read(&self.frame.world, e, &mut buf)?;
        Ok(buf)
    }

    /// Deferred world changes; applied after the stage, in order.
    pub fn commands_mut(&mut self) -> &mut Commands {
        &mut self.commands
    }

    /// Take an entity from the current module's pre-reserved spawn pool and queue making it
    /// alive (empty). `None` when the pool is empty (counted as an overflow, warned once per
    /// frame and module) or no module is running.
    pub fn spawn(&mut self) -> Option<Entity> {
        let id = self.current?;
        let e = self.modules.get_mut(id.0 as usize).and_then(Option::as_mut).and_then(|s| s.spawn_pool.pop_front());
        match e {
            Some(e) => {
                self.commands.spawn_reserved_with(e, (super::SpawnedBy::new(id),));
                Some(e)
            }
            None => {
                let n = self.overflow.entry(id).or_insert(0);
                *n += 1;
                if *n == 1 {
                    let name = self.module_name(id).unwrap_or("?").to_string();
                    self.log(LogLevel::Warn, format!("[{name}] spawn pool is empty; engine.world.spawn returns the null handle (the pool grows for the next frame)"));
                }
                None
            }
        }
    }

    pub fn types(&self) -> &TypeRegistry {
        &self.frame.types
    }

    /// A copy of the engine's time for this frame.
    pub fn time(&self) -> &Time {
        &self.frame.time
    }

    /// A copy of the engine's input state for this frame.
    pub fn input(&self) -> &InputState {
        &self.frame.input
    }

    /// Script events readable this frame.
    pub fn events(&self) -> &[ScriptEvent] {
        &self.frame.events_in
    }

    /// The payloads of the script events `event` (`namespace.name`) readable this frame, in
    /// send order. Entries whose length differs from the registered payload size are skipped.
    pub fn events_named<'a>(&'a self, event: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        let size = self.frame.event_sizes.get(event).copied();
        self.frame.events_in.iter().filter(move |e| e.event == event && Some(e.payload.len()) == size).map(|e| e.payload.as_slice())
    }

    /// Send the script event `event` (`namespace.name`, registered) with `payload` (exactly the
    /// registered payload struct's size; empty for an event without payload); readable next
    /// frame by scripts and Rust systems. An unregistered event or a payload of the wrong size
    /// is refused. Counts against the 4096 events per frame cap like a script's send: beyond
    /// it the event is dropped (and counted, `Scripting::dropped_events_last_frame`), which is
    /// not an error. The cap is applied when the lanes merge, in lane order.
    pub fn send_event(&mut self, event: &str, payload: &[u8]) -> Result<(), ScriptEventError> {
        match self.frame.event_sizes.get(event) {
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

    /// Whether module `id` is a mod, that is, limited to the namespaces of its grant (a game
    /// module may import anything). Host functions of plugins use this for their own access rules.
    pub fn is_mod(&self, id: ModuleId) -> bool {
        self.slot(id).is_some_and(|s| s.allowed.is_some())
    }

    /// Name of module `id`.
    pub fn module_name(&self, id: ModuleId) -> Option<&str> {
        self.frame.module_names.get(id.0 as usize).map(String::as_str)
    }

    /// Log a line (emitted through the engine logger after the stage).
    pub fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.logs.len() < MAX_LOG_LINES {
            self.logs.push(LogLine { level, text: text.into() });
        }
    }

    /// A warning logged once per `key` (ever), however many lanes ask.
    pub(crate) fn warn_once(&mut self, key: String, text: String) {
        self.warns.push((key, text));
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
        self.slot_checked(id)?;
        let slot = self.modules[idx].as_mut().expect("checked above");
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
        let slot = self.modules[idx].as_mut().expect("slot stays in the lane");
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
