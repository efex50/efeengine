# efeengine roadmap

Living document. Tick items off (`- [x]`) as they land and edit freely. Each task
lists the main files it touches and how to tell it's done.
Tags: **[N]** native (SDL3) · **[W]** web (wasm) · **[S]** shared by both.

_Last updated: 2026-11-14_

**Order:** IO → game systems → hecs world → rendering (wgpu).
Rendering is deliberately last: until Phase 4 the current wgpu code stays as a
**placeholder**. The native triangle keeps working as it does now, and on the web
nothing is drawn, which is fine. Earlier phases are tested through logs, the
native window and headless unit tests, not through rendering.

## Where to focus now

**Phase 1, starting with the shared foundation (1.1–1.5).**
1. Make the window report its real size and send resize/focus/close events on both platforms (1.1–1.2).
2. Define one `InputEvent` type and an `InputState` you can query (1.3–1.4).
3. Deliver input through the engine instead of the global message queue (1.5). Today every key event ends in a `"Todo window event"` warning.

After that, native (1.6–1.7) and web (1.8–1.10) input can proceed in parallel.

On the web you can already test input without graphics: the requestAnimationFrame (rAF) loop runs `tick()` (and therefore `poll_events`) even though no renderer exists.

## Phase 0: Housekeeping (small, do alongside Phase 1)

- [ ] 0.1 Commit or finish the work in progress in `engine.rs`, `thread_pool.rs`, `systems/mod.rs` and `Cargo.toml` (the `init_global_pool_auto` refactor).
- [ ] 0.2 Stop the console flood: `sdl_backend.rs` calls `global_alert("event got!")` for every unhandled SDL event, including each mouse move. Turn it into a debug-level log, or remove it.
- [ ] 0.3 Refresh `crates/core/structure.md`: it still lists hecs as dev-only.
- [ ] 0.4 Fix CI (`.github/workflows/rust.yml`). It needs nightly + `rust-src`, SDL3 dev packages, and a wasm `cargo check` step; it should also run `cargo fmt --check` and `clippy`.
- [ ] 0.5 Results of the health scan (2026-11-14). Native and wasm `cargo check` both pass.
  - The `Engine::new` doc example (`engine.rs:53`) is stale and makes `cargo test` fail: it passes 2 arguments, and the function takes 1.
  - Remove or use the unused dependencies `env_logger`, `serde_json` and `console_log`. `hecs` is used only in a test until Phase 3.
  - Unused `Rc`/`RefCell` import on wasm (`lib.rs:16`).
  - Install clippy for nightly: `rustup component add --toolchain nightly clippy`.
  - Baseline in `crates/core/src` (2,417 lines): 38 `unwrap`, 11 `expect`, 30 `unsafe`, 9 statics, 1 `todo!()` (`thread_pool/web.rs:48`), 17 `println!`/`dbg!`.

## Phase 1: IO system (window + input), SDL3 and web

Goal: one platform-agnostic window, event and input-state API that behaves the same in both builds. No rendering work.

### Window size and window events
- [ ] 1.1 **[W] Real canvas.** `WebWindow` resolves the `HtmlCanvasElement` once (id from `SetCanvasId`, default `"canvas"`). It sets the backing-store size to CSS size × `devicePixelRatio`, and `size()` returns those physical pixels instead of `(0,0)`.
  - Add `web-sys` as a direct dependency with the features this phase needs: `Document`, `HtmlCanvasElement`, `KeyboardEvent`, `PointerEvent`, `WheelEvent`, `FocusEvent`, `ResizeObserver*`, `MediaQueryList`, `AddEventListenerOptions`, …
  - The placeholder renderer keeps its own `getElementById` until Phase 4.

  _Files:_ `Cargo.toml`, `window/web_backend.rs`. _Done:_ the logged size matches the canvas at DPR 1 and 2.
- [ ] 1.2 **[S] Window events.**
  - Native uses `size_in_pixels()` and the SDL `PixelSizeChanged`/`DisplayScaleChanged`, focus and close events.
  - Web uses a `ResizeObserver` (`devicePixelContentBoxSize`), `focus`/`blur` and `visibilitychange`.
  - Both send `Resized`, `ScaleFactorChanged`, `Focused` and `CloseRequested`, plus an initial `Resized` at startup. The engine stores the current size and scale.

  Connecting this to `renderer.resize()` is task 4.3. _Files:_ `sdl_backend.rs`, `web_backend.rs`, `engine.rs`. _Done:_ resizing the window or the tab logs correct sizes.

