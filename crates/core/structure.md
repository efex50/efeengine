# efeengine-core

The core library of the efeengine game engine. It provides `Engine` (windowing, logger, wgpu or native SDL_Renderer graphics, an `efecs` (`hecs`) `World` with built-in components, a `TypeRegistry` of script-visible layouts, deferred world/engine `Commands`, per-frame input state and filters, resources, typed events, scenes/prefabs/editor reflection, and a hybrid sequential/parallel system schedule), `EngineConfig` and one-shot `StartupConfig`, an engine-owned message queue plus a global queue for JS and workers, a platform-neutral input system (keyboard, mouse, touch, IME text editing, gamepads, window events), a fixed-timestep frame loop, a headless mode for tests and API/schema export, a cross-platform runtime-resizable thread pool (native threads or wasm Web Workers), the `efevm` scripting/mod runtime (with optional Cranelift JIT and `.efa`/`.wasm` loading), and the `prelude` that games import. A game crate depends on `efeengine-core` and uses `#[main]` from `efeengine-macros`. Rendering splits extraction from drawing: `Stage::RenderExtract` builds a plain-data `RenderFrame` (`Frame2D` / `Frame3D` per window), drawn by a `Renderer2D` (default `SpriteRenderer`) and an optional `Renderer3D`, or a `CustomRenderer`, followed by `RenderOverlay` and `Draw2DOverlay` passes.

## At a glance
| | |
|---|---|
| Path | `crates/core` |
| Kind | library, `crate-type = ["cdylib", "rlib"]` (`cdylib` is consumed by `wasm-bindgen`), edition 2024 |
| Depends on (workspace) | `efeengine-macros` (`crates/core/macros`), `efecs` (`crates/efecs`, forked `hecs`, imported and re-exported as `hecs`), `efevm` (`crates/efevm`, bytecode VM, `StructDesc`/`FieldDesc`/`Prim`, `HostRegistry`, `ApiIndex`), `efemod` (`crates/efemod`, reader only on the engine path; `[dev-dependencies]` enables `write` for tests), `efa` (`crates/efa`, optional, enabled by default via `efa` feature), `efevm-jit` (`crates/efevm-jit`, native optional via `jit`), `efe-wasm` (`crates/efe-wasm`, native optional via `wasm-mods`) |
| Used by | Workspace crates: `crates/editor-link`, `crates/ui` (`efeengine-ui`), `crates/world-ui` (`efeengine-world-ui`). Examples: `coin_dash`, `coin_dash_mods`, `coin_dash_scripts`, `coin_dash_wasm`, `custom_renderer`, `editor_demo`, `input_test`, `multi_window`, `script_demo`, `test_example`, `ui_demo` (plus `xtask`) |
| Key external deps | Cross-platform: `wgpu` 30.0.2 (plus `webgl` on `wasm32`), `glam` 0.34.3 (`bytemuck`), `bytemuck` 1.25.2 (`derive`), `flume` 0.12.0 (`default-features = false`, `spin`), `oneshot` 0.2.1 (`std`), `thiserror` 2.0.21, `serde` 1.0.229 (`derive`), `serde_json` 1.0.145 (component schema export and tests), `ron` 0.12.3 (scenes and prefabs), `blake3` 1.8.7 (`std`), `miniz_oxide` 0.9.1 & `crc32fast` 1.5.5 (PNG decoding), `once_cell` 1.21.4, `raw-window-handle` 0.6.2, `log` 0.4.35. Native only: `sdl3` 0.22.1 (`raw-window-handle`), `sdl3-sys` 0.7.3, `pollster` 1.0.2, `env_logger` 0.11.11; Windows build-dep `vcpkg` 0.2.15. Wasm32 only: `wasm-bindgen` 0.2.130, `wasm-bindgen-futures` 0.4.80, `serde-wasm-bindgen` 0.6.5, `web-sys` 0.3.107, `js-sys` 0.3.107, `console_error_panic_hook` 0.1.7, `console_log` 1.1.0, `gloo-timers` 0.4.1, `ts-rs` 12.0.1 |
| Targets | native (SDL3 window + wgpu or `SDL_Renderer`) and `wasm32-unknown-unknown` (DOM canvas + WebGPU/WebGL2, Web Worker pool) |
| Features | `default = ["efa"]`; `efa` (enables `dep:efa` for runtime `.efa` and folder-mod compilation); `wasm-mods` (`efa`, `efa/wasm`, `dep:efe-wasm`; native dev-time `.wasm` mod translation); `jit` (`dep:efevm-jit`; native Cranelift script JIT); `debug_log` (enables `debug_*` logging helpers); `static` (`sdl3/build-from-source`, `sdl3/static-link`) |
| Tests | Unit tests across `src/` (`lib.rs`, `engine.rs`, `config`, `events`, `resources`, `time`, `types`, `components`, `schedule`, `parallel`, `log`, `windows`, `thread_pool`, `input/{keys,state,gamepad}`, `window/{headless,sdl_impls}`, `render/{draw2d,draw2d_wgpu,debug_draw,extra_surface,gpu,offscreen,png,sdl_draw2d,sprite_renderer}`, `scripting/{mod,cache,load}`), doctests (`engine.rs`, `types.rs`, `window/mod.rs`), and 30 integration test files in `tests/` (see Layout) |

