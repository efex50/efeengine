# efeengine roadmap

Living document. Tick items off (`- [x]`) as they land and edit freely. Each task
lists the main files it touches and how to tell it's done.
Tags: **[N]** native (SDL3) · **[W]** web (wasm) · **[S]** shared by both.

_Last updated: 2026-11-16_

**Order:** IO → game systems → hecs world → rendering (wgpu: 2D first, then 3D) → UI (Phase 6) → editor and live iteration (Phase 7, last). Scripting (Phase 5) needs only the world, so it can run alongside rendering. UI needs the Phase 4 renderer.
Rendering is deliberately last: until Phase 4 the current wgpu code stays as a
**placeholder**. The native triangle keeps working as it does now, and on the web
nothing is drawn, which is fine. Earlier phases are tested through logs, the
native window and headless unit tests, not through rendering.

## Where to focus now

**Phase 1 is done apart from the manual checks below, then Phase 3.**
1. Check the native-window and browser input paths by hand with `cargo xtask run input_test native` and `cargo xtask run input_test web`: they are compile-checked only so far. Include a gamepad (1.12) in both runs.
2. Phase 3 (hecs world, `Commands`, type registry), now that the input system, schedule, fixed-step loop and headless mode (Phases 1–2) are in.
3. Scripting E3–E5 (host registry, API index, engine bindings, mods) is done, together with 3.1–3.4. Next for Phase 3: 3.5–3.7. Not yet checked by hand: `cargo xtask run script_demo native` (window) and a web dist. See `crates/efescript/ROADMAP.md`.
4. Scripting modules and `.efmod` mod packages are done (5.9, efescript E8); next for scripting is the language (efescript E6).
5. Engine loading and caches are done (efescript E10): one `Engine::load` for `.efa`, `.efvm`, `.efmod` and mod folders, a memory and disk `ScriptCache`, pluggable mod sources and asset handlers, and the new `crates/efa` compiler crate.

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

  Connecting this to `renderer.resize()` is task 4A.4. _Files:_ `sdl_backend.rs`, `web_backend.rs`, `engine.rs`. _Done:_ resizing the window or the tab logs correct sizes.

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
- [x] 2.8 **[S] Hybrid parallel scheduling.** _Done: `engine/parallel.rs` (`Executor`, `Access`, `ParCtx`), `Engine::add_par_system`, `Threads` / `Engine::set_threads` / `Message::SetThreads`, par `velocity_system` and `propagate_transforms`, script lanes with buffered writes, spawn pools and `ScriptStats`; resources, events and host closures are `Send + Sync`. A frame with `Threads::Single` and `Threads::Fixed(n)` gives a byte-identical world (`tests/parallel.rs`); benchmark in `tests/parallel_bench.rs`; coin_dash movement and collisions are par systems._
  - Structure goes through `Commands`, Rust data through declared `Access`, scripts through per-lane buffers merged in a fixed order.
  - Docs: `docs/THREADING.md` (user guide), `docs/PARALLEL_SPEC.md` (contract and deviations).

## Phase 3: World with hecs

Goal: entities and components replace `EngineWorld`'s `Vec<GameObject>`.

The world is designed so that scripts (Phase 5) can use it later without touching Rust types or pointers: script-visible data is plain bytes with a described layout, entities are numbers, and every structural change goes through `Commands`.

