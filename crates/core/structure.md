# efeengine-core

The core library of the efeengine game engine. It provides the `Engine` type (window, logger, wgpu graphics context, a tiny object world), a global message queue, a cross-platform thread pool (native threads or wasm Web Workers) and the `prelude` that games import. A game is a small crate that depends on this crate and uses the `#[main]` attribute re-exported from `efeengine-macros`. The code is early-stage: the renderer is a hard-coded animated-triangle demo and several modules are stubs (see the last section).

## At a glance
| | |
|---|---|
| Path | `crates/core` |
| Kind | library, `crate-type = ["cdylib", "rlib"]` (cdylib is what wasm-bindgen consumes) |
| Depends on (workspace) | `efeengine-macros` (`crates/core/macros`) |
| Used by | `examples/test_example` (via `efeengine_core::prelude::*`) |
| Key external deps | `wgpu` 27 (graphics), `sdl3` / `sdl3-sys` (native window, events, logging), `flume` (message queue and job channels), `oneshot` (job results), `hecs` (only used in tests, not wired into the engine), `serde` / `serde_json` (message and event serialisation), `bytemuck` (uniform buffer), `once_cell` (lazy statics), `pollster` (block on async graphics init), `raw-window-handle`, `env_logger` (declared, not used in src). wasm only: `wasm-bindgen`, `web-sys`, `js-sys`, `serde-wasm-bindgen`, `console_error_panic_hook`, `gloo-timers`, `ts-rs`, `console_log`, `wasm-bindgen-futures`. Dev only: `pyo3` (used only by an experiment test in `systems/mod.rs`) |
| Targets | native (SDL3 window + wgpu `Backends::PRIMARY`) and `wasm32-unknown-unknown` (canvas + WebGL/WebGPU, Web Worker threads) |
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
    ├── engine.rs         Engine, EngineStatus, EngineState, game loops, WASM_PATH
    └── engine
        ├── flags.rs      `enum Flags { Flag(String) }` placeholder, unused
        ├── messages.rs   global flume message queue, `Message` enum, wasm `send_message`
        ├── log.rs        Logger trait, async global logger on a dedicated worker, SDL / console loggers
        ├── world.rs      EngineWorld: Vec<GameObject> (position, rotation)
        ├── systems/mod.rs  GameSystem trait (empty default methods), deprecated add_system(); tests with pyo3 and hecs
        ├── renderer/mod.rs GraphicsContext, SurfaceManager (pipeline + uniform), unused RendererState
        ├── window
        │   ├── mod.rs        InnerWindow / WindowManager traits, GameWindowManager wrapper
        │   ├── events.rs     WindowEvents (KeyUp/KeyDown), KeyCode, KeyMod
        │   ├── sdl_backend.rs  native SdlWindowManager (800x600 resizable window, event pump)
        │   ├── sdl_impls.rs    SDL Scancode/Keycode/Mod -> KeyCode/KeyMod conversions
        │   └── web_backend.rs  wasm WebWindow (mostly stubs)
        ├── thread_pool.rs  platform-neutral API: Jobs, JobHandle, GLOBAL_POOL, spawn_global, worker sizing
        └── thread_pool
            ├── native.rs   std::thread workers sharing one flume receiver
            ├── web.rs      Web Worker based pool, `worker_loop` / `worker_loop_once` wasm exports
            ├── worker.js   JS bootstrap for a persistent worker (template)
            └── worker_once.js  JS bootstrap for a one-shot worker (template, unused; contains a debug setTimeout/console.log)
