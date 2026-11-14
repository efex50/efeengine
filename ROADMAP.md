# efeengine roadmap

Living document. Tick items off (`- [x]`) as they land and edit freely. Each task
lists the main files it touches and how to tell it's done.
Tags: **[N]** native (SDL3) · **[W]** web (wasm) · **[S]** shared by both.

_Last updated: 2026-11-14_

## Where to focus now

**Phase 1, starting with tasks 1.1–1.3.**
- You can't test IO on the web until something is on screen there, and the web build currently draws nothing: `wasm_loop_start` never initialises graphics.
- `WebWindow::size()` returns `(0,0)` and nothing calls `resize()`.
- These three tasks are both the base of the IO system and the blocker for the triangle, so they unblock everything after them.

Then do the shared input foundation (1.4–1.6), after which native and web input (1.7–1.11) can proceed in parallel.

## Phase 0: Housekeeping (small, do alongside 1.1)

- [ ] 0.1 Commit or finish the work in progress in `engine.rs`, `thread_pool.rs`, `systems/mod.rs` and `Cargo.toml` (the `init_global_pool_auto` refactor).
- [ ] 0.2 Stop the console flood: `sdl_backend.rs` calls `global_alert("event got!")` for every unhandled SDL event, including each mouse move. Turn it into a debug-level log, or remove it.
- [ ] 0.3 Refresh `crates/core/structure.md`: it still mentions pyo3 tests and lists hecs as dev-only.
- [ ] 0.4 Fix CI (`.github/workflows/rust.yml`). It needs nightly + `rust-src`, SDL3 dev packages, and a wasm `cargo check` step; it should also run `cargo fmt --check` and `clippy`.
- [ ] 0.5 Results of the health scan (2026-11-14). Native and wasm `cargo check` both pass.
  - The `Engine::new` doc example (`engine.rs:53`) is stale and makes `cargo test` fail: it passes 2 arguments, and the function takes 1.
  - Remove or use the unused dependencies `env_logger`, `serde_json` and `console_log`. `hecs` is used only in a test until Phase 4.
  - Unused `Rc`/`RefCell` import on wasm (`lib.rs:16`).
  - Install clippy for nightly: `rustup component add --toolchain nightly clippy`.
  - Baseline in `crates/core/src` (2,417 lines): 38 `unwrap`, 11 `expect`, 30 `unsafe`, 9 statics, 1 `todo!()` (`thread_pool/web.rs:48`), 17 `println!`/`dbg!`.

## Phase 1: IO system (window + input), SDL3 and web

Goal: one platform-agnostic event and input-state API that behaves the same in both builds.

### Unblock web and resize (do first)
- [ ] 1.1 **[W] Initialise graphics on web.** Replace `pollster::block_on` with `wasm_bindgen_futures::spawn_local`: build the renderer, store it in the engine, then start the requestAnimationFrame (rAF) loop. Until the renderer exists, a tick must not try to render. _Files:_ `engine.rs` (`wasm_loop_start`). _Done:_ the browser console logs graphics init.
- [ ] 1.2 **[W] Real canvas.** `WebWindow` resolves the `HtmlCanvasElement` once (id from `SetCanvasId`, default `"canvas"`). The backing-store size is CSS size × `devicePixelRatio`, and `size()` returns those physical pixels. The renderer takes the element from the window instead of calling `getElementById("canvas")`. Add the `web-sys` features (`Document`, `HtmlCanvasElement`, `ResizeObserver*`, …) as a direct dependency. _Files:_ `Cargo.toml`, `window/web_backend.rs`, `renderer/mod.rs`. _Done:_ size isn't 0, and output is crisp at DPR 2.
- [ ] 1.3 **[S] Resize pipeline.**
  - Native uses `size_in_pixels()` and the SDL `PixelSizeChanged`/`DisplayScaleChanged` events.
  - Web uses a `ResizeObserver` (`devicePixelContentBoxSize`).
  - Both call `resize()` on the renderer.
  - Skip frames while the size is 0, and send an initial `Resized` event at startup.

  _Files:_ `sdl_backend.rs`, `web_backend.rs`, `engine.rs`, `renderer/mod.rs`. _Done:_ dragging the window or resizing the tab never stretches or logs surface errors.