## Layout
```text
crates/core
├── Cargo.toml            package metadata, features, per-target deps, Windows vcpkg metadata
├── build.rs              Windows only: locates SDL3 via vcpkg (`VCPKGRS_DYNAMIC=1`); no-op elsewhere
├── LICENSE
├── structure.md          this file
├── macros/               `efeengine-macros` proc-macro crate (see `macros/structure.md`)
├── shaders/
│   └── video.wgsl        textured quad sampler (unused placeholder)
├── src
│   ├── lib.rs            `extern crate self as efeengine_core`, domain facades (`math`, `render`, `input`, `scripting`, `mods`, `cache`, `ecs`, `window`, `clipboard`, `thread`, `log`), crate re-exports, wasm `zort` export, `Message` JSON shape test
│   ├── prelude.rs        everyday imports (`Engine`, `EngineConfig`, `Stage`, `Ctx`, `ParCtx`, `Plugin`, built-in components, `#[main]`, `#[script_component]`, `#[derive(Component)]`, `#[derive(ScriptType)]`, `hecs`, `glam`, input/log helpers, `wasm_bindgen` on wasm)
│   ├── engine.rs         `Engine`, `EngineState`, `Lifecycle`, window/graphics/overlay hooks, `frame`/`tick`/`run_frames`/`game_loop`, API/schema export (`EFE_API_EXPORT`, `--efe-api-export`), `WASM_PATH`
│   └── engine
│       ├── config.rs     `EngineConfig`, `StartupConfig`, `Threads`, `GpuMode`, `GraphicsMode`, `GraphicsBackend`, `ScriptBackend`, `JitConfig`, `set_startup_config`, `startup_config`
│       ├── error.rs      `EngineError` (`Window`, `Graphics`, `Config`, `ThreadPool`), `EngineResult`
│       ├── time.rs       `Time`: clamped delta (`MAX_DELTA` = 0.25 s), elapsed, frame counter, fixed-step accumulator & alpha
│       ├── resources.rs  `Resources` type map with per-resource change ticks (`Res`, `ResMut` stamps on `DerefMut`)
│       ├── events.rs     double-buffered `Events<T>` (`send`, `extend_readable`, `update`) and type-erased `EventRegistry`
│       ├── parallel.rs   `Executor` (pool-backed parallel batch runner), `Access`, `ParCtx` (checked queries, change ticks, `par_for_each`, `res_changed`, `game_input`), `ParCommands`
│       ├── schedule.rs   `Stage` (7 stages), `Schedule` (lazy sort by `(order, idx)` + parallel batching), `SystemEntry` (`.order()`, `.name()`, `.enabled()`, `.run_if()`), `RunCtx`, `Ctx`, `ScheduleCommands`, `ScheduleError`
│       ├── order.rs      stage order constants (`INPUT` -1000, `EARLY`, `SCRIPTS`, `USER` 0, `PHYSICS` 500, `TRANSFORM` 900, `LATE`)
│       ├── plugin.rs     `Plugin` trait (`name`, `dependencies`, `build`), `PluginError`
│       ├── commands.rs   `Commands` (`EngineCommands` alias), `EngineCommand`, `EngineTask`, queued `WorldOp`s and `ScheduleOp`s, `apply`, `append`
│       ├── components.rs built-in components: `Transform`, `Velocity`, `MeshHandle`, `TextureHandle`, `Camera`, `Sprite`, `Visibility`, `EditorId`, `Parent`, `GlobalTransform` (all `#[script_component]`), plus Rust-only `EditorOnly`, `Children`, `Name`
│       ├── types.rs      `ScriptType`, `ComponentInfo`, `ScriptField`, `ScriptBool`, `TypeRegistry`, `TypeEntry`, `DynComponent`/`DynView`/`DynComponentMut`/`DynViewMut`, `CompError`, `TypeError`, `entity_to_bits`/`entity_from_bits`
│       ├── inspect.rs    tool/editor reflection: `EntityRow`, `EntityData`, `InspectedComponent`, `editable_entities`, `inspect_entity`, `set_component_field`, `addable_components`, entity tree capture/spawn/duplicate/delete
│       ├── scene.rs      versioned RON `Scene` (`SCENE_FORMAT = 1`), `SceneEntity`, `SceneValue`, `SceneError`, `SystemInfo`, `SceneOwned`, `SceneLoaded`/`SceneUnloaded`, `Snapshot`/`RestartSnapshot`/`RestoreReport`, global systems, scene capture/load/switch/restore
│       ├── prefab.rs     versioned RON `Prefab` (`PREFAB_FORMAT = 1`), `PrefabInstance`, `PrefabApplyReport`, prefab directory/registry, placement with overrides, capture/link/unpack/propagation
│       ├── schema.rs     JSON component schema export (`"format": "efe-schema"`, `Engine::schema`, `export_schema`, `schema_path`)
│       ├── flags.rs      `enum Flags { Flag(String) }` unused placeholder
│       ├── messages.rs   global `MESSAGE_SYSTEM` flume channel, `Message` enum, `send_message`, `message_sender`, `recv_messages`, wasm `send_message` export
│       ├── log.rs        `Logger` trait, `LogMsg`, dedicated-worker global logger (`global_*`, `debug_*`, `set_global_logger`), SDL / console / `LogCrateLogger` backends
│       ├── transform.rs  `velocity_system` (`FixedUpdate`), `propagate_transforms` (`PostUpdate`)
│       ├── windows.rs    `Windows` and `WindowState` (per-window size, scale_factor, focused, fullscreen)
│       ├── input.rs      declares and re-exports `input/*`
│       ├── input
│       │   ├── capture.rs  `InputCapture` (`pointer`/`keyboard` UI capture flags), `GameInput` (filtered view respecting capture and per-press ownership)
│       │   ├── events.rs   `InputEvent` (with `captured`), `InputEventKind` (incl. `TextEditing`, `Window`), `WindowEvent`, `WindowId`, `WheelUnit`, `TouchPhase`
│       │   ├── filter.rs   `InputFilter` trait and `Filter` (`Pass` / `Captured`)
│       │   ├── gamepad.rs  `GamepadState`, `Gamepad`, `GamepadId`, `GamepadButton`, `GamepadAxis` (per-frame snapshot)
│       │   ├── keys.rs     `KeyCode`, `Key`, `NamedKey`, `Modifiers`, `ElementState`, `MouseButton`
│       │   └── state.rs    `InputState`: pressed/just_pressed/just_released, per-window pointer & focus, wheel, touches, text, modifiers, `game_input()`
│       ├── render
│       │   ├── mod.rs              `Gpu`, `Target`, `Renderer2D`, `Renderer3D`, `CustomRenderer`, `RenderOverlay`, `Draw2DOverlay`, `Overlay2D`, `OverlayTextures`, `OVERLAY_TEXTURE_BASE`, `ClearRenderer`
│       │   ├── renderers.rs        `Renderers` slot container and pass order (custom, or 3D then 2D; secondary window clear fallback)
│       │   ├── gpu.rs              `GpuContext` (wgpu `Instance`, `Adapter`, `Device`, `Queue`, main + secondary surfaces)
│       │   ├── extra_surface.rs    `ExtraSurface` / `SurfaceFrame` (secondary window surfaces; Direct `wgpu::Surface` or WebGL2 CPU-readback Copy path)
│       │   ├── frame.rs            `RenderFrame`, `WindowFrame`, `Frame2D`, `Frame3D`, `CameraView`, `SpriteInstance`, `MeshInstance`, `RenderWarnings`
│       │   ├── extract.rs          `render_extract` system, `camera_matrices`, per-window camera selection (`EditorOnly` cameras preferred)
│       │   ├── draw2d.rs           `Draw2D` trait, `DrawList`, `DrawCommand`, `DrawState`, `Vertex2D`, `Rect`, `RectI`, `Blend`, `Filter`
│       │   ├── draw2d_wgpu.rs      `GpuTextures` shared texture table, `WgpuDraw2D`
│       │   ├── draw2d.wgsl         WGSL shader for `WgpuDraw2D`
│       │   ├── debug_draw.rs       `DebugDraw2D` resource (world and screen lines/rects/circles; 1-frame lifetime)
│       │   ├── overlay2d.rs        `Overlays2D` runner (shared between wgpu and SDL modes, manages `OVERLAY_TEXTURE_BASE` uploads)
│       │   ├── sprite_renderer.rs  `SpriteRenderer` (default `Renderer2D`, instanced batched sprites + `WgpuDraw2D` pass)
│       │   ├── sprite.wgsl         WGSL shader for `SpriteRenderer`
│       │   ├── sdl_draw2d.rs       native `SdlDraw2D`, `SdlSpriteRenderer`, `SdlMode` (2D fallback on `SDL_Renderer`)
│       │   ├── offscreen.rs        native `OffscreenGpu`, `encode_png`, `write_png`
│       │   ├── png.rs              pure-Rust PNG decoder (`Image::from_png`, `PngError`)
│       │   └── textures.rs         `Image`, `Textures`, `TextureChanges`, `TextureError`
│       ├── scripting
│       │   ├── mod.rs              `Scripting`, `ModuleId`, `ScriptEvent`, `ScriptEventError`, `ScriptError`, JIT backend attachment, `Send`/`Sync` compile-time assertions
│       │   ├── host.rs             `ScriptHost`, `ModuleSlot`, `LogLevel`, `LogLine`, take/run/restore re-entrancy guard
│       │   ├── api_engine.rs       `engine.{world, script, random, math, input, time, events, log, mods}` host functions
│       │   ├── mods.rs             `ModOptions`, `load_module`, `load_mod`, `unload`, `unload_cascade`, hot reload (`watch`, `reload_module`)
│       │   ├── components.rs       mod-defined `[[meta.component]]` parsing/layout (`ModComponent`), dynamic registration, `SpawnedBy`, swap stash & migration
│       │   ├── packages.rs         `.efmod` package loading (`ModPolicy`, `Grant`, `DefaultPolicy`, `ModError`, `LoadReport`, `PackageInfo`, `Reloaded`, `UnloadReport`, `watch_package`)
│       │   ├── provides.rs         section 13 provide trampolines and left-to-right 8-aligned copy ABI
│       │   ├── system.rs           `Script` component and `script_system` (first in `Update`, `init`/`frame`/per-entity `entry`, parallel lanes)
│       │   ├── load.rs             `Engine::load`, `load_input`, `LoadInput`, `Loaded`, `LoadError`, `ModSource`, `ModItem`, `ModBody`, native `DirSource`, `AssetHandler`
│       │   ├── reload.rs           `reload_mods`, `reload_mod`, `unload_mods`, transitive dependent swap & rollback
│       │   └── cache.rs            `ScriptCache`, `CacheConfig`, `CacheDir`, `CacheStats`, `CACHE_FORMAT` (memory + disk `.efvm`/`.deps` caches)
│       ├── window
│       │   ├── mod.rs              `InnerWindow` trait, `CursorIcon`, `WindowDesc`, `WebTarget`, `GameWindowManager` (drop-order safe, event injection)
│       │   ├── clipboard.rs        `clipboard_text` / `set_clipboard_text` (SDL on native, fallback store when SDL video is down, unsupported on web)
│       │   ├── headless.rs         `HeadlessWindow` (all targets)
│       │   ├── sdl_backend.rs      native `SdlWindowManager` (multi-window, event pump, text input, cursors)
│       │   ├── sdl_impls.rs        SDL scancode/keycode/modifier/button conversions
│       │   ├── web_backend.rs      wasm `WebWindow` (multi-canvas DOM listeners, DPR & size sync, `canvas_of`, `ResizeObserver`, pointer lock)
│       │   └── web_impls.rs        DOM `KeyboardEvent` conversions, `preventDefault` policy, `is_editable_target` (skips inputs and `[data-efui-layer]`)
│       ├── thread_pool.rs          `Jobs`, `JobHandle`, `GLOBAL_POOL`, `spawn_global`, `spawn_job_global`, `new_worker_global`, sizing & `KillGate`
│       └── thread_pool
│           ├── native.rs           `std::thread` pool sharing a `flume` channel
│           ├── web.rs              Web Worker pool (`worker_loop`, `worker_loop_once` wasm exports)
│           ├── worker.js           JS bootstrap template for persistent workers
│           └── worker_once.js      JS bootstrap template for one-shot workers (unused; contains debug `setTimeout`/`console.log`)
└── tests
    ├── api_index.rs                registered host fns/types/events/mods appear in `ApiIndex`
    ├── change_detection.rs         `Changed`/`Added` via `Ctx::query` & `ParCtx::query`, command ticks, `engine.world.changed`/`tick`
    ├── components_derive.rs        `#[script_component]` and `#[derive(Component)]`
    ├── engine_api_extras.rs        `engine.random`, `engine.math`, `Engine::watch_source`, `Commands::unload_mods`
    ├── events.rs                   typed `ScriptEvent`s, mod-provided events, `Events::send_now`
    ├── headless.rs                 headless frame loop, fixed-step counts, `Startup`/`Shutdown`, `quit`, injected input, messages
    ├── input_filter.rs             `InputFilter`, captured events, `TextEditing`, `SetTextInputArea`, `RenderOverlay` bookkeeping
    ├── inspect.rs                  editor reflection (`editable_entities`, `inspect_entity`, `set_component_field`, entity tree ops)
    ├── jit.rs                      Cranelift JIT execution, `Grant::jit`, `ScriptBackend`, hot-reload re-enable (`jit` feature, native)
    ├── load.rs                     `LoadInput`, `Engine::load`, `ScriptCache` (memory/disk), folder mods, `AssetHandler`, `[meta]`
    ├── mod_components.rs           `[[meta.component]]` registration, script/Rust access, unload cleanup, swap migration, grant limits
    ├── order.rs                    system `order` numbers and `Plugin` dependencies/ownership
    ├── packages.rs                 `.efmod` policies, grants, id clashes, dependency resolution, assets, memory limits, `watch_package`
    ├── parallel.rs                 determinism (`Threads::Single` vs `Threads::Fixed(4)`) across par/exclusive/script systems
    ├── parallel_bench.rs           `#[ignore]` 10k-entity parallel schedule benchmark
    ├── prefab.rs                   RON `Prefab` files, placement with overrides, propagation, save-back, instance API
    ├── provides.rs                 mod `provide`s, copy ABI, dependents, unload cascade, hot-reload compatibility rules
    ├── readonly.rs                 `#[script_component(readonly)]` read/write enforcement (status 5)
    ├── reload.rs                   content-hash skip, `reload_mod`, cascading swaps, failure rollback, `reload_mods`, `unload_mods`
    ├── render.rs                   headless camera selection, matrices, sprite gathering, `Transform`/`GlobalTransform`, textures
    ├── resource_ticks.rs           resource change ticks (`Ctx`/`ParCtx`/`Engine::res_changed`) and `GameInput` capture
    ├── restart_restore.rs          `RestartSnapshot` and lenient `restore_lenient` (`RestoreReport`)
    ├── scene.rs                    RON `Scene` load, save, round-trip, validation (`SceneError`)
    ├── scene_switch.rs             `load_scene`, `unload_scene`, `SceneOwned`, global systems, `SceneLoaded`/`SceneUnloaded`
    ├── script_lanes.rs             parallel script lanes, buffered `set`, lane merge order, spawn pools
    ├── scripting.rs                `engine.*` host calls, budgets, traps, re-entrancy, `Script` component
    ├── snapshot.rs                 `Snapshot` capture/restore and entity reference remapping
    ├── systems.rs                  API registration lock (`api_open`/`reopen_api`), enable/`run_if`/remove, deferred `ScheduleCommands`, `.efapi` fragments
    ├── windows.rs                  multi-window ids, per-window state/events, camera targeting, runtime create/close
    └── world.rs                    `Commands` ops, dead/stale entities, `TypeRegistry` layouts and byte round-trips
```

## Main concepts

### Engine and frame loop (`src/engine.rs`)
`Engine` owns `windows: GameWindowManager`, `lifecycle: Lifecycle` (`Created`, `Running`, `Paused`, `ShuttingDown`, `Terminated`), `logger: Box<dyn Logger>`, `renderer: Renderers`, `overlays: Vec<Box<dyn RenderOverlay>>`, `overlays_2d: Overlays2D`, `world_overlays_2d: Overlays2D`, `graphics_hooks`, `input_filters`, `gpu: Option<GpuContext>`, native `sdl: Option<SdlMode>` (both declared before `windows` so surfaces/renderers drop before the window), `state: EngineState`, `event_buf`, `config`, `events: EventRegistry`, `schedule: Schedule`, `plugins`, `exec: Executor`, `commands: Commands`, `scene_state: SceneState`, `max_fps`, private message channel (`msg_tx`/`msg_rx`), `clock`, and native `api_export`.

`EngineState` holds `world: hecs::World`, `types: TypeRegistry`, `windows: Windows`, `input: InputState`, `gamepads: GamepadState`, `time: Time`, `resources: Resources`, and `scripting: Scripting`.

`Engine::new(title)` calls `Engine::with_config(EngineConfig::new(title))`. `with_config`:
1. Merges `EngineConfig` with `startup_config()`, validates (`fixed_dt > 0`, non-zero window size unless headless, `threads != 0`), and checks `EFE_API_EXPORT` / `--efe-api-export` (if enabled by `cfg.api_export`, forces headless mode).
2. Unless `headless`: initializes the `log` crate backend (`env_logger` on native, `console_log` on wasm), initializes the global thread pool (`init_global_pool` or `init_global_pool_auto`), and installs `console_error_panic_hook` on wasm.
3. Creates `GameWindowManager`, `Logger` (`LogCrateLogger` when headless, else `NewDefaultLogger()`), `Scripting` (forwarding `cfg.script_cache`, `cfg.script_backend`, `cfg.script_jit`, and `cfg.title`), `EngineState` (inserting `Textures`, `DebugDraw2D`, `RenderFrame`, `RenderWarnings`), `Executor`, and `Schedule` (pre-registering `script_system` in `Update`, `velocity_system` in `FixedUpdate`, `propagate_transforms` in `PostUpdate`, and `render_extract` in `RenderExtract`).
4. Registers the 10 built-in script components via `Engine::register::<T>()`: `Transform`, `Velocity`, `MeshHandle`, `TextureHandle`, `Camera`, `Sprite`, `Visibility`, `Parent`, `GlobalTransform`, `EditorId`. (`EditorOnly`, `Children`, and `Name` are Rust-only.)

`Engine::frame(now_secs)` runs one tick:
1. `time.advance(now_secs)` computes clamped delta (`<= 0.25 s`) and up to 8 fixed steps (larger backlogs are dropped).
2. `events.update_all()` swaps pending event buffers into readable buffers.
3. `input.begin_frame()`, `windows.poll_events(&mut buf)` (injected events first, then backend events), plus `gamepads` polling. Each event passes through `input_filters` (`Filter::Captured` sets `InputEvent::captured`), then updates `state.windows` / `GpuContext` surfaces / window close handling, and `input.apply(ev)`.
4. `drain_messages()` drains the engine's queue (and `MESSAGE_SYSTEM` when not headless), handles control messages (`Say`, `Log`, `ChangeTitle`, `Start`, `Stop`, `Kill`, `SetThreads`, `SetFrameRate`, wasm `SetCanvasId`), and forwards all messages into readable `Events<Message>`.
5. If `Lifecycle::Created` -> `Running`, runs `Stage::Startup` once. While `Paused`, stages and rendering are skipped.
6. Runs `PreUpdate`, `FixedUpdate` (n times with `delta == fixed_delta`), `Update`, `PostUpdate`, and `RenderExtract`, applying `Commands` (world ops, control `EngineCommand`s, and `ScheduleOp`s) after every stage. Calls `maybe_check_change_ticks`.
7. If graphics are initialized (`GpuContext` on wgpu, or `SdlMode` on native SDL fallback), renders every active window (main + secondary canvases/windows in wgpu mode; main window in SDL mode): 3D pass (if a 3D camera exists), 2D pass, `world_overlays_2d`, `overlays` (`RenderOverlay`), `overlays_2d` (`Draw2DOverlay`), then presents. Handles `Outdated`/`Lost`/suboptimal surfaces; restores unconsumed `TextureChanges` if rendering was skipped (e.g. 0x0 minimized).
8. If shutdown was requested, runs `Stage::Shutdown` once and enters `Lifecycle::Terminated`.

`game_loop(self)` closes API registration (`close_api()`), exports `.efapi` and `.efschema.json` if `api_export` is set (and returns without running frames), and runs either `sdl_loop()` (native, blocks on `init_graphics()` via `pollster` unless headless, sleeps to `max_fps`) or `wasm_loop_start()` (wasm, spawns async `init_graphics()` then drives `requestAnimationFrame`).

### Configuration (`src/engine/config.rs`)
- `StartupConfig`: one-shot global configuration (`set_startup_config` / `startup_config`), deserialized from JS camelCase in `__engine_start({ ... })`: `wasmJsPath`, `threads`, `canvasId`, `uiLayerId`, `externalApi`, `graphicsBackend` (`Auto | Webgpu | Webgl`), `gpuMode` (`Auto | Wgpu | WgpuGl | Sdl`; native also reads `EFE_GPU_MODE` when `Auto`).
- `EngineConfig`: `title`, `size` (800x600 logical), `vsync`, `fixed_dt` (1/60), `max_fps` (`Some(60.0)`), `threads` (`Threads::Auto | Fixed(n) | Single`), `canvas_id`, `headless`, `script_cache: CacheConfig`, `script_backend: ScriptBackend` (`Interpreter | Jit | Auto`), `script_jit: JitConfig`, `api_export` (`bool`, default `true`).

### Schedule, parallel execution, commands, and plugins (`schedule.rs`, `parallel.rs`, `order.rs`, `commands.rs`, `plugin.rs`, `resources.rs`, `events.rs`, `time.rs`)
- `Stage`: `Startup`, `PreUpdate`, `FixedUpdate`, `Update`, `PostUpdate`, `RenderExtract`, `Shutdown`.
- `Schedule`: systems are registered with `add_system` (`FnMut(&mut Ctx)`) or `add_par_system` (`Access`, `FnMut(&ParCtx) + Send`) returning `SystemEntry` (`.order(i32)`, `.name(&str)`, `.enabled(bool)`, `.run_if(fn(&RunCtx) -> bool)`). Sorted by `(order, registration_index)`; consecutive non-conflicting parallel systems run concurrently in batches through `Executor` (`ParCtx` provides checked queries with per-system change ticks, `par_for_each`, `res`, `res_mut`, `res_changed`, `game_input`, `ParCommands`).
- `Commands` (`EngineCommands`): queues structural world edits (`spawn`, `spawn_empty`, `despawn`, `insert`, `insert_bundle`, `remove`, `insert_raw`, `remove_raw`), control requests (`EngineCommand`: `Task(EngineTask)`, `Quit`, `SetTitle`, `SetFrameRate`, `SetCursorVisible`, `SetCursorGrab`, `SetFullscreen`, `SetCursorIcon`, `SetTextInput`, `SetTextInputArea`, `ReloadMods`, `UnloadMods`, `LoadScene`, `UnloadScene`, `CreateWindow`, `CloseWindow`, `SetWindowTitle`, `SetWindowFullscreen`), and deferred `ScheduleCommands` (`add`, `set_enabled`, `remove`).
- `Plugin`: `name`, `dependencies`, `build(&mut Engine)`. Systems and components added inside `Plugin::build` are tagged with the plugin's name. `add_plugin` is rejected with `PluginError::Closed` once the game loop starts.
- `Resources`: `TypeId`-keyed map with per-resource `Tick` tracking (`ResMut` stamps on `DerefMut`; `Ctx::res_changed`, `ParCtx::res_changed`, `Engine::res_changed`).
- `Events<T>`: double-buffered typed queue; `send` queues for frame N+1, `send_now` / `send_batch_now` (`extend_readable`) makes events readable immediately in the same frame.

### Input and windows (`input/`, `window/`, `windows.rs`)
- `InputState` folds `InputEvent`s (`Key`, `Text`, `TextEditing`, `MouseMotion`, `MouseButton`, `MouseWheel`, `Touch`, `Window(WindowEvent)`) into `pressed` / `just_pressed` / `just_released`, per-window pointer position and focus, summed `wheel` / `mouse_delta`, `touches`, `text`, and `modifiers`.
- `InputFilter` (`Engine::add_input_filter`) inspects every raw event before `InputState::apply` and can return `Filter::Captured`. `InputState::game_input()` returns a `GameInput` view respecting `InputCapture` (`pointer`/`keyboard`) and per-press ownership (a captured press never becomes a game press, while a game press always delivers its release).
- `GamepadState` snapshots connected gamepads once per frame (SDL gamepad subsystem natively; `navigator.getGamepads()` standard mapping on web).
- `Windows` maintains `WindowState` per `WindowId` (`WindowId::MAIN == 0`, secondary windows created via `Engine::create_window(WindowDesc)` or `Commands::create_window`).
- `GameWindowManager` wraps `InnerWindow`: `HeadlessWindow` (all targets), `SdlWindowManager` (native), `WebWindow` (wasm, supports `WebTarget::{Body, Element, Canvas}`; `Popup` returns unsupported). `clipboard` provides `clipboard_text` / `set_clipboard_text`.

### Rendering (`render/`, `transform.rs`)
- `transform.rs`: `velocity_system` (`FixedUpdate`) integrates `Transform` from `Velocity`; `propagate_transforms` (`PostUpdate`) computes `GlobalTransform` and rebuilds `Children` from `Parent` chains (handling stale parents and cycles).
- `extract.rs`: `render_extract` builds `RenderFrame` (`d2`/`d3` for the main window plus `windows: Vec<WindowFrame>` for secondary windows). Per window, picks the active camera of each kind (`ORTHOGRAPHIC`/`PERSPECTIVE`) preferring `EditorOnly` cameras, then lowest entity id; gathers visible `Sprite`s sorted by `(layer, entity id)` and `MeshHandle`s, plus `DebugDraw2D` commands and drained `TextureChanges`.
- `GpuContext` (`gpu.rs`) + `ExtraSurface` (`extra_surface.rs`): manages wgpu `Instance`, `Adapter`, `Device`, `Queue`, main surface, and secondary window surfaces (Direct `wgpu::Surface` or WebGL2 CPU-readback `ImageData` copy path). On native `GpuMode::Auto` falls back to `SdlMode` (`sdl_draw2d.rs`) if no wgpu adapter exists (or when `GpuMode::Sdl` is selected); `Engine::on_graphics_ready` notifies plugins of `GraphicsMode::{Wgpu, Sdl}`.
- Built-in renderers & overlays: `SpriteRenderer` (`Renderer2D`, instanced quads + `WgpuDraw2D` for `Frame2D::draw`), `ClearRenderer`, `Overlays2D` (`Draw2DOverlay` working on both wgpu and SDL modes with `OVERLAY_TEXTURE_BASE` texture uploads), `RenderOverlay` (raw wgpu overlay), `DebugDraw2D` resource, `Textures` + `Image::from_png` (`png.rs`), and native `OffscreenGpu` + `encode_png`/`write_png` (`offscreen.rs`).

### Types, scripting, mods, and cache (`types.rs`, `scripting/`)
- `TypeRegistry`: stores `ScriptType` components (`Storage::Static`, id = `fnv1a32(name)`) and runtime mod components (`Storage::Dynamic`). Provides checked byte read/write/insert/remove and `DynView`/`DynViewMut` (`DynComponent`/`DynComponentMut`) accessors. `Engine::register::<T: ComponentInfo>()` is the unified registration call (`#[script_component]` or `#[derive(Component)]`); registration closes when `game_loop` starts (`Engine::api_open`, `reopen_api`).
- `Scripting`: one `efevm::HostRegistry<ScriptHost>` and one `efevm::Instance<ScriptHost>` per loaded module.
  - `api_engine.rs` registers `engine.world`, `engine.script`, `engine.random`, `engine.math`, `engine.input`, `engine.time`, `engine.events` (v2 typed events), `engine.log`, and `engine.mods` (v2 package asset/meta access).
  - `script_system` (`Update`, `order::SCRIPTS`) runs `init` on load, `frame(dt)` per frame, and `entry(entity, dt, state)` per `Script` component, partitioning modules into parallel lanes when accesses permit and buffering writes with read-your-own-writes and deterministic lane merge.
  - `mods.rs` & `provides.rs`: modules with section 13 interfaces define `<name>` namespaces, validate copied structs, register typed `ScriptEvent`s and provide trampolines (8-aligned scratch copy ABI), track dependents, and support `unload` / `unload_cascade`.
  - `components.rs`: parses `[[meta.component]]` from `mod.toml`, registers `mod:<mod>/<Name>` dynamic components in `TypeRegistry` and `hecs::World`, tracks script-spawned entities with `SpawnedBy(ModuleId)`, despawns/unregisters on unload, and migrates compatible/changed layouts on swap.
  - `packages.rs`, `load.rs`, `reload.rs`: `Engine::load` / `load_input` handle `Efa`, `Efvm`, `Package` (`.efmod`), and `Folder` (`mod.toml` + `.efa` or native `wasm-mods` `.wasm`). Packages and folder mods enforce game matching, id-clash checks, dependency resolution (`efemod::resolve`), `ModPolicy` (`Grant` limits on namespaces, budget, memory pages, JIT, component types/sizes), and `AssetHandler` `check`/`commit`/`unload` hooks. `ModSource` / `DirSource` support batch `load_mods`, `reload_mods` (with transitive dependent rebuild/swap and rollback), and `unload_mods`.
  - `cache.rs`: `ScriptCache` provides a 64 MiB blake3-keyed memory cache for decoded modules/packages and a disk cache (`<cache>/efa/v2/<xx>/<hash>.{efvm,deps}`) for `.efa`/folder compiles and `.wasm` translations.
  - `jit` feature (native only): `Scripting::attach_backend` enables `efevm_jit` per slot according to `EngineConfig::script_backend` (`Auto | Jit | Interpreter`) and `Grant::jit` (default `false` for mods).

### Scenes, prefabs, editor reflection, and schema (`scene.rs`, `prefab.rs`, `inspect.rs`, `schema.rs`)
- `scene.rs`: RON `Scene` (`format: 1`, `name`, `requires`, `systems`, `entities`) serialized via `TypeRegistry` + `SceneValue` (`Bool`, `Int`, `UInt`, `Float`, `List`) with entity-bit remapping on load. Supports `load_scene` / `unload_scene` (`SceneOwned`, `SceneLoaded`/`SceneUnloaded` events, global systems), `Snapshot` (full world + registered snapshot resources), and `RestartSnapshot` (`restore_lenient` returning `RestoreReport` for hot-reload/restart workflows).
- `prefab.rs`: RON `Prefab` (`format: 1`, `name`, `entities`), `PrefabInstance` tracking, `place_prefab` with root/`#<local>:<component>` overrides, `capture_prefab`, `link_prefab`, `unpack_prefab`, and `apply_prefab_to_world` (`PrefabApplyReport`).
- `inspect.rs`: editor-facing entity tree (`editable_entities` -> `EntityRow` skipping `EditorOnly` and read-only-only entities), `inspect_entity` (`InspectedComponent`), `set_component_field`, `addable_components`, `add_component_default`, `remove_component`, `EntityData` capture/spawn/duplicate/delete.
- `schema.rs`: JSON component/struct schema (`"format": "efe-schema"`, `version: 1`) exported by `Engine::schema` / `export_schema` / `write_schema`, and written automatically as `<api>.efschema.json` alongside `.efapi` on `EFE_API_EXPORT` / `--efe-api-export`.