- [x] 3.1 **[S] Math and components.** Add `glam`, plus `Transform`, `Velocity`, `Name` and a `MeshHandle` placeholder. Components that scripts may see are `#[repr(C)]` and `bytemuck::Pod`, so they can be copied to and from VM memory as bytes. _Done: `glam` (with `bytemuck`), `Transform`, `Velocity`, `MeshHandle` (`#[repr(C)]`, `Pod`, `#[derive(ScriptType)]`) and the Rust-only `Name` in `engine/components.rs`. Tests: `components::tests::layouts_match_offset_of`._
- [x] 3.2 **[S] `hecs::World` in `EngineState`.** Re-export hecs instead of wrapping it. Remove `GameObject` and `add_object`, and port the example. Rust systems use typed hecs queries; don't build a byte-based custom ECS. _Done: `EngineState.world` is a `hecs::World`; `GameObject`/`EngineWorld`/`add_object` are gone; `test_example` spawns `(Transform, MeshHandle)`. The placeholder renderer draws one triangle per `(Transform, MeshHandle)`; that path and the native window are compile-checked only (no window was opened)._
- [x] 3.3 **[S] `Commands`.** Spawn, despawn, insert and remove through a command buffer applied between stages, so despawning inside a query is safe. Rust systems and (later) scripts use the same path. _Done: `Commands` (`engine/commands.rs`, `EngineCommands` is an alias): `cx.spawn`, `despawn`, `insert`, `remove`, byte-level `insert_raw`/`remove_raw`, applied in order after every stage; operations on dead entities are ignored. Tests: `tests/world.rs` (`despawn_inside_a_query_is_safe`, `hecs_reserved_entities`, …)._
- [x] 3.4 **[S] Type registry.** `engine.register_component::<T>()` records, per type: a stable name, a numeric id, size, alignment, the field layout (name, offset, primitive type) and copy-in/copy-out functions. _Done: `TypeRegistry` (`engine/types.rs`): name, id (`fnv1a32(name)`), size, alignment, `efevm::StructDesc` layout, byte read/write/insert/remove; `#[derive(ScriptType)]` in `efeengine-macros`; runtime layout-only types via `Engine::register_struct`; `entity_to_bits`/`entity_from_bits`. `Engine::register_component` registers in the type registry and in the scripting index in one call. Scenes (3.7) do not use it yet. Tests: `tests/world.rs`, `types::tests`, `tests/api_index.rs`._
  - A derive macro in `efeengine-macros` generates the field layout.
  - Types can also be registered at runtime (by a mod), not only at compile time.
  - Entities cross any boundary as `u64` (`Entity::to_bits`), so a stale handle fails cleanly.
  - The same registry later feeds scenes (3.7), the scripting host API (5.2) and the API manifest (5.3).
- [x] 3.5 **[S] Render extract (data only).** _Done, and replaced: instead of a `RenderQueue`, the `RenderExtract` system builds a `RenderFrame` resource (`engine/render/frame.rs`): the one active camera with ready view/projection matrices, sorted `SpriteInstance`s, `MeshInstance`s, texture-store changes (once each), frame number and time. Also new: components `Camera`, `Sprite`, `TextureHandle`, `Visibility`, `Parent`, `Children`, `GlobalTransform`, the `Textures` store, and transform propagation (`PostUpdate`). Tests: `tests/render.rs`. See `docs/RENDERING.md`._
- [x] 3.6 **[S] Movement system and second example.** Velocity moves the transform in FixedUpdate, giving constant speed at any frame rate (unit-tested). _Done: `transform::velocity_system` (fixed delta), test `velocity_integrates_with_the_fixed_delta`. Second example: `examples/custom_renderer` (a renderer template)._
- [ ] 3.7 **[S] Scenes (optional).** Load and save scenes with RON/serde, driven by the type registry from 3.4.

## Phase 4: Rendering with wgpu (2D first, then 3D)

Goal: a 2D renderer first (4A), then 3D (4B). Each one renders, animates and resizes correctly natively, in Chrome (WebGPU) and in Firefox (WebGL2).

### Phase 4A: 2D renderer (first)