### Shared foundation
- [ ] 1.4 **[S] Event types.** Add an `InputEvent` enum (sketch below) and the types it uses: `Modifiers`, `ElementState`, a logical `Key` (`Named(NamedKey) | Character(String)`), `WindowId`, and a `timestamp_ns` normalised across platforms. Keep `KeyCode` as the *physical* key only, and add the keys it's missing (IntlBackslash, PrintScreen, ContextMenu, F13–F24, media keys). Remove the SDL-specific `raw: u16` and the unused `WindowEvent` stub. _Files:_ `window/events.rs` (or a new `input/` module).

  ```text
  InputEvent = Key { physical, logical, state, repeat, mods }
             | Text(String)
             | MouseMotion { pos, delta } | MouseButton | MouseWheel { delta, unit }
             | Touch { id, phase, pos }
             | Window(Resized | ScaleFactorChanged | Focused | CloseRequested)
  ```
- [ ] 1.5 **[S] `InputState` resource.**
  - `begin_frame()`, `apply(&InputEvent)`.
  - Queries: `pressed`, `just_pressed`, `just_released`, `mouse_pos`, `mouse_delta`, `wheel`, `text`, `modifiers`, `focused`.
  - Key repeats must not count as `just_pressed`, and losing focus clears held keys.
  - Unit-tested, with no window needed.

  _Files:_ new `engine/input.rs`, `engine.rs`, `prelude.rs`.
- [ ] 1.6 **[S] Engine-owned input path.**
  - `InnerWindow::poll_events(&mut self, out: &mut Vec<InputEvent>)`.
  - Tick order: `begin_frame`, then poll, then apply, then update.
  - Remove `Message::WindowEvent` and the "Todo window event" handler, so input no longer goes through the global queue and the one-tick delay disappears. `Message` stays for control commands and the JS bridge.

  _Files:_ `window/mod.rs`, `messages.rs`, `engine.rs`.

### Native (SDL3)
- [ ] 1.7 **[N] Full SDL mapping.**
  - Keys with `repeat`, and a logical key from the keycode, which is layout-aware.
  - Text input: call `start_text_input`.
  - Mouse motion, buttons and wheel.
  - Window events: resize, scale, focus, close; map `Quit` to `CloseRequested`.
  - Touch.

  _Files:_ `sdl_backend.rs`, `sdl_impls.rs`.
- [ ] 1.8 **[N] Thread-safety cleanup.**
  - Drop `Send + Sync` from `InnerWindow`, and remove the `unsafe impl Send/Sync` on the window types (SDL is main-thread only).
  - Delete the unused `Mutex<Sdl>`.
  - On Kill, end the loop cleanly instead of calling `process::exit(0)`, so `Drop` runs.

### Web (DOM)
- [ ] 1.9 **[W] Keyboard and focus.**
  - Listeners for `keydown`/`keyup`: `code` gives the physical key, `key` the logical key, plus `repeat` and the modifiers.
  - Clear held keys on `blur` and `visibilitychange`.
  - Canvas gets `tabindex=0` and takes focus on pointerdown.
  - Call `preventDefault` selectively, for arrows, Space and Tab.
  - Register the listeners from Rust with `web-sys` closures stored in `WebWindow`; don't add a JS shim.

  _Files:_ new `window/web_input.rs`, `templates/web/index.html`.
- [ ] 1.10 **[W] Pointer and wheel.**
  - Pointer events (mouse, touch and pen) with `setPointerCapture`, and coordinates in canvas physical pixels.
  - Wheel events using `deltaMode`.
  - Suppress `contextmenu`, and add `touch-action: none` to the canvas CSS.
- [ ] 1.11 **[W] DPR changes and text input.** Detect zoom and DPR changes with a `matchMedia` listener. Take text from `beforeinput`/`input`, falling back to the `key` value for printable keys.

### Polish
- [ ] 1.12 **[S] Window API.**
  - `scale_factor()` and `logical_size()`.
  - `set_title` that also changes `document.title` on web.
  - Cursor visibility and grab, and pointer lock (relative mouse mode on native, `requestPointerLock` on web).
- [ ] 1.13 **[S] Gamepad (can wait).** SDL's gamepad subsystem on native, and polling `navigator.getGamepads()` once per frame on web.
- [ ] 1.14 **[S] Example and docs.** Make `test_example` move the triangle with WASD and the mouse, as a manual test on both platforms, and update `structure.md`.