### Shared foundation
- [ ] 1.3 **[S] Event types.** Add an `InputEvent` enum (sketch below) and the types it uses: `Modifiers`, `ElementState`, a logical `Key` (`Named(NamedKey) | Character(String)`), `WindowId`, and a `timestamp_ns` normalised across platforms. Keep `KeyCode` as the *physical* key only, and add the keys it's missing (IntlBackslash, PrintScreen, ContextMenu, F13–F24, media keys). Remove the SDL-specific `raw: u16` and the unused `WindowEvent` stub. _Files:_ `window/events.rs` (or a new `input/` module).

  ```text
  InputEvent = Key { physical, logical, state, repeat, mods }
             | Text(String)
             | MouseMotion { pos, delta } | MouseButton | MouseWheel { delta, unit }
             | Touch { id, phase, pos }
             | Window(Resized | ScaleFactorChanged | Focused | CloseRequested)
  ```
- [ ] 1.4 **[S] `InputState` resource.**
  - `begin_frame()`, `apply(&InputEvent)`.
  - Queries: `pressed`, `just_pressed`, `just_released`, `mouse_pos`, `mouse_delta`, `wheel`, `text`, `modifiers`, `focused`.
  - Key repeats must not count as `just_pressed`, and losing focus clears held keys.
  - Unit-tested, with no window needed.

  _Files:_ new `engine/input.rs`, `engine.rs`, `prelude.rs`.
- [ ] 1.5 **[S] Engine-owned input path.**
  - `InnerWindow::poll_events(&mut self, out: &mut Vec<InputEvent>)`.
  - Tick order: `begin_frame`, then poll, then apply, then update.
  - Remove `Message::WindowEvent` and the "Todo window event" handler, so input no longer goes through the global queue and the one-tick delay disappears. `Message` stays for control commands and the JS bridge.

  _Files:_ `window/mod.rs`, `messages.rs`, `engine.rs`.

### Native (SDL3)
- [ ] 1.6 **[N] Full SDL mapping.**
  - Keys with `repeat`, and a logical key from the keycode, which is layout-aware.
  - Text input: call `start_text_input`.
  - Mouse motion, buttons and wheel.
  - Touch.
  - Map `Quit` to `CloseRequested`.

  _Files:_ `sdl_backend.rs`, `sdl_impls.rs`.
- [ ] 1.7 **[N] Thread-safety cleanup.**
  - Drop `Send + Sync` from `InnerWindow`, and remove the `unsafe impl Send/Sync` on the window types (SDL is main-thread only).
  - Delete the unused `Mutex<Sdl>`.
  - On Kill, end the loop cleanly instead of calling `process::exit(0)`, so `Drop` runs.

### Web (DOM)
- [ ] 1.8 **[W] Keyboard and focus.**
  - Listeners for `keydown`/`keyup`: `code` gives the physical key, `key` the logical key, plus `repeat` and the modifiers.
  - Clear held keys on `blur` and `visibilitychange`.
  - Canvas gets `tabindex=0` and takes focus on pointerdown.
  - Call `preventDefault` selectively, for arrows, Space and Tab.
  - Register the listeners from Rust with `web-sys` closures stored in `WebWindow`; don't add a JS shim.

  _Files:_ new `window/web_input.rs`, `templates/web/index.html`.
- [ ] 1.9 **[W] Pointer and wheel.**
  - Pointer events (mouse, touch and pen) with `setPointerCapture`, and coordinates in canvas physical pixels.
  - Wheel events using `deltaMode`.
  - Suppress `contextmenu`, and add `touch-action: none` to the canvas CSS.
- [ ] 1.10 **[W] DPR changes and text input.** Detect zoom and DPR changes with a `matchMedia` listener. Take text from `beforeinput`/`input`, falling back to the `key` value for printable keys.

### Polish
- [ ] 1.11 **[S] Window API.**
  - `scale_factor()` and `logical_size()`.
  - `set_title` that also changes `document.title` on web.
  - Cursor visibility and grab, and pointer lock (relative mouse mode on native, `requestPointerLock` on web).
- [ ] 1.12 **[S] Gamepad (can wait).** SDL's gamepad subsystem on native, and polling `navigator.getGamepads()` once per frame on web.
- [ ] 1.13 **[S] Input test example.** An example that logs `InputState` changes (keys, mouse, focus, size), so the same manual test runs natively and in the browser console. Unit tests cover `InputState`. Update `structure.md`.

