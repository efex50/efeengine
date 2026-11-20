use std::sync::{Arc, Mutex};

use once_cell::sync::{Lazy, OnceCell};
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use crate::{debug_info, engine::{config::{EngineConfig, Threads}, error::EngineError, events::EventRegistry, parallel::Executor, input::{Filter, InputFilter, GamepadState, InputEvent, InputEventKind, InputState, WindowEvent, WindowId}, messages::Message, resources::{Res, Resources}, schedule::{Access, Ctx, EngineCommand, Commands, ParCtx, Schedule, ScheduleError, Stage, SystemEntry}, plugin::{Plugin, PluginError}, scripting::{CacheConfig, CacheDir, CacheStats, ScriptCache, CACHE_FORMAT, AssetHandler, LoadError, LoadInput, Loaded, ModSource, LoadReport, Reloaded, UnloadReport, ModError, ModOptions, ModPolicy, ModuleId, PackageInfo, ScriptError, ScriptEvent, Scripting}, time::Time, types::{ScriptType, TypeError, TypeRegistry}, window::{GameWindowManager, WindowDesc}}, log::{LogCrateLogger, Logger, NewDefaultLogger}, recv_messages};
use crate::engine::components::{Camera, EditorId, GlobalTransform, MeshHandle, Parent, Sprite, TextureHandle, Transform, Velocity, Visibility};
use crate::engine::render::{self as rendering, gpu::GpuContext, CustomRenderer, RenderFrame, RenderOverlay, RenderWarnings, Renderer2D, Renderer3D, Renderers, Textures};
use crate::{global_info, thread_pool::{init_global_pool, init_global_pool_auto}};
pub mod window;
pub mod windows;
pub use windows::{WindowState, Windows};
pub mod messages;
pub mod log;
pub mod render;
pub mod transform;
pub mod flags;
pub mod commands;
pub mod components;
pub mod types;
pub mod thread_pool;
pub mod schedule;
pub mod order;
pub mod plugin;
pub mod parallel;
pub mod error;
pub mod resources;
pub mod events;
pub mod time;
pub mod config;
pub mod input;
pub mod scripting;
pub mod scene;
pub mod schema;

pub static WASM_PATH:OnceCell<String> =OnceCell::new();
#[cfg(target_family = "wasm")]
#[wasm_bindgen]
#[allow(deprecated)]
#[deprecated(note = "pass `wasmJsPath` in the config given to `__engine_start` instead")]
pub fn set_wasmjs_path(path:String){
    WASM_PATH.set(path);
}

/// Route the `log` crate (used by wgpu, naga and `config.rs`) somewhere visible:
/// the terminal on native (filter with `RUST_LOG`, default `warn`), the browser
/// console on the web. Without this, wgpu validation errors are silently dropped.
/// Calling it more than once is harmless.
fn init_log_crate_logger() {
    #[cfg(not(target_family = "wasm"))]
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).try_init();
    #[cfg(target_family = "wasm")]
    let _ = console_log::init_with_level(::log::Level::Warn);
}

/// Where the engine is in its life. One value, no contradictory flag combinations.
///
/// `Created` -> `Running` (first frame; `Startup` runs there) <-> `Paused` (`Message::Stop` /
/// `Message::Start`) -> `ShuttingDown` (`Message::Kill`, `commands.quit()`, close request) ->
/// `Terminated` (`Shutdown` ran, once; every later `frame()` is a no-op).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle{
    /// Built, no frame ran yet.
    Created,
    /// Frames run their schedule and advance game time.
    Running,
    /// Window events and messages are still handled, but no stage runs, nothing renders and
    /// game time stands still.
    Paused,
    /// A shutdown was requested; `Shutdown` runs when the current frame is aborted.
    ShuttingDown,
    /// `Shutdown` has run. Terminal.
    Terminated,
}



/// How many participants the scheduler uses for `threads`. `Auto` is the pool size (from the
/// core count or `EFE_THREADS`), and `Single` headless.
fn resolve_threads(threads: Threads, headless: bool) -> usize {
    match threads {
        Threads::Single => 1,
        Threads::Fixed(n) => n.max(1),
        Threads::Auto if headless => 1,
        Threads::Auto => crate::thread_pool::global_pool_size().max(1),
    }
}

pub struct EngineState{
    /// The game world (entities and components).
    pub world : hecs::World,
    /// Script-visible types: built-in components are registered at engine creation. Only
    /// `Engine::register_component` / `register_struct` add to it (and to the API index); the
    /// registry's `register*` methods are crate-private, so a public `&mut` here cannot bypass them.
    pub types: TypeRegistry,
    /// Per-window state (size, scale, focus, fullscreen); the main window is also `Ctx::window`.
    pub windows: Windows,
    pub input: InputState,
    pub gamepads: GamepadState,
    pub time: Time,
    pub resources: Resources,
    /// The scripting runtime: host registry (the API index) and loaded modules.
    pub(crate) scripting: Scripting,
}

/// A one-shot hook for `Engine::on_graphics_ready`.
type GraphicsHook = Box<dyn FnOnce(&mut Engine, crate::config::GraphicsMode)>;

pub struct Engine{
    // NOTE: field order matters. Fields drop in declaration order, and the surface inside
    // `gpu` was created from `windows`' raw window handle, so `renderer` and `gpu` must be
    // declared (and so dropped) before `windows`.
    renderer: Renderers,
    /// Drawn after the renderers, before present, in add order.
    overlays: Vec<Box<dyn RenderOverlay>>,
    /// Draw2D overlays (work in both rendering modes), drawn after `overlays`.
    overlays_2d: rendering::Overlays2D,
    /// One-shot hooks run once graphics came up (see `on_graphics_ready`).
    graphics_hooks: Vec<GraphicsHook>,
    /// Run for every raw input event before `InputState::apply`, in add order.
    input_filters: Vec<Box<dyn InputFilter>>,
    gpu: Option<GpuContext>,
    /// SDL mode (native only): the window's `SDL_Renderer` instead of `gpu`. Exactly one of the
    /// two is set once graphics is up. Declared before `windows` (the renderer needs the window).
    #[cfg(not(target_arch = "wasm32"))]
    sdl: Option<rendering::SdlMode>,
    pub windows:GameWindowManager,
    lifecycle: Lifecycle,
    pub logger:Box<dyn Logger>,
    pub state:EngineState,
    /// Input events of the current tick; reused every tick.
    event_buf: Vec<InputEvent>,
    config: EngineConfig,
    events: EventRegistry,
    schedule: Schedule,
    /// Names of the added plugins, in add order.
    plugins: Vec<String>,
    /// Runs the parallel batches of a stage.
    exec: Executor,
    commands: Commands,
    /// The active scene and the global systems (docs/SCENES.md, phase S2).
    scene_state: scene::SceneState,
    /// Frame-rate cap of the native loop; `None` = uncapped.
    max_fps: Option<f32>,
    /// This engine's own message queue.
    msg_tx: flume::Sender<Message>,
    msg_rx: flume::Receiver<Message>,
    /// Synthetic clock of `run_frames` (seconds); also the last wasm clock reading.
    clock: f64,
    #[cfg(not(target_family = "wasm"))]
    start: std::time::Instant,
    /// `EFE_API_EXPORT` (or `set_api_export_path`): `game_loop` writes the API index there
    /// and returns without running frames.
    #[cfg(not(target_family = "wasm"))]
    api_export: Option<std::path::PathBuf>,
}
impl Engine {

    /// Create a new game instance with the given window title.
    ///
    /// ```no_run
    /// use efeengine_core::prelude::*;
    /// use efeengine_core::engine::error::EngineError;
    /// let engine = Engine::new("title")?;
    /// engine.game_loop();
    /// # Ok::<(), EngineError>(())
    /// ```
    pub fn new(title: impl Into<String>) -> Result<Engine, EngineError> {
        Self::with_config(EngineConfig::new(title))
    }