```

## Main concepts

### Engine and its loop (`src/engine.rs`)
`Engine` owns `windows: GameWindowManager`, a private `status: EngineStatus`, `logger: Box<dyn Logger>`, `graphics_context: Option<GraphicsContext>` and `state: EngineState` (which holds `world: EngineWorld`).

`Engine::new(title)` (a) starts the global thread pool via `init_global_pool_auto()`, (b) on wasm installs `console_error_panic_hook`, (c) creates the window manager (SDL window on native, `WebWindow` on wasm), (d) creates the default logger and an empty world. Status starts as `Uninitialized` and nothing in the code moves it out of that state except messages.

`Engine::tick()` does, in order: `handle_messages()` (drain the global queue), `windows.poll_events()`, then, if a graphics context and surface manager exist, `SurfaceManager::render(...)`; render errors are only logged (no surface-lost recovery). `Engine::render()` is an empty stub.

`game_loop(self)` dispatches by target:
- native: `sdl_loop()` blocks on `init_graphics()` (via `pollster`), then loops `tick()` + `sleep(10 ms)`. It breaks on `Stopped`; on `Kill` it calls `std::process::exit(0)`. Status `Running` has no effect on whether ticks happen (ticks always run).
- wasm: `wasm_loop_start()` wraps the engine in `Rc<RefCell<..>>` and drives `tick()` from `requestAnimationFrame` until status is `Kill`. It does not call `init_graphics()`, so on wasm there is no graphics context and nothing is drawn (see gaps).

`init_graphics()` creates `GraphicsContext` (wgpu instance, adapter, device, queue) and then a `SurfaceManager` bound to the window, stored in `windows.surface_manager`.

### Messages and the flume queue (`src/engine/messages.rs`)
`MESSAGE_SYSTEM` is a `Lazy` unbounded `flume` channel (sender, receiver) in a static. `send_message(msg)`, `message_sender()` (cloneable sender) and `recv_messages()` (a `Drain` over what is queued now) are the access functions. Producers: the SDL backend (key events, quit), JS (wasm export `send_message`, which deserialises a `JsValue` with `serde-wasm-bindgen`), any thread. Consumer: `Engine::handle_messages()` once per tick. `Message` derives `Serialize/Deserialize`; handling today:
- `Say(s)` logs info, `Log(LogMsg)` logs, `ChangeTitle(s)` sets the window title.
- `Start` / `Stop` / `Kill` set `EngineStatus` to `Running` / `Stopped` / `Kill`.
- `SetCanvasId(s)` (wasm only) forwards to the web window.
- `SetFrameRate(_)` is accepted and ignored; `WindowEvent(_)` only emits a `"Todo window event"` warning; `Null` is a no-op.

### Window backends (`src/engine/window/`)
`InnerWindow` (`Send + Sync + HasWindowHandle + HasDisplayHandle`) is the backend trait: `set_title`, `size`, `poll_events` (+ `set_canvas_id` on wasm). `GameWindowManager` boxes one backend, holds the `surface_manager`, delegates raw-window-handle traits, and implements `WindowManager` (`size`, `poll_events`, `set_title`). It has `unsafe impl Send/Sync`.
- Native `SdlWindowManager`: sets SDL hints/log priority to verbose, creates an 800x600 centred resizable window, polls the SDL event pump each tick. `Quit` sends `Message::Kill`; `KeyDown`/`KeyUp` are converted (via `sdl_impls.rs`) into `Message::WindowEvent(WindowEvents::KeyDown/KeyUp {...})`; every other SDL event is logged as an alert (`"event got! ..."`), including resize and mouse events.
- Web `WebWindow`: stores canvas id and title; `size()` returns `(0,0)` and `poll_events()` does nothing (both log a TODO); handle impls return dummy values.
- `events.rs` defines `WindowEvents` (only `KeyUp`, `KeyDown`), the `KeyCode` enum (letters, digits, F1-F12, arrows, modifiers, numpad, punctuation, `Unknown`), and the bit-flag `KeyMod` (SDL-compatible values, helpers `contains`, `is_shift/ctrl/alt`, `BitOr`). `PhysicalKey` and `LogicalKey` are aliases of `KeyCode`. An empty `WindowEvent` enum also exists, unused.

### Renderer (`src/engine/renderer/mod.rs`)
Two layers exist:
- `GraphicsContext` + `SurfaceManager` (marked "ai gen code, todo rewrite") are what the engine uses. `GraphicsContext::new()` requests an adapter with no compatible surface (`Backends::PRIMARY`) and a device (WebGL2 downlevel limits on wasm). `SurfaceManager::new` creates the surface (wasm: canvas element with id `"canvas"` looked up in the DOM; native: `SurfaceTargetUnsafe::from_window`), picks an sRGB format and first present mode, builds a uniform buffer (`Uniforms { position: [f32;2], time: f32 }`), a bind group and a render pipeline from `shaders/triangle_anim.wgsl` (embedded with `include_str!`, `vs_main`/`fs_main`, 3-vertex draw, `TriangleStrip` topology, back-face culling, no vertex buffers, no depth).
- `render()` clears to black, then for each `GameObject` in `state.world.objects` writes the object's position into the uniform buffer, increments `time` by 0.005 and draws 3 vertices. Since `queue.write_buffer` happens during pass recording, all draws in one frame see the last written uniform values (queue writes are applied before the submit); with two objects both end up at the last object's position. `SurfaceManager::resize` exists but nothing calls it.
- `RendererState` (surface/device/queue/config bound to `Arc<GameWindowManager>`, `resize`) is a rewrite in progress and is never constructed.

### Logger (`src/engine/log.rs`)
`Logger` trait (`logger_name`, `info`, `warn`, `error`, `alert`, `log(&LogMsg)`). `LogMsg` is `Info/Warn/Alert/Error(String)` with `get_msg()` (prefixed) and `get_inner()`. The free functions `global_info/warn/alert/error` send commands over a flume channel to a process-wide logger that runs in a dedicated pool worker (`new_worker_global()`), created lazily on first use; `set_global_logger` swaps the logger at runtime; dropping the sender sends `Shutdown`. `debug_info/warn/alert/error` do the same only when this crate's `debug_log` feature is on. Default loggers: native `sdl3::Log` (every level goes to `SDL_Log`, no level distinction), wasm `web::Log` (console functions). `Engine` also holds its own separate `Box<dyn Logger>` used for engine-internal messages.

### World and systems (`world.rs`, `systems/mod.rs`)
`EngineWorld` is a plain `Vec<GameObject>`; `GameObject { position: [f32;2], rotation: f32 }`; `add_object(position)` pushes one with rotation 0. `hecs` is a dependency but only the unit tests in `systems/mod.rs` use it, the engine world does not. `GameSystem` has default empty static functions (`init`, `_update`, `_physics_update`, `_render_update`, `_exit`) and is not called anywhere; `add_system()` is an empty `#[deprecated]` function.