- [x] 4A.1 **[S] One `Renderer` owned by `Engine`.** _Done (engine side): the engine owns the GPU (`render::gpu::GpuContext`: instance, adapter, device, queue, surface, configure) and a `Box<dyn Renderer>` set with `Engine::set_renderer` (default `ClearRenderer`). The window no longer owns a surface; `RendererState` and the triangle renderer and shaders are deleted. The renderer consumes the `RenderFrame` (not a `RenderQueue`). Init failure is a logged error, not a panic. Also covers the surface part of 4A.4/4A.5 (resize reconfigure, Lost/Outdated/suboptimal reconfigure, Timeout skip, frames skipped at size 0)._
  - The game's drawing code is the user's `Renderer` (template: `examples/custom_renderer`). **The drawing tasks 4A.8 and later are for the user's renderer, not the engine crate.**
- [ ] 4A.2 **[W] Backends.** Enable wgpu's `webgl` feature for wasm (it's off today, so browsers without WebGPU can't render) and use `BROWSER_WEBGPU | GL`. Create the surface *before* the adapter and pass `compatible_surface`. Pick device limits by backend (downlevel limits only on GL).
- [ ] 4A.3 **[W] Initialise graphics on web.** Replace `pollster::block_on` with `wasm_bindgen_futures::spawn_local`: build the renderer, store it in the engine, then start rendering in the rAF loop. Use the canvas element from `WebWindow` (1.1), not `getElementById`.
- [ ] 4A.4 **[S] Resize.** Connect the `Resized`/`ScaleFactorChanged` events from 1.2 to `renderer.resize()`. Skip frames while the size is 0.
- [ ] 4A.5 **[S] Surface errors.**
  - `Lost`/`Outdated`: reconfigure and skip the frame.
  - `Timeout`: skip.
  - `OutOfMemory`: exit.
  - Also reconfigure when the frame is `suboptimal`.
  - Remove the `expect`/`unwrap` calls on adapter, device and surface.
- [ ] 4A.6 **[S] Timing and present mode.** Use the `Time` resource from 2.2 instead of the renderer's `time += 0.005`, which runs once per object per frame. Prefer `Fifo` explicitly instead of `present_modes[0]`, and let vsync pace native frames.
- [ ] 4A.7 **[W] Colour format.** Choose the surface format deliberately (sRGB view format or gamma in the shader), so colours match between native, WebGPU and WebGL.
- [ ] 4A.8 **[S] Vertex buffer.** Add a `Vertex { pos, color }` type, its buffer layout, and a shader that reads vertex attributes. That removes the vertex array hard-coded in WGSL.
- [ ] 4A.9 **[S] Per-object drawing.** Today every object writes the *same* uniform buffer before submit, so all objects draw at the last one's position (`renderer/mod.rs` ~365). Use an instance buffer or dynamic offsets. Switch to `TriangleList`.
- [ ] 4A.10 **[S] Orthographic 2D camera.** A globals/camera uniform with the aspect ratio (no stretching after resize), then an orthographic 2D camera. _Engine side done: the `Camera` component and `CameraView` carry ready column-major `view` / `projection` / `view_projection` matrices that follow the window size (1 world unit = `zoom` pixels, y up). Uploading them to a uniform is the renderer's part._
- [x] 4A.11 **[S] Textured sprites and texture/asset handle.** _Engine side done: `TextureHandle`, `Sprite`, `Image` and the `Textures` store with exactly-once change delivery; no PNG decoder yet (no pure-Rust `png` crate was available offline), so `Image::new` takes decoded RGBA8._ A `Texture` handle type and sampled quads with a sprite shader. _Done when_ a PNG sprite draws at the right size natively and in both browsers. _Done by the built-in `SpriteRenderer` (opt-in) and a pure-Rust PNG decoder (`Image::from_png`, `Textures::add_png`); `examples/coin_dash` draws PNG sprites. Native verified; browsers not yet checked._
- [x] 4A.12 **[S] Sprite batching (instancing).** Draw many sprites with one instance buffer per texture, reusing the approach from 4A.9. _Done when_ 10,000 sprites with one texture draw in one call (checked in the debug stats). _Done in `SpriteRenderer`: one instanced draw per run of same-texture sprites (instance data in a vertex buffer). The "10,000 sprites in one call" check and debug stats are not done._
- [x] 4A.13 **[S] Draw order and alpha blending.** A z/layer sort before submit, and alpha blending with premultiplied alpha. _Done when_ transparent sprites layer correctly over opaque ones. _Done in `SpriteRenderer`: layer sort comes from the extract, straight-alpha blending (not premultiplied)._
- [x] 4A.14 **[S] Sprite sheets and animation.** Atlas rectangles (UVs) and frame timing from the `Time` resource (2.2). _Done when_ a sheet animates at its set frame rate, whatever the render rate. _Done in `coin_dash`: a 4-frame coin sheet animated by switching `uv_min`/`uv_max` from `Time` (8 fps); no engine-side animation component._
- [x] 4A.15 **[S] Tilemaps.** A grid of tiles drawn as batched sprites from one atlas. _Done when_ a 100×100 map pans with the camera in one draw call per layer. _Done as a demo in `coin_dash`: a 40x30 grid of tile sprites from one atlas on a low layer (one draw per texture run). No tilemap type or culling; the "100x100 map" check is not done._
- [ ] 4A.16 **[S] 2D text.** Glyph rendering with `glyphon` or `fontdue` for in-world text. _Done when_ a string draws in the same place natively and in both browsers.
- [ ] 4A.17 **[S] 2D debug draw.** Lines and rects in world or screen space, drawn over the sprite pass. _Done when_ a bounding box can be drawn over a sprite.
- [ ] 4A.18 **[S] Cross-platform check.** Native, Chrome and Firefox all show the identical scene. Window and canvas resize and browser zoom reconfigure without errors. Returning to a background tab doesn't cause a huge delta-time jump.