    /// Create a new game instance from a full [`EngineConfig`].
    ///
    /// With `cfg.headless` no thread pool, panic hook, `log` backend or SDL logger is set up.
    pub fn with_config(cfg: EngineConfig) -> Result<Engine, EngineError> {
        let mut cfg = cfg.merged_with_startup();
        cfg.validate()?;
        // `EFE_API_EXPORT=<path>`: write the API index instead of running; never opens a window.
        #[cfg(not(target_family = "wasm"))]
        let api_export = std::env::var_os("EFE_API_EXPORT").filter(|p| !p.is_empty()).map(std::path::PathBuf::from);
        #[cfg(not(target_family = "wasm"))]
        if api_export.is_some() {
            cfg.headless = true;
        }

        let headless = cfg.headless;
        if !headless {
            init_log_crate_logger();

            // init thread pool
            match cfg.threads {
                Threads::Fixed(n) => init_global_pool(n),
                Threads::Auto | Threads::Single => init_global_pool_auto(),
            }

            #[cfg(target_family = "wasm")]
            {
                std::panic::set_hook(Box::new(console_error_panic_hook::hook));
            }
        }
        let w = GameWindowManager::new(&cfg)?;
        let logger: Box<dyn Logger> = if headless {
            Box::new(LogCrateLogger)
        } else {
            debug_info("engine initilazition finished");
            Box::new(NewDefaultLogger())
        };

        let mut scripting = Scripting::new(cfg.script_cache.clone());
        scripting.set_backend(cfg.script_backend, cfg.script_jit.clone());
        scripting.set_game(&cfg.title);

        let state = EngineState{
            world: hecs::World::new(),
            types: TypeRegistry::new(),
            windows: Windows::new(),
            input: InputState::new(),
            gamepads: GamepadState::new(),
            time: Time::new(cfg.fixed_dt),
            resources: Resources::new(),
            scripting,
        };
        let mut state = state;
        state.resources.insert(Textures::new());
        state.resources.insert(rendering::DebugDraw2D::new());
        state.resources.insert(RenderFrame::default());
        state.resources.insert(rendering::RenderWarnings::default());
        let (msg_tx, msg_rx) = flume::unbounded();
        let exec = Executor::new(resolve_threads(cfg.threads, headless));
        let mut schedule = Schedule::new();
        // The script system runs first in `Update`; game systems added later run after it.
        schedule.add(Stage::Update, Box::new(scripting::script_system), "script_system").order(order::SCRIPTS);
        // Built-in systems: motion, hierarchy, render extract.
        schedule.add_par(Stage::FixedUpdate, transform::velocity_access(), Box::new(transform::velocity_system), "velocity_system").order(order::PHYSICS - 1);
        schedule.add_par(Stage::PostUpdate, transform::propagate_access(), Box::new(transform::propagate_transforms), "propagate_transforms").order(order::TRANSFORM);
        schedule.add(Stage::RenderExtract, Box::new(rendering::render_extract), "render_extract").order(order::USER);

        let mut engine = Self {
            renderer: Renderers::new(),
            overlays: Vec::new(),
            overlays_2d: Default::default(),
            graphics_hooks: Vec::new(),
            input_filters: Vec::new(),
            gpu: None,
            #[cfg(not(target_arch = "wasm32"))]
            sdl: None,
            windows: w,
            lifecycle: Lifecycle::Created,
            logger,
            state,
            event_buf: Vec::new(),
            max_fps: cfg.max_fps.and_then(sanitize_fps),
            config: cfg,
            events: EventRegistry::new(),
            schedule,
            plugins: Vec::new(),
            exec,
            commands: Commands::default(),
            scene_state: scene::SceneState::default(),
            msg_tx,
            msg_rx,
            clock: 0.0,
            #[cfg(not(target_family = "wasm"))]
            start: std::time::Instant::now(),
            #[cfg(not(target_family = "wasm"))]
            api_export,
        };
        engine.register::<Transform>().expect("built-in components register");
        engine.register::<Velocity>().expect("built-in components register");
        engine.register::<MeshHandle>().expect("built-in components register");
        engine.register::<TextureHandle>().expect("built-in components register");
        engine.register::<Camera>().expect("built-in components register");
        engine.register::<Sprite>().expect("built-in components register");
        engine.register::<Visibility>().expect("built-in components register");
        engine.register::<Parent>().expect("built-in components register");
        engine.register::<GlobalTransform>().expect("built-in components register");
        engine.register::<EditorId>().expect("built-in components register");
        Ok(engine)
    }

    /// The (merged and validated) configuration this engine was created with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Add a system to `stage`. Systems of a stage run sorted by `(order, registration index)`;
    /// the default order is [`order::USER`]. Chain `.order(n)` / `.name(..)` on the result.
    pub fn add_system(&mut self, stage: Stage, f: impl FnMut(&mut Ctx<'_>) + 'static) -> SystemEntry<'_> {
        let name = std::any::type_name_of_val(&f);
        self.schedule.add(stage, Box::new(f), name)
    }

    /// Add a parallel system to `stage`. `access` declares the components and resources it
    /// reads and writes; systems whose accesses do not conflict run at the same time (see
    /// `docs/THREADING.md`). The result is the same for every thread count.
    pub fn add_par_system(&mut self, stage: Stage, access: Access, f: impl FnMut(&ParCtx<'_>) + Send + 'static) -> SystemEntry<'_> {
        let name = std::any::type_name_of_val(&f);
        self.schedule.add_par(stage, access, Box::new(f), name)
    }

    /// Add a plugin: checks that its name is new and its dependencies are added, then runs
    /// [`Plugin::build`]. Systems added during `build` are tagged with the plugin's name.
    /// Plugins may add plugins from `build` (their dependencies must come first).
    pub fn add_plugin(&mut self, p: impl Plugin) -> Result<&mut Self, PluginError> {
        let name = p.name().to_string();
        if !self.api_open() {
            return Err(PluginError::Closed(name));
        }
        if self.has_plugin(&name) {
            return Err(PluginError::AlreadyAdded(name));
        }
        if let Some(missing) = p.dependencies().iter().find(|d| !self.has_plugin(d)) {
            return Err(PluginError::MissingDependency { plugin: name, missing: (*missing).to_string() });
        }
        self.plugins.push(name.clone());
        let prev = self.schedule.swap_owner(Some(name.as_str().into()));
        p.build(self);
        self.schedule.swap_owner(prev);
        Ok(self)
    }

    /// Whether a plugin named `name` was added.
    pub fn has_plugin(&self, name: &str) -> bool {
        self.plugins.iter().any(|p| p == name)
    }

    /// Names of the added plugins, in add order.
    pub fn plugins(&self) -> &[String] {
        &self.plugins
    }

    /// Enable or disable every system named `name` (the name set with `.name(..)`, else the
    /// type name of its closure; if several systems share a name, all are changed). A disabled
    /// system is skipped, does not split batches and does not advance its change tick, so when
    /// re-enabled it sees every change since it last ran. Errors if no system has that name.
    pub fn set_system_enabled(&mut self, name: &str, enabled: bool) -> Result<(), ScheduleError> {
        if self.schedule.set_enabled(name, enabled) == 0 {
            return Err(ScheduleError::UnknownSystem(name.to_string()));
        }
        Ok(())
    }

    /// Whether the system `name` is enabled (with several of that name: whether any is);
    /// `None` if there is no such system. A system whose `run_if` is false still counts as enabled.
    pub fn is_system_enabled(&self, name: &str) -> Option<bool> {
        self.schedule.is_enabled(name)
    }

    /// Remove every system named `name` from all stages and return how many were removed
    /// (an error if none). Systems can be removed at any time from outside the schedule; from
    /// inside a system use `ctx.schedule().remove(..)`.
    pub fn remove_system(&mut self, name: &str) -> Result<usize, ScheduleError> {
        match self.schedule.remove(name) {
            0 => Err(ScheduleError::UnknownSystem(name.to_string())),
            n => Ok(n),
        }
    }

    /// `(name, enabled)` of each system of `stage` in execution order.
    pub fn system_states(&self, stage: Stage) -> Vec<(&str, bool)> {
        self.schedule.system_states(stage)
    }

    /// System names of `stage` in execution order (disabled systems included).
    pub fn system_names(&self, stage: Stage) -> Vec<&str> {
        self.schedule.system_names(stage)
    }

    /// Owning plugin of each system of `stage` (`None` = added outside any plugin), in execution order.
    pub fn system_owners(&self, stage: Stage) -> Vec<Option<&str>> {
        self.schedule.system_owners(stage)
    }

    /// Number of batches of `stage` (systems in one batch run at the same time).
    #[doc(hidden)]
    pub fn batch_count(&mut self, stage: Stage) -> usize {
        self.schedule.batch_count(stage)
    }

    /// The current parallelism: the number of threads (the caller plus pool workers) that run
    /// a parallel batch.
    pub fn threads(&self) -> usize {
        self.exec.threads()
    }

    /// Change the parallelism at runtime. The global pool grows if needed (never shrinks).
    pub fn set_threads(&mut self, threads: Threads) {
        self.config.threads = threads;
        self.exec = Executor::new(resolve_threads(threads, self.config.headless));
    }

    /// Register component type `T` (a `#[script_component]` or `#[derive(Component)]`
    /// type): script components are added to the type registry and the API index, tracked ones
    /// get change tracking. Inside [`Plugin::build`] the plugin is recorded as the owner,
    /// otherwise the owner is `engine`. Returns the component id (`None` for Rust-only
    /// components). Fails with `TypeError::Closed` once the game has started.
    pub fn register<T: types::ComponentInfo>(&mut self) -> Result<Option<u32>, TypeError> {
        T::register(self)
    }

    /// The owner recorded for types registered now: the plugin being built, else `engine`.
    #[doc(hidden)]
    pub fn current_owner(&self) -> String {
        self.schedule.current_owner().unwrap_or(types::ENGINE_OWNER).to_string()
    }

    #[doc(hidden)]
    pub fn check_registration_open(&self, what: &str) -> Result<(), TypeError> {
        self.check_api_open(|| format!("component `{what}`"))
    }

    /// Register component type `T` (owner `engine`); returns its id. This is the one way to
    /// make a component visible to scripts: it registers the type in the [`TypeRegistry`]
    /// (byte-level access for `engine.world.*`) and its layout in the API index.
    pub fn register_component<T: ScriptType + hecs::Component>(&mut self) -> Result<u32, TypeError> {
        self.register_component_owned::<T>(types::ENGINE_OWNER)
    }

    /// Like [`register_component`](Self::register_component) with an owner (the game's name or
    /// a mod name), recorded in the index.
    pub fn register_component_owned<T: ScriptType + hecs::Component>(&mut self, owner: &str) -> Result<u32, TypeError> {
        self.check_api_open(|| format!("component `{}`", std::any::type_name::<T>()))?;
        let id = self.state.types.register_owned::<T>(owner)?;
        self.index_or_rollback(id)
    }

    /// Register a layout-only struct (no component; e.g. from a mod); returns its id. It is
    /// also put in the API index, so functions can take `&Name` parameters.
    pub fn register_struct(&mut self, desc: efevm::StructDesc) -> Result<u32, TypeError> {
        self.check_api_open(|| format!("struct `{}`", desc.name))?;
        let id = self.state.types.register_struct(desc)?;
        self.index_or_rollback(id)
    }

    fn check_api_open(&self, what: impl FnOnce() -> String) -> Result<(), TypeError> {
        if self.state.scripting.api_open() { Ok(()) } else { Err(TypeError::Closed { what: what() }) }
    }

    /// Whether API registration is still open: `register_host_fn`, `register_component(_owned)`,
    /// `register_struct`, `register_event` and `add_plugin` work from `Engine::new` until the game
    /// loop starts running (or the first frame runs when the engine is driven by `frame` /
    /// `run_frames` / `tick`). After that they return an error ("API registration is closed once
    /// the game loop starts; register in setup or Plugin::build; mods declare theirs in
    /// mod.toml"). Mods are exempt: what a mod declares registers at load, whenever that is.
    /// Not locked: systems, resources, Rust events, the renderer. Gate late features with
    /// `SystemEntry::enabled` / `run_if` instead (docs/SYSTEMS.md).
    pub fn api_open(&self) -> bool {
        self.state.scripting.api_open()
    }

