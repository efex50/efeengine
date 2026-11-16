use std::sync::{Arc, Mutex};

use once_cell::sync::{Lazy, OnceCell};
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use crate::{debug_info, engine::{config::{EngineConfig, Threads}, error::EngineError, events::EventRegistry, parallel::Executor, input::{GamepadState, InputEvent, InputEventKind, InputState, WindowEvent}, messages::Message, resources::{Res, Resources}, schedule::{Access, Ctx, EngineCommand, Commands, ParCtx, Schedule, Stage}, scripting::{CacheConfig, CacheDir, CacheStats, ScriptCache, CACHE_FORMAT, AssetHandler, LoadError, LoadInput, Loaded, ModSource, LoadReport, Reloaded, UnloadReport, ModError, ModOptions, ModPolicy, ModuleId, PackageInfo, ScriptError, ScriptEvent, Scripting}, time::Time, types::{ScriptType, TypeError, TypeRegistry}, window::GameWindowManager}, log::{LogCrateLogger, Logger, NewDefaultLogger}, recv_messages, window::WindowManager};
use crate::engine::components::{Camera, GlobalTransform, MeshHandle, Parent, Sprite, TextureHandle, Transform, Velocity, Visibility};
use crate::engine::render::{self as rendering, gpu::GpuContext, ClearRenderer, RenderFrame, Renderer, Textures};
use crate::{global_info, thread_pool::{init_global_pool, init_global_pool_auto}};
pub mod window;
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
pub mod parallel;
pub mod error;
pub mod resources;
pub mod events;
pub mod time;
pub mod config;
pub mod input;
pub mod scripting;

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

#[derive(Debug,PartialEq, Eq)]
pub enum EngineStatus{
    Uninitialized,
    Initializing,
    Ready,
    Stopped,
    Running,
    Kill,
}



/// Last known state of the main window, kept up to date from window events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowState{
    /// Drawable size in physical pixels.
    pub size: (u32, u32),
    pub scale_factor: f64,
    pub focused: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { size: (0, 0), scale_factor: 1.0, focused: true }
    }
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
    pub window: WindowState,
    pub input: InputState,
    pub gamepads: GamepadState,
    pub time: Time,
    pub resources: Resources,
    /// The scripting runtime: host registry (the API index) and loaded modules.
    pub(crate) scripting: Scripting,
}

pub struct Engine{
    // NOTE: field order matters. Fields drop in declaration order, and the surface inside
    // `gpu` was created from `windows`' raw window handle, so `renderer` and `gpu` must be
    // declared (and so dropped) before `windows`.
    renderer: Box<dyn Renderer>,
    gpu: Option<GpuContext>,
    pub windows:GameWindowManager,
    status:EngineStatus,
    pub logger:Box<dyn Logger>,
    pub state:EngineState,
    /// Input events of the current tick; reused every tick.
    event_buf: Vec<InputEvent>,
    config: EngineConfig,
    events: EventRegistry,
    schedule: Schedule,
    /// Runs the parallel batches of a stage.
    exec: Executor,
    commands: Commands,
    /// `Startup` has run.
    started: bool,
    /// `Shutdown` has run.
    shut_down: bool,
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
        scripting.set_game(&cfg.title);

        let state = EngineState{
            world: hecs::World::new(),
            types: TypeRegistry::new(),
            window: WindowState::default(),
            input: InputState::new(),
            gamepads: GamepadState::new(),
            time: Time::new(cfg.fixed_dt),
            resources: Resources::new(),
            scripting,
        };
        let mut state = state;
        state.resources.insert(Textures::new());
        state.resources.insert(RenderFrame::default());
        state.resources.insert(rendering::RenderWarnings::default());
        let (msg_tx, msg_rx) = flume::unbounded();
        let exec = Executor::new(resolve_threads(cfg.threads, headless));
        let mut schedule = Schedule::new();
        // The script system runs first in `Update`; game systems added later run after it.
        schedule.add(Stage::Update, Box::new(scripting::script_system));
        // Built-in systems: motion, hierarchy, render extract.
        schedule.add_par(Stage::FixedUpdate, transform::velocity_access(), Box::new(transform::velocity_system));
        schedule.add_par(Stage::PostUpdate, transform::propagate_access(), Box::new(transform::propagate_transforms));
        schedule.add(Stage::RenderExtract, Box::new(rendering::render_extract));

