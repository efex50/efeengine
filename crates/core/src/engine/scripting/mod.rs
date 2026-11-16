//! Scripting (ROADMAP E5, `crates/efescript/docs/e3-design.md` section 10).
//!
//! One VM per world: [`Scripting`] owns the one `HostRegistry<ScriptHost>` and every loaded
//! module with its own `efevm::Instance`. The registry is the only table of what scripts can
//! call: instances link against `registry.imports()` and the API index is `registry.index()`.
//!
//! - `host.rs`: [`ScriptHost`], the state host functions see, and the module table.
//! - `api_engine.rs`: the `engine.*` host functions.
//! - `mods.rs`: loading, unloading, declared provides (section 13), dependents and hot reload.
//! - `packages.rs`: `.efmod` packages: policy and grant, load sequence, assets, reload.
//! - `provides.rs`: the trampoline of a provide and the copy ABI across mods.
//! - `system.rs`: the [`Script`] component and the script system.

mod api_engine;
mod cache;
mod host;
mod load;
mod mods;
mod packages;
mod provides;
mod reload;
mod system;

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

use efevm::{
    Caller, HostFn, HostOk, HostRegistry, LinkError, LoadError as VmLoadError, EventDesc, RegistryError, Results, StructDesc, Args, Trap,
};
use hecs::World;

pub use cache::{CacheConfig, CacheDir, CacheStats, ScriptCache, CACHE_FORMAT};
pub use host::{LogLevel, LogLine, ScriptHost};
#[cfg(not(target_family = "wasm"))]
pub use load::DirSource;
pub use load::{AssetHandler, LoadError, LoadInput, Loaded, ModBody, ModItem, ModSource};
pub use mods::ModOptions;
pub use packages::{DefaultPolicy, Grant, LoadReport, ModError, ModPolicy, ModRequest, PackageInfo, Reloaded, UnloadReport};
pub use system::Script;
pub(crate) use host::ModuleSlot;

// Send audit (docs/PARALLEL_SPEC.md 7.1): scripts run on worker threads, so these must stay
// thread-safe. A `Rc`, `RefCell` or `Cell` added to any of them fails to compile here.
const _: fn() = || {
    fn send<T: Send>() {}
    fn sync<T: Sync>() {}
    send::<efevm::Instance<ScriptHost>>();
    send::<efevm::Module>();
    sync::<efevm::Module>();
    sync::<efevm::HostRegistry<ScriptHost>>();
    sync::<efevm::Imports<ScriptHost>>();
    send::<ScriptHost>();
};
pub(crate) use system::script_system;

use crate::engine::{input::InputState, time::Time, types::TypeRegistry};

/// Default per-module, per-frame budget in VM units.
pub const DEFAULT_BUDGET: u64 = 1_000_000;

/// Default most memory pages (64 KiB each) of a module (`efevm::Limits::default()`).
pub const DEFAULT_MEMORY_PAGES: u32 = 1024;

/// Handle of a loaded module. Ids are never reused: after unload the id stays valid and
/// every call into it fails cleanly ("module X unloaded").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub u32);

/// A script event in flight: the registered event it is (`namespace.name`) and its payload
/// bytes, exactly the size of the event's registered payload struct (empty for an event
/// without payload). Sent by scripts (`engine.events.send`, which checks it by construction)
/// or by Rust through [`Ctx::send_script_event`](crate::engine::schedule::Ctx::send_script_event) /
/// [`Engine::send_script_event`](crate::Engine::send_script_event) /
/// [`ScriptHost::send_event`], which refuse an unregistered event or a payload of the wrong
/// size with a [`ScriptEventError`]. Delivered like every `Events<T>`: readable during the
/// next frame. Build one by hand only if you keep the invariant; readers skip entries whose
/// payload length differs from the registered size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptEvent {
    /// Qualified name, `namespace.name`, of a registered event.
    pub event: String,
    pub payload: Vec<u8>,
}

/// Why a Rust-side send of a script event was refused (the event is not queued).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptEventError {
    /// No event `event` (`namespace.name`) is registered.
    Unknown { event: String },
    /// The payload has `got` bytes, the registered payload struct has `expected`.
    Payload { event: String, expected: usize, got: usize },
}