    /// Open API registration again. Dev only (an editor or hot reload of game code); a shipped
    /// game never needs this. It does not re-run anything, and the next frame does not close it
    /// again once the first frame has run (until then the first frame still closes it).
    #[doc(hidden)]
    pub fn reopen_api(&mut self) -> &mut Self {
        self.state.scripting.set_api_open(true);
        self
    }

    /// Close API registration (what the game loop does when it starts).
    pub(crate) fn close_api(&mut self) {
        self.state.scripting.set_api_open(false);
    }

    /// Index the just-registered type `id`; if the index refuses it, remove it from the
    /// [`TypeRegistry`] again so both tables stay identical.
    fn index_or_rollback(&mut self, id: u32) -> Result<u32, TypeError> {
        if let Err(e) = self.index_type(id) {
            self.state.types.unregister(id);
            return Err(e);
        }
        Ok(id)
    }

    /// Put the (just registered) type `id` in the scripting registry: the same `StructDesc`.
    fn index_type(&mut self, id: u32) -> Result<(), TypeError> {
        let desc = self.state.types.get(id).map(|t| t.desc().clone()).ok_or(TypeError::Invalid("type vanished".into()))?;
        self.state.scripting.register_type(desc).map_err(|e| TypeError::Invalid(e.to_string()))
    }

    pub fn types(&self) -> &TypeRegistry {
        &self.state.types
    }

    // ---- scripting (E5) ----

    /// The scripting runtime: the host registry (the API index) and the loaded modules.
    pub fn scripting(&self) -> &Scripting {
        &self.state.scripting
    }

    pub fn scripting_mut(&mut self) -> &mut Scripting {
        &mut self.state.scripting
    }

    /// Define a namespace for the game's own host functions (`owner`: usually the game's name;
    /// `engine` is reserved).
    pub fn define_namespace(&mut self, name: &str, version: u32, owner: &str, doc: &str) -> Result<(), efevm::RegistryError> {
        self.state.scripting.define_namespace(name, version, owner, doc)
    }