## Phase 2: Triangle on both platforms

Goal: the same triangle renders, animates and resizes correctly natively, in Chrome (WebGPU) and in Firefox (WebGL2).

- [ ] 2.1 **[S] One `Renderer` owned by `Engine`.**
  - It holds Device, Queue, Surface, config and pipeline, and moves out of the window (the window currently owns the `SurfaceManager`).
  - Delete the dead, half-built `RendererState`.
  - Fallible `async fn new(target, size) -> Result<Self, RenderError>`.
- [ ] 2.2 **[S] Fix per-object drawing.** Every object writes the *same* uniform buffer before submit, so all objects draw at the last one's position (`renderer/mod.rs` ~365). Use an instance buffer or dynamic offsets. Switch to `TriangleList`. _Done:_ both example objects appear at ±0.5.
- [ ] 2.3 **[S] Vertex buffer.** Add a `Vertex { pos, color }` type, its buffer layout, and a `triangle.wgsl` that reads vertex attributes. That removes the vertex array hard-coded in WGSL.
- [ ] 2.4 **[S] Surface errors.**
  - `Lost`/`Outdated`: reconfigure and skip the frame.
  - `Timeout`: skip.
  - `OutOfMemory`: exit.
  - Also reconfigure when the frame is `suboptimal`.
  - Remove the `expect`/`unwrap` calls on adapter, device and surface.
- [ ] 2.5 **[S] Frame timing.**
  - Shared `Engine::frame(now)`: clamped delta time, a fixed-step accumulator, and render interpolation alpha.
  - Native: `Instant` for timing, and drop the `sleep(10ms)` in favour of vsync (prefer `Fifo` explicitly instead of `present_modes[0]`).
  - Web: use the rAF timestamp; `Instant` and `sleep` panic on wasm.
  - Replace the renderer's `time += 0.005`, which runs once per object per frame, with real elapsed time.
- [ ] 2.6 **[W] Backends.** Enable wgpu's `webgl` feature for wasm and use `BROWSER_WEBGPU | GL`. Create the surface *before* the adapter and pass `compatible_surface`. Pick device limits by backend (downlevel limits only on GL).
- [ ] 2.7 **[W] Colour format.** Choose the surface format deliberately (sRGB view format or gamma in the shader), so colours match between native, WebGPU and WebGL.
- [ ] 2.8 **[S] Aspect-ratio uniform.** Add a globals/camera uniform so the triangle isn't stretched after a resize.
- [ ] 2.9 **[S] Cross-platform check.** Native, Chrome and Firefox all show the identical triangle. Window and canvas resize and browser zoom reconfigure without errors. Returning to a background tab doesn't cause a huge delta-time jump.

## Phase 3: Engine loop and game systems

Goal: games run their own code each frame, with time, input and resources available.

- [ ] 3.1 **[S] Errors and config.**
  - `EngineError` (thiserror), with `Engine::new` returning `Result`.
  - `EngineConfig { title, size, vsync, fixed_dt, threads, canvas_id }`; the 800×600 size and the `"canvas"` id are hard-coded today.
- [ ] 3.2 **[S] `Time` resource.** Delta, fixed delta, elapsed time, frame count and interpolation alpha, with a clock that works on wasm (`web-time` or `performance.now()`).
- [ ] 3.3 **[S] Resources and `Ctx`.** A type-indexed resource store, and a `Ctx` that gives a system access to the world, resources, time, input, commands, events and the render queue.
- [ ] 3.4 **[S] Schedule.**
  - Stages: `Startup`, `PreUpdate`, `FixedUpdate`, `Update`, `PostUpdate`, `RenderExtract`, `Shutdown`.
  - `engine.add_system(stage, |cx| …)`.
  - FixedUpdate runs N times per frame from the time accumulator.
  - Replaces the unused `GameSystem` trait, `add_system` and the empty `GameRunner`.