impl fmt::Display for ScriptEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptEventError::Unknown { event } => write!(f, "script event `{event}` is not registered"),
            ScriptEventError::Payload { event, expected, got } => {
                write!(f, "script event `{event}` carries {expected} payload bytes, {got} were given")
            }
        }
    }
}

impl std::error::Error for ScriptEventError {}

/// Why loading, unloading or reloading a module failed.
#[derive(Debug)]
pub enum ScriptError {
    Load(VmLoadError),
    /// Every problem `Imports::check_module` found.
    Link { module: String, errors: Vec<LinkError> },
    /// The module imports namespaces, or refers to events of namespaces, that its
    /// [`ModOptions::allowed`] list does not permit (its own namespace always is).
    Disallowed { module: String, namespaces: Vec<String> },
    /// Not a valid module name (identifier).
    InvalidName(String),
    /// A loaded module, or a namespace of another owner, already has this name.
    NameTaken(String),
    NoSuchModule(ModuleId),
    /// A game module (`load_module`) has a section 13 interface: only mods provide.
    NotAMod(String),
    /// A struct the module's interface copies from elsewhere is not in the registry with the
    /// same layout. `expected` describes the registered layout (or says it is not registered),
    /// `found` the module's copy.
    StructMismatch { name: String, expected: String, found: String },
    /// Unload refused: loaded modules still depend on `module` (they import from its
    /// namespaces, refer to its events in section 14 or copy its structs).
    Needed { module: String, by: Vec<String> },
    /// Hot reload refused, the old version keeps running.
    ReloadRefused { module: String, reasons: Vec<String> },
    Registry(RegistryError),
    Io(String),
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Load(e) => write!(f, "{e}"),
            ScriptError::Link { module, errors } => {
                write!(f, "module `{module}` cannot be linked:")?;
                for e in errors {
                    write!(f, "\n  {e}")?;
                }
                Ok(())
            }
            ScriptError::Disallowed { module, namespaces } => write!(
                f,
                "module `{module}` uses namespace{} {} (imports or events) which it is not allowed to use",
                if namespaces.len() == 1 { "" } else { "s" },
                namespaces.join(", ")
            ),
            ScriptError::InvalidName(n) => write!(f, "`{n}` is not a valid module name (an identifier)"),
            ScriptError::NameTaken(n) => write!(f, "the name `{n}` is already used by a loaded module or another owner's namespace"),
            ScriptError::NoSuchModule(id) => write!(f, "no module with id {}", id.0),
            ScriptError::NotAMod(n) => write!(
                f,
                "module `{n}` has an interface section (it provides functions), which only mods may have: load it with load_mod"
            ),
            ScriptError::StructMismatch { name, expected, found } => write!(
                f,
                "struct `{name}` differs from the registered layout: registered ({expected}), the module has ({found})"
            ),
            ScriptError::Needed { module, by } => write!(f, "module `{module}` is still needed by {}", by.join(", ")),
            ScriptError::ReloadRefused { module, reasons } => {
                write!(f, "module `{module}` cannot be reloaded:")?;
                for r in reasons {
                    write!(f, "\n  {r}")?;
                }
                Ok(())
            }
            ScriptError::Registry(e) => write!(f, "{e}"),
            ScriptError::Io(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for ScriptError {}

impl From<RegistryError> for ScriptError {
    fn from(e: RegistryError) -> Self {
        ScriptError::Registry(e)
    }
}

/// Summary of a module slot (see [`Scripting::modules`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleInfo {
    pub id: ModuleId,
    pub name: String,
    pub loaded: bool,
    pub version: u32,
    pub budget: u64,
}

const HISTORY: usize = 256;

/// What the most recent run of scripts did (see [`Scripting::stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptStats {
    /// `(entity, component)` pairs written by more than one lane (the later lane won).
    pub write_conflicts: u64,
    /// `engine.world.spawn` calls refused because a module's pool was empty.
    pub spawn_overflows: u64,
    /// Number of lanes that ran.
    pub lanes: usize,
    /// Script events dropped over the per-frame cap.
    pub dropped_events: usize,
}