    /// Register a host function in the one registry: it becomes callable by modules loaded
    /// afterwards and appears in the API index; its owner is its namespace's owner. The
    /// closure gets the [`ScriptHost`](scripting::ScriptHost) (world, commands, time, input, ...).
    pub fn register_host_fn<F>(&mut self, desc: efevm::HostFn, f: F) -> Result<(), efevm::RegistryError>
    where
        F: Fn(&mut efevm::Caller<'_, scripting::ScriptHost>, &efevm::Args<'_>, &mut efevm::Results<'_>) -> Result<efevm::HostOk, efevm::Trap> + Send + Sync + 'static,
    {
        self.state.scripting.register_host_fn(desc, f)
    }

    /// Register a script event of the game: its namespace must be defined with
    /// [`define_namespace`](Self::define_namespace) (owner = that namespace's owner), its name an
    /// identifier, its doc not blank, its payload (if any) a registered struct (see
    /// [`register_struct`](Self::register_struct)). It is what makes the event usable by scripts
    /// (modules list it in their events section and call `engine.events.*`) and what puts it in
    /// the API index. The engine itself registers no events today (`engine.*` namespaces are
    /// reserved for it).
    pub fn register_event(&mut self, desc: efevm::EventDesc) -> Result<(), efevm::RegistryError> {
        self.state.scripting.register_event(desc)
    }

    /// Send the registered script event `event` (`namespace.name`) with `payload` bytes from
    /// outside a system; readable by scripts and systems during the next frame. Refused if the
    /// event is not registered or the payload is not exactly its struct's size.
    pub fn send_script_event(&mut self, event: &str, payload: &[u8]) -> Result<(), scripting::ScriptEventError> {
        self.state.scripting.check_event(event, payload.len())?;
        self.events.send(ScriptEvent { event: event.to_string(), payload: payload.to_vec() });
        Ok(())
    }

    /// Load a game script module (all namespaces may be imported); runs its `init` at once.
    pub fn load_module(&mut self, name: &str, bytes: &[u8]) -> Result<ModuleId, ScriptError> {
        let id = self.state.scripting.load_module(name, bytes)?;
        self.run_script_inits();
        Ok(id)
    }

    /// Load a mod (only `opts.allowed` namespaces may be imported). What its module declares
    /// as provides (section 13) is registered, and so in the index, when this returns (before
    /// its `init` runs, which happens at once). See [`Scripting::load_mod`].
    pub fn load_mod(&mut self, bytes: &[u8], opts: ModOptions) -> Result<ModuleId, ScriptError> {
        let id = self.state.scripting.load_mod(bytes, opts)?;
        self.run_script_inits();
        Ok(id)
    }

    /// Unload a module: its instance is dropped and its namespace, functions and types leave
    /// the index. Refused with [`ScriptError::Needed`] while another loaded module imports
    /// from its namespaces or copies its structs; see
    /// [`unload_module_and_dependents`](Self::unload_module_and_dependents).
    pub fn unload_module(&mut self, id: ModuleId) -> Result<(), ScriptError> {
        let name = self.state.scripting.unload(id)?;
        let EngineState { scripting, resources, types, world, .. } = &mut self.state;
        scripting.release_module(id, resources);
        scripting.release_module_state(id, &name, world, types, false);
        Ok(())
    }

    /// Unload `id` and every loaded module that depends on it, transitively, dependents first
    /// (the latest loaded first). Returns the unloaded modules in that order, `id` last.
    pub fn unload_module_and_dependents(&mut self, id: ModuleId) -> Result<Vec<ModuleId>, ScriptError> {
        let removed = self.state.scripting.unload_cascade(id)?;
        let EngineState { scripting, resources, types, world, .. } = &mut self.state;
        for (id, name) in &removed {
            scripting.release_module(*id, resources);
            scripting.release_module_state(*id, name, world, types, false);
        }
        Ok(removed.into_iter().map(|(id, _)| id).collect())
    }

    // ---- mod packages (E8) ----

    /// Set the game's policy for mod packages: what each package is granted. Until set, the
    /// [`DefaultPolicy`](scripting::DefaultPolicy) grants no namespace.
    pub fn set_mod_policy(&mut self, policy: impl ModPolicy + 'static) {
        self.state.scripting.set_mod_policy(policy);
    }

    /// Install an asset handler: it takes over the assets of packages and mod folders that it
    /// `matches` (see [`AssetHandler`]). The first matching handler owns an asset.
    pub fn add_asset_handler(&mut self, h: impl AssetHandler + 'static) {
        self.state.scripting.add_asset_handler(h);
    }

    /// Register a source of mods for [`load_mods`](Self::load_mods) and hot reload.
    pub fn add_mod_source(&mut self, s: impl ModSource + 'static) {
        self.state.scripting.add_mod_source(s);
    }

    /// The engine's compile and decode cache.
    pub fn cache(&mut self) -> &mut ScriptCache {
        &mut self.state.scripting.cache
    }

    /// Load one input (see [`LoadInput`]); runs `init` at once. `Efa` and `Efvm` are game
    /// code (no policy); `Package` and `Folder` go through the mod pipeline (game check, id
    /// clash, dependencies, policy, asset check, link, asset commit). See `scripting/packages.rs`.
    pub fn load_input(&mut self, input: LoadInput<'_>) -> Result<Loaded, LoadError> {
        let loaded = match input {
            #[cfg(feature = "efa")]
            LoadInput::Efa { name, files, root } => {
                let scripting = &mut self.state.scripting;
                let index = scripting.registry().index();
                let game = scripting.registry().game().to_string();
                let (module, _) = scripting
                    .cache
                    .compile(&game, root, files, &index, &efa::asm::AsmOptions::default())
                    .map_err(LoadError::Compile)?;
                Loaded::Module(scripting.load_module_decoded(name, module)?)
            }
            LoadInput::Efvm { name, bytes } => Loaded::Module(self.state.scripting.load_module(name, bytes)?),
            LoadInput::Package { label, bytes } => {
                let b = self.state.scripting.read_built(label, bytes)?;
                let EngineState { scripting, resources, types, world, .. } = &mut self.state;
                Loaded::Package(scripting.load_package_item(b, resources, world, types)?)
            }
            #[cfg(feature = "efa")]
            LoadInput::Folder { label, files, assets } => {
                let b = self.state.scripting.build_folder(label, files, assets, None)?;
                let EngineState { scripting, resources, types, world, .. } = &mut self.state;
                Loaded::Package(scripting.load_package_item(b, resources, world, types)?)
            }
        };
        self.run_script_inits();
        Ok(loaded)
    }

    /// Load a file or folder: a mod folder (has `mod.toml`), `.efa`, `.efvm` (game modules named
    /// after the file stem) or `.efmod`. A mod whose id is already loaded fails with
    /// [`ModError::AlreadyLoaded`]; see [`reload_mod`](Self::reload_mod).
    #[cfg(not(target_family = "wasm"))]
    pub fn load(&mut self, path: impl AsRef<std::path::Path>) -> Result<Loaded, LoadError> {
        self.with_path_input(path.as_ref(), Self::load_input)
    }

    /// Reload the mod at `path` (an `.efmod` file or a mod folder): see
    /// [`reload_mod_input`](Self::reload_mod_input).
    #[cfg(not(target_family = "wasm"))]
    pub fn reload_mod(&mut self, path: impl AsRef<std::path::Path>) -> Result<Reloaded, LoadError> {
        self.with_path_input(path.as_ref(), Self::reload_mod_input)
    }

    /// Read `path` into the [`LoadInput`] its kind calls for and hand it to `f`.
    #[cfg(not(target_family = "wasm"))]
    fn with_path_input<T>(
        &mut self,
        path: &std::path::Path,
        f: impl for<'a> FnOnce(&mut Engine, LoadInput<'a>) -> Result<T, LoadError>,
    ) -> Result<T, LoadError> {
        let io = |e: std::io::Error| LoadError::Io(format!("{}: {e}", path.display()));
        let display = path.display().to_string();
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if path.is_dir() {
            if !path.join("mod.toml").is_file() {
                return Err(LoadError::UnknownKind(display));
            }
            #[cfg(feature = "efa")]
            {
                let assets = efa::mod_folder::list_assets(path).map_err(LoadError::Io)?;
                let files = efa::asm::FsSource::with_prefix(path, display);
                return f(self, LoadInput::Folder { label: &file_name, files: &files, assets: &assets });
            }
            #[cfg(not(feature = "efa"))]
            return Err(LoadError::CompilerDisabled);
        }
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "efa" => {
                #[cfg(feature = "efa")]
                {
                    let (files, root) = efa::asm::FsSource::for_file(&display);
                    if !path.is_file() {
                        return Err(LoadError::Io(format!("{display}: not a file")));
                    }
                    f(self, LoadInput::Efa { name: &stem, files: &files, root: &root })
                }
                #[cfg(not(feature = "efa"))]
                Err(LoadError::CompilerDisabled)
            }
            "efvm" => {
                let bytes = std::fs::read(path).map_err(io)?;
                f(self, LoadInput::Efvm { name: &stem, bytes: &bytes })
            }
            "efmod" => {
                let bytes = std::fs::read(path).map_err(io)?;
                f(self, LoadInput::Package { label: &file_name, bytes: &bytes })
            }
            _ => Err(LoadError::UnknownKind(display)),
        }
    }

    /// Replace a loaded mod by a new version of it (`Package` or `Folder`; a game module is
    /// [`LoadError::NotAMod`]). The loaded mod is the one with the same manifest id:
    ///
    /// - none loaded: it is loaded like [`load_input`](Self::load_input) ([`Reloaded::Loaded`]);
    /// - same content hash: nothing happens ([`Reloaded::Unchanged`]);
    /// - otherwise the mod and its loaded dependents are unloaded (asset handlers get
    ///   `unload`), the new version loads, and the dependents load again against it (folder
    ///   dependents are recompiled). If any step fails, everything is unloaded again and the
    ///   previous versions are loaded back; the error is [`ModError::ReloadFailed`] with
    ///   `restored` telling whether that worked. A game module that depends on the mod makes
    ///   the reload fail up front, nothing changed.
    ///
    /// Module ids change (an id is never reused) and `init` runs again for every module that
    /// was loaded again.
    pub fn reload_mod_input(&mut self, input: LoadInput<'_>) -> Result<Reloaded, LoadError> {
        let EngineState { scripting, resources, types, world, .. } = &mut self.state;
        let reloaded = match input {
            #[cfg(feature = "efa")]
            LoadInput::Efa { name, .. } => return Err(LoadError::NotAMod(name.to_string())),
            LoadInput::Efvm { name, .. } => return Err(LoadError::NotAMod(name.to_string())),
            LoadInput::Package { label, bytes } => {
                let b = scripting.read_built(label, bytes)?;
                let (manifest, hash) = (b.pkg.manifest.clone(), b.hash.clone());
                scripting.reload_one(&manifest, &hash, label, Box::new(move |_| Ok((b, None))), resources, types, world)
            }
            #[cfg(feature = "efa")]
            LoadInput::Folder { label, files, assets } => {
                let (manifest, hash) = Scripting::peek_folder(files)?;
                let (l, h) = (label.to_string(), hash.clone());
                let build = Box::new(move |s: &mut Scripting| s.build_folder(&l, files, assets, Some(h)).map(|b| (b, None)));
                scripting.reload_one(&manifest, &hash, label, build, resources, types, world)
            }
        };
        // Also after a failure: a restored version runs its `init` again.
        self.run_script_inits();
        Ok(reloaded?)
    }

    /// Load the mods of every registered source ([`add_mod_source`](Self::add_mod_source)):
    /// every source is listed, the manifests are read, the dependencies are resolved over all
    /// items and the loaded packages, items load in dependency order (ties by id) and the
    /// dependents of an item that fails are skipped (`ResolveProblem::DependencyFailed`).
    /// `init`s run after all loads. With no source the report is empty.
    ///
    /// A mod whose id is already loaded with the same contents is listed in `skipped`; with
    /// other contents it fails with [`ModError::AlreadyLoaded`] (use
    /// [`reload_mods`](Self::reload_mods)).
    pub fn load_mods(&mut self) -> LoadReport {
        let EngineState { scripting, resources, types, world, .. } = &mut self.state;
        let report = scripting.load_mods(resources, types, world, false);
        self.run_script_inits();
        report
    }

    /// Bring the loaded mods in line with the registered sources, with one dependency
    /// resolve: new mods load, mods whose content hash changed are replaced (the rules of
    /// [`reload_mod_input`](Self::reload_mod_input), all changed mods swapped together so
    /// that mods can change in step), unchanged mods are `skipped`, and mods that came from a
    /// source that no longer lists them are unloaded (`unloaded`, dependents included).
    pub fn reload_mods(&mut self) -> LoadReport {
        let EngineState { scripting, resources, types, world, .. } = &mut self.state;
        let report = scripting.load_mods(resources, types, world, true);
        self.run_script_inits();
        report
    }

    /// Unload every mod (packages and folders, however they were loaded), dependents before
    /// their dependencies, calling the asset handlers' `unload`. Game modules stay loaded; a
    /// mod that a game module still depends on stays too and is listed in `failed`.
    pub fn unload_mods(&mut self) -> UnloadReport {
        let EngineState { scripting, resources, types, world, .. } = &mut self.state;
        scripting.unload_mods(resources, types, world)
    }

    /// Hot reload the package `id` from `path` (see `Scripting::watch_package`).
    #[cfg(not(target_family = "wasm"))]
    pub fn watch_package(&mut self, id: ModuleId, path: impl AsRef<std::path::Path>) -> Result<(), ScriptError> {
        self.state.scripting.watch_package(id, path)
    }

    /// The manifest, grant and assets of the package behind `id`.
    pub fn package(&self, id: ModuleId) -> Option<&PackageInfo> {
        self.state.scripting.package(id)
    }

    /// An asset of the package behind `id` (path without `assets/`).
    pub fn package_asset(&self, id: ModuleId, path: &str) -> Option<&[u8]> {
        self.state.scripting.package_asset(id, path)
    }

    /// Hot reload the game script `id` from its `.efa` source `path` (see [`Scripting::watch_source`]).
    #[cfg(all(not(target_family = "wasm"), feature = "efa"))]
    pub fn watch_source(&mut self, id: ModuleId, path: impl AsRef<std::path::Path>) -> Result<(), ScriptError> {
        self.state.scripting.watch_source(id, path)
    }

    /// Hot reload `id` from `path` (see [`Scripting::watch`]).
    #[cfg(not(target_family = "wasm"))]
    pub fn watch_module(&mut self, id: ModuleId, path: impl AsRef<std::path::Path>) -> Result<(), ScriptError> {
        self.state.scripting.watch(id, path)
    }

    /// What the most recent run of scripts did (write conflicts, spawn overflows, lanes, dropped events).
    pub fn script_stats(&self) -> scripting::ScriptStats {
        self.state.scripting.stats()
    }

    /// Run the pending `init`s outside a frame (with the engine's world) and apply what they queued.
    fn run_script_inits(&mut self) {
        let events: Vec<ScriptEvent> = self.events.get_or_empty::<ScriptEvent>().iter().cloned().collect();
        {
            let Engine { state, commands, exec, .. } = self;
            let EngineState { world, types, input, time, scripting, .. } = state;
            scripting.enter(world, types, time, input, events);
            scripting.run_inits(exec);
            scripting.leave(world, commands);
        }
        for ev in self.state.scripting.take_events() {
            self.events.send(ev);
        }
        self.apply_commands();
        self.flush_script_logs();
    }

    /// One line per outcome of a `load_mods` / `reload_mods` through the engine logger.
    fn log_load_report(&mut self, r: &LoadReport) {
        self.logger.info(&format!(
            "mods: {} loaded, {} reloaded, {} unchanged, {} unloaded, {} failed",
            r.loaded.len(), r.reloaded.len(), r.skipped.len(), r.unloaded.len(), r.failed.len()
        ));
        for (label, _) in &r.reloaded {
            self.logger.info(&format!("mods: reloaded {label}"));
        }
        for (label, _) in &r.loaded {
            self.logger.info(&format!("mods: loaded {label}"));
        }
        for id in &r.unloaded {
            self.logger.info(&format!("mods: unloaded {id}"));
        }
        for (label, e) in &r.failed {
            self.logger.error(&format!("mods: {label}: {e}"));
        }
    }

    /// Send the log lines scripts produced through the engine logger.
    fn flush_script_logs(&mut self) {
        for l in self.state.scripting.drain_logs() {
            match l.level {
                scripting::LogLevel::Info => self.logger.info(&l.text),
                scripting::LogLevel::Warn => self.logger.warn(&l.text),
                scripting::LogLevel::Error => self.logger.error(&l.text),
            }
        }
    }

    // ---- API index (E4/E5) ----

    /// Everything scripts can call or see, as plain data: a snapshot of the host registry
    /// (engine functions, game functions, mod exports, component and struct layouts). The
    /// game name is the config title unless set with [`set_game_name`](Self::set_game_name).
    pub fn api_index(&self) -> efevm::ApiIndex {
        self.state.scripting.registry().index()
    }

    /// [`api_index`](Self::api_index) as the `.efapi` JSON text.
    pub fn export_api_index(&self) -> String {
        self.api_index().to_json()
    }

    /// Set the `game` name recorded in the index (default: the config title).
    pub fn set_game_name(&mut self, name: &str) {
        self.state.scripting.set_game(name);
    }

    /// Write [`export_api_index`](Self::export_api_index) to `path`.
    #[cfg(not(target_family = "wasm"))]
    pub fn write_api_index(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        std::fs::write(path, self.export_api_index())
    }

    /// Write the component schema ([`export_schema`](Self::export_schema)) to `path`.
    #[cfg(not(target_family = "wasm"))]
    pub fn write_schema(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        std::fs::write(path, self.export_schema())
    }

    /// Make `game_loop` write the API index to `path` and return without running frames (what
    /// `EFE_API_EXPORT=<path>` does at creation). `None` turns it off.
    #[cfg(not(target_family = "wasm"))]
    pub fn set_api_export_path(&mut self, path: Option<std::path::PathBuf>) {
        self.api_export = path;
    }

    pub fn world(&self) -> &hecs::World {
        &self.state.world
    }

    /// The world's current change tick (see `docs/THREADING.md`, "Change ticks").
    pub fn change_tick(&self) -> hecs::Tick {
        self.state.world.change_tick()
    }

    /// Record add/change ticks for component `T` (forwards to the world). Change filters on an
    /// untracked component match everything. `Transform` and `GlobalTransform` are tracked.
    pub fn track_changes<T: hecs::Component>(&mut self) -> &mut Self {
        self.state.world.track_changes::<T>();
        self
    }

    pub fn world_mut(&mut self) -> &mut hecs::World {
        &mut self.state.world
    }

    /// Register event type `T` (it is also registered on first send).
    pub fn add_event<T: Send + Sync + 'static>(&mut self) -> &mut Self {
        self.events.register::<T>();
        self
    }

    pub fn insert_resource<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        let tick = self.state.world.increment_change_tick();
        self.state.resources.set_tick(tick);
        self.state.resources.insert(value);
        self
    }

    pub fn resources(&self) -> &Resources {
        &self.state.resources
    }

    /// Exclusive access to the resources. Advances the world's change tick (like applying
    /// commands does), so writes through it are newer than any tick saved before the call.
    pub fn resources_mut(&mut self) -> &mut Resources {
        let tick = self.state.world.increment_change_tick();
        self.state.resources.set_tick(tick);
        &mut self.state.resources
    }

    /// Whether resource `T` was inserted or changed after `since` (e.g. a tick saved from
    /// [`change_tick`](Self::change_tick) earlier); `false` if missing.
    pub fn res_changed<T: Send + Sync + 'static>(&self, since: hecs::Tick) -> bool {
        self.state.resources.is_changed::<T>(since, self.state.world.change_tick())
    }

    /// The texture store. Changes made here reach the renderer through `RenderFrame::textures`.
    pub fn textures(&self) -> Res<'_, Textures> {
        self.state.resources.get::<Textures>().expect("Textures resource")
    }

    pub fn textures_mut(&mut self) -> &mut Textures {
        self.state.resources.get_mut::<Textures>().expect("Textures resource")
    }

    /// The last built [`RenderFrame`] (rebuilt every frame in `Stage::RenderExtract`).
    pub fn render_frame(&self) -> Res<'_, RenderFrame> {
        self.state.resources.get::<RenderFrame>().expect("RenderFrame resource")
    }

    /// Install the 2D renderer (replaces the default [`SpriteRenderer`](rendering::SpriteRenderer)
    /// and any [`CustomRenderer`]). If the GPU is already up, `init` runs at once; otherwise
    /// when it comes up.
    pub fn set_renderer_2d<R2: Renderer2D>(&mut self, renderer: R2) {
        let gpu = self.gpu.as_ref().map(|g| g.gpu());
        let builtin = std::any::TypeId::of::<R2>() == std::any::TypeId::of::<rendering::SpriteRenderer>();
        self.renderer.set_2d(Box::new(renderer), builtin, gpu.as_ref());
    }

    /// Install the 3D renderer (replaces any [`CustomRenderer`]). It runs before the 2D one on
    /// frames with an active perspective camera. `init` as in [`Engine::set_renderer_2d`].
    pub fn set_renderer_3d(&mut self, renderer: impl Renderer3D) {
        let gpu = self.gpu.as_ref().map(|g| g.gpu());
        self.renderer.set_3d(Box::new(renderer), gpu.as_ref());
    }

    /// Install a [`CustomRenderer`]: it gets the whole frame and surface and replaces both the
    /// 2D and the 3D renderer until one of those is set again. `init` as in
    /// [`Engine::set_renderer_2d`].
    pub fn set_renderer(&mut self, renderer: impl CustomRenderer) {
        let gpu = self.gpu.as_ref().map(|g| g.gpu());
        self.renderer.set_custom(Box::new(renderer), gpu.as_ref());
    }

    /// Add a [`RenderOverlay`]: drawn on top of the renderer's output every rendered frame, in
    /// add order. If the GPU is already up, `init` runs at once; otherwise when it comes up.
    /// Never called without a GPU (headless).
    pub fn add_render_overlay(&mut self, overlay: impl RenderOverlay) -> &mut Self {
        let mut overlay: Box<dyn RenderOverlay> = Box::new(overlay);
        if let Some(gpu) = &self.gpu {
            overlay.init(&gpu.gpu());
            for id in gpu.windows.keys() {
                if let Some(g) = gpu.gpu_for(*id) {
                    overlay.init(&g);
                }
            }
        }
        self.overlays.push(overlay);
        self
    }

    /// Runs `f` once, when graphics came up and the mode is known (`GraphicsMode::Wgpu` or `Sdl`;
    /// what `auto` resolved to), before the render overlays get their GPU `init` and before the
    /// first frame, so `f` may add overlays suited to the mode. Never runs without graphics
    /// (headless, or a failed init). Add it during setup.
    pub fn on_graphics_ready(&mut self, f: impl FnOnce(&mut Engine, crate::config::GraphicsMode) + 'static) -> &mut Self {
        self.graphics_hooks.push(Box::new(f));
        self
    }

    fn run_graphics_hooks(&mut self, mode: crate::config::GraphicsMode) {
        for h in std::mem::take(&mut self.graphics_hooks) {
            h(self, mode);
        }
    }

    /// Add a [`Draw2DOverlay`]: extra 2D drawing over the frame through [`Draw2D`](rendering::Draw2D),
    /// in the wgpu and in the SDL mode, for every rendered window (SDL mode: the main one). Drawn after
    /// the [`RenderOverlay`]s, in add order. Never called without a window (headless). Texture
    /// changes from before the first rendered frame are visible to it; add overlays during setup.
    pub fn add_draw2d_overlay(&mut self, overlay: impl rendering::Draw2DOverlay) -> &mut Self {
        self.overlays_2d.add(Box::new(overlay));
        self
    }

    /// Add an [`InputFilter`]: asked about every raw input event (also in headless mode and for
    /// injected events) before `InputState::apply`, in add order; any `Captured` marks the
    /// event (`InputEvent::captured`).
    pub fn add_input_filter(&mut self, f: impl InputFilter) -> &mut Self {
        self.input_filters.push(Box::new(f));
        self
    }

    pub fn time(&self) -> &Time {
        &self.state.time
    }

    pub fn input(&self) -> &InputState {
        &self.state.input
    }

    pub fn gamepads(&self) -> &GamepadState {
        &self.state.gamepads
    }

    /// A sender for this engine's own message queue.
    pub fn sender(&self) -> flume::Sender<Message> {
        self.msg_tx.clone()
    }

    /// Queue an input event for the next frame (tests, headless mode).
    pub fn inject(&mut self, ev: InputEvent) {
        self.windows.inject(ev);
    }

    /// `false` once a shutdown was requested (kill, quit, close request). A paused engine
    /// is still running in this sense.
    pub fn is_running(&self) -> bool {
        !matches!(self.lifecycle, Lifecycle::ShuttingDown | Lifecycle::Terminated)
    }

    /// `true` while paused by `Message::Stop`.
    pub fn is_paused(&self) -> bool {
        self.lifecycle == Lifecycle::Paused
    }

    pub fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    /// Ask for a shutdown (no effect once terminated).
    fn request_shutdown(&mut self) {
        if self.lifecycle != Lifecycle::Terminated {
            self.lifecycle = Lifecycle::ShuttingDown;
        }
    }

    fn set_frame_rate(&mut self, f: f32) {
        self.max_fps = sanitize_fps(f);
    }

    /// Run `Shutdown` (and apply its commands) unless it already ran.
    fn shutdown_once(&mut self) {
        if self.lifecycle == Lifecycle::ShuttingDown {
            self.run_stage(Stage::Shutdown);
            self.lifecycle = Lifecycle::Terminated;
        }
    }

    /// If the engine was killed, run `Shutdown` once and return `true`: the caller must
    /// abort the rest of the frame.
    fn abort_if_killed(&mut self) -> bool {
        if matches!(self.lifecycle, Lifecycle::ShuttingDown | Lifecycle::Terminated) {
            self.shutdown_once();
            true
        } else {
            false
        }
    }

    /// End the engine's life (loop exit through a failed frame request): request shutdown and
    /// run `Shutdown` once.
    #[cfg(target_family = "wasm")]
    fn finish(&mut self) {
        self.request_shutdown();
        self.shutdown_once();
    }

    /// Engine-side handling of control messages (by reference; the message stays readable).
    fn handle_message(&mut self, msg: &Message){
        match msg {
            #[cfg(target_family = "wasm")]
            Message::SetCanvasId(s) => {
                self.windows.set_canvas_id(s.clone());
            }
            Message::Null => (),
            Message::Say(msg) =>{
                self.logger.info(msg);
            },
            Message::Stop =>{
                if self.lifecycle == Lifecycle::Running {
                    self.lifecycle = Lifecycle::Paused;
                }
            },
            Message::Start =>{
                if self.lifecycle == Lifecycle::Paused {
                    self.lifecycle = Lifecycle::Running;
                }
            },
            Message::Kill => {
                self.request_shutdown();
            }
            Message::SetFrameRate(f) => {
                self.set_frame_rate(*f);
            },
            Message::SetThreads(n) => {
                self.set_threads(Threads::Fixed((*n).max(1)));
                let size = crate::thread_pool::global_pool_size();
                self.logger.info(&format!("threads: {}, pool size: {size}", self.threads()));
            },
            Message::ChangeTitle(tit) => {
                self.windows.set_title(WindowId::MAIN, tit.to_string());
            },
            Message::Log(log_msg) => {
                self.logger.log(log_msg);
            },
        }
    }

    /// Drain this engine's queue (and the global one when not headless), handle the
    /// control messages and make all of them readable this frame as `Events<Message>`.
    fn drain_messages(&mut self) {
        let mut msgs: Vec<Message> = self.msg_rx.drain().collect();
        if !self.windows.is_headless() {
            msgs.extend(recv_messages());
        }
        for m in &msgs {
            self.handle_message(m);
        }
        if !msgs.is_empty() {
            self.events.get_mut::<Message>().extend_readable(msgs);
        }
    }

    /// Apply what systems queued: world changes (in order), then control requests.
    fn apply_commands(&mut self) {
        // One tick for everything applied: newer than every system of the stage, so each of
        // them (the one that queued it included) sees it as added/changed on its next run.
        let tick = self.state.world.increment_change_tick();
        self.state.resources.set_tick(tick);
        self.commands.apply(&mut self.state.world, &self.state.types);
        let sched = self.commands.take_sched();
        if !sched.is_empty() {
            self.schedule.apply_ops(sched);
        }
        let cmds: Vec<EngineCommand> = self.commands.drain().collect();
        for c in cmds {
            match c {
                EngineCommand::Quit => self.request_shutdown(),
                EngineCommand::Task(t) => {
                    if let Some(f) = t.take() {
                        f(self);
                    }
                }
                EngineCommand::SetTitle(t) => self.windows.set_title(WindowId::MAIN, t),
                EngineCommand::SetFrameRate(f) => self.set_frame_rate(f),
                // Cursor shape / visibility go to the window the pointer is in; grab and text input to the focused one.
                EngineCommand::SetCursorVisible(v) => self.windows.set_cursor_visible(self.state.input.pointer_window().unwrap_or(self.state.input.focus_window()), v),
                EngineCommand::SetCursorIcon(i) => self.windows.set_cursor_icon(self.state.input.pointer_window().unwrap_or(self.state.input.focus_window()), i),
                EngineCommand::SetCursorGrab(g) => self.windows.set_cursor_grab(self.state.input.focus_window(), g),
                EngineCommand::SetFullscreen(on) => self.windows.set_fullscreen(WindowId::MAIN, on),
                EngineCommand::SetTextInput(on) => self.windows.set_text_input(self.state.input.focus_window(), on),
                EngineCommand::SetTextInputArea { x, y, w, h, cursor } => {
                    self.windows.set_text_input_area(self.state.input.focus_window(), x, y, w, h, cursor)
                }
                EngineCommand::CreateWindow { id, desc } => {
                    if let Err(e) = self.open_window(id, desc) {
                        self.logger.error(&format!("create_window: {e}"));
                        // The id was handed out already: tell the game it will never exist.
                        self.windows.inject(InputEvent::with_time(InputEventKind::Window(WindowEvent::Closed), 0, id));
                    }
                }
                EngineCommand::CloseWindow(id) => self.close_window(id),
                EngineCommand::SetWindowTitle(id, t) => self.windows.set_title(id, t),
                EngineCommand::SetWindowFullscreen(id, on) => self.windows.set_fullscreen(id, on),
                EngineCommand::LoadScene(scene) => {
                    if let Err(e) = self.load_scene(&scene) {
                        self.logger.error(&format!("load_scene: {e}"));
                    }
                }
                EngineCommand::UnloadScene => self.unload_scene(),
                EngineCommand::ReloadMods => {
                    let report = self.reload_mods();
                    self.log_load_report(&report);
                }
                EngineCommand::UnloadMods => {
                    let report = self.unload_mods();
                    self.logger.info(&format!("mods: {} unloaded, {} could not be unloaded", report.unloaded.len(), report.failed.len()));
                    for id in &report.unloaded {
                        self.logger.info(&format!("mods: unloaded {id}"));
                    }
                    for (id, e) in &report.failed {
                        self.logger.warn(&format!("mods: {id} stays loaded: {e}"));
                    }
                }
            }
        }
    }

    /// Run every system of `stage` once, then apply the commands they queued.
    fn run_stage(&mut self, stage: Stage) {
        {
            let Engine { schedule, state, events, commands, exec, .. } = self;
            let EngineState { world, types, windows, input, gamepads, time, resources, scripting } = state;
            let mut cx = Ctx::new(world, &*types, resources, &*time, &*input, &*gamepads, &*windows, commands, scripting, events, exec);
            schedule.run(stage, &mut cx, exec);
        }
        self.apply_commands();
        self.flush_script_logs();
    }

    fn run_fixed_step(&mut self) {
        self.state.time.set_in_fixed(true);
        self.run_stage(Stage::FixedUpdate);
        self.state.time.set_in_fixed(false);
    }

    /// Create the GPU context and the surface, then `init` the renderer. Without a window
    /// (headless) there is nothing to do; a failure is logged and the engine keeps ticking
    /// without rendering.
    pub async fn init_graphics(&mut self) {
        use crate::config::GpuMode;
        self.logger.info("Initializing Graphics Context...");
        let mode = GpuMode::effective();
        #[cfg(not(target_arch = "wasm32"))]
        if mode == GpuMode::Sdl {
            match self.init_sdl_mode() {
                Ok(()) => {
                    self.run_graphics_hooks(crate::config::GraphicsMode::Sdl);
                    self.logger.info("Graphics Context Initialized (SDL mode)!")
                }
                Err(e) => self.logger.error(&format!("graphics init failed: {e}")),
            }
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if mode == GpuMode::Sdl {
            self.logger.warn("gpu mode \"sdl\" does not exist on the web (SDL_Renderer is native only); using wgpu");
        }
        match GpuContext::new(&self.windows).await {
            Ok(gpu) => {
                self.run_graphics_hooks(crate::config::GraphicsMode::Wgpu);
                self.renderer.init(&gpu.gpu());
                for o in &mut self.overlays {
                    o.init(&gpu.gpu());
                }
                self.gpu = Some(gpu);
                self.sync_window_surfaces();
                self.logger.info("Graphics Context Initialized!");
            }
            #[cfg(not(target_arch = "wasm32"))]
            Err(e) if mode == GpuMode::Auto && !self.windows.is_headless() => {
                self.logger.warn(&format!("no usable wgpu graphics ({e}); falling back to SDL mode (2D only)"));
                match self.init_sdl_mode() {
                    Ok(()) => {
                        self.run_graphics_hooks(crate::config::GraphicsMode::Sdl);
                        self.logger.info("Graphics Context Initialized (SDL mode)!")
                    }
                    Err(e2) => self.logger.error(&format!("graphics init failed: wgpu: {e}; SDL: {e2}")),
                }
            }
            Err(e) => self.logger.error(&format!("graphics init failed: {e}")),
        }
    }

    /// Create the window's `SDL_Renderer` (SDL mode).
    #[cfg(not(target_arch = "wasm32"))]
    fn init_sdl_mode(&mut self) -> Result<(), String> {
        // SDL mode draws the main window only (an `SDL_Renderer` owns its textures, so every window
        // would need its own texture table): close secondary windows opened before graphics came up.
        let extra: Vec<WindowId> = self.windows.secondary().to_vec();
        if !extra.is_empty() {
            self.logger.error(&format!(
                "SDL mode (SDL_Renderer) draws the main window only; closing {} secondary window(s). Use the wgpu modes for multiple windows",
                extra.len()
            ));
            for id in extra {
                self.close_window(id);
            }
        }
        let window = self.windows.sdl_window_ptr(WindowId::MAIN);
        // SAFETY: the pointer is the main SDL window owned by `self.windows`; `self.sdl` is
        // declared before `windows` in `Engine`, so the renderer is destroyed first.
        let draw = unsafe { rendering::SdlDraw2D::for_window(window) }?;
        crate::global_info(&format!("graphics backend: SDL_Renderer ({})", draw.renderer_name()));
        self.sdl = Some(rendering::SdlMode::new(draw));
        Ok(())
    }

    /// Open a secondary window (`WindowDesc`): the id is returned at once; the window gets its
    /// first `Resized` / `ScaleFactorChanged` and a `Created` event in the next frame. The GPU
    /// surface is made now when graphics is up, else when it comes up. Fails when the platform
    /// cannot open the window, or in SDL mode (`docs/MULTI_WINDOW.md`).
    ///
    /// ```no_run
    /// use efeengine_core::prelude::*;
    /// use efeengine_core::window::WindowDesc;
    /// let mut engine = Engine::new("game")?;
    /// let map = engine.create_window(WindowDesc::new("Map").web_element("#sidebar").size(320, 240))?;
    /// engine.world_mut().spawn((Camera::new_2d().target(map), Transform::default()));
    /// # Ok::<(), efeengine_core::engine::error::EngineError>(())
    /// ```
    pub fn create_window(&mut self, desc: WindowDesc) -> Result<WindowId, EngineError> {
        let id = self.commands.alloc_window();
        self.open_window(id, desc)?;
        Ok(id)
    }

    fn open_window(&mut self, id: WindowId, desc: WindowDesc) -> Result<(), EngineError> {
        #[cfg(not(target_arch = "wasm32"))]
        if self.sdl.is_some() {
            return Err(EngineError::Window("SDL mode (SDL_Renderer) draws the main window only; use a wgpu GPU mode for more windows".into()));
        }
        self.windows.create_window(id, &desc)?;
        self.state.windows.insert(id, desc.size);
        self.windows.inject(InputEvent::with_time(InputEventKind::Window(WindowEvent::Created), 0, id));
        self.sync_window_surfaces();
        Ok(())
    }

    /// Close a secondary window: its surface, camera targets and UI go away; a `Closed` event is
    /// delivered next frame. Ignored for the main window (use `commands.quit()`) and unknown ids.
    pub fn close_window(&mut self, id: WindowId) {
        if id.is_main() || !self.windows.exists(id) {
            return;
        }
        // The surface must go before the window it was created from.
        if let Some(gpu) = &mut self.gpu {
            gpu.remove_window(id);
        }
        self.windows.close_window(id);
        self.state.windows.remove(id);
        self.windows.inject(InputEvent::with_time(InputEventKind::Window(WindowEvent::Closed), 0, id));
    }

    /// Give every open secondary window a GPU surface (and the overlays their `init` for it).
    fn sync_window_surfaces(&mut self) {
        let Some(gpu) = &mut self.gpu else { return };
        for &id in self.windows.secondary() {
            if gpu.windows.contains_key(&id.0) {
                continue;
            }
            let size = self.windows.size(id);
            let scale = self.windows.scale_factor(id);
            match gpu.add_window(&self.windows, id, size, scale) {
                Ok(()) => {
                    if let Some(g) = gpu.gpu_for(id.0) {
                        for o in &mut self.overlays {
                            o.init(&g);
                        }
                    }
                }
                Err(e) => self.logger.error(&format!("window {}: no GPU surface ({e}); it will stay blank", id.0)),
            }
        }
    }

    /// Reconfigure the surface of `window` and tell the renderer / overlays.
    fn resize_surface(&mut self, window: WindowId, size: (u32, u32)) {
        let Some(gpu) = &mut self.gpu else { return };
        if window.is_main() {
            if gpu.resize(size) {
                self.renderer.resize(&gpu.gpu());
                for o in &mut self.overlays {
                    o.resize(&gpu.gpu());
                }
            }
        } else if gpu.resize_window(window, size) {
            if let Some(g) = gpu.gpu_for(window.0) {
                for o in &mut self.overlays {
                    o.resize(&g);
                }
            }
        }
    }

    /// Acquire each window's surface, let the renderer draw its part of the frame, present. Skipped
    /// without a GPU (headless, failed init). If no window could be drawn, the frame's texture
    /// changes are put back so the renderer still sees them once.
    fn render_frame_to_surface(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(sdl) = &mut self.sdl {
            let (w, h) = self.windows.size(WindowId::MAIN);
            if w > 0 && h > 0 && self.state.resources.contains::<RenderFrame>() {
                let frame = self.state.resources.get::<RenderFrame>().expect("RenderFrame resource");
                let scale = self.windows.scale_factor(WindowId::MAIN) as f32;
                sdl.render(&frame, self.renderer.has_custom(), self.renderer.has_installed_2d(), !self.overlays.is_empty(), &mut self.overlays_2d, scale);
            } else if let (Some(f), Some(t)) = (self.state.resources.get_mut::<RenderFrame>().map(|f| std::mem::take(&mut f.textures)), self.state.resources.get_mut::<Textures>()) {
                t.requeue(f);
            }
            return;
        }
        let Engine { renderer, overlays, overlays_2d, gpu, state, logger, .. } = self;
        let Some(gpu) = gpu else { return };
        if !state.resources.contains::<RenderFrame>() {
            return;
        }
        // The frame's texture changes go to the first window that gets drawn; later windows get none.
        let mut delivered = false;
        let mut warnings = state.resources.remove::<RenderWarnings>().unwrap_or_default();
        let size = (gpu.config.width, gpu.config.height);
        if !gpu.is_suspended() && size.0 > 0 && size.1 > 0 {
            use wgpu::CurrentSurfaceTexture as Cst;
            let (output, suboptimal) = match gpu.surface.get_current_texture() {
                Cst::Success(t) => (Some(t), false),
                Cst::Suboptimal(t) => (Some(t), true),
                // Surface no longer matches the window: reconfigure, next frame renders.
                Cst::Outdated | Cst::Lost => {
                    gpu.reconfigure();
                    renderer.resize(&gpu.gpu());
                    for o in overlays.iter_mut() {
                        o.resize(&gpu.gpu());
                    }
                    (None, false)
                }
                // Skip this frame (timeout, or the window is occluded / minimized).
                Cst::Timeout | Cst::Occluded => (None, false),
                Cst::Validation => {
                    logger.error("Render error: surface texture validation failed");
                    (None, false)
                }
            };
            if let Some(output) = output {
                let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
                {
                    let frame = state.resources.get::<RenderFrame>().expect("RenderFrame resource");
                    renderer.render(&gpu.gpu(), &view, &frame, &frame.textures, &mut warnings);
                }
                for o in overlays.iter_mut() {
                    o.render(&gpu.gpu(), &view);
                }
                {
                    let frame = state.resources.get::<RenderFrame>().expect("RenderFrame resource");
                    overlays_2d.sync_textures(&gpu.gpu(), &frame.textures);
                }
                overlays_2d.render_wgpu(&gpu.gpu(), &view);
                delivered = true;
                gpu.queue.present(output);
                if suboptimal {
                    gpu.reconfigure();
                }
            }
        }
        // Secondary windows (same device, one surface each).
        let ids: Vec<u32> = gpu.windows.keys().copied().collect();
        let empty = rendering::TextureChanges::default();
        for id in ids {
            let Some(mut w) = gpu.windows.remove(&id) else { continue };
            let g = rendering::Gpu { surface_format: w.surface.format(), size: w.size, scale_factor: w.scale_factor, window: id, ..gpu.gpu() };
            if g.surface_format != gpu.config.format {
                if !warnings.window_format_mismatch {
                    warnings.window_format_mismatch = true;
                    logger.error(&format!(
                        "window {id}: surface format {:?} differs from the main window's {:?}; secondary windows are not drawn",
                        g.surface_format, gpu.config.format
                    ));
                }
            } else if let Some(sf) = w.surface.acquire(&g) {
                {
                    let frame = state.resources.get::<RenderFrame>().expect("RenderFrame resource");
                    let tex = if delivered { &empty } else { &frame.textures };
                    renderer.render(&g, &sf.view, &frame, tex, &mut warnings);
                    overlays_2d.sync_textures(&g, tex);
                }
                delivered = true;
                for o in overlays.iter_mut() {
                    o.render(&g, &sf.view);
                }
                overlays_2d.render_wgpu(&g, &sf.view);
                w.surface.present(&g, sf);
            }
            drop(g);
            gpu.windows.insert(id, w);
        }
        state.resources.insert(warnings);
        if !delivered {
            let changes = state.resources.get_mut::<RenderFrame>().map(|f| std::mem::take(&mut f.textures));
            if let (Some(changes), Some(t)) = (changes, state.resources.get_mut::<Textures>()) {
                t.requeue(changes);
            }
        }
    }

    fn handle_window_event(&mut self, ev: &InputEvent) {
        let InputEventKind::Window(w) = &ev.kind else { return };
        let id = ev.window;
        self.state.windows.apply(id, w);
        match *w {
            WindowEvent::Resized { width, height } => {
                self.resize_surface(id, (width, height));
            }
            WindowEvent::ScaleFactorChanged { scale } => {
                if let Some(gpu) = &mut self.gpu {
                    let s = crate::engine::render::gpu::sanitize_scale(scale);
                    if id.is_main() {
                        if gpu.scale_factor != s {
                            gpu.scale_factor = s;
                            for o in &mut self.overlays {
                                o.resize(&gpu.gpu());
                            }
                        }
                    } else if let Some(sw) = gpu.windows.get_mut(&id.0) {
                        if sw.scale_factor != s {
                            sw.scale_factor = s;
                            if let Some(g) = gpu.gpu_for(id.0) {
                                for o in &mut self.overlays {
                                    o.resize(&g);
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::Focused(_) | WindowEvent::FullscreenChanged(_) | WindowEvent::Created | WindowEvent::Closed => {}
            WindowEvent::CloseRequested => {
                if id.is_main() {
                    self.logger.info("close requested");
                    self.request_shutdown();
                } else {
                    // The event is delivered to the game this frame; the window goes now.
                    self.close_window(id);
                }
            }
        }
    }

    /// The one shared frame function: runs one full frame at time `now_secs`
    /// (seconds, any monotonic origin). Frame order: `docs/design-io-and-loop.md` section 9.
    ///
    /// The first frame has delta 0, so N frames at `dt == fixed_dt` run N-1 fixed steps.
    /// Kill (from `CloseRequested`, `Message::Kill` or `commands.quit()`) aborts the rest of
    /// the frame (remaining stages and render are skipped), then `Shutdown` runs once;
    /// `Startup` is skipped if the engine is killed before it. After that this is a no-op.
    ///
    /// While paused (`Message::Stop`) a frame only polls window events and handles messages
    /// (so `Start`, `Kill` and close requests still work): no stage runs, nothing renders,
    /// and game time does not advance (the clock is only tracked, so resuming does not see
    /// the paused time as one big delta).
    pub fn frame(&mut self, now_secs: f64){
        let first = self.lifecycle == Lifecycle::Created;
        if first {
            // The first frame closes API registration (`game_loop` closed it already).
            self.close_api();
            self.lifecycle = Lifecycle::Running;
        }
        if self.lifecycle == Lifecycle::Terminated {
            return;
        }
        let steps = if self.lifecycle == Lifecycle::Paused {
            self.state.time.track(now_secs);
            0
        } else {
            self.state.time.advance(now_secs)
        };
        self.events.update_all();
        // Poll first so input/resize events from this frame are handled this frame.
        self.state.input.begin_frame();
        self.event_buf.clear();
        self.windows.poll_events(&mut self.event_buf);
        let events = std::mem::take(&mut self.event_buf);
        let mut events = events;
        for ev in &mut events {
            let mut captured = false;
            for f in &mut self.input_filters {
                captured |= f.filter(ev) == Filter::Captured;
            }
            ev.captured = captured;
            self.handle_window_event(ev);
            self.state.input.apply(ev);
        }
        self.event_buf = events;
        // Kill aborts the rest of the frame (remaining stages and render are skipped),
        // then Shutdown runs once.
        if self.abort_if_killed() {
            return;
        }
        self.state.gamepads.begin_frame();
        self.windows.poll_gamepads(&mut self.state.gamepads);
        self.drain_messages();
        if self.abort_if_killed() {
            return;
        }

        if first {
            self.run_stage(Stage::Startup);
            if self.abort_if_killed() {
                return;
            }
        }
        if self.lifecycle == Lifecycle::Paused {
            return;
        }
        self.run_stage(Stage::PreUpdate);
        if self.abort_if_killed() {
            return;
        }
        for _ in 0..steps {
            self.run_fixed_step();
            if self.abort_if_killed() {
                return;
            }
        }
        for stage in [Stage::Update, Stage::PostUpdate, Stage::RenderExtract] {
            self.run_stage(stage);
            if self.abort_if_killed() {
                return;
            }
        }

        self.state.world.maybe_check_change_ticks();
        self.state.resources.check_ticks(self.state.world.change_tick());
        self.render_frame_to_surface();
    }

    /// Run `n` frames with a synthetic clock advancing `dt` seconds per frame (continuing
    /// from where the previous `run_frames` stopped), stopping early when the engine is killed.
    /// The first frame ever has delta 0, so N frames at `dt == fixed_dt` give N-1 fixed steps.
    pub fn run_frames(&mut self, n: usize, dt: f64){
        for _ in 0..n {
            if !self.is_running() {
                break;
            }
            let t = self.clock;
            self.frame(t);
            self.clock = t + dt;
        }
    }

    /// Read the platform clock and run one frame.
    pub fn tick(&mut self){
        #[cfg(not(target_family = "wasm"))]
        let now = self.start.elapsed().as_secs_f64();
        #[cfg(target_family = "wasm")]
        let now = {
            use wgpu::web_sys;
            match web_sys::window().and_then(|w| w.performance()) {
                Some(p) => p.now() / 1000.0,
                None => self.clock + 1.0 / 60.0,
            }
        };
        #[cfg(target_family = "wasm")]
        {
            self.clock = now;
        }
        self.frame(now);
    }

    /// takes ownership of the game and starts the game loop until killed
    pub fn game_loop(mut self){
        #[cfg(not(target_family = "wasm"))]
        {
            // `EFE_API_EXPORT`: the game registered its functions between `new` and here, so
            // this is the place to write the index; no frame runs.
            if let Some(path) = self.api_export.take() {
                match self.write_api_index(&path) {
                    Ok(()) => self.logger.info(&format!("API index written to {}", path.display())),
                    Err(e) => self.logger.error(&format!("cannot write the API index to {}: {e}", path.display())),
                }
                // the component schema goes next to it: `<path>` with the extension `efschema.json`
                let schema = schema::schema_path(&path);
                match self.write_schema(&schema) {
                    Ok(()) => self.logger.info(&format!("component schema written to {}", schema.display())),
                    Err(e) => self.logger.error(&format!("cannot write the schema to {}: {e}", schema.display())),
                }
                return;
            }
            self.close_api();
            self.sdl_loop();
        }
        #[cfg(target_family = "wasm")]
        self.wasm_loop_start();
    }
    #[cfg(target_family = "wasm")]
    pub fn wasm_loop_start(mut self){
        self.close_api();
        // The browser can't block on async GPU init (no pollster here), so init
        // graphics on the JS event loop first and start ticking once it is done.
        wasm_bindgen_futures::spawn_local(async move {
            if !self.windows.is_headless() {
                self.init_graphics().await;
            }
            self.wasm_run_frames();
        });
    }

    /// The rAF loop. It keeps scheduling frames while paused (`Message::Stop`) and ends when
    /// the engine is killed or a frame request fails (Shutdown runs once).
    #[cfg(target_family = "wasm")]
    fn wasm_run_frames(self){
        use std::rc::Rc;
        use std::cell::RefCell;

        type Slot = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
        type Shared = Rc<RefCell<Option<Engine>>>;

        // Ends the loop: finish the engine (Shutdown once) and drop it so its GPU resources are
        // released. The engine borrow is released before anything else runs.
        fn terminate(engine: &Shared) {
            let taken = engine.borrow_mut().take();
            if let Some(mut e) = taken {
                e.finish();
                e.logger.info("Killing the engine");
            }
        }

        // The engine lives in an `Option` so it can be taken (dropped) on termination.
        let engine: Shared = Rc::new(RefCell::new(Some(self)));
        let f: Slot = Rc::new(RefCell::new(None));
        let g = f.clone();
        let engine_c = engine.clone();
        *g.borrow_mut() = Some(Closure::<dyn FnMut(f64)>::new(move |timestamp_ms: f64| {
            let engine = &engine_c;
            // Run the frame in its own scope: no `engine` borrow is held while scheduling.
            let keep_going = {
                let mut guard = engine.borrow_mut();
                match guard.as_mut() {
                    Some(e) => {
                        e.clock = timestamp_ms / 1000.0;
                        e.frame(timestamp_ms / 1000.0);
                        e.is_running()
                    }
                    None => false,
                }
            };
            let scheduled = keep_going && match f.borrow().as_ref() {
                Some(cb) => request_animation_frame(cb),
                None => false,
            };
            if !scheduled {
                // Kill or a failed request: shut down and drop the engine.
                terminate(engine);
                // The closure slot owns this very closure (through `f` it captured: an Rc
                // cycle) and we are running inside it, so it must not be dropped here.
                // Drop it from a later microtask, after this call has returned.
                let slot = f.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    slot.borrow_mut().take();
                });
            }
        }));

        // 3. Start the loop
        let started = match g.borrow().as_ref() {
            Some(cb) => request_animation_frame(cb),
            None => false,
        };
        if !started {
            // Nothing is running yet, so the closure can be dropped directly.
            terminate(&engine);
            g.borrow_mut().take();
        }
    }


    #[cfg(not(target_family = "wasm"))]
    fn sdl_loop(&mut self) {
        if !self.windows.is_headless() {
            pollster::block_on(self.init_graphics());
        }
        'main:loop {
            let frame_start = std::time::Instant::now();
            self.tick();
            // Leave the loop so `game_loop` returns and the engine is dropped normally.
            // Kill already ran Shutdown inside `frame`. A paused engine keeps looping.
            if !self.is_running() { break 'main; }
            if self.is_paused() && frame_budget(self.max_fps).is_none() {
                // Nothing renders while paused: do not spin.
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            // Frame limiter. vsync pacing comes with the renderer in Phase 4.
            if let Some(target) = frame_budget(self.max_fps) {
                if let Some(rest) = target.checked_sub(frame_start.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        }
    }
}

/// `Some(fps)` only for a finite, positive rate.
fn sanitize_fps(f: f32) -> Option<f32> {
    if f.is_finite() && f > 0.0 { Some(f) } else { None }
}

/// Time budget of one frame for the given cap; `None` when uncapped or the rate is invalid.
#[cfg(not(target_family = "wasm"))]
fn frame_budget(max_fps: Option<f32>) -> Option<std::time::Duration> {
    let fps = sanitize_fps(max_fps?)?;
    std::time::Duration::try_from_secs_f64(1.0 / fps as f64).ok()
}

/// Returns `false` when the request could not be made (the frame loop must stop).
#[cfg(target_family = "wasm")]
fn request_animation_frame(f: &Closure<dyn FnMut(f64)>) -> bool {
    use wasm_bindgen::JsCast;
    use wgpu::web_sys;

    let Some(window) = web_sys::window() else {
        ::log::error!("no global `window` exists; the frame loop stops");
        return false;
    };
    if window.request_animation_frame(f.as_ref().unchecked_ref()).is_err() {
        ::log::error!("`requestAnimationFrame` failed; the frame loop stops");
        return false;
    }
    true
}


#[cfg(test)]
mod tests{
    use std::{pin::Pin, sync::mpsc};

    #[test]
    #[cfg(not(target_family = "wasm"))]
    fn frame_budget_handles_invalid_rates() {
        use super::frame_budget;
        let d = frame_budget(Some(60.0)).expect("60 fps has a budget");
        assert!((d.as_secs_f64() * 1000.0 - 16.667).abs() < 0.01, "{d:?}");
        assert_eq!(frame_budget(None), None);
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e-35] {
            assert_eq!(frame_budget(Some(bad)), None, "{bad}");
        }
    }

    #[test]
    fn sanitize_fps_rejects_invalid() {
        use super::sanitize_fps;
        assert_eq!(sanitize_fps(30.0), Some(30.0));
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(sanitize_fps(bad), None);
        }
    }

    #[test]
    #[cfg(not(target_family = "wasm"))]

    fn platform(){
        let s = sdl3::get_platform();
        println!("{}",s);
    }
    #[test]
    fn mpsc_test(){
        let (tx,rx) = mpsc::channel::<String>();
        
        std::thread::spawn(move||{
            let mut i = 0;
            for x in 0..10{
                tx.send(format!("asfasfas no {i}"));
                i+=1;
            };
        });

        std::thread::sleep(std::time::Duration::from_secs(2));
        loop {
            match rx.recv() {
                Ok(msg) => println!("received \"{}\"",msg),
                Err(err) => {println!("err:{:?}",err);break;},
            }
            
            
        }

    }


    #[test]
    fn failed_index_registration_is_rolled_back() {
        use super::*;
        use crate::engine::config::EngineConfig;
        let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
        // `Foo` is in the scripting index but not in the type registry, so indexing the new
        // `Foo` (a different layout) is refused and the registration must not stay behind.
        e.state.scripting.register_type(efevm::StructDesc::new("Foo", 4, 4)).unwrap();
        let before = e.types().len();
        assert!(e.register_struct(efevm::StructDesc::new("Foo", 8, 8)).is_err());
        assert!(e.types().by_name("Foo").is_none());
        assert!(e.types().get(TypeRegistry::id_of_name("Foo")).is_none());
        assert_eq!(e.types().len(), before);
        // the index still holds only the original layout
        assert_eq!(e.api_index().struct_type("Foo").map(|d| d.size), Some(4));
    }

    #[test]
    fn parrrralel(){
        let t = std::thread::available_parallelism().unwrap();
        let h = std::thread::spawn(|| {
            
        });
        println!("{}",t)
    }

}