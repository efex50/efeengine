# efeengine-core

The core library of the efeengine game engine. It provides the `Engine` type (window, logger, wgpu graphics context, a tiny object world), a one-shot startup configuration (`StartupConfig`), a global message queue, a cross-platform, runtime-resizable thread pool (native threads or wasm Web Workers) and the `prelude` that games import. A game is a small crate that depends on this crate and uses the `#[main]` attribute re-exported from `efeengine-macros`. The code is early-stage: the renderer is a hard-coded animated-triangle demo and several modules are stubs (see the last section).

## At a glance
| | |
|---|---|
| Path | `crates/core` |
| Kind | library, `crate-type = ["cdylib", "rlib"]` (cdylib is what wasm-bindgen consumes) |
| Depends on (workspace) | `efeengine-macros` (`crates/core/macros`) |
| Used by | `examples/test_example` (via `efeengine_core::prelude::*`) |
| Key external deps | `wgpu` 27 (graphics; on wasm also the `webgl` feature for the WebGL2 backend), `sdl3` / `sdl3-sys` (native window, events, logging), `flume` (message queue and job channels), `oneshot` (job results), `hecs` (planned for the game world in ROADMAP Phase 3; today only used by an experiment test in `systems/mod.rs`), `serde` (message, event and config serialisation), `serde_json` (dev-dependency, used by tests only), `bytemuck` (uniform buffer), `once_cell` (lazy statics, `OnceCell` config), `pollster` (block on async graphics init, native), `raw-window-handle`, `log` (the facade used by wgpu/naga and `config.rs`). Native only: `env_logger` (prints `log` output to the terminal, `RUST_LOG`, default `warn`). wasm only: `wasm-bindgen`, `web-sys` (direct dependency; features for workers/Blob/URL and for the window backend: Document, Element, HtmlCanvasElement, KeyboardEvent, Navigator, Performance, ...), `js-sys`, `serde-wasm-bindgen`, `console_error_panic_hook`, `console_log` (routes `log` output to the browser console, level `warn`), `gloo-timers`, `ts-rs`, `wasm-bindgen-futures`. |
| Targets | native (SDL3 window + wgpu `Backends::PRIMARY`) and `wasm32-unknown-unknown` (canvas + WebGPU or WebGL2, Web Worker threads) |
| Features | `default = []`; `debug_log` enables the `debug_*` log functions (otherwise they are no-ops); `static` = `sdl3/build-from-source` + `sdl3/static-link` (build and statically link SDL3 instead of using a system one) |

## Layout
```text
crates/core
├── Cargo.toml            package, features, per-target deps, vcpkg metadata (Windows)
├── build.rs              Windows only: finds SDL3 through vcpkg (VCPKGRS_DYNAMIC=1); no-op elsewhere
├── LICENSE
├── macros/               efeengine-macros proc-macro crate (see macros/structure.md)
├── shaders/
│   ├── triangle_anim.wgsl  the only shader actually used (include_str! in renderer)
│   ├── triangle.wgsl       static triangle, not referenced
│   └── video.wgsl          textured quad sampler, not referenced
└── src
    ├── lib.rs            module declarations, glob re-exports, wasm `zort` test export, serde test
    ├── prelude.rs        what games import: Engine, log, messages, `thread`, `main`, wasm_bindgen
    ├── gamerunner.rs     contains only `pub struct GameRunner {}` (empty stub; `lib.rs` declares the module but does not re-export it)
    ├── engine.rs         Engine, EngineStatus, EngineState, WindowState, game loops, WASM_PATH, deprecated `set_wasmjs_path`
    └── engine
        ├── config.rs     StartupConfig / GraphicsBackend, set_startup_config, startup_config (set from JS on the web)
        ├── flags.rs      `enum Flags { Flag(String) }` placeholder, unused
        ├── messages.rs   global flume message queue, `Message` enum, wasm `send_message`
        ├── log.rs        Logger trait, async global logger on a dedicated worker, SDL / console loggers
        ├── world.rs      EngineWorld: Vec<GameObject> (position, rotation)
        ├── systems/mod.rs  GameSystem trait (empty default `&mut self` methods), deprecated add_system(); experiment test with hecs
        ├── renderer/mod.rs GraphicsContext, SurfaceManager (pipeline + uniform), backend selection, unused RendererState
        ├── window
        │   ├── mod.rs        InnerWindow / WindowManager traits, GameWindowManager wrapper
        │   ├── events.rs     WindowEvents (keys, Resized, Focused, CloseRequested, ScaleFactorChanged), KeyCode, KeyMod
        │   ├── sdl_backend.rs  native SdlWindowManager (800x600 resizable window, event pump)
        │   ├── sdl_impls.rs    SDL Scancode/Keycode/Mod -> KeyCode/KeyMod conversions
        │   ├── web_backend.rs  wasm WebWindow (canvas, DOM listeners, canvas size sync)
        │   └── web_impls.rs    DOM KeyboardEvent -> KeyCode/KeyMod conversions, preventDefault policy (wasm)
        ├── thread_pool.rs  platform-neutral API: Jobs, JobHandle, GLOBAL_POOL, spawn_global, worker sizing/resizing, KillGate
        └── thread_pool
            ├── native.rs   std::thread workers sharing one flume channel
            ├── web.rs      Web Worker based pool (same design as native), `worker_loop` / `worker_loop_once` wasm exports
            ├── worker.js   JS bootstrap for a persistent worker (template)
            └── worker_once.js  JS bootstrap for a one-shot worker (template, unused; contains a debug setTimeout/console.log)
```