### Thread pool (`src/engine/thread_pool.rs` + `thread_pool/`)
Common API: `Jobs::{Job(Box<dyn FnOnce()+Send>), Kill}`, `JobHandle<T>` (oneshot receiver: `join`, `is_finished`, `try_recv`, `recv_timeout`), `GLOBAL_POOL: Lazy<ThreadPool>`, `spawn_global(f)`, `spawn_job_global(job)`, `new_worker_global()` (a `WorkerHandle` with its own queue and thread, used by the logger), `init_global_pool(n)` (stores `n` in a `static mut CORES` and forces the pool), `init_global_pool_auto()`, and `worker_count(cores)` which reserves 1/2/3/4 cores for main/render/audio threads depending on the core count (always at least 1 worker). Native override: env var `EFE_THREADS`. If the pool is touched before init it defaults to 2 threads.
- native (`native.rs`): N `std::thread`s share one `Arc<Mutex<flume::Receiver<Jobs>>>`; dedicated workers get their own channel; drop closes the sender and joins. `change_worker_amount` can grow but panics on shrink.
- web (`web.rs`): each worker is a module `Worker` created from a Blob URL built from `worker.js` with `*project_path*` replaced by the wasm-bindgen JS glue URL (`WASM_PATH`, set from JS with the exported `set_wasmjs_path`). The Worker is sent `{module, memory, id, ptr}`; the JS does `initSync({module, memory})` (shared memory) and calls the exported `worker_loop(ptr)`, which blocks on the flume receiver and runs jobs until `Kill`. Dedicated workers use ids from 31000. `detect_cores()` returns 1 if the page is not `crossOriginIsolated`, otherwise `hardware_concurrency`. `worker_loop_once` + `worker_once.js` implement a one-job worker type (`WorkerType::Once`) that nothing selects yet.