/// The scripting runtime of an engine: the host registry (the API index), the host state and
/// the loaded modules.
pub struct Scripting {
    pub(crate) registry: HostRegistry<ScriptHost>,
    /// Every module slot, by module id. During the script phase the slots of a lane live in the
    /// lane's [`ScriptHost`] and come back at the merge.
    pub(crate) modules: Vec<host::ModuleSlot>,
    /// Log lines of the current frame, not yet emitted.
    logs: Vec<LogLine>,
    /// Script events sent in the current frame (after the merge), not yet sent to the engine.
    events_out: Vec<ScriptEvent>,
    /// Commands of the merged lanes of the current frame, appended to the stage by `leave`.
    pending: crate::engine::commands::Commands,
    /// The snapshot between `enter` and `leave`.
    frame: Option<host::FrameView>,
    /// `TypeRegistry` copy shared with the snapshot, refreshed when the registry changes.
    types: Arc<TypeRegistry>,
    stats: ScriptStats,
    history: VecDeque<LogLine>,
    pub(crate) policy: Box<dyn ModPolicy>,
    pub(crate) warned: HashSet<String>,
    pub(crate) cache: ScriptCache,
    pub(crate) handlers: Vec<Box<dyn AssetHandler>>,
    /// `(module, handler index)` of every handler that committed assets for a module.
    pub(crate) committed: Vec<(ModuleId, usize)>,
    pub(crate) sources: Vec<Box<dyn ModSource>>,
    pub(crate) origins: Vec<packages::Origin>,
    pub(crate) next_watch_check: f64,
}

impl Scripting {
    pub(crate) fn new(cache: CacheConfig) -> Self {
        let mut registry = HostRegistry::new();
        api_engine::register_engine_api(&mut registry).expect("the engine API registers");
        Scripting {
            registry,
            modules: Vec::new(),
            logs: Vec::new(),
            events_out: Vec::new(),
            pending: crate::engine::commands::Commands::new(),
            frame: None,
            types: Arc::new(TypeRegistry::new()),
            stats: ScriptStats::default(),
            history: VecDeque::new(),
            policy: Box::new(DefaultPolicy),
            warned: HashSet::new(),
            cache: ScriptCache::new(cache),
            handlers: Vec::new(),
            committed: Vec::new(),
            sources: Vec::new(),
            origins: Vec::new(),
            next_watch_check: 0.0,
        }
    }

    /// The one table of callable functions and visible types (read-only: functions are added
    /// with [`define_namespace`](Self::define_namespace) / [`register_host_fn`](Self::register_host_fn),
    /// types only with `Engine::register_component` / `Engine::register_struct`).
    pub fn registry(&self) -> &HostRegistry<ScriptHost> {
        &self.registry
    }

    /// Define a namespace for the game's own host functions. `owner` is usually the game's
    /// name; `engine` is reserved.
    pub fn define_namespace(&mut self, name: &str, version: u32, owner: &str, doc: &str) -> Result<(), RegistryError> {
        if owner == api_engine::OWNER {
            return Err(RegistryError::NamespaceOwner {
                namespace: name.to_string(),
                owner: owner.to_string(),
                existing: "the engine (reserved)".to_string(),
            });
        }
        self.registry.define_namespace(name, version, owner, doc)
    }