## Main concepts

### Engine and its loop (`src/engine.rs`)
`Engine` owns `windows: GameWindowManager`, a private `status: EngineStatus`, `logger: Box<dyn Logger>`, `graphics_context: Option<GraphicsContext>` and `state: EngineState`. `EngineState` holds `world: EngineWorld` and `window: WindowState { size, scale_factor, focused }` (physical-pixel size, DPI scale, focus; kept up to date from window events, initial value `(0,0)` / `1.0` / `true` until the first events arrive).

`Engine::new(title)` (a) calls `init_log_crate_logger()`, which installs a `log` crate backend (`env_logger` on native, `console_log` on wasm; harmless if called twice) so wgpu validation errors and config warnings are visible, then (b) starts the global thread pool: `init_global_pool(n)` if the startup config has `threads`, otherwise `init_global_pool_auto()`; reading the startup config also locks it (see config below), (c) on wasm installs `console_error_panic_hook`, (d) creates the window manager (SDL window on native, `WebWindow` on wasm), (e) creates the default logger and an empty world. Status starts as `Uninitialized` and only messages move it out of that state.

`Engine::tick()` does, in order: `windows.poll_events()` (window/input events are turned into messages), `handle_messages()` (drain the global queue, so this frame's input is handled this frame), then, if a graphics context and surface manager exist, `SurfaceManager::render(...)`. A `SurfaceError::Outdated` / `Lost` result reconfigures the surface to the current window size (the next frame renders); other render errors are logged. `Engine::render()` is an empty stub.

`handle_messages()` also applies window events: `Resized` updates `state.window.size` and reconfigures the surface (`resize_surface`, ignored for 0x0 and when there is no graphics yet), `ScaleFactorChanged` and `Focused` update `state.window`, `CloseRequested` logs and sets status `Kill`. Key events are ignored by the engine for now (they stay available to anything that drains the queue).

`game_loop(self)` dispatches by target:
- native: `sdl_loop()` blocks on `init_graphics()` (via `pollster`), then loops `tick()` + `sleep(10 ms)`. It breaks on `Stopped` and on `Kill`, so `game_loop` returns and the engine is dropped normally (no `process::exit`). Status `Running` has no effect on whether ticks happen (ticks always run).
- wasm: `wasm_loop_start()` runs `init_graphics()` inside `wasm_bindgen_futures::spawn_local` (the browser cannot block on async GPU init) and only then starts the `requestAnimationFrame` loop (`wasm_run_frames`, engine in `Rc<RefCell<..>>`), which calls `tick()` until status is `Kill`. If graphics init fails the error is logged and the loop still runs without rendering.

`init_graphics()` calls `GraphicsContext::new(&windows)`, which returns `Result<(GraphicsContext, SurfaceManager), String>`; on success it stores the context in `graphics_context` and the surface manager in `windows.surface_manager`, on error it logs and leaves both `None`.

### Startup config (`src/engine/config.rs`)
`StartupConfig` is a one-shot, optional configuration set before `Engine::new` runs. Fields (JS uses camelCase): `wasmJsPath` (wasm-bindgen glue path used to spawn pool workers), `threads` (pool size; absent = automatic), `canvasId` (canvas element id; absent = `"canvas"`), `externalApi` (reserved, only stored) and `graphicsBackend` (`GraphicsBackend`: `"auto"` | `"webgpu"` | `"webgl"`, default `auto`, web only). On the web the JS page calls `__engine_start({ ... })` (generated by `#[main]`, see `templates/web/main.js`), which deserialises the object with `serde-wasm-bindgen`, calls `set_startup_config`, then runs the game's `main`. `set_startup_config` only accepts the first call (later calls warn and return `Err`), and `startup_config()` falls back to the default and locks it in, so the config has to be set before `Engine::new` (or any other reader) runs. Setting `wasmJsPath` also fills `WASM_PATH`, which the web thread pool reads. The old exported `set_wasmjs_path` (wasm) is `#[deprecated]` and only sets `WASM_PATH`. On native there is no JS, so the config is always the default unless Rust code calls `set_startup_config` itself.

### Messages and the flume queue (`src/engine/messages.rs`)
`MESSAGE_SYSTEM` is a `Lazy` unbounded `flume` channel (sender, receiver) in a static. `send_message(msg)`, `message_sender()` (cloneable sender) and `recv_messages()` (a `Drain` over what is queued now) are the access functions. Producers: the window backends (key, resize, focus, close, scale events), JS (wasm export `send_message`, which deserialises a `JsValue` with `serde-wasm-bindgen`), any thread. Consumer: `Engine::handle_messages()` once per tick. `Message` derives `Serialize/Deserialize`; handling today:
- `Say(s)` logs info, `Log(LogMsg)` logs, `ChangeTitle(s)` sets the window title.
- `Start` / `Stop` / `Kill` set `EngineStatus` to `Running` / `Stopped` / `Kill`.
- `SetThreads(n)` resizes the global thread pool at runtime (`set_global_pool_size`, clamped to >= 1) and logs the new size.
- `SetCanvasId(s)` (wasm only) forwards to the web window; it changes which canvas events and size sync use, but a surface that was already created is not recreated.
- `WindowEvent(..)`: `Resized`, `ScaleFactorChanged`, `Focused` and `CloseRequested` are handled as described in the engine section; key events are accepted and dropped.
- `SetFrameRate(_)` is accepted and ignored; `Null` is a no-op.

### Window backends (`src/engine/window/`)
`InnerWindow` (`HasWindowHandle + HasDisplayHandle`) is the backend trait: `set_title`, `size` (physical pixels), `scale_factor` (default 1.0), `poll_events` (+ `set_canvas_id`, `canvas`, `canvas_id` on wasm). It is deliberately not `Send`/`Sync`: SDL windows and the event pump must stay on the main thread, so `GameWindowManager` (which boxes one backend, holds the `surface_manager`, delegates raw-window-handle traits and implements `WindowManager`: `size`, `scale_factor`, `poll_events`, `set_title`) and `Engine` are not `Send` either. Field order in `GameWindowManager` matters: the surface is dropped before the window it was created from.
- Native `SdlWindowManager`: sets SDL hints/log priority to verbose, creates an 800x600 centred resizable window. On the first poll it sends the initial `Resized` and `ScaleFactorChanged`; afterwards each tick it drains the SDL event pump and sends `Message::WindowEvent(..)` for: `Quit` and window `CloseRequested` -> `CloseRequested`; `FocusGained/FocusLost` -> `Focused`; `PixelSizeChanged` -> `Resized` (pixel size, so it matches the surface on HiDPI); `DisplayChanged` -> `ScaleFactorChanged` when the display scale differs; `KeyDown`/`KeyUp` (converted via `sdl_impls.rs`, `KeyDown` carries `repeat`). Other SDL events (mouse, ...) are silently ignored.
- Web `WebWindow` (`web_backend.rs`, with key conversion in `web_impls.rs`): works with a `<canvas>` looked up by id (startup config `canvasId`, default `"canvas"`, changeable with `SetCanvasId`). It registers DOM listeners that push events into a local queue which `poll_events` forwards as messages: `keydown`/`keyup` on `window` (ignoring IME composition; `preventDefault` only for game keys such as arrows, Space, Tab, paging keys and Backspace, and never with Ctrl/Alt/Meta or in input/textarea/select/contenteditable), `focus`/`blur` on `window` and `visibilitychange` on `document` (a hidden tab counts as unfocused; duplicates are dropped). When focus is lost, a `KeyUp` is synthesised for every held key first, because the browser does not deliver them. The canvas gets `tabindex="0"` if missing and is focused on `pointerdown`. Every `poll_events` call (and once at construction) syncs the canvas drawing-buffer size to CSS size x `devicePixelRatio` and reports changes as `Resized`, and changes of `devicePixelRatio` as `ScaleFactorChanged`; a 0x0 layout is ignored. `size()` returns the canvas buffer size. The raw-window-handle impls are dummy values (wgpu is given the canvas element directly). Listeners are removed on drop.
- `events.rs` defines `WindowEvents`: `KeyUp`/`KeyDown` (with `repeat`), `Resized { width, height }` (physical pixels), `Focused { focused }`, `CloseRequested`, `ScaleFactorChanged { scale }`; all carry a `window_id` (always 0 on web). It also has the `KeyCode` enum (letters, digits, F1-F12, arrows, modifiers, numpad, punctuation, `Unknown`) and the bit-flag `KeyMod` (SDL-compatible values, helpers `contains`, `is_shift/ctrl/alt`, `BitOr`). `PhysicalKey` and `LogicalKey` are aliases of `KeyCode`. There are no mouse events yet.

### Renderer (`src/engine/renderer/mod.rs`)
Two layers exist:
- `GraphicsContext` + `SurfaceManager` (marked "ai gen code, todo rewrite") are what the engine uses. `GraphicsContext::new(&GameWindowManager) -> Result<(GraphicsContext, SurfaceManager), String>` does, in order: create the wgpu `Instance`, create the surface, request an adapter compatible with that surface (the surface comes first because WebGL can only provide an adapter for a given canvas), request a device, and build the `SurfaceManager`. Failures are returned as `Err(String)` instead of panicking. Limits start from `Limits::downlevel_webgl2_defaults()` when the adapter backend is GL and `Limits::default()` otherwise, then `using_resolution(adapter.limits())` so large/HiDPI canvases are not capped at the defaults.
- Backend selection: native uses `Backends::PRIMARY` and `SurfaceTargetUnsafe::from_window`. On the web `web_instance()` decides from the startup config's `graphicsBackend` before the canvas gets any context (a canvas with a WebGPU context can never get a WebGL one): `webgl` -> WebGL2 only; `auto` on desktop Linux (user agent contains "Linux" but not "Android"/"CrOS") -> WebGL2, because Chrome's WebGPU on Linux sets up fine but then fails to present to the canvas; otherwise `new_instance_with_webgpu_detection` with `BROWSER_WEBGPU | GL`, which drops WebGPU if the browser has no working adapter. The web surface is created from the canvas element of the window (`canvas_id`); a missing canvas is an error.
- `SurfaceManager::new` picks an sRGB format and the first present mode, builds a uniform buffer (`Uniforms { position: [f32;2], time: f32 }` plus padding), a bind group and a render pipeline from `shaders/triangle_anim.wgsl` (embedded with `include_str!`, `vs_main`/`fs_main`, 3-vertex draw, `TriangleStrip` topology, back-face culling, no vertex buffers, no depth). `resize(size, device)` reconfigures the surface and is called by the engine on `Resized` and after `Outdated`/`Lost`.
- `render()` returns `Result<(), SurfaceError>`. It clears to black, then for each `GameObject` in `state.world.objects` writes the object's position into the uniform buffer, increments `time` by 0.005 and draws 3 vertices. Since `queue.write_buffer` happens during pass recording, all draws in one frame see the last written uniform values (queue writes are applied before the submit); with two objects both end up at the last object's position.
- `RendererState` (surface/device/queue/config bound to `Arc<GameWindowManager>`, `resize`) is a rewrite in progress and is never constructed.

### Logger (`src/engine/log.rs`)
`Logger` trait (`logger_name`, `info`, `warn`, `error`, `alert`, `log(&LogMsg)`). `LogMsg` is `Info/Warn/Alert/Error(String)` with `get_msg()` (prefixed) and `get_inner()`. The free functions `global_info/warn/alert/error` send commands over a flume channel to a process-wide logger that runs in a dedicated pool worker (`new_worker_global()`), created lazily on first use; `set_global_logger` swaps the logger at runtime; dropping the sender sends `Shutdown`. `debug_info/warn/alert/error` do the same only when this crate's `debug_log` feature is on. Default loggers: native `sdl3::Log` (every level goes to `SDL_Log`, no level distinction), wasm `web::Log` (console functions). `Engine` also holds its own separate `Box<dyn Logger>` used for engine-internal messages.

### World and systems (`world.rs`, `systems/mod.rs`)
`EngineWorld` is a plain `Vec<GameObject>`; `GameObject { position: [f32;2], rotation: f32 }`; `add_object(position)` pushes one with rotation 0. `hecs` is planned to become the game world (ROADMAP Phase 3); today it is a normal dependency that only an experiment test in `systems/mod.rs` uses, and the engine world does not. `GameSystem` has default empty `&mut self` methods (`init`, `engin_update`, `engin_physics_update`, `engin_render_update`, `engin_exit`) and is not called anywhere; `add_system()` is an empty `#[deprecated]` function.

### Thread pool (`src/engine/thread_pool.rs` + `thread_pool/`)
Common API: `Jobs::{Job(Box<dyn FnOnce()+Send>), Kill}`, `JobHandle<T>` (oneshot receiver: `join`, `is_finished`, `try_recv`, `recv_timeout`), `GLOBAL_POOL: Lazy<ThreadPool>`, `spawn_global(f)`, `spawn_job_global(job)`, `new_worker_global()` (a `WorkerHandle` with its own queue and thread, used by the logger).

Sizing: `CORES` is an `AtomicUsize` holding the requested size (0 = nothing requested; the pool then defaults to 2 workers). `init_global_pool(n)` stores `n` (at least 1) and forces the pool. `init_global_pool_auto()` calls `detect_cores()` (native: `available_parallelism`; web: 1 if the page is not `crossOriginIsolated`, otherwise `hardware_concurrency`) and `worker_count(cores)`, which reserves 1/2/3/4 cores for main/render/audio threads depending on the core count (always at least 1 worker). On native the env var `EFE_THREADS` overrides the automatic count (only on the automatic path; an explicit `threads` in the startup config wins over it). The size can change at runtime: `set_global_pool_size(n)` (clamped to >= 1; only records the size if the pool does not exist yet), `global_pool_size()` (current target) and `Message::SetThreads(n)`. Resizing is meant to be called from the main thread.

Shared design (native and web): all resizable workers pull from one shared flume channel. Growing spawns workers; shrinking queues `Jobs::Kill` messages and never blocks, so busy workers finish their current job and then exit. `KillGate` counts pending "please exit" requests: a worker that pops a `Kill` only exits if it can take one request from the gate, so stale kills (a later grow cancelled the shrink) are ignored. Dedicated workers have no gate: any `Kill` stops them.
- native (`native.rs`): N `std::thread`s share the channel; a panicking job is caught so it does not kill the worker; finished workers are joined on the next resize; dropping the pool sends kills and joins workers (and dedicated workers, see gaps).
- web (`web.rs`): each worker is a module `Worker` created from a Blob URL built from `worker.js` with `*project_path*` replaced by the wasm-bindgen JS glue URL (`WASM_PATH`, filled from the startup config's `wasmJsPath` or the deprecated `set_wasmjs_path`). The Worker is sent `{module, memory, id, ptr}`; the JS does `initSync({module, memory})` (shared memory) and calls the exported `worker_loop(ptr)`, which blocks on the flume receiver and runs jobs until it takes a `Kill`. Resizing is implemented and non-blocking: exited workers are only `terminate()`d (dropped) on the next resize call. Dedicated workers use ids from 31000. `worker_loop_once` + `worker_once.js` implement a one-job worker type (`WorkerType::Once`) that nothing selects yet.

### Frame flow
1. Platform callback: native loop iteration (10 ms sleep) or `requestAnimationFrame`.
2. `tick()` -> `windows.poll_events()`: SDL events / DOM listener queue (plus the web canvas size sync) become `Message`s.
3. `handle_messages()`: drain the queue (the events just produced, messages from JS, other threads, logger requests); update status / title / logger / `state.window` / surface size.
4. If graphics are initialised: `SurfaceManager::render` draws one triangle per `GameObject`; `Outdated`/`Lost` triggers a surface reconfigure.

### Prelude (`src/prelude.rs`)
Re-exports `Engine`, `EngineStatus`, `log::*`, `engine::*`, `config::{StartupConfig, set_startup_config, startup_config}`, `messages::*`, a `thread` module (`GLOBAL_POOL`, `Jobs`, `init_global_pool`, `set_global_pool_size`, `global_pool_size`, `spawn_global`, `spawn_job_global`; on wasm also `gloo_timers::callback`), the `main` macro, and on wasm the `wasm_bindgen` crate (needed by the macro expansion).

## Public API
What a game uses (`use efeengine_core::prelude::*;`):
- `#[main]` attribute (see `macros/structure.md`).
- `Engine::new(title)`, `engine.state.world.add_object([x, y])`, `engine.windows` (public field), `engine.logger`, `engine.game_loop()`.
- Logging: `global_info/warn/alert/error`, `debug_*`, `set_global_logger`, `Logger`, `LogMsg`.
- Messages: `send_message`, `message_sender`, `recv_messages`, `Message` variants `SetFrameRate(f32)`, `WindowEvent(WindowEvents)`, `Start`, `Stop`, `Kill`, `Log(LogMsg)`, `Say(String)`, `ChangeTitle(String)`, `SetThreads(usize)`, `SetCanvasId(String)` (wasm), `Null`.
- Threads: `thread::spawn_global`, `thread::spawn_job_global`, `thread::Jobs`, `thread::GLOBAL_POOL`, `thread::init_global_pool`, `thread::set_global_pool_size`, `thread::global_pool_size` (re-exported at `efeengine_core::thread` through the prelude glob).
- Startup config: `StartupConfig`, `GraphicsBackend` (via `config::`), `set_startup_config`, `startup_config`.
- Engine state: `engine.state.window` (`WindowState { size, scale_factor, focused }`).
- Other modules reachable through `engine::*`: `window` (`events::{WindowEvents, KeyCode, KeyMod}`), `renderer`, `world`, `systems`, `flags`, `config`, `WASM_PATH`.
- Exports to JS via `wasm_bindgen` (wasm32 only): `send_message(JsValue)`, `set_wasmjs_path(String)` (deprecated, use the config), `__init_thread_pool(usize)`, `worker_loop(ptr)`, `worker_loop_once(ptr)` (called from the worker scripts), `zort()` (leftover test export returning a console-logging closure; also logs "safasfi"). The `#[main]` macro adds `__engine_start(config)` (takes the optional startup config object, then runs the game's `main`) and an empty `start` function in the game crate; the page calls `__engine_start({...})` after `init()` (see `templates/web/main.js`).

Example game (`examples/test_example/src/lib.rs`): `#[main] fn main() { let mut w = Engine::new("hello"); w.state.world.add_object([0.0, 0.]); global_info("sa"); w.game_loop(); }`. The example crate is both a `cdylib` (wasm) and a `native` binary pointing at the same `lib.rs`.

## Platform notes
- Workspace root `.cargo/config.toml` sets, for `wasm32-unknown-unknown`, rustflags: `+atomics`, `+bulk-memory`, `--shared-memory`, `--import-memory`, `--max-memory=2147483648` (2 GB), and exports of `__wasm_init_tls`, `__tls_size`, `__tls_align`, `__tls_base`, `__heap_base`, `__data_end` (needed by the wasm-bindgen threads transform). Atomics require a nightly toolchain with `-Z build-std` (std must be rebuilt with atomics); the page must be cross-origin isolated (COOP/COEP) for `SharedArrayBuffer`, otherwise the pool degrades to a single worker.
- Native needs SDL3. By default it links a system SDL3 (on Windows through vcpkg: `build.rs` uses the `vcpkg` crate, `VCPKGRS_DYNAMIC=1`; `[package.metadata.vcpkg]` has a hard-coded Scoop vcpkg root `C:/Users/efe/scoop/apps/vcpkg/current`). The `static` feature builds SDL3 from source and links it statically.
- wgpu backends: native `PRIMARY` (Vulkan/Metal/DX12); wasm picks WebGPU or WebGL2 per `graphicsBackend` (see Renderer; `auto` means WebGL2 on desktop Linux). The wasm surface is found by the canvas id from the startup config (default `"canvas"`). The unused `RendererState` still hard-codes `GL | BROWSER_WEBGPU` on wasm.
- Native `Kill` (or a close request) breaks the loop so `game_loop` returns; wasm just stops requesting frames.
- Several log strings and comments are in Turkish.

## Status and known gaps
- Wasm rendering depends on the page: it needs a `<canvas>` with the configured id, and WebGPU-vs-WebGL2 selection is a heuristic (user agent for the Linux case). If graphics init fails the engine keeps running without drawing. The web raw-window-handle impls are dummies.
- `Message::SetFrameRate` is ignored; key events are delivered as messages but the engine does not consume them; there are no mouse/touch/gamepad events; `Engine::render()` is empty.
- `StartupConfig.external_api` is stored but unused; `graphicsBackend` is ignored on native; `SetCanvasId` after graphics init does not recreate the surface.
- `Stopped` and `Kill` end the native loop, but `Running` vs other statuses is not otherwise used; the `Ready` / `Initializing` states are never set.
- Renderer is demo code: one hard-coded shader, one uniform buffer rewritten per object inside a single pass, so multiple objects overlap at the last object's position. `RendererState` is an unfinished rewrite, never used. `shaders/triangle.wgsl` and `shaders/video.wgsl` are unreferenced.
- World is a plain `Vec`; `hecs` is not integrated yet (planned for Phase 3); `GameSystem` and `add_system` are unused stubs (`systems/mod.rs`); `Flags` and `gamerunner::GameRunner` are empty placeholders.
- Thread pool: errors from `send` are ignored throughout; dropping the native pool joins dedicated workers whose channels may never close (deadlock risk, noted in a comment); resizing is expected from a single (main) thread; `WorkerHandleOnce` and `worker_loop_once` are unused; the Windows vcpkg root is machine-specific.
- `JobHandle::join` on wasm blocks the calling thread (warns "experimental"); the wasm pool needs the wasm-bindgen glue path (`wasmJsPath` in the startup config, or the deprecated `set_wasmjs_path`) to have been set first (`WASM_PATH.get().unwrap()` panics otherwise).
- Two logging paths exist: the engine's own `Logger` (`global_info` etc.) and the `log` crate backend above (for third-party crates). Native `Logger` levels map to plain `SDL_Log` (no severity); the global logger permanently occupies one dedicated worker thread.
- Tests: the thread pool has real native tests (grow/shrink, jobs across resizes, global size API); the rest are mostly experiments (hecs prints, mpsc, serde printing) with few assertions. `serde_json` is a dev-dependency used only by the serde test.
- Leftover/debug code: wasm `zort()` export (`lib.rs`), `worker_once.js` `setTimeout` console.log.