### Phase 4B: 3D renderer (after 4A)

Must keep working on WebGL2 (wasm). WebGL2 has downlevel limits: no storage buffers, no compute, small uniform buffers (16 KB minimum), and smaller texture sizes. Check each task against them.

- [ ] 4B.1 **[S] Perspective camera.** A `Camera` component (view and projection, from `Transform`), with the aspect ratio kept on resize. _Done when_ a rotating cube keeps its shape after a window resize.
- [ ] 4B.2 **[S] Depth buffer.** A `Depth32Float` target, recreated on resize, with depth test and write in the 3D pipelines. _Done when_ overlapping meshes sort correctly from every angle.
- [ ] 4B.3 **[S] Mesh assets and glTF loading.** Mesh data in vertex and index buffers behind `MeshHandle`, loaded with the `gltf` crate. _Done when_ a sample `.glb` renders natively and in both browsers.
- [ ] 4B.4 **[S] Materials: unlit, then PBR metallic-roughness.** Unlit (base colour and texture) first, then metallic-roughness (base colour, metallic, roughness, normal map). _Done when_ the same glTF looks the same natively and in both browsers.
- [ ] 4B.5 **[S] Lights (directional, point).** Lights in a uniform array with a fixed maximum count, since WebGL2 has no storage buffers. _Done when_ a lit sphere shades correctly with one of each.
- [ ] 4B.6 **[S] Shadow maps for the directional light.** A depth-only pass from the light, sampled with a comparison sampler and PCF. Keep the shadow map within downlevel texture limits. _Done when_ a cast shadow lines up with its object natively and on WebGL2.
- [ ] 4B.7 **[S] Frustum culling.** Test each mesh's bounding sphere against the camera frustum before drawing. _Done when_ off-screen meshes are not submitted (counted in the debug stats).
- [ ] 4B.8 **[S] Render graph or pass list.** Ordered passes (shadow, 3D scene, 2D sprites, UI) so 2D sprites and UI draw on top of 3D. The UI slot is reserved for 6.2. _Done when_ a sprite draws over a 3D scene in the right order.
- [ ] 4B.9 **[S] Skybox and environment.** A cube-map skybox drawn after the opaque pass at maximum depth. Cube maps work on WebGL2. _Done when_ the sky stays behind geometry and fixed while the camera rotates.
- [ ] 4B.10 **[S] Skinned meshes (later).** glTF skins with joint matrices. WebGL2 limits uniform size, so cap the joint count or move joints into a texture. _Done when_ a sample animated character plays natively and in both browsers.

