//! Scripting (ROADMAP E5, `crates/efescript/docs/e3-design.md` section 10).
//!
//! One VM per world: [`Scripting`] owns the one `HostRegistry<ScriptHost>` and every loaded
//! module with its own `efevm::Instance`. The registry is the only table of what scripts can
//! call: instances link against `registry.imports()` and the API index is `registry.index()`.
//!
//! - `host.rs`: [`ScriptHost`], the state host functions see, and the module table.
//! - `api_engine.rs`: the `engine.*` host functions.
//! - `mods.rs`: loading, unloading, mod exports and hot reload.
//! - `system.rs`: the [`Script`] component and the script system.

mod api_engine;
mod host;
mod mods;
mod system;

use std::collections::{HashSet, VecDeque};
use std::fmt;

use efevm::{
    Caller, HostFn, HostOk, HostRegistry, LinkError, LoadError, RegistryError, Results, StructDesc, Args, Trap,
};
use hecs::World;

pub use host::{LogLevel, LogLine, ScriptHost};
pub use mods::ModOptions;
pub use system::Script;
pub(crate) use system::script_system;

use crate::engine::{input::InputState, time::Time, types::TypeRegistry};

/// Default per-module, per-frame budget in VM units.
pub const DEFAULT_BUDGET: u64 = 1_000_000;

/// Handle of a loaded module. Ids are never reused: after unload the id stays valid and
/// every call into it fails cleanly ("module X unloaded").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub u32);

/// A named event with a byte payload, sent by scripts (`engine.events.send`) or Rust
/// (`Ctx::send`). Delivered like every `Events<T>`: readable during the next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptEvent {
    pub name: String,
    pub data: Vec<u8>,
}

/// Why loading, unloading or reloading a module failed.
#[derive(Debug)]
pub enum ScriptError {
    Load(LoadError),
    /// Every problem `Imports::check_module` found.
    Link { module: String, errors: Vec<LinkError> },
    /// The module imports namespaces its [`ModOptions::allowed`] list does not permit.
    Disallowed { module: String, namespaces: Vec<String> },
    /// Not a valid module name (identifier).
    InvalidName(String),
    /// A loaded module, or a namespace of another owner, already has this name.
    NameTaken(String),
    NoSuchModule(ModuleId),
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
                "module `{module}` imports namespace{} {} which it is not allowed to use",
                if namespaces.len() == 1 { "" } else { "s" },
                namespaces.join(", ")
            ),
            ScriptError::InvalidName(n) => write!(f, "`{n}` is not a valid module name (an identifier)"),
            ScriptError::NameTaken(n) => write!(f, "the name `{n}` is already used by a loaded module or another owner's namespace"),
            ScriptError::NoSuchModule(id) => write!(f, "no module with id {}", id.0),
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

/// The scripting runtime of an engine: the host registry (the API index), the host state and
/// the loaded modules.
pub struct Scripting {
    pub(crate) registry: HostRegistry<ScriptHost>,
    pub(crate) host: ScriptHost,
    history: VecDeque<LogLine>,
    pub(crate) warned: HashSet<String>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) next_watch_check: f64,
}

impl Scripting {
    pub(crate) fn new() -> Self {
        let mut registry = HostRegistry::new();
        api_engine::register_engine_api(&mut registry).expect("the engine API registers");
        Scripting {
            registry,
            host: ScriptHost::new(),
            history: VecDeque::new(),
            warned: HashSet::new(),
            #[cfg(not(target_family = "wasm"))]
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

    /// Put a struct or component layout in the index (called by `Engine::register_*`).
    pub(crate) fn register_type(&mut self, desc: StructDesc) -> Result<(), RegistryError> {
        self.registry.register_type(desc)
    }

    pub(crate) fn set_game(&mut self, name: &str) {
        self.registry.set_game(name);
    }

    /// Summaries of every module slot, loaded or not, in id order.
    pub fn modules(&self) -> Vec<ModuleInfo> {
        self.host
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| ModuleInfo { id: ModuleId(i as u32), name: m.name.clone(), loaded: m.loaded, version: m.version, budget: m.budget })
            .collect()
    }

    /// The id of the loaded module called `name`.
    pub fn module_id(&self, name: &str) -> Option<ModuleId> {
        self.host.modules.iter().position(|m| m.loaded && m.name == name).map(|i| ModuleId(i as u32))
    }

    pub fn is_loaded(&self, id: ModuleId) -> bool {
        self.host.modules.get(id.0 as usize).is_some_and(|m| m.loaded)
    }

    /// Change a module's per-frame budget (VM units).
    pub fn set_budget(&mut self, id: ModuleId, units: u64) -> Result<(), ScriptError> {
        let m = self.host.modules.get_mut(id.0 as usize).ok_or(ScriptError::NoSuchModule(id))?;
        m.budget = units;
        Ok(())
    }

    /// The most recent script log lines (oldest first, at most 256), for tests and tools.
    pub fn log_lines(&self) -> impl Iterator<Item = &LogLine> {
        self.history.iter()
    }

    /// Pending log lines: moves them to the history and returns them.
    pub(crate) fn drain_logs(&mut self) -> Vec<LogLine> {
        let out = std::mem::take(&mut self.host.logs);
        for l in &out {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(l.clone());
        }
        out
    }

    pub(crate) fn take_events(&mut self) -> Vec<ScriptEvent> {
        std::mem::take(&mut self.host.events_out)
    }

    pub(crate) fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        self.host.log(level, text);
    }

    pub(crate) fn warn_once(&mut self, key: String, text: String) {
        if self.warned.insert(key) {
            self.log(LogLevel::Warn, text);
        }
    }

    /// Move the world and this frame's data into the host.
    pub(crate) fn enter(&mut self, world: &mut World, types: &TypeRegistry, time: &Time, input: &InputState, events: Vec<ScriptEvent>) {
        self.host.world = std::mem::take(world);
        if self.host.types.generation() != types.generation() {
            self.host.types = types.clone();
        }
        self.host.time.clone_from(time);
        self.host.input.clone_from(input);
        self.host.events_in = events;
    }

    /// Move the world back and append the host's commands to `commands`.
    pub(crate) fn leave(&mut self, world: &mut World, commands: &mut crate::engine::commands::Commands) {
        *world = std::mem::take(&mut self.host.world);
        commands.append(&mut self.host.commands);
        self.host.events_in.clear();
    }

    /// Whether the script system has anything to do.
    pub(crate) fn is_active(&self) -> bool {
        self.host.modules.iter().any(|m| m.loaded)
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
}
