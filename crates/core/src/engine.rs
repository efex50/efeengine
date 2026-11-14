use std::sync::{Arc, Mutex};

use once_cell::sync::{Lazy, OnceCell};
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use crate::{debug_info, engine::{config::EngineConfig, error::EngineError, events::EventRegistry, input::{GamepadState, InputEvent, InputEventKind, InputState, WindowEvent}, messages::Message, resources::Resources, schedule::{Ctx, EngineCommand, EngineCommands, Schedule, Stage}, time::Time, window::GameWindowManager}, log::{LogCrateLogger, Logger, NewDefaultLogger}, recv_messages, renderer::GraphicsContext, window::WindowManager, world::EngineWorld};
use crate::{global_info, thread_pool::{init_global_pool, init_global_pool_auto}};
pub mod window;
pub mod messages;
pub mod log;
pub mod renderer;
pub mod flags;
pub mod world;
pub mod thread_pool;
pub mod schedule;
pub mod error;
pub mod resources;
pub mod events;
pub mod time;
pub mod config;
pub mod input;

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

pub struct EngineState{
    pub world : EngineWorld,
    pub window: WindowState,
    pub input: InputState,
    pub gamepads: GamepadState,
    pub time: Time,
    pub resources: Resources,
}

pub struct Engine{
    pub windows:GameWindowManager,
    status:EngineStatus,
    pub logger:Box<dyn Logger>,
    pub graphics_context: Option<GraphicsContext>,
    pub state:EngineState,
    /// Input events of the current tick; reused every tick.
    event_buf: Vec<InputEvent>,
    config: EngineConfig,
    events: EventRegistry,
    schedule: Schedule,
    commands: EngineCommands,
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
        let cfg = cfg.merged_with_startup();
        cfg.validate()?;

        let headless = cfg.headless;
        if !headless {
            init_log_crate_logger();

            // init thread pool
            match cfg.threads {
                Some(n) => init_global_pool(n),
                None => init_global_pool_auto(),
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

        let world = EngineWorld::new();

        let state = EngineState{
            world,
            window: WindowState::default(),
            input: InputState::new(),
            gamepads: GamepadState::new(),
            time: Time::new(cfg.fixed_dt),
            resources: Resources::new(),
        };
        let (msg_tx, msg_rx) = flume::unbounded();

        Ok(Self {
            windows: w,
            status:EngineStatus::Uninitialized,
            logger,
            graphics_context:None,
            state,
            event_buf: Vec::new(),
            max_fps: cfg.max_fps.and_then(sanitize_fps),
            config: cfg,
            events: EventRegistry::new(),
            schedule: Schedule::new(),
            commands: EngineCommands::default(),
            started: false,
            shut_down: false,
            msg_tx,
            msg_rx,
            clock: 0.0,
            #[cfg(not(target_family = "wasm"))]
            start: std::time::Instant::now(),
        })
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

    /// Register event type `T` (it is also registered on first send).
    pub fn add_event<T: 'static>(&mut self) -> &mut Self {
        self.events.register::<T>();
        self
    }

    pub fn insert_resource<T: 'static>(&mut self, value: T) -> &mut Self {
        self.state.resources.insert(value);
        self
    }

    pub fn resources(&self) -> &Resources {
        &self.state.resources
    }

    pub fn resources_mut(&mut self) -> &mut Resources {
        &mut self.state.resources
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
                crate::thread_pool::set_global_pool_size(*n);
                let size = crate::thread_pool::global_pool_size();
                self.logger.info(&format!("thread pool size: {size}"));
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

    /// Apply the control requests systems made.
    fn apply_commands(&mut self) {
        let cmds: Vec<EngineCommand> = self.commands.drain().collect();
        for c in cmds {
            match c {
                EngineCommand::Quit => self.status = EngineStatus::Kill,
                EngineCommand::SetTitle(t) => self.windows.set_title(t),
                EngineCommand::SetFrameRate(f) => self.set_frame_rate(f),
                EngineCommand::SetCursorVisible(v) => self.windows.set_cursor_visible(v),
                EngineCommand::SetCursorGrab(g) => self.windows.set_cursor_grab(g),
                EngineCommand::SetTextInput(on) => self.windows.set_text_input(on),
            }
        }
    }

    /// Run every system of `stage` once, then apply the commands they queued.
    fn run_stage(&mut self, stage: Stage) {
        {
            let Engine { schedule, state, events, commands, .. } = self;
            let EngineState { world, window, input, gamepads, time, resources } = state;
            let mut cx = Ctx::new(world, resources, &*time, &*input, &*gamepads, &*window, commands, events);
            schedule.run(stage, &mut cx);
        }
        self.apply_commands();
    }

    fn run_fixed_step(&mut self) {
        self.state.time.set_in_fixed(true);
        self.run_stage(Stage::FixedUpdate);
        self.state.time.set_in_fixed(false);
    }

    // YENİ: Async Grafik Bağlamı Başlatıcı
    pub async fn init_graphics(&mut self) {
        self.logger.info("Initializing Graphics Context...");
        match GraphicsContext::new(&self.windows).await {
            Ok((graphics_context, surface_manager)) => {
                self.graphics_context = Some(graphics_context);
                self.windows.surface_manager = Some(surface_manager);
                self.logger.info("Graphics Context Initialized!");
            }
            // The engine keeps ticking (messages, input) without rendering.
            Err(e) => self.logger.error(&format!("graphics init failed: {e}")),
        }
    }
    pub fn render(&mut self){
        
    }

    /// Reconfigure the window surface to `size` (physical pixels).
    /// No-op without a graphics context/surface, or for a 0x0 (minimized) size.
    fn resize_surface(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if let (Some(context), Some(sm)) = (&self.graphics_context, &mut self.windows.surface_manager) {
            sm.resize(size, &context.device);
        }
    }
    
    /// Engine-side handling of window events (state, surface, close).
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

        // Çizim Mantığı:
        let mut render_result = Ok(());
        if let Some(ref context) = self.graphics_context {
            if let Some(ref mut sm) = self.windows.surface_manager {
                render_result = sm.render(&context.device, &context.queue,&self.state);
            }
        }
        match render_result {
            Ok(()) => {}
            // Surface no longer matches the window: reconfigure, next frame renders.
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                let size = self.windows.size();
                self.resize_surface(size);
            }
            Err(e) => self.logger.error(&format!("Render error: {:?}", e)),
        }
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
        self.sdl_loop();
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
    fn parrrralel(){
        let t = std::thread::available_parallelism().unwrap();
        let h = std::thread::spawn(|| {
            
        });
        println!("{}",t)
    }

}