    /// Register a host function (its namespace must be defined). This is the only way to make
    /// a function callable, and it is what puts it in the index. Scripts of other modules
    /// load against the registry as it is when they are loaded.
    pub fn register_host_fn<F>(&mut self, desc: HostFn, f: F) -> Result<(), RegistryError>
    where
        F: Fn(&mut Caller<'_, ScriptHost>, &Args<'_>, &mut Results<'_>) -> Result<HostOk, Trap> + Send + Sync + 'static,
    {
        self.registry.register(desc, f)
    }

    /// Register a script event of the game (its namespace must be defined with
    /// [`define_namespace`](Self::define_namespace); the owner is the namespace's owner). This is
    /// the only way to make an event visible to scripts and what puts it in the index. Modules
    /// refer to it in section 14 and use it with `engine.events.send/count/read`. Namespaces of
    /// the engine are reserved (the engine registers its own events itself).
    pub fn register_event(&mut self, desc: EventDesc) -> Result<(), RegistryError> {
        if self.registry.namespace(&desc.namespace).is_some_and(|n| n.owner == api_engine::OWNER) {
            return Err(RegistryError::NamespaceOwner {
                namespace: desc.namespace.clone(),
                owner: "(a game)".to_string(),
                existing: "the engine (reserved)".to_string(),
            });
        }
        self.registry.register_event(desc)
    }

    /// The payload size in bytes of the registered event `event` (`namespace.name`; 0 without
    /// payload), `None` if it is not registered.
    pub fn event_payload_size(&self, event: &str) -> Option<usize> {
        let (ns, name) = event.rsplit_once('.')?;
        let desc = self.registry.event(ns, name)?;
        Some(desc.payload.as_ref().and_then(|p| self.registry.struct_type(p)).map_or(0, |t| t.size as usize))
    }

    /// Check a Rust-side send: `event` (`namespace.name`) is registered and `len` is the size of
    /// its payload struct (0 without payload).
    pub fn check_event(&self, event: &str, len: usize) -> Result<(), ScriptEventError> {
        match self.event_payload_size(event) {
            None => Err(ScriptEventError::Unknown { event: event.to_string() }),
            Some(expected) if expected != len => Err(ScriptEventError::Payload { event: event.to_string(), expected, got: len }),
            Some(_) => Ok(()),
        }
    }

    /// How many script events scripts sent beyond the per-frame cap (4096) and were dropped in
    /// the most recent run of scripts (one frame, or one `init` pass outside a frame). The count
    /// is logged once per such run as a warning.
    pub fn dropped_events_last_frame(&self) -> usize {
        self.stats.dropped_events
    }

    /// What the most recent run of scripts (one frame, or one `init` pass outside a frame) did:
    /// write conflicts, spawn pool overflows, lanes and dropped events.
    pub fn stats(&self) -> ScriptStats {
        self.stats
    }

    /// Put a struct or component layout in the index (called by `Engine::register_*`).
    pub(crate) fn register_type(&mut self, desc: StructDesc) -> Result<(), RegistryError> {
        self.registry.register_type(desc)
    }

    pub(crate) fn set_game(&mut self, name: &str) {
        self.registry.set_game(name);
    }

    /// Summaries of every module slot, loaded or not, in id order.
    pub fn modules(&self) -> Vec<ModuleInfo> {
        self.modules
            .iter()
            .enumerate()
            .map(|(i, m)| ModuleInfo { id: ModuleId(i as u32), name: m.name.clone(), loaded: m.loaded, version: m.version, budget: m.budget })
            .collect()
    }

    /// The id of the loaded module called `name`.
    pub fn module_id(&self, name: &str) -> Option<ModuleId> {
        self.modules.iter().position(|m| m.loaded && m.name == name).map(|i| ModuleId(i as u32))
    }

    pub fn is_loaded(&self, id: ModuleId) -> bool {
        self.modules.get(id.0 as usize).is_some_and(|m| m.loaded)
    }

    /// Change a module's per-frame budget (VM units).
    pub fn set_budget(&mut self, id: ModuleId, units: u64) -> Result<(), ScriptError> {
        let m = self.modules.get_mut(id.0 as usize).ok_or(ScriptError::NoSuchModule(id))?;
        m.budget = units;
        Ok(())
    }

    /// The most recent script log lines (oldest first, at most 256), for tests and tools.
    pub fn log_lines(&self) -> impl Iterator<Item = &LogLine> {
        self.history.iter()
    }

    /// Pending log lines: moves them to the history and returns them.
    pub(crate) fn drain_logs(&mut self) -> Vec<LogLine> {
        let out = std::mem::take(&mut self.logs);
        for l in &out {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(l.clone());
        }
        out
    }

    pub(crate) fn take_events(&mut self) -> Vec<ScriptEvent> {
        std::mem::take(&mut self.events_out)
    }

    pub(crate) fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.logs.len() < host::MAX_LOG_LINES {
            self.logs.push(LogLine { level, text: text.into() });
        }
    }

    pub(crate) fn warn_once(&mut self, key: String, text: String) {
        if self.warned.insert(key) {
            self.log(LogLevel::Warn, text);
        }
    }

    /// Move the world and this frame's data into the snapshot the lanes will share.
    pub(crate) fn enter(&mut self, world: &mut World, types: &TypeRegistry, time: &Time, input: &InputState, events: Vec<ScriptEvent>) {
        let event_sizes = self
            .registry
            .events()
            .map(|e| {
                let size = e.payload.as_ref().and_then(|p| self.registry.struct_type(p)).map_or(0, |t| t.size as usize);
                (e.qualified(), size)
            })
            .collect();
        if self.types.generation() != types.generation() {
            self.types = Arc::new(types.clone());
        }
        self.frame = Some(host::FrameView {
            world: std::mem::take(world),
            types: self.types.clone(),
            time: time.clone(),
            input: input.clone(),
            events_in: events,
            event_sizes,
            module_names: Vec::new(),
            jobs: Vec::new(),
        });
    }