### Frame flow
1. Platform callback: native loop iteration (10 ms sleep) or `requestAnimationFrame`.
2. `tick()` -> `handle_messages()`: drain the queue (messages from JS, SDL events converted earlier, other threads, logger requests); update status / title / logger.
3. `windows.poll_events()`: SDL events become `Message`s which are handled on the next tick (one tick latency).
4. If graphics are initialised: `SurfaceManager::render` draws one triangle per `GameObject`.

### Prelude (`src/prelude.rs`)
Re-exports `Engine`, `EngineStatus`, `log::*`, `engine::*`, `messages::*`, a `thread` module (`GLOBAL_POOL`, `Jobs`, `init_global_pool`, `spawn_global`, `spawn_job_global`; on wasm also `gloo_timers::callback`), the `main` macro, and on wasm the `wasm_bindgen` crate (needed by the macro expansion).

## Public API
What a game uses (`use efeengine_core::prelude::*;`):
- `#[main]` attribute (see `macros/structure.md`).
- `Engine::new(title)`, `engine.state.world.add_object([x, y])`, `engine.windows` (public field), `engine.logger`, `engine.game_loop()`.
- Logging: `global_info/warn/alert/error`, `debug_*`, `set_global_logger`, `Logger`, `LogMsg`.
- Messages: `send_message`, `message_sender`, `recv_messages`, `Message` variants `SetFrameRate(f32)`, `WindowEvent(WindowEvents)`, `Start`, `Stop`, `Kill`, `Log(LogMsg)`, `Say(String)`, `ChangeTitle(String)`, `SetCanvasId(String)` (wasm), `Null`.
- Threads: `thread::spawn_global`, `thread::spawn_job_global`, `thread::Jobs`, `thread::GLOBAL_POOL`, `thread::init_global_pool` (re-exported at `efeengine_core::thread` through the prelude glob).
- Other modules reachable through `engine::*`: `window` (`events::{WindowEvents, KeyCode, KeyMod}`), `renderer`, `world`, `systems`, `flags`, `WASM_PATH`.
- Exports to JS via `wasm_bindgen` (wasm32 only): `send_message(JsValue)`, `set_wasmjs_path(String)`, `__init_thread_pool(usize)`, `worker_loop(ptr)`, `worker_loop_once(ptr)` (called from the worker scripts), `zort()` (leftover test export returning a console-logging closure; also logs "safasfi"). The `#[main]` macro adds `__engine_start()` and a `start` function in the game crate.

Example game (`examples/test_example/src/lib.rs`): `#[main] fn main() { let mut w = Engine::new("hello"); w.state.world.add_object([0.5, 0.]); w.state.world.add_object([-0.5, 0.]); global_info("sa"); w.game_loop(); }`. The example crate is both a `cdylib` (wasm) and a `native` binary pointing at the same `lib.rs`.