## Phase 5: Scripting with efescript (future)

Goal: game and mod logic written in efescript runs inside the engine, calls engine and mod functions through a typed host API that can grow at runtime, and has tooling that always knows that API.
Depends on Phase 3 (world, `Commands`, type registry). It does not depend on Phase 4, so it can start before rendering is finished.

Today's efescript API is unstable and static (fixed syscall numbers compiled into the VM). It will be redesigned, not patched. Design rules that keep JIT and AOT possible later:
- A **syscall is just a script call into the host**: one mechanism for engine functions, mod functions and built-ins.
- Host functions are **registered at runtime, by name, with a typed signature**. Scripts import them by name; ids are resolved when a module is loaded, never hard-coded.
- **One calling convention** (ABI) shared by the interpreter, the JIT and AOT-compiled code.
- The bytecode is a stable, verifiable format with an import table and type information. No self-modifying code, and no raw host pointers visible to scripts.

- [x] 5.1 **VM as an embeddable library.** _Done: the new VM is `crates/efevm` (efescript E1): no global state, `Result`s instead of panics, resumable `run(budget)`, no I/O. The legacy VM was deleted (efescript E3.6). `cargo check -p efevm --target wasm32-unknown-unknown` passes; there is no CI for it (efescript E1.8)._
  - A `Vm` / `Module` / `Instance` API with no global or static state.
  - Errors instead of panics on bad input; no `todo!()` opcodes; no raw pointers from threads into the runtime.
  - Resumable `run(budget)` with an instruction limit, so an endless loop costs one frame.
  - wasm-safe I/O (no direct `std::fs`/stdin in the VM core).
  - Fix the operand-decoding bugs listed in `crates/efescript/structure.md` as part of the rewrite.
- [x] 5.2 **Dynamic, structured host calls.** _Done: `efevm::HostRegistry` (efescript E3): runtime registration by name with typed, named, documented signatures, versioned namespaces, link errors by name. The registry lives in efevm, so the engine depends on efevm only._
  - `HostRegistry::register(name, signature, function)` usable at any time, including by a loaded mod that exposes functions to other mods.
  - Signatures use a small type system: integers, floats, bool, entity, string/slice (pointer + length in VM memory), and structs described by the type registry (3.4).
  - Namespaced names (`engine.world.spawn`, `mymod.inventory.add`), with a version per namespace.
  - Modules list their imports; loading fails with a clear error when an import is missing or its signature differs.
  - The efescript crate defines the host trait; it must not depend on the engine.
- [x] 5.3 **API index (the "PDB" for mod developers).** Details: `crates/efescript/ROADMAP.md`, "The API index" and E4. _Done: efescript E4: `<game>.efapi` (format in `crates/efevm/API_INDEX.md`) from `cargo xtask api-export`, `cargo xtask build`/`dist` and `Engine::export_api_index`; modules record the index hash (section 12); the assembler, language server and debugger read only the file; the index tests are `crates/efescript/tests/api_index.rs` and `crates/core/tests/api_index.rs`._
  - The registry *is* the index: registering a host function or a component type is the only way to make it visible to scripts, so every syscall an engine system or a mod adds is in the index automatically.
  - Exported to a versioned file, `<game>.efapi`: functions with parameter names, types and docs, struct and component layouts, namespaces, owners, and a content hash.
  - Written at build time (`cargo xtask api-export`, and next to the game by `cargo xtask build`/`dist`) and at runtime (`Engine::export_api_index`), so functions registered dynamically by mods are included.
  - Tools (assembler, language server, debugger) read only this file, for any game built with the engine.
  - A compiled module records the index hash it was built against, for compatibility checks.
  - A test fails when something callable is missing from the index or undocumented.