    /// Move the world back and append the merged lanes' commands to `commands`.
    pub(crate) fn leave(&mut self, world: &mut World, commands: &mut crate::engine::commands::Commands) {
        if let Some(f) = self.frame.take() {
            *world = f.world;
        }
        commands.append(&mut self.pending);
        if self.stats.dropped_events > 0 {
            let n = self.stats.dropped_events;
            self.log(LogLevel::Warn, format!("{n} script events dropped this frame (cap {})", host::MAX_EVENTS));
        }
    }

    /// Whether the script system has anything to do.
    pub(crate) fn is_active(&self) -> bool {
        self.modules.iter().any(|m| m.loaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::config::EngineConfig;
    use crate::Engine;
    use efevm::{FuncType, FunctionBuilder, ModuleBuilder, ValType};

    /// A module that is running cannot be entered again (here through a game host function
    /// that calls back into it): a clean trap, logged, no panic, no deadlock.
    #[test]
    fn reentering_a_running_module_is_a_clean_trap() {
        let mut e = Engine::with_config(EngineConfig::headless()).unwrap();
        e.define_namespace("test", 1, "testgame", "t").unwrap();
        e.register_host_fn(HostFn::new("test", "again").doc("Call frame of module 0 again."), |c, _a, _r| {
            match c.host().run_export(ModuleId(0), "frame", &[0]) {
                Ok(_) => Ok(HostOk::Continue),
                Err(err) => Err(Trap::Host(err.to_string())),
            }
        })
        .unwrap();
        let mut mb = ModuleBuilder::new();
        let again = mb.import("test", "again", FuncType::new([], []));
        let mut f = FunctionBuilder::new(4);
        f.call(again, 0);
        f.ret(0);
        let id = mb.add_func(FuncType::new([ValType::F32], []), f);
        mb.export_func("frame", id);
        e.load_module("rec", &mb.finish().unwrap().encode()).unwrap();
        e.run_frames(2, 0.016);
        let errs: Vec<_> = e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).collect();
        assert_eq!(errs.len(), 2);
        assert!(errs[0].text.contains("already running") && errs[0].text.contains("rec"), "{}", errs[0].text);
    }

    /// A reload can change the namespace version only while nothing depends on the mod
    /// (the file-based reload keeps the version; package reloads pass the manifest's).
    #[test]
    fn a_version_change_is_refused_while_dependents_are_loaded() {
        use efevm::{ApiType, Interface, Param, Provide};
        let provider = || {
            let mut mb = ModuleBuilder::new();
            let mut f = FunctionBuilder::new(2);
            f.ret(0);
            let id = mb.add_func(FuncType::new([ValType::I64], [ValType::I64]), f);
            mb.interface(Interface {
                provides: vec![Provide {
                    name: "id".into(),
                    func: id,
                    doc: "Identity.".into(),
                    params: vec![Param::new("x", ApiType::I64)],
                    results: vec![Param::new("y", ApiType::I64)],
                }],
                ..Default::default()
            });
            mb.finish().unwrap()
        };
        let mut e = Engine::with_config(EngineConfig::headless()).unwrap();
        let id = e.load_mod(&provider().encode(), ModOptions::new("p")).unwrap();
        // no dependents: the version may change
        e.scripting_mut().reload_module(id.0 as usize, provider(), 2).unwrap();
        assert_eq!(e.api_index().namespace("p").unwrap().version, 2);
        let mut mb = ModuleBuilder::new();
        mb.import("p", "id", FuncType::new([ValType::I64], [ValType::I64]));
        e.load_mod(&mb.finish().unwrap().encode(), ModOptions::new("q").allow("p")).unwrap();
        let err = e.scripting_mut().reload_module(id.0 as usize, provider(), 3).unwrap_err();
        assert!(matches!(&err, ScriptError::ReloadRefused { reasons, .. } if reasons[0].contains("version changes from 2 to 3")), "{err}");
        assert_eq!(e.api_index().namespace("p").unwrap().version, 2);
        e.scripting_mut().reload_module(id.0 as usize, provider(), 2).unwrap();
    }
}