## Platform notes
- Workspace root `.cargo/config.toml` sets, for `wasm32-unknown-unknown`, rustflags: `+atomics`, `+bulk-memory`, `--shared-memory`, `--import-memory`, `--max-memory=2147483648` (2 GB), and exports of `__wasm_init_tls`, `__tls_size`, `__tls_align`, `__tls_base`, `__heap_base`, `__data_end` (needed by the wasm-bindgen threads transform). Atomics require a nightly toolchain with `-Z build-std` (std must be rebuilt with atomics); the page must be cross-origin isolated (COOP/COEP) for `SharedArrayBuffer`, otherwise the pool degrades to a single worker.
- Native needs SDL3. By default it links a system SDL3 (on Windows through vcpkg: `build.rs` uses the `vcpkg` crate, `VCPKGRS_DYNAMIC=1`; `[package.metadata.vcpkg]` has a hard-coded Scoop vcpkg root `C:/Users/efe/scoop/apps/vcpkg/current`). The `static` feature builds SDL3 from source and links it statically.
- wgpu backends: native `PRIMARY` (Vulkan/Metal/DX12); wasm `GL | BROWSER_WEBGPU` in the unused `RendererState`, and the used `GraphicsContext` instance also uses `PRIMARY` (on wasm that selects WebGPU only). The wasm surface is found by DOM id `"canvas"`.
- Native `Kill` terminates via `std::process::exit(0)`; wasm just stops requesting frames.
- Several log strings and comments are in Turkish.

## Status and known gaps
- Wasm renders nothing: `wasm_loop_start` never calls `init_graphics()` (`src/engine.rs:271`), so `graphics_context` stays `None`; `WebWindow::size()` returns `(0,0)` and `poll_events()` is a TODO (`window/web_backend.rs:217-227`); `GraphicsContext` uses `Backends::PRIMARY` (`renderer/mod.rs:147`), and the canvas id is hard-coded to `"canvas"` instead of using `SetCanvasId` (`renderer/mod.rs:190`).
- `Message::WindowEvent` is not handled, only warned (`engine.rs:222`); `Message::SetFrameRate` is ignored (`engine.rs:213`); `Engine::render()` is empty (`engine.rs:243`).
- Native loop never resizes the surface (`SurfaceManager::resize` unused); SDL resize/mouse events are only logged as alerts (`sdl_backend.rs:163`); only keyboard events exist in `WindowEvents`.
- `Stopped` breaks the native loop, but `Running` vs other statuses is not otherwise used; the `Ready` / `Initializing` states are never set.
- Renderer is demo code: one hard-coded shader, one uniform buffer rewritten per object inside a single pass, so multiple objects overlap at the last object's position (`renderer/mod.rs:361-372`). `RendererState` is an unfinished rewrite, never used. `shaders/triangle.wgsl` and `shaders/video.wgsl` are unreferenced.
- World is a plain `Vec`; `hecs` is not integrated; `GameSystem` and `add_system` are unused stubs (`systems/mod.rs`); `Flags` and `gamerunner::GameRunner` are empty placeholders.
- Thread pool: errors from `send` are ignored throughout; native `Worker` ignores `Jobs::Kill` (it does not break, `native.rs`); dropping the native pool joins dedicated workers whose channels may never close (deadlock risk, noted in a comment); `change_worker_amount` panics on shrink (native) and is `todo!()` on web (`web.rs:359`); `WorkerHandleOnce` and `worker_loop_once` are unused; `static mut CORES` is accessed with `unsafe`; the Windows vcpkg root is machine-specific.
- `JobHandle::join` on wasm blocks the calling thread (warns "experimental"); the wasm pool needs `set_wasmjs_path` to have been called first (`WASM_PATH.get().unwrap()` panics otherwise; the setter ignores its `Result`).
- Logger: all native levels map to plain `SDL_Log` (no severity); the global logger permanently occupies one dedicated worker thread.
- Tests are mostly experiments (pyo3 sleeps, hecs prints, mpsc, serde printing) with few assertions; `pyo3` as a dev-dependency requires a Python install to build the tests. `env_logger`, `bytemuck` derive features and `serde_json` are declared; `env_logger` is unused.
- Leftover/debug code: wasm `zort()` export (`lib.rs:27`), `worker_once.js` `setTimeout` console.log.