- [x] 5.4 **Engine bindings.** _Done: efescript E5.1/E5.2: engine host functions in six `engine.*` namespaces, the `Script` component and the script system in `Update`, one VM per world. Tested headless (`crates/core/tests/scripting.rs`, `examples/script_demo/tests/e2e.rs`)._
  - The engine registers its host functions: world access (`get`/`set` component bytes, queries), `Commands` (spawn, despawn, insert, remove), input, time, events and logging.
  - A `Script { module, entry, state }` component, and a `ScriptSystem` in the `Update` stage that calls each script's entry with the entity id and delta time.
  - One VM per world, not per entity. Script-private data lives in VM memory; hecs component types are not created dynamically.
- [x] 5.5 **Mods.** Load and unload modules at runtime, per-mod budgets and permissions (which namespaces a mod may call), and hot reload of scripts on native. _Done: efescript E5.3: `load_mod`/`unload_module`, per-mod budgets and namespace permissions, mod-exported functions in the index, hot reload on native (tested headless)._
- [ ] 5.6 **Language frontend and language server.**
  - The high-level efescript language (only lexer and AST stubs exist today) compiles to the bytecode with typed imports.
  - The language server reads the index file from 5.3 and reloads it when the file changes, so completion, signatures and type errors follow the live API automatically.