        let mut engine = Self {
            renderer: Box::new(ClearRenderer),
            gpu: None,
            windows: w,
            status:EngineStatus::Uninitialized,
            logger,
            state,
            event_buf: Vec::new(),
            max_fps: cfg.max_fps.and_then(sanitize_fps),
            config: cfg,
            events: EventRegistry::new(),
            schedule,
            exec,
            commands: Commands::default(),
            started: false,
            shut_down: false,
            msg_tx,
            msg_rx,
            clock: 0.0,
            #[cfg(not(target_family = "wasm"))]
            start: std::time::Instant::now(),
            #[cfg(not(target_family = "wasm"))]
            api_export,
        };
        engine.register_component::<Transform>().expect("built-in components register");
        engine.register_component::<Velocity>().expect("built-in components register");
        engine.register_component::<MeshHandle>().expect("built-in components register");
        engine.register_component::<TextureHandle>().expect("built-in components register");
        engine.register_component::<Camera>().expect("built-in components register");
        engine.register_component::<Sprite>().expect("built-in components register");
        engine.register_component::<Visibility>().expect("built-in components register");
        engine.register_component::<Parent>().expect("built-in components register");
        engine.register_component::<GlobalTransform>().expect("built-in components register");
        Ok(engine)
    }

    /// The (merged and validated) configuration this engine was created with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Add a system to `stage`; systems of a stage run in insertion order.
    pub fn add_system(&mut self, stage: Stage, f: impl FnMut(&mut Ctx<'_>) + 'static) -> &mut Self {
        self.schedule.add(stage, Box::new(f));
        self
    }

    /// Add a parallel system to `stage`. `access` declares the components and resources it
    /// reads and writes; systems whose accesses do not conflict run at the same time (see
    /// `docs/THREADING.md`). The result is the same for every thread count.
    pub fn add_par_system(&mut self, stage: Stage, access: Access, f: impl FnMut(&ParCtx<'_>) + Send + 'static) -> &mut Self {
        let mut access = access;
        access.set_default_name(std::any::type_name_of_val(&f));
        self.schedule.add_par(stage, access, Box::new(f));
        self
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

    /// Register component type `T` (owner `engine`); returns its id. This is the one way to
    /// make a component visible to scripts: it registers the type in the [`TypeRegistry`]
    /// (byte-level access for `engine.world.*`) and its layout in the API index.
    pub fn register_component<T: ScriptType + hecs::Component>(&mut self) -> Result<u32, TypeError> {
        self.register_component_owned::<T>(types::ENGINE_OWNER)
    }

    /// Like [`register_component`](Self::register_component) with an owner (the game's name or
    /// a mod name), recorded in the index.
    pub fn register_component_owned<T: ScriptType + hecs::Component>(&mut self, owner: &str) -> Result<u32, TypeError> {
        let id = self.state.types.register_owned::<T>(owner)?;
        self.index_or_rollback(id)
    }

    /// Register a layout-only struct (no component; e.g. from a mod); returns its id. It is
    /// also put in the API index, so functions can take `&Name` parameters.
    pub fn register_struct(&mut self, desc: efevm::StructDesc) -> Result<u32, TypeError> {
        let id = self.state.types.register_struct(desc)?;
        self.index_or_rollback(id)
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
        self.state.scripting.release_module(id, &mut self.state.resources);
        self.state.types.remove_owner(&name);
        Ok(())
    }

    /// Unload `id` and every loaded module that depends on it, transitively, dependents first
    /// (the latest loaded first). Returns the unloaded modules in that order, `id` last.
    pub fn unload_module_and_dependents(&mut self, id: ModuleId) -> Result<Vec<ModuleId>, ScriptError> {
        let removed = self.state.scripting.unload_cascade(id)?;
        for (id, name) in &removed {
            self.state.scripting.release_module(*id, &mut self.state.resources);
            self.state.types.remove_owner(name);
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
                Loaded::Package(self.state.scripting.load_package_item(b, &mut self.state.resources)?)
            }
            #[cfg(feature = "efa")]
            LoadInput::Folder { label, files, assets } => {
                let b = self.state.scripting.build_folder(label, files, assets, None)?;
                Loaded::Package(self.state.scripting.load_package_item(b, &mut self.state.resources)?)
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
        let EngineState { scripting, resources, types, .. } = &mut self.state;
        let reloaded = match input {
            #[cfg(feature = "efa")]
            LoadInput::Efa { name, .. } => return Err(LoadError::NotAMod(name.to_string())),
            LoadInput::Efvm { name, .. } => return Err(LoadError::NotAMod(name.to_string())),
            LoadInput::Package { label, bytes } => {
                let b = scripting.read_built(label, bytes)?;
                let (manifest, hash) = (b.pkg.manifest.clone(), b.hash.clone());
                scripting.reload_one(&manifest, &hash, label, Box::new(move |_| Ok((b, None))), resources, types)
            }
            #[cfg(feature = "efa")]
            LoadInput::Folder { label, files, assets } => {
                let (manifest, hash) = Scripting::peek_folder(files)?;
                let (l, h) = (label.to_string(), hash.clone());
                let build = Box::new(move |s: &mut Scripting| s.build_folder(&l, files, assets, Some(h)).map(|b| (b, None)));
                scripting.reload_one(&manifest, &hash, label, build, resources, types)
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
        let EngineState { scripting, resources, types, .. } = &mut self.state;
        let report = scripting.load_mods(resources, types, false);
        self.run_script_inits();
        report
    }

    /// Bring the loaded mods in line with the registered sources, with one dependency
    /// resolve: new mods load, mods whose content hash changed are replaced (the rules of
    /// [`reload_mod_input`](Self::reload_mod_input), all changed mods swapped together so
    /// that mods can change in step), unchanged mods are `skipped`, and mods that came from a
    /// source that no longer lists them are unloaded (`unloaded`, dependents included).
    pub fn reload_mods(&mut self) -> LoadReport {
        let EngineState { scripting, resources, types, .. } = &mut self.state;
        let report = scripting.load_mods(resources, types, true);
        self.run_script_inits();
        report
    }

    /// Unload every mod (packages and folders, however they were loaded), dependents before
    /// their dependencies, calling the asset handlers' `unload`. Game modules stay loaded; a
    /// mod that a game module still depends on stays too and is listed in `failed`.
    pub fn unload_mods(&mut self) -> UnloadReport {
        let EngineState { scripting, resources, types, .. } = &mut self.state;
        scripting.unload_mods(resources, types)
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

    /// Make `game_loop` write the API index to `path` and return without running frames (what
    /// `EFE_API_EXPORT=<path>` does at creation). `None` turns it off.
    #[cfg(not(target_family = "wasm"))]
    pub fn set_api_export_path(&mut self, path: Option<std::path::PathBuf>) {
        self.api_export = path;
    }

    pub fn world(&self) -> &hecs::World {
        &self.state.world
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
        self.state.resources.insert(value);
        self
    }

    pub fn resources(&self) -> &Resources {
        &self.state.resources
    }

    pub fn resources_mut(&mut self) -> &mut Resources {
        &mut self.state.resources
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

    /// Install the game's renderer (replaces the default [`ClearRenderer`]). If the GPU is
    /// already up, `init` runs at once; otherwise when it comes up.
    pub fn set_renderer(&mut self, renderer: impl Renderer) {
        self.renderer = Box::new(renderer);
        if let Some(gpu) = &self.gpu {
            self.renderer.init(&gpu.gpu());
        }
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

    /// `false` once the engine was killed.
    pub fn is_running(&self) -> bool {
        self.status != EngineStatus::Kill
    }

    fn set_frame_rate(&mut self, f: f32) {
        self.max_fps = sanitize_fps(f);
    }

    /// Run `Shutdown` (and apply its commands) unless it already ran.
    fn shutdown_once(&mut self) {
        if !self.shut_down {
            self.shut_down = true;
            self.run_stage(Stage::Shutdown);
        }
    }

    /// If the engine was killed, run `Shutdown` once and return `true`: the caller must
    /// abort the rest of the frame.
    fn abort_if_killed(&mut self) -> bool {
        if self.status == EngineStatus::Kill {
            self.shutdown_once();
            true
        } else {
            false
        }
    }

    /// End the engine's life (loop exit through Kill, Stop or a failed frame request):
    /// mark it killed and run `Shutdown` once.
    fn finish(&mut self) {
        self.status = EngineStatus::Kill;
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
                self.status = EngineStatus::Stopped;
            },
            Message::Start =>{
                self.status = EngineStatus::Running;
            },
            Message::Kill => {
                self.status = EngineStatus::Kill;
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
                self.windows.set_title(tit.to_string());
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
        self.commands.apply(&mut self.state.world, &self.state.types);
        let cmds: Vec<EngineCommand> = self.commands.drain().collect();
        for c in cmds {
            match c {
                EngineCommand::Quit => self.status = EngineStatus::Kill,
                EngineCommand::SetTitle(t) => self.windows.set_title(t),
                EngineCommand::SetFrameRate(f) => self.set_frame_rate(f),
                EngineCommand::SetCursorVisible(v) => self.windows.set_cursor_visible(v),
                EngineCommand::SetCursorGrab(g) => self.windows.set_cursor_grab(g),
                EngineCommand::SetTextInput(on) => self.windows.set_text_input(on),
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
            let EngineState { world, types, window, input, gamepads, time, resources, scripting } = state;
            let mut cx = Ctx::new(world, &*types, resources, &*time, &*input, &*gamepads, &*window, commands, scripting, events, exec);
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
        self.logger.info("Initializing Graphics Context...");
        match GpuContext::new(&self.windows).await {
            Ok(gpu) => {
                self.renderer.init(&gpu.gpu());
                self.gpu = Some(gpu);
                self.logger.info("Graphics Context Initialized!");
            }
            Err(e) => self.logger.error(&format!("graphics init failed: {e}")),
        }
    }

    /// Reconfigure the surface and tell the renderer.
    fn resize_surface(&mut self, size: (u32, u32)) {
        if let Some(gpu) = &mut self.gpu {
            if gpu.resize(size) {
                self.renderer.resize(&gpu.gpu());
            }
        }
    }

    /// Acquire the surface, let the renderer draw the frame, present. Skipped without a GPU
    /// (headless, failed init). If the frame could not be drawn, its texture changes are put
    /// back so the renderer still sees them once.
    fn render_frame_to_surface(&mut self) {
        let Engine { renderer, gpu, state, logger, .. } = self;
        let Some(gpu) = gpu else { return };
        if !state.resources.contains::<RenderFrame>() {
            return;
        }
        let mut drawn = false;
        let size = (gpu.config.width, gpu.config.height);
        if size.0 > 0 && size.1 > 0 {
            match gpu.surface.get_current_texture() {
                Ok(output) => {
                    let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let frame = state.resources.get::<RenderFrame>().expect("RenderFrame resource");
                    renderer.render(&gpu.gpu(), &view, &frame);
                    drop(frame);
                    drawn = true;
                    let suboptimal = output.suboptimal;
                    output.present();
                    if suboptimal {
                        gpu.reconfigure();
                    }
                }
                // Surface no longer matches the window: reconfigure, next frame renders.
                Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                    gpu.reconfigure();
                    renderer.resize(&gpu.gpu());
                }
                Err(wgpu::SurfaceError::Timeout) => {}
                Err(e) => logger.error(&format!("Render error: {e:?}")),
            }
        }
        if !drawn {
            let changes = state.resources.get_mut::<RenderFrame>().map(|f| std::mem::take(&mut f.textures));
            if let (Some(changes), Some(t)) = (changes, state.resources.get_mut::<Textures>()) {
                t.requeue(changes);
            }
        }
    }

    fn handle_window_event(&mut self, ev: &InputEvent) {
        let InputEventKind::Window(w) = &ev.kind else { return };
        match *w {
            WindowEvent::Resized { width, height } => {
                self.state.window.size = (width, height);
                self.resize_surface((width, height));
            }
            WindowEvent::ScaleFactorChanged { scale } => {
                self.state.window.scale_factor = scale;
            }
            WindowEvent::Focused(f) => {
                self.state.window.focused = f;
            }
            WindowEvent::CloseRequested => {
                self.logger.info("close requested");
                self.status = EngineStatus::Kill;
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
    pub fn frame(&mut self, now_secs: f64){
        if self.status == EngineStatus::Kill && self.shut_down {
            return;
        }
        let steps = self.state.time.advance(now_secs);
        self.events.update_all();
        // Poll first so input/resize events from this frame are handled this frame.
        self.state.input.begin_frame();
        self.event_buf.clear();
        self.windows.poll_events(&mut self.event_buf);
        let events = std::mem::take(&mut self.event_buf);
        for ev in &events {
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

        if !self.started {
            self.started = true;
            self.run_stage(Stage::Startup);
            if self.abort_if_killed() {
                return;
            }
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
                return;
            }
            self.sdl_loop();
        }
        #[cfg(target_family = "wasm")]
        self.wasm_loop_start();
    }
    #[cfg(target_family = "wasm")]
    pub fn wasm_loop_start(mut self){
        // The browser can't block on async GPU init (no pollster here), so init
        // graphics on the JS event loop first and start ticking once it is done.
        wasm_bindgen_futures::spawn_local(async move {
            if !self.windows.is_headless() {
                self.init_graphics().await;
            }
            self.wasm_run_frames();
        });
    }

    /// The rAF loop. `Message::Stop` currently ends the loop on both platforms (Shutdown runs
    /// once); `Start` after `Stop` does not resume, same as native.
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
                        !matches!(e.status, EngineStatus::Kill | EngineStatus::Stopped)
                    }
                    None => false,
                }
            };
            let scheduled = keep_going && match f.borrow().as_ref() {
                Some(cb) => request_animation_frame(cb),
                None => false,
            };
            if !scheduled {
                // Kill, Stop or a failed request: shut down and drop the engine.
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
            // Kill already ran Shutdown inside `frame`; Stop runs it here (once).
            if self.status == EngineStatus::Stopped {
                self.finish();
                break 'main;
            }
            if self.status == EngineStatus::Kill { break 'main; }
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