# efeengine roadmap

Living document. Tick items off (`- [x]`) as they land and edit freely. Each task
lists the main files it touches and how to tell it's done.
Tags: **[N]** native (SDL3) · **[W]** web (wasm) · **[S]** shared by both.

_Last updated: 2026-11-14_

**Order:** IO → game systems → hecs world → rendering (wgpu). Scripting (Phase 5) needs only the world, so it can run alongside rendering.
Rendering is deliberately last: until Phase 4 the current wgpu code stays as a
**placeholder**. The native triangle keeps working as it does now, and on the web
nothing is drawn, which is fine. Earlier phases are tested through logs, the
native window and headless unit tests, not through rendering.

## Where to focus now

**Phase 1 is done apart from the manual checks below, then Phase 3.**
1. Check the native-window and browser input paths by hand with `cargo xtask run input_test native` and `cargo xtask run input_test web`: they are compile-checked only so far. Include a gamepad (1.12) in both runs.
2. Phase 3 (hecs world, `Commands`, type registry), now that the input system, schedule, fixed-step loop and headless mode (Phases 1–2) are in.

Input now flows through the engine, not the global message queue, and is polled and applied in the same frame it is used.

## Phase 0: Housekeeping (small, do alongside Phase 1)

- [x] 0.1 Commit or finish the work in progress in `engine.rs`, `thread_pool.rs`, `systems/mod.rs` and `Cargo.toml` (the `init_global_pool_auto` refactor). _Done: committed (thread pool refactor, pyo3 removal)._
- [x] 0.2 Stop the console flood: `sdl_backend.rs` calls `global_alert("event got!")` for every unhandled SDL event, including each mouse move. Turn it into a debug-level log, or remove it. _Done: unhandled SDL events are silent now._
- [x] 0.3 Refresh `crates/core/structure.md`: it still lists hecs as dev-only. _Done: structure.md refreshed; hecs is documented as the planned game-world ECS and re-exported as `efeengine_core::hecs`._
- [x] 0.4 Fix CI (`.github/workflows/rust.yml`). It needs nightly + `rust-src`, SDL3 dev packages, and a wasm `cargo check` step; it should also run `cargo fmt --check` and `clippy`. _Done: native check+test run in a Fedora container with SDL3-devel (Ubuntu 24.04 has no SDL3, and `--static` is broken: sdl3-src ships without SDL's `test/` dir); wasm check with build-std; fmt+clippy job is `continue-on-error` until the code is formatted (~730 rustfmt diffs today)._
- [x] 0.5 Results of the health scan (2026-11-14). Native and wasm `cargo check` both pass.
  - The `Engine::new` doc example (`engine.rs:53`) is stale and makes `cargo test` fail: it passes 2 arguments, and the function takes 1. _Fixed._
  - Remove or use the unused dependencies `env_logger`, `serde_json` and `console_log`. `hecs` is used only in a test until Phase 3. _Done: env_logger/console_log now route the `log` crate (wgpu errors) to the terminal / browser console; serde_json is a dev-dependency; hecs is re-exported._
  - Unused `Rc`/`RefCell` import on wasm (`lib.rs:16`). _Fixed._
  - Install clippy for nightly: `rustup component add --toolchain nightly clippy`. _Done (clippy + rustfmt installed)._
  - Baseline in `crates/core/src` (2,417 lines): 38 `unwrap`, 11 `expect`, 30 `unsafe`, 9 statics, 0 `todo!()` (the web pool resize is implemented), 17 `println!`/`dbg!`.
- [ ] 0.6 Run `cargo fmt --all` once (one big formatting commit), fix clippy warnings, then remove `continue-on-error` from the lint job in `.github/workflows/rust.yml`.

## Phase 1: IO system (window + input), SDL3 and web

Goal: one platform-agnostic window, event and input-state API that behaves the same in both builds. No rendering work.

### Window size and window events
- [x] 1.1 **[W] Real canvas.** `WebWindow` resolves the `HtmlCanvasElement` once (id from `SetCanvasId`, default `"canvas"`). It sets the backing-store size to CSS size × `devicePixelRatio`, and `size()` returns those physical pixels instead of `(0,0)`. _Done (size is synced every frame instead of a ResizeObserver; renderer uses the same canvas)._
  - Add `web-sys` as a direct dependency with the features this phase needs: `Document`, `HtmlCanvasElement`, `KeyboardEvent`, `PointerEvent`, `WheelEvent`, `FocusEvent`, `ResizeObserver*`, `MediaQueryList`, `AddEventListenerOptions`, …
  - The placeholder renderer keeps its own `getElementById` until Phase 4.

  _Files:_ `Cargo.toml`, `window/web_backend.rs`. _Done:_ the logged size matches the canvas at DPR 1 and 2.
- [x] 1.2 **[S] Window events.** _Done. Native scale changes are caught through the window `DisplayChanged` and the SDL `Display { ContentScaleChanged }` events (sdl3 0.14 has no window-level DisplayScaleChanged). Engine keeps size/scale/focus in `EngineState.window`._
  - Native uses `size_in_pixels()` and the SDL `PixelSizeChanged`/`DisplayChanged`/`ContentScaleChanged`, focus and close events.
  - Web polls the canvas size every frame (CSS size × `devicePixelRatio`; no `ResizeObserver`), and uses `focus`/`blur` and `visibilitychange`.
  - Both send `Resized`, `ScaleFactorChanged`, `Focused` and `CloseRequested`, plus an initial `Resized` at startup. The engine stores the current size and scale.

  Connecting this to `renderer.resize()` is task 4.3. _Files:_ `sdl_backend.rs`, `web_backend.rs`, `engine.rs`. _Done:_ resizing the window or the tab logs correct sizes.

### Shared foundation
- [x] 1.3 **[S] Event types.** _Done: `engine/input/{keys,events}.rs` (`InputEvent` wraps `InputEventKind` with timestamp and `WindowId`); `window/events.rs`, `KeyMod`, `raw: u16` and the aliases are gone._ Add an `InputEvent` enum (sketch below) and the types it uses: `Modifiers`, `ElementState`, a logical `Key` (`Named(NamedKey) | Character(String)`), `WindowId`, and a `timestamp_ns` normalised across platforms. Keep `KeyCode` as the *physical* key only, and add the keys it's missing (IntlBackslash, PrintScreen, ContextMenu, F13–F24, media keys). Remove the SDL-specific `raw: u16` and the unused `WindowEvent` stub. _Files:_ `window/events.rs` (or a new `input/` module).

  ```text
  InputEvent = Key { physical, logical, state, repeat, mods }
             | Text(String)
             | MouseMotion { pos, delta } | MouseButton | MouseWheel { delta, unit }
             | Touch { id, phase, pos }
             | Window(Resized | ScaleFactorChanged | Focused | CloseRequested)
  ```
- [x] 1.4 **[S] `InputState` resource.** _Done: `engine/input/state.rs`, unit-tested, lives at `EngineState.input` (also `cx.input`)._
  - `begin_frame()`, `apply(&InputEvent)`.
  - Queries: `pressed`, `just_pressed`, `just_released`, `mouse_pos`, `mouse_delta`, `wheel`, `text`, `modifiers`, `focused`.
  - Key repeats must not count as `just_pressed`, and losing focus clears held keys.
  - Unit-tested, with no window needed.

  _Files:_ new `engine/input.rs`, `engine.rs`, `prelude.rs`.
- [x] 1.5 **[S] Engine-owned input path.** _Done: `Message::WindowEvent` is removed; input is polled and applied in the same frame it is used; `GameWindowManager::inject` feeds tests._
  - `InnerWindow::poll_events(&mut self, out: &mut Vec<InputEvent>)`.
  - Tick order: `begin_frame`, then poll, then apply, then update.
  - Remove `Message::WindowEvent` and the "Todo window event" handler, so input no longer goes through the global queue and the one-tick delay disappears. `Message` stays for control commands and the JS bridge.

  _Files:_ `window/mod.rs`, `messages.rs`, `engine.rs`.

### Native (SDL3)
- [x] 1.6 **[N] Full SDL mapping.** _Done: keys (physical and logical, with a `SDL_GetKeyFromScancode` fallback), text, mouse, wheel, touch (never `Cancelled`: sdl3 0.14.42 has no finger-cancel event), one `CloseRequested` per poll. Compile-checked only, not run in a native window._
  - Keys with `repeat`, and a logical key from the keycode, which is layout-aware.
  - Text input: call `start_text_input`.
  - Mouse motion, buttons and wheel.
  - Touch.
  - Map `Quit` to `CloseRequested`.

  _Files:_ `sdl_backend.rs`, `sdl_impls.rs`.
- [x] 1.7 **[N] Thread-safety cleanup.** _Done: Kill now breaks the loop so Drop runs._
  - Drop `Send + Sync` from `InnerWindow`, and remove the `unsafe impl Send/Sync` on the window types (SDL is main-thread only).
  - Delete the unused `Mutex<Sdl>`.
  - On Kill, end the loop cleanly instead of calling `process::exit(0)`, so `Drop` runs.

### Web (DOM)
- [x] 1.8 **[W] Keyboard and focus.** _Done (code in `web_backend.rs` / `web_impls.rs`; held keys are released as KeyUp on blur/hidden)._
  - Listeners for `keydown`/`keyup`: `code` gives the physical key, `key` the logical key, plus `repeat` and the modifiers.
  - Clear held keys on `blur` and `visibilitychange`.
  - Canvas gets `tabindex=0` and takes focus on pointerdown.
  - Call `preventDefault` selectively, for arrows, Space and Tab.
  - Register the listeners from Rust with `web-sys` closures stored in `WebWindow`; don't add a JS shim.

  _Files:_ new `window/web_input.rs`, `templates/web/index.html`.
- [x] 1.9 **[W] Pointer and wheel.** _Done: pointer events with capture (touch -> `Touch`, mouse/pen -> mouse events), non-passive wheel, `contextmenu` suppressed, `touch-action: none` in `templates/web/index.html`. Compile-checked only, not run in a browser._
  - Pointer events (mouse, touch and pen) with `setPointerCapture`, and coordinates in canvas physical pixels.
  - Wheel events using `deltaMode`.
  - Suppress `contextmenu`, and add `touch-action: none` to the canvas CSS.
- [x] 1.10 **[W] DPR changes and text input.** _Done: DPR changes come from a `matchMedia` listener (re-armed from the per-frame poll) plus the per-frame poll; text comes from the keydown `key` (single grapheme, no Ctrl/Meta) and `compositionend`, with no `beforeinput` or hidden-input IME path. Compile-checked only, not run in a browser._ Detect zoom and DPR changes with a `matchMedia` listener. Take text from the keydown `key` value for printable keys and from `compositionend`.

### Polish
- [x] 1.11 **[S] Window API.** _Done: `scale_factor()`, `logical_size()`, `set_title` (also `document.title`), cursor visibility, grab/pointer lock, `set_text_input`, exposed as `EngineCommands`. Native window and browser behaviour is compile-checked only._
  - `scale_factor()` and `logical_size()`.
  - `set_title` that also changes `document.title` on web.
  - Cursor visibility and grab, and pointer lock (relative mouse mode on native, `requestPointerLock` on web).
- [x] 1.12 **[S] Gamepad (can wait).** SDL's gamepad subsystem on native, and polling `navigator.getGamepads()` once per frame on web. _Done: GamepadState (input/gamepad.rs), polled once per frame (SDL gamepad subsystem with hotplug; navigator.getGamepads() with the "standard" mapping only); read via cx.gamepads. Compile-checked only (no pad was available)._
- [x] 1.13 **[S] Input test example.** _Done: `examples/input_test` (`cargo xtask run input_test native|web`), `structure.md` files updated; the manual run in a native window and a browser is still to be done by hand (compile-checked only)._ An example that logs `InputState` changes (keys, mouse, focus, size), so the same manual test runs natively and in the browser console. Unit tests cover `InputState`. Update `structure.md`.

## Phase 2: Engine loop and game systems

Goal: games run their own code each frame, with time, input and resources available.

- [x] 2.1 **[S] Errors and config.** _Done: `EngineError` (`error.rs`), `Engine::new` / `with_config` return `Result`; `EngineConfig` (in `config.rs`) also has `max_fps` and `headless`, and fills `threads` / `canvas_id` from the startup config._
  - `EngineError` (thiserror), with `Engine::new` returning `Result`.
  - `EngineConfig { title, size, vsync, fixed_dt, threads, canvas_id }`; the 800×600 size and the `"canvas"` id are hard-coded today.
- [x] 2.2 **[S] `Time` and frame loop.** _Done: `time.rs`; `Engine::frame(now)` is shared, delta is clamped to 0.25 s, at most 8 fixed steps per frame; native frame limiter honours `SetFrameRate`, web uses the rAF timestamp._
  - `Time` resource: delta, fixed delta, elapsed time, frame count and interpolation alpha.
  - A shared `Engine::frame(now)` with clamped delta time and a fixed-step accumulator.
  - Native uses `Instant` and replaces the fixed `sleep(10ms)` with a frame limiter that honours `Message::SetFrameRate`. vsync-based pacing comes with the renderer in Phase 4.
  - Web uses the rAF timestamp; `Instant` and `sleep` panic on wasm (or use `web-time`).
- [x] 2.3 **[S] Resources and `Ctx`.** _Done: `resources.rs`, `schedule.rs` (`Ctx`, `EngineCommands`)._ A type-indexed resource store, and a `Ctx` that gives a system access to the world, resources, time, input, commands and events.
- [x] 2.4 **[S] Schedule.** _Done: `Stage`, `Schedule`, `Engine::add_system`; `GameSystem`, the old `add_system` and `GameRunner` are deleted._
  - Stages: `Startup`, `PreUpdate`, `FixedUpdate`, `Update`, `PostUpdate`, `RenderExtract`, `Shutdown`.
  - `engine.add_system(stage, |cx| …)`.
  - FixedUpdate runs N times per frame from the time accumulator.
  - Replaces the unused `GameSystem` trait, `add_system` and the empty `GameRunner`.
- [x] 2.5 **[S] Typed events.** _Done: `events.rs` (`Events<T>`, `EventRegistry`); control messages are readable the same frame as `Events<Message>`._ Double-buffered `Events<T>`. Control messages from the flume queue turn into typed events at the start of each tick, so flume is only used for cross-thread and JS input.
- [x] 2.6 **[S] Headless mode.** _Done: `HeadlessWindow` (no separate null renderer; rendering is skipped without a surface), engine-owned message queue (`engine.sender()`), `Engine::run_frames`, `tests/headless.rs`._ A mock window and a null renderer behind traits, so `cargo test -p efeengine-core` can run 100 ticks without SDL or a GPU. Make the message queue engine-owned so tests don't share it.
- [x] 2.7 **[S] Thread pool hardening.** _Done, except that `CORES` stays an `AtomicUsize` (it is written more than once). The pool no longer panics on `size == 0`, shrink or a missing wasm JS path (the web pool then has zero workers and logs a warning, so its jobs never run); dedicated workers get `Kill` on drop; no `todo!()` left in `web.rs`; the module doc says wgpu, SDL and DOM objects stay off the pool._
  - Replace `static mut CORES` (done as an `AtomicUsize`).
  - Fix the `todo!()` in `thread_pool/web.rs` and the panic on pool shrink.
  - Send Kill to dedicated workers on drop.
  - Don't panic when `set_wasmjs_path` was never called.
  - Keep wgpu, SDL and DOM objects off the pool.

## Phase 3: World with hecs

Goal: entities and components replace `EngineWorld`'s `Vec<GameObject>`.

The world is designed so that scripts (Phase 5) can use it later without touching Rust types or pointers: script-visible data is plain bytes with a described layout, entities are numbers, and every structural change goes through `Commands`.

- [ ] 3.1 **[S] Math and components.** Add `glam`, plus `Transform`, `Velocity`, `Name` and a `MeshHandle` placeholder. Components that scripts may see are `#[repr(C)]` and `bytemuck::Pod`, so they can be copied to and from VM memory as bytes.
- [ ] 3.2 **[S] `hecs::World` in `EngineState`.** Re-export hecs instead of wrapping it. Remove `GameObject` and `add_object`, and port the example. Rust systems use typed hecs queries; don't build a byte-based custom ECS.
- [ ] 3.3 **[S] `Commands`.** Spawn, despawn, insert and remove through a command buffer applied between stages, so despawning inside a query is safe. Rust systems and (later) scripts use the same path.
- [ ] 3.4 **[S] Type registry.** `engine.register_component::<T>()` records, per type: a stable name, a numeric id, size, alignment, the field layout (name, offset, primitive type) and copy-in/copy-out functions.
  - A derive macro in `efeengine-macros` generates the field layout.
  - Types can also be registered at runtime (by a mod), not only at compile time.
  - Entities cross any boundary as `u64` (`Entity::to_bits`), so a stale handle fails cleanly.
  - The same registry later feeds scenes (3.7), the scripting host API (5.2) and the API manifest (5.3).
- [ ] 3.5 **[S] Render extract (data only).** A `RenderExtract` system queries `(&Transform, &MeshHandle)` into a `RenderQueue`. Until Phase 4, a thin adapter feeds the placeholder renderer from the queue, so it never reads the world directly.
- [ ] 3.6 **[S] Movement system and second example.** Velocity moves the transform in FixedUpdate, giving constant speed at any frame rate (unit-tested).
- [ ] 3.7 **[S] Scenes (optional).** Load and save scenes with RON/serde, driven by the type registry from 3.4.

## Phase 4: Rendering with wgpu (last)

Goal: replace the placeholder. The same scene renders, animates and resizes correctly natively, in Chrome (WebGPU) and in Firefox (WebGL2).

- [ ] 4.1 **[S] One `Renderer` owned by `Engine`.**
  - It holds Device, Queue, Surface, config and pipeline, and moves out of the window (the window currently owns the `SurfaceManager`).
  - Delete the dead, half-built `RendererState`.
  - Fallible `async fn new(target, size) -> Result<Self, RenderError>`.
  - It consumes the `RenderQueue` from 3.4.
- [ ] 4.2 **[W] Initialise graphics on web.** Replace `pollster::block_on` with `wasm_bindgen_futures::spawn_local`: build the renderer, store it in the engine, then start rendering in the rAF loop. Use the canvas element from `WebWindow` (1.1), not `getElementById`.
- [ ] 4.3 **[S] Resize.** Connect the `Resized`/`ScaleFactorChanged` events from 1.2 to `renderer.resize()`. Skip frames while the size is 0.
- [ ] 4.4 **[S] Per-object drawing.** Today every object writes the *same* uniform buffer before submit, so all objects draw at the last one's position (`renderer/mod.rs` ~365). Use an instance buffer or dynamic offsets. Switch to `TriangleList`.
- [ ] 4.5 **[S] Vertex buffer.** Add a `Vertex { pos, color }` type, its buffer layout, and a shader that reads vertex attributes. That removes the vertex array hard-coded in WGSL.
- [ ] 4.6 **[S] Surface errors.**
  - `Lost`/`Outdated`: reconfigure and skip the frame.
  - `Timeout`: skip.
  - `OutOfMemory`: exit.
  - Also reconfigure when the frame is `suboptimal`.
  - Remove the `expect`/`unwrap` calls on adapter, device and surface.
- [ ] 4.7 **[S] Timing and present mode.** Use the `Time` resource from 2.2 instead of the renderer's `time += 0.005`, which runs once per object per frame. Prefer `Fifo` explicitly instead of `present_modes[0]`, and let vsync pace native frames.
- [ ] 4.8 **[W] Backends.** Enable wgpu's `webgl` feature for wasm (it's off today, so browsers without WebGPU can't render) and use `BROWSER_WEBGPU | GL`. Create the surface *before* the adapter and pass `compatible_surface`. Pick device limits by backend (downlevel limits only on GL).
- [ ] 4.9 **[W] Colour format.** Choose the surface format deliberately (sRGB view format or gamma in the shader), so colours match between native, WebGPU and WebGL.
- [ ] 4.10 **[S] Camera.** A globals/camera uniform with the aspect ratio (no stretching after resize), then an orthographic 2D camera.
- [ ] 4.11 **[S] Cross-platform check.** Native, Chrome and Firefox all show the identical scene. Window and canvas resize and browser zoom reconfigure without errors. Returning to a background tab doesn't cause a huge delta-time jump.

## Phase 5: Scripting with efescript (future)

Goal: game and mod logic written in efescript runs inside the engine, calls engine and mod functions through a typed host API that can grow at runtime, and has tooling that always knows that API.
Depends on Phase 3 (world, `Commands`, type registry). It does not depend on Phase 4, so it can start before rendering is finished.

Today's efescript API is unstable and static (fixed syscall numbers compiled into the VM). It will be redesigned, not patched. Design rules that keep JIT and AOT possible later:
- A **syscall is just a script call into the host**: one mechanism for engine functions, mod functions and built-ins.
- Host functions are **registered at runtime, by name, with a typed signature**. Scripts import them by name; ids are resolved when a module is loaded, never hard-coded.
- **One calling convention** (ABI) shared by the interpreter, the JIT and AOT-compiled code.
- The bytecode is a stable, verifiable format with an import table and type information. No self-modifying code, and no raw host pointers visible to scripts.

- [ ] 5.1 **VM as an embeddable library.**
  - A `Vm` / `Module` / `Instance` API with no global or static state.
  - Errors instead of panics on bad input; no `todo!()` opcodes; no raw pointers from threads into the runtime.
  - Resumable `run(budget)` with an instruction limit, so an endless loop costs one frame.
  - wasm-safe I/O (no direct `std::fs`/stdin in the VM core).
  - Fix the operand-decoding bugs listed in `crates/efescript/structure.md` as part of the rewrite.
- [ ] 5.2 **Dynamic, structured host calls.**
  - `HostRegistry::register(name, signature, function)` usable at any time, including by a loaded mod that exposes functions to other mods.
  - Signatures use a small type system: integers, floats, bool, entity, string/slice (pointer + length in VM memory), and structs described by the type registry (3.4).
  - Namespaced names (`engine.world.spawn`, `mymod.inventory.add`), with a version per namespace.
  - Modules list their imports; loading fails with a clear error when an import is missing or its signature differs.
  - The efescript crate defines the host trait; it must not depend on the engine.
- [ ] 5.3 **API manifest (the "PDB" for mod developers).**
  - The registry can export everything it knows to a versioned file: functions with signatures and docs, struct and component layouts, namespaces, and a content hash.
  - Exported at build time (`cargo xtask api-export`) and at runtime, so functions registered dynamically by mods are included.
  - A compiled module records the manifest hash it was built against, for compatibility checks.
- [ ] 5.4 **Engine bindings.**
  - The engine registers its host functions: world access (`get`/`set` component bytes, queries), `Commands` (spawn, despawn, insert, remove), input, time, events and logging.
  - A `Script { module, entry, state }` component, and a `ScriptSystem` in the `Update` stage that calls each script's entry with the entity id and delta time.
  - One VM per world, not per entity. Script-private data lives in VM memory; hecs component types are not created dynamically.
- [ ] 5.5 **Mods.** Load and unload modules at runtime, per-mod budgets and permissions (which namespaces a mod may call), and hot reload of scripts on native.
- [ ] 5.6 **Language frontend and language server.**
  - The high-level efescript language (only lexer and AST stubs exist today) compiles to the bytecode with typed imports.
  - The language server reads the manifest from 5.3 and reloads it when the file changes, so completion, signatures and type errors follow the live API automatically.
- [ ] 5.7 **AOT compilation.** Compile modules ahead of time to native code (and to wasm for the web build), using the same ABI and import resolution as the interpreter.
- [ ] 5.8 **JIT compilation.** Native only at first (for example with Cranelift). Browsers cannot run generated machine code, so the web build uses the interpreter or AOT-to-wasm modules.

## Later: missing parts

| Area | What | Priority |
|---|---|---|
| 2D rendering | Textured sprites, sprite batching, depth or sorting, blending | after Phase 4 |
| Assets | `AssetServer` with handles; `std::fs` natively, async `fetch` on web; start with `include_bytes!`. The data side can start before Phase 4. | after Phase 3 |
| Text and UI | Font rendering (`glyphon`/`fontdue`), debug overlay, FPS counter | after sprites |
| Audio | `kira` or `cpal` (both work natively and on web via WebAudio). Independent of rendering, so it can come earlier if needed. | later |
| Scripting | Moved to Phase 5. | after Phase 3 |
| Logging | Use `log` levels (`env_logger` is used as the log-crate logger on native). On a 1-core web build, use a direct logger instead of the worker logger. | soon |
| Profiling | `tracing`/`puffin` spans around each stage | later |
| Hot reload | Native file watcher for assets and shaders | later |
| Tooling | ✅ Done: `cargo xtask` (`xtask/`) is a Rust port of `build.py`, adding `serve` and `--host`. Still to decide: keep both, or delete `build.py` and `scripts/*.py`. | decide |
| Web publishing | GitHub Pages deploy using the `coi-serviceworker` workaround (Pages can't send COOP/COEP headers), or Netlify/Cloudflare using `_headers` | when there's something to show |
| Docs | Keep `structure.md` files current, write a "making a game" guide, more examples | ongoing |

## Bugs to fix along the way

- efepages: a read or write that ends exactly on a page boundary across several pages drops the last page (verified: writing 8192 bytes and reading them back returns 4096).
- efescript: operand-decoding bugs and `todo!()` opcodes, listed in `crates/efescript/structure.md`.
- calc: integer division by zero panics.
- Renderer bugs are collected in Phase 4 (shared uniform buffer, missing `webgl` feature, no surface-error handling).

## Notes

- **Rewriting the build script in Rust (`xtask`).** A native-only binary crate `xtask/` with the same commands as `build.py`, run as `cargo xtask` through a `[alias] xtask = "run --package xtask --"` entry in `.cargo/config.toml`. Manifests would be parsed with `cargo_metadata`/`toml`, and the COOP/COEP dev server is about 100 lines on `TcpListener` (or use `tiny_http`). That gives one language for the whole repo with no Python dependency. The costs: more code, a few seconds to compile it on first use, and it must be excluded from wasm checks.
  Suggested crates (they're built only when you run `cargo xtask`, so they don't affect the engine or wasm builds):
  - `clap` (derive) for subcommands and flags;
  - `tiny_http` for the COOP/COEP dev server, with no async runtime;
  - `dialoguer` for the arrow-key example and native/web menu;
  - `cargo_metadata` for examples and the target dir;
  - `anyhow` for errors;
  - optionally `notify` for a rebuild-on-change `watch` command.