- [ ] 3.5 **[S] Typed events.** Double-buffered `Events<T>`. Control messages from the flume queue turn into typed events at the start of each tick, so flume is only used for cross-thread and JS input.
- [ ] 3.6 **[S] Headless mode.** A mock window and a null renderer behind traits, so `cargo test -p efeengine-core` can run 100 ticks without SDL or a GPU. Make the message queue engine-owned so tests don't share it.
- [ ] 3.7 **[S] Thread pool hardening.**
  - Replace `static mut CORES` with `OnceCell`.
  - Fix the `todo!()` in `thread_pool/web.rs` and the panic on pool shrink.
  - Send Kill to dedicated workers on drop.
  - Don't panic when `set_wasmjs_path` was never called.
  - Keep wgpu, SDL and DOM objects off the pool.

## Phase 4: World with hecs

Goal: entities and components replace `EngineWorld`'s `Vec<GameObject>`.

- [ ] 4.1 **[S] Math and components.** Add `glam`, plus `Transform`, `Velocity`, `Name` and a `MeshHandle` placeholder.
- [ ] 4.2 **[S] `hecs::World` in `EngineState`.** Re-export hecs instead of wrapping it. Remove `GameObject` and `add_object`, and port the example.
- [ ] 4.3 **[S] `Commands`.** Spawn, despawn and insert through a command buffer applied between stages, so despawning inside a query is safe.
- [ ] 4.4 **[S] Render extract.** A `RenderExtract` system queries `(&Transform, &MeshHandle)` into a `RenderQueue`, and the renderer only reads that queue, never the world.
- [ ] 4.5 **[S] Movement system and second example.** Velocity moves the transform in FixedUpdate, giving constant speed at any frame rate (unit-tested).
- [ ] 4.6 **[S] Scenes (optional).** Load and save scenes with RON/serde and hecs serialization.

## Later: missing parts

| Area | What | Priority |
|---|---|---|
| 2D rendering | Textured sprites, sprite batching, orthographic camera, depth or sorting, blending | after Phase 4 |
| Assets | `AssetServer` with handles; `std::fs` natively, async `fetch` on web; start with `include_bytes!` | after Phase 2 |
| Text and UI | Font rendering (`glyphon`/`fontdue`), debug overlay, FPS counter | after sprites |
| Audio | `kira` or `cpal` (both work natively and on web via WebAudio) | later |
| Scripting | Run the efescript VM inside the Update stage, talking to the engine through `Message`s and `Commands`. First make the VM safe to host: no panics on bad input, no raw pointers, wasm-safe I/O (see `crates/efescript/structure.md`). | after Phase 4 |
| Logging | Use `log` levels (`env_logger` is declared but unused). On a 1-core web build, use a direct logger instead of the worker logger. | soon |
| Profiling | `tracing`/`puffin` spans around each stage | later |
| Hot reload | Native file watcher for assets and shaders | later |
| Tooling | Rewrite `build.py` as a Rust `xtask` crate (`cargo xtask run test_example web`); details in the notes. Optional. | when convenient |
| Web publishing | GitHub Pages deploy using the `coi-serviceworker` workaround (Pages can't send COOP/COEP headers), or Netlify/Cloudflare using `_headers` | when there's something to show |
| Docs | Keep `structure.md` files current, write a "making a game" guide, more examples | ongoing |

## Bugs to fix along the way

- efepages: a read or write that ends exactly on a page boundary across several pages drops the last page (verified: writing 8192 bytes and reading them back returns 4096).
- efescript: operand-decoding bugs and `todo!()` opcodes, listed in `crates/efescript/structure.md`.
- calc: integer division by zero panics.

## Notes

- **Rewriting the build script in Rust (`xtask`).** A native-only binary crate `xtask/` with the same commands as `build.py`, run as `cargo xtask` through a `[alias] xtask = "run --package xtask --"` entry in `.cargo/config.toml`. Manifests would be parsed with `cargo_metadata`/`toml`, and the COOP/COEP dev server is about 100 lines on `TcpListener` (or use `tiny_http`). That gives one language for the whole repo with no Python dependency. The costs: more code, a few seconds to compile it on first use, and it must be excluded from wasm checks.
  Suggested crates (they're built only when you run `cargo xtask`, so they don't affect the engine or wasm builds):
  - `clap` (derive) for subcommands and flags;
  - `tiny_http` for the COOP/COEP dev server, with no async runtime;
  - `dialoguer` for the arrow-key example and native/web menu;
  - `cargo_metadata` for examples and the target dir;
  - `anyhow` for errors;
  - optionally `notify` for a rebuild-on-change `watch` command.