### Thread pool & messages (`thread_pool.rs`, `thread_pool/`, `messages.rs`, `log.rs`)
- `thread_pool`: `GLOBAL_POOL` (`flume` MPMC channel + `KillGate` non-blocking shrink) backed by `std::thread` natively (`native.rs`) or shared-memory Web Workers from a `Blob` URL built from `worker.js` + `WASM_PATH` on wasm (`web.rs`). Dedicated workers (`new_worker_global`) power the async logger (`log.rs`).
- `messages.rs`: `MESSAGE_SYSTEM` global `flume` queue plus per-`Engine` `sender()` queue; wasm exports `send_message(JsValue)`.

## Public API
- `efeengine_core::prelude::*`: everyday game/plugin API (`Engine`, `EngineConfig`, `Threads`, `order`, `Plugin`, `Stage`, `Ctx`, `ParCtx`, `Access`, `SystemEntry`, `RunCtx`, `Commands`, built-in components, `ScriptType`, `ScriptBool`, `ComponentInfo`, `Res`, `ResMut`, `Events`, `Time`, input types, `DebugDraw2D`, `SpriteRenderer`, `Textures`, `global_*`/`debug_*`, `#[main]`, `#[script_component]`, `#[derive(Component)]`, `#[derive(ScriptType)]`, `hecs`/`glam` essentials, `wasm_bindgen` on wasm32).
- Crate-root domain facades (`src/lib.rs`): `math`, `render`, `input`, `scripting`, `mods`, `cache`, `ecs`, `window`, `clipboard`, `thread`, `log`.
- Crate-root re-exports: `hecs`, `glam`, `wgpu`, `bytemuck`, `efevm`, `efemod`, `efa` (feature `efa`), `efevm_jit` (native `jit`), `efe_wasm` (native `wasm-mods`), `wasm_bindgen` (wasm32).
- Full internals under `efeengine_core::engine::*`.
- Wasm exports (`wasm_bindgen`): `send_message`, deprecated `set_wasmjs_path`, `__init_thread_pool`, `worker_loop`, `worker_loop_once`, `zort`.