- [ ] 5.7 **AOT compilation.** Compile modules ahead of time to native code (and to wasm for the web build), using the same ABI and import resolution as the interpreter.
- [ ] 5.8 **JIT compilation.** Native only at first (for example with Cranelift). Browsers cannot run generated machine code, so the web build uses the interpreter or AOT-to-wasm modules.
- [x] 5.9 **Modules and mod packages.** Details: `crates/efescript/ROADMAP.md`, E8.
  _Done: multi-file `.efa` with a linker, `.efmod` 7z packages (`crates/efemod`), `efescript pack`/`inspect`, `cargo xtask mods`, `Engine::load_package` with a deny-by-default `ModPolicy`, dependency order, declared provides with a copy ABI, unload/reload rules and typed script events in the index (efescript E8.1–E8.13)._
  - Mods are written as many files and linked into one module at build time (multi-file `.efa`, a linker, later the language's `use`).
  - A mod ships as one `.efmod` package (like a `.jar`): manifest with id, version, dependencies, requested permissions and the engine index it was built against, plus its module, its own `.efapi` and its assets.
  - `Engine::load_package(bytes)` and a `mods/` folder on native; load order from dependencies; the game grants permissions, the package only requests them.
  - A mod's exports are declared in its module, so they reach the index without running it; strings and structs passed between mods are copied between their separate memories.

## Phase 6: UI (editor with egui, game UI with iced)

Goal: two UI toolkits for two audiences, both optional. **egui** for the people building
games (editor, inspectors, console, profiler); **iced** for the people playing them (menus,
settings, inventory, dialogs). The core stays toolkit-agnostic. Full proposal, principles and
risks: [`ui-roadmap.md`](ui-roadmap.md); this phase maps it onto the engine as it is.

Depends on Phase 4: both toolkits draw through wgpu, and today the renderer is a placeholder
that the window owns. Until 4A.1 (one `Renderer` owned by `Engine`) and 4A.4 (resize) land there
is nowhere to put a UI pass. Some editor work (6.4) also needs the type registry from 3.4,
which is done.

What the engine already decides for this phase (from the code, 2026-11-15):
- **Windowing is SDL3 natively and the DOM on the web, not winit.** egui is backend-agnostic: the engine feeds it `egui::RawInput` built from `InputEvent` and draws with `egui-wgpu`, so this is low risk. iced's normal shell is `iced_winit`; embedding it in the engine means driving `iced_runtime`'s `UserInterface` with engine events and drawing with `iced_wgpu` (iced's own "integration" pattern). This is the main thing 6.1 must prove.
- **wgpu version lockstep.** The engine uses wgpu 27 (with `webgl` on wasm). `egui-wgpu` and `iced_wgpu` each pin a wgpu version, so the engine, egui and iced must agree on one wgpu release, or the UI crate cannot share the device. Upgrades then move together.
- **Web.** The web build uses wasm threads (atomics, shared memory) and the DOM backend; both toolkits must work there with the engine's canvas, not their own.
- **Scripts and mods.** iced's model (Rust types, messages, `view` functions) cannot be written from efescript. Game UI that mods may add needs a data-driven layer exposed through host functions, so it is in the API index like everything else (efescript roadmap E8).

- [ ] 6.1 **Platform check (ui-roadmap Phase 0).** Pick egui and iced versions whose wgpu matches the engine's; build two proof-of-concept apps on the engine's renderer (native SDL3 and web canvas), not on winit. Record a compatibility matrix (native, web WebGPU, web WebGL; Android and iOS only if they become targets) and minimum versions. _Done when_ both draw a widget inside an engine window natively and in the browser, or the matrix says why not.
- [ ] 6.2 **UI pass in the renderer.** A `RenderStage::Ui` after the scene pass: the renderer owns surface acquire and present; UI integrations record into an encoder they are given, with an explicit order (scene, game UI, editor overlay). Texture registration and lifetime, clipping and DPI (logical vs physical pixels from 1.2/1.11) are defined here once for both toolkits.
- [ ] 6.3 **Input ownership.** Before game systems run, UI layers see the frame's `InputEvent`s in order (editor overlay, then game UI) and mark what they consumed; `InputState` exposes "consumed by UI" so a click on a menu does not also fire the weapon. Text input, IME and clipboard go through the existing window API (`set_text_input`).
- [ ] 6.4 **egui developer tools (ui-roadmap Phases 1 and 4), crate `efeengine-editor`.** First a stats panel (FPS, frame time, entity count), then: an entity and component inspector driven by the type registry (3.4) so every registered component is editable without per-type code; a log viewer on the engine logger; a script panel (loaded modules and mods, budgets, traps, the live API index); later a profiler (with the `tracing`/`puffin` item) and an asset browser. Not linked into game builds unless the game opts in.
- [ ] 6.5 **iced game UI (ui-roadmap Phases 2 and 3), crate `efeengine-ui`.** A main menu and a settings screen with a custom theme, fonts and images; measure startup cost, binary size (native and wasm), memory and frame time. Then the public API: how a game creates, updates and draws its UI, how UI actions become engine events or `Commands`, threading and lifecycle rules. No iced types in `efeengine-core`.
- [ ] 6.6 **Script-facing UI.** A small, data-driven UI description (panels, labels, buttons, lists, bound to script state) that scripts and mods build through host functions registered in the API index, rendered by the game UI crate. Mod permissions decide whether a mod may open UI. Decide in 6.5 whether this sits on iced or on a simpler immediate-mode layer.
- [ ] 6.7 **Escape hatches and docs.** A game can use iced, another toolkit or its own UI without the editor; document how to bypass the built-in game UI; examples for each; platform smoke tests.

Open questions carried from `ui-roadmap.md`: iced maturity on the web and on mobile; text shaping, fonts and localization (shared with 4A.16, 2D text, above); the editor's own window and lifecycle versus the game's (answered by Phase 7: the game runs in its own process).

## Phase 7: Editor and live iteration (last)

Goal: an editor in the Unreal style, where the user writes normal Rust systems and the editor builds them, with no ABI risk. Rust has no stable ABI, so the editor **never loads game code in its own process**. The game process is the authority in both edit mode and play mode: it holds the world, runs the systems and renders. The editor is a client that only does UI, and it talks to the game over a local socket. This is not a full client-server engine (no world copied every frame, no headless server plus a rendering client); multiplayer may reuse the serialization later, but nothing here depends on it.

Depends on Phase 6 (6.4 egui tools, 6.2 UI pass), Phase 4A (a renderer to show scenes) and the type registry (3.4).

How changes apply:

| Change | How it applies | Restart? |
|---|---|---|
| `.efa` scripts, mods | the VM reloads them (`ScriptCache`, `reload_mods`), already in | no |
| Rust system bodies | Subsecond hot-patches the running game (7.6) | no |
| Component layouts, new types | rebuild, then restore the scene snapshot (7.7) | yes, made to look seamless |

- [ ] 7.1 **[S] Project layout.** A game project is a normal cargo crate that depends on `efeengine-core` at a pinned version: `Cargo.toml`, `project.toml` (scenes, start scene, targets), `src/` (Rust systems), `scripts/`, `mods/`, `assets/`, `scenes/`. Cargo fetches and builds the engine source; a custom engine fork is selected with `[patch]`, not downloaded by the editor. _Done when_ `efeengine new mygame` creates a project that `cargo run` starts.
- [ ] 7.2 **[S] Component schema export.** Extend `EFE_API_EXPORT` so the game also writes its component and resource schema (names, fields, types, layout) from the type registry, next to the script API index. The editor reads it to build inspectors without linking game code. _Done when_ script_demo's `Transform` and a game-defined component appear in the exported schema.
- [ ] 7.3 **[S] Scene format.** Save and load the world (entities plus components) using the schema: a versioned text format for diffs, plus a snapshot API (`Engine::snapshot()` / `restore()`). _Done when_ a scene round-trips byte for byte and a snapshot restores a running world.
- [ ] 7.4 **[N] Editor protocol.** A local socket between the editor and the game process. Editor → game: set a component field, spawn or despawn, pause, step, play or stop, reload. Game → editor: entity list, component values (polled or as diffs), logs, stats. Built on `Commands`, the engine message queue and field-level reflection, the same layout access scripts use (`Transform.id`, `Transform.position`). _Done when_ an external tool moves an entity in a running script_demo.
- [ ] 7.5 **[N] Editor app, edit mode and play mode.** A prebuilt egui app (6.4) that runs `cargo build`, shows the errors in a panel and starts the game process.
  - **Edit mode:** game systems are paused, and only rendering and gizmos run. Edits go to the world and are saved to the scene file.
  - **Play mode:** snapshot the scene, then run the systems. Values can be changed live. Stop restores the snapshot unless "keep changes" is chosen.
  - **Viewport:** at first, the game's own window next to the editor. Embedding it is possible on Windows and X11 but not on Wayland or the web; sharing a GPU texture across processes comes later.
  - **Toolchain:** a doctor step checks the Rust toolchain, like `build.py doctor`.
  - _Done when_ you can edit a scene, press play, change a value live, press stop and get the scene back.
- [ ] 7.6 **[N] Hot-patching systems with Subsecond (Dioxus).** Run the scheduler's system calls through `subsecond::HotFn`, so systems registered with `add_system` pick up new code on the next frame. Builds go through `dx` (or its linker shim), driven by `xtask` and the editor. Only function bodies of the game crate can be patched; struct layouts, statics and engine crates cannot. Native first; wasm only if it proves stable. **Start with a small test:** hot-patch one script_demo system and measure the cost per call. _Done when_ editing a system body changes the running game in under a second.
- [ ] 7.7 **[S] Seamless restart.** When a change cannot be patched (a component layout, a new type), the editor snapshots the scene, rebuilds, restarts the game and restores the snapshot, mapping fields by name through the schema, with added fields set to their defaults. _Done when_ adding a field to a component keeps the scene and the camera position.

## Later: missing parts

| Area | What | Priority |
|---|---|---|
| Assets | `AssetServer` with handles; `std::fs` natively, async `fetch` on web; start with `include_bytes!`. The data side can start before Phase 4. | after Phase 3 |
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
- ~~efescript: operand-decoding bugs and `todo!()` opcodes~~ gone with the legacy VM (efescript E3.6).
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