## Phase 2: Engine loop and game systems

Goal: games run their own code each frame, with time, input and resources available.

- [ ] 2.1 **[S] Errors and config.**
  - `EngineError` (thiserror), with `Engine::new` returning `Result`.
  - `EngineConfig { title, size, vsync, fixed_dt, threads, canvas_id }`; the 800×600 size and the `"canvas"` id are hard-coded today.
- [ ] 2.2 **[S] `Time` and frame loop.**
  - `Time` resource: delta, fixed delta, elapsed time, frame count and interpolation alpha.
  - A shared `Engine::frame(now)` with clamped delta time and a fixed-step accumulator.
  - Native uses `Instant` and replaces the fixed `sleep(10ms)` with a frame limiter that honours `Message::SetFrameRate`. vsync-based pacing comes with the renderer in Phase 4.
  - Web uses the rAF timestamp; `Instant` and `sleep` panic on wasm (or use `web-time`).
- [ ] 2.3 **[S] Resources and `Ctx`.** A type-indexed resource store, and a `Ctx` that gives a system access to the world, resources, time, input, commands and events.
- [ ] 2.4 **[S] Schedule.**
  - Stages: `Startup`, `PreUpdate`, `FixedUpdate`, `Update`, `PostUpdate`, `RenderExtract`, `Shutdown`.
  - `engine.add_system(stage, |cx| …)`.
  - FixedUpdate runs N times per frame from the time accumulator.
  - Replaces the unused `GameSystem` trait, `add_system` and the empty `GameRunner`.
- [ ] 2.5 **[S] Typed events.** Double-buffered `Events<T>`. Control messages from the flume queue turn into typed events at the start of each tick, so flume is only used for cross-thread and JS input.
- [ ] 2.6 **[S] Headless mode.** A mock window and a null renderer behind traits, so `cargo test -p efeengine-core` can run 100 ticks without SDL or a GPU. Make the message queue engine-owned so tests don't share it.
- [ ] 2.7 **[S] Thread pool hardening.**
  - Replace `static mut CORES` with `OnceCell`.
  - Fix the `todo!()` in `thread_pool/web.rs` and the panic on pool shrink.
  - Send Kill to dedicated workers on drop.
  - Don't panic when `set_wasmjs_path` was never called.
  - Keep wgpu, SDL and DOM objects off the pool.

## Phase 3: World with hecs

Goal: entities and components replace `EngineWorld`'s `Vec<GameObject>`.

- [ ] 3.1 **[S] Math and components.** Add `glam`, plus `Transform`, `Velocity`, `Name` and a `MeshHandle` placeholder.
- [ ] 3.2 **[S] `hecs::World` in `EngineState`.** Re-export hecs instead of wrapping it. Remove `GameObject` and `add_object`, and port the example.
- [ ] 3.3 **[S] `Commands`.** Spawn, despawn and insert through a command buffer applied between stages, so despawning inside a query is safe.
- [ ] 3.4 **[S] Render extract (data only).** A `RenderExtract` system queries `(&Transform, &MeshHandle)` into a `RenderQueue`. Until Phase 4, a thin adapter feeds the placeholder renderer from the queue, so it never reads the world directly.
- [ ] 3.5 **[S] Movement system and second example.** Velocity moves the transform in FixedUpdate, giving constant speed at any frame rate (unit-tested).
- [ ] 3.6 **[S] Scenes (optional).** Load and save scenes with RON/serde and hecs serialization.

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

## Later: missing parts

| Area | What | Priority |
|---|---|---|
| 2D rendering | Textured sprites, sprite batching, depth or sorting, blending | after Phase 4 |
| Assets | `AssetServer` with handles; `std::fs` natively, async `fetch` on web; start with `include_bytes!`. The data side can start before Phase 4. | after Phase 3 |
| Text and UI | Font rendering (`glyphon`/`fontdue`), debug overlay, FPS counter | after sprites |
| Audio | `kira` or `cpal` (both work natively and on web via WebAudio). Independent of rendering, so it can come earlier if needed. | later |
| Scripting | Run the efescript VM inside the Update stage, talking to the engine through `Message`s and `Commands`. First make the VM safe to host: no panics on bad input, no raw pointers, wasm-safe I/O (see `crates/efescript/structure.md`). | after Phase 3 |
| Logging | Use `log` levels (`env_logger` is declared but unused). On a 1-core web build, use a direct logger instead of the worker logger. | soon |
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