## Platform notes
- Workspace `.cargo/config.toml` enables `+atomics`, `+bulk-memory`, `--shared-memory`, `--import-memory`, and `--max-memory=2147483648` on `wasm32-unknown-unknown` (requires nightly `-Z build-std`). Browser pages must be cross-origin isolated (COOP/COEP) for `SharedArrayBuffer` workers; without `wasmJsPath` in `StartupConfig`, the wasm pool starts with 0 workers.
- Native builds require SDL3 (system SDL3, Windows vcpkg via `build.rs`, or the `static` feature).
- Graphics backend selection: native tries Vulkan/Metal/DX12, then GL, then `SdlMode` (under `GpuMode::Auto`); wasm picks WebGPU or WebGL2 per `GraphicsBackend` (`Auto` uses WebGL2 on desktop Linux).

## Status and known gaps
- Native window and browser DOM input/backend paths (SDL events, DOM listeners, pointer lock, DPR changes, gamepads, WebGL2 `ExtraSurface` copy path) are verified mainly by compilation and manual examples; automated tests run headless (plus an `#[ignore]` GPU offscreen test in `render/offscreen.rs`).
- `EngineConfig::vsync` is stored and only used by `SdlDraw2D::for_window`; the wgpu swapchain/native loop still relies on first present mode and `max_fps` sleep pacing.
- `StartupConfig::external_api` is stored but unused; `SetCanvasId` after graphics init does not recreate an existing surface; `WebTarget::Popup` is not implemented.
- Built-in rendering covers 2D sprites (`SpriteRenderer`), `Draw2D` primitives, and `DebugDraw2D` (`Renderer3D` is a trait slot with no built-in 3D implementation yet; `shaders/video.wgsl` is unused). PNG decoding rejects interlaced and 16-bit images.
- `Flags` (`engine/flags.rs`) is an unused placeholder.
- Thread pool: `WorkerHandleOnce` / `worker_loop_once` / `worker_once.js` are unused (`worker_once.js` contains a debug `setTimeout`/`console.log`); `JobHandle::join` on wasm warns experimental when blocking.
- `lib.rs` still contains a leftover wasm `zort()` test export.
