# Multiple windows (as built)

> **Web: Not Supported (TODO).** `create_window` fails on wasm with "Not Supported". The web sections below describe the previous, canvas-based implementation and stay as reference until it is re-enabled (see `docs/WEB_POPUP_PLAN.md`).

`ROADMAP.md` "Later: multiple windows". One engine, one world, one wgpu device, any number of
windows: extra SDL windows natively, a canvas mounted in any page element on the web. Cameras
choose the window they draw to, UI layers choose the window they are shown in, input events say
which window they came from. A game that never creates a window behaves exactly as before.

Example: `examples/multi_window` (main window follows the player, a "Map" window shows the whole
playfield from a second camera, a UI label layer in each window). Tests: `crates/core/tests/windows.rs`,
`crates/ui/tests/windows.rs`, plus unit tests in `window/headless.rs` and `windows.rs`.

## API

```rust
use efeengine_core::window::WindowDesc;
use efeengine_core::input::WindowId;                       // WindowId(0) = WindowId::MAIN

let map = engine.create_window(WindowDesc::new("Map").web_element("#sidebar").size(320, 240))?;
engine.world_mut().spawn((Camera::new_2d().target(map).zoom(0.5), Transform::default()));
ui.layer_in(map, "map-hud", Layer::top_left(0));           // or Layer::top_left(0).window(map)

// at runtime, from a system: the id is reserved at once, the window opens after the stage
let id = cx.commands.create_window(WindowDesc::new("Late"));
cx.commands.close_window(id);
cx.commands.set_window_title(id, "x");  cx.commands.set_window_fullscreen(id, true);
```

- **`WindowId(u32)`** (in `efeengine_core::input`, with `InputEvent::window` from the start). 0 is the main
  window; `create_window` hands out 1, 2, ... and never reuses an id. `WindowId` converts to and from `u32`
  (`Camera::target` and `Layer::window` are plain numbers so scripts and the ECS layout need no new type).
- **`WindowDesc { title, size, resizable, web }`** with the builder `new(title)`, `size(w, h)`,
  `resizable(bool)`, `web_element(selector)`, `web_canvas(selector)`. `size` is the logical size
  natively; on the web it is only the fallback while the element has no layout. `WebTarget`:
  `Body` (default: a fixed `size` box appended to `<body>`), `Element(sel)` (a new canvas inside the
  first element matching `sel`) and `Canvas(sel)` (use an existing `<canvas>`).
- **`Engine::create_window(desc) -> Result<WindowId, EngineError>`** works before and after
  `game_loop` / `init_graphics` (the GPU surface is made when graphics is up). **`Commands::create_window`**
  returns the id immediately; if opening fails the error is logged and a `WindowEvent::Closed` for that
  id arrives next frame. `Engine::close_window(id)` / `Commands::close_window`.
- **`Camera::target`** (plus the builders `Camera::new_2d()`, `.target(window)`, `.zoom(z)`,
  `.clear_color(c)`). A camera aimed at a window that does not exist draws nowhere.
- **`Layer::window(w)`** and **`UiTree::layer_in(window, name, layer)`**; queries `layers_in(window)`,
  `layer_window(id)`, `window_of(node)`, `layer_windows()`. Moving an existing layer to another window with
  `layer_in` (same name) is a `UiChange::Layer`; its nodes follow.
- **`Windows`** (`state.windows`, `Ctx::windows`, `ParCtx::windows`, `RunCtx::windows`): `main()`,
  `get(id)`, `contains`, `iter`, `len`, `focused()`; each `WindowState { size, scale_factor, focused,
  fullscreen }` + `logical_size()`. `Ctx::window` stays and is the main window's state. Kept up to date
  from window events.
- **Events** (`InputEvent { window, kind: Window(..) }`): `Resized`, `ScaleFactorChanged`, `Focused` (as
  before, now per window) and new `Created`, `Closed`, `FullscreenChanged(bool)`.
  `CloseRequested` of the main window quits (as before); of a secondary window the engine removes the
  window right away and delivers `CloseRequested` then `Closed` (the latter next frame). The first poll of a new
  window reports `Created`, then its `Resized` and `ScaleFactorChanged`.
- **Input state**: `InputState::mouse_pos()` is the MAIN window's pointer (unchanged for single-window
  games); `mouse_pos_in(window)`, `pointer_window()` (where the pointer was last seen), `focus_window()`,
  `focused_in(window)`. `focused()` is "any window of the game has focus"; held keys and buttons are
  only released when the last focused window loses focus (switching between two windows of the game keeps
  them). Mouse buttons, wheel and deltas stay global (sums), the event list tells the window. Key and text
  events carry the focused window. `InputCapture` is global.
- **Commands**: `set_cursor_icon` and `set_cursor_visible` act on the window the pointer is in, `set_cursor_grab`, `set_text_input`,
  `set_text_input_area` on the focused window (the UI backend of the window the user is in asks for them);
  `set_title` / `set_fullscreen` are the main window's, the `*_window` variants take an id.
- **Fullscreen sync (the old desync)**: `WindowEvent::FullscreenChanged(bool)` and
  `WindowState::fullscreen`. Native: SDL's enter / leave fullscreen events. Web: the document's
  `fullscreenchange`, so leaving fullscreen with the browser's Esc now reaches the game (reported for the
  window that asked, the main one for a fullscreen the game did not request). `coin_dash`'s settings
  checkbox can follow it by reading the event.

## Rendering

- **Frame**: `RenderFrame { d2, d3 }` are the MAIN window's frame parts (so `CustomRenderer`s, tests and
  `Frame2D` users see what they saw); `RenderFrame::windows: Vec<WindowFrame { window, d2, d3 }>` has one entry
  per open secondary window, and `RenderFrame::window(id)` finds either. The extract picks, per window and
  kind (2D / 3D), the active camera with the lowest entity id whose `target` is that window; sprites and meshes
  are shared by all windows (copied into a window's frame only when it has a camera of that kind). `CameraView`
  matrices use the window's own size. Debug draw (`DebugDraw2D`) belongs to the main window.
  A window with no camera is cleared and draws nothing.
- **Renderers**: the same `Renderer2D` / `Renderer3D` instances draw every window; `Gpu` gained `window: u32`
  and its `size`, `scale_factor`, `surface_format` are those of the target window. `Renderer::resize`
  is called for the main window only: a renderer with size-dependent resources keys them by `gpu.window`
  and checks `gpu.size` in `render`. `SpriteRenderer` needed no change (its camera comes from the frame, the
  Draw2D pass uses `gpu.size`, calls are sequential so its uniform and instance buffers are safe to reuse).
  Texture changes are delivered with the first window that is drawn in a frame; later windows get an empty list.
  A `CustomRenderer` draws the main window only; secondary windows are cleared (one warning).
- **GPU**: one wgpu device. `GpuContext` keeps the main surface and one `ExtraSurface` per secondary window
  (`windows` map), created with `ExtraSurface::from_surface` (native, from the SDL window's raw handles on the
  same instance) or `ExtraSurface::for_canvas` (web). The frame order is: main window (renderers, then overlays,
  present), then each secondary window the same way. A window whose surface format differs from the main one
  is not drawn (one error log; in practice native surfaces agree, WebGL2 copy-path canvases are
  `Rgba8UnormSrgb` and so is the WebGL2 main surface).
- **Overlays**: `RenderOverlay::init` / `resize` / `render` are called per window (`gpu.window`), in add order.
  An overlay added after a window exists is `init`ed for it too.
- **WebGL2**: the main canvas owns the GL context, so a secondary canvas is an `ExtraSurface` copy
  path: render into an offscreen texture, read it back, `putImageData` (one frame of latency, a CPU pass;
  see `docs/ui/API.md` 17.3 / 17.4). WebGPU: a real surface per canvas on the shared device (compile-checked,
  not run: headless Firefox 159 has no WebGPU).
- **SDL mode** (`EFE_GPU_MODE=sdl`, or `auto` falling back to it) draws the main window only: an `SDL_Renderer`
  owns its textures, so every window would need its own texture table and its own copy of every texture
  change. `create_window` fails there with a clear error; windows created before graphics came up are closed with
  an error log when SDL mode is chosen (`Closed` events follow). Use `wgpu` / `wgpu-gl` for multiple windows.

## Native (SDL)

`SdlWindowManager` keeps a map engine id -> SDL window (plus SDL id -> engine id for events), resizable
windows built with default placement. Every `InnerWindow` method takes a `WindowId`. Mouse, key and text
events use the SDL window id they carry; touch (SDL gives no window id for fingers) goes to the main window.
`wgpu-gl` (EGL on Wayland/X11) and Vulkan both ran two windows in the example. Per-window vsync means two
`present`s per frame; `ExtraSurface` uses the first present mode the surface offers.

## Web

`WebWindow` keeps a `CanvasWin` per window (canvas, listeners, last size, cursor state) and one `Shared` queue.
Page-wide listeners (keyboard, composition, focus, pointer lock, fullscreen) exist once; keyboard events go to
`key_window`, which is the canvas the user last pressed on or focused (moving it reports `Focused(false)`
for the old and `Focused(true)` for the new window). Pointer / wheel / contextmenu listeners and the pointer state
(last position, button mask, touches) are per canvas. `devicePixelRatio` changes are reported for every window.
A canvas's drawing buffer follows its CSS size times `devicePixelRatio`, checked every frame (not a
`ResizeObserver`: the main canvas already worked like this and the result is the same one frame later).

`create_window` on the web: `web_element(sel)` makes `<div style="position:relative;width:100%;height:100%">`
holding the `<canvas style="width:100%;height:100%">` inside the element (a `min-height` of `size.1` is added
only if the element has no height yet), `web_canvas(sel)` takes an existing canvas (its parent should be a positioning
context; the UI backend makes it `position: relative` when it is `static`), `Body` appends a `size` box to `<body>`.
A missing element is an error from `create_window`. Closing removes the wrapper (an existing canvas stays).
A window's fullscreen request goes to its wrapper (the main window's to the page element, as before).
`efeengine_core::window::canvas_of(id)` returns the canvas of a window (thread-local registry; used by the UI).

`templates/web/index.html` is unchanged: a game that wants a host element for a window provides its own
`examples/<name>/web/index.html` (the existing per-example override); `examples/multi_window/web/index.html`
adds `<aside id="sidebar">` with `<div id="map-box">`.

## UI

- `Layer` has `window: u32` (default 0). Only the layers of a window are shown there; `z` ordering and `modal`
  act inside one window (`UiTree::is_below_modal` compares layers of the same window; `modal_active()` is still "any
  window", so a modal dialog captures the pointer and keyboard of the whole game via `InputCapture`).
- **One backend instance per window.** `UiBackendCx::window` tells an instance its window; a backend shows
  `tree.layers_in(cx.window)`. The plugin keeps a slot per window: slot 0 is the one given to `UiPlugin` (the main window),
  other instances come from the window-backend factory when the window exists AND has a layer
  (`UiPlugin::with_window_backends(|window| Some(Box::new(..)))`; the default makes a `WebBackend` on wasm and an
  `IcedBackend` natively with the `iced` feature, nothing for a headless engine or plugin). A backend that returns
  `handles_all_windows() == true` (the headless one, so `UiTest`) is a single instance for every window and sees the
  whole journal. Instances of closed windows are dropped.
- The journal goes to every instance (each skips what is not its window's; `Layer` changes move subtrees between
  windows on the web backend). An instance made later is synced with `UiTree::snapshot_changes()` (theme, fonts, every node
  parents first) plus the images those nodes need.
- Input: the engine input filter routes each event to the instance of `ev.window`; capture flags, text input and cursor
  requests of all instances are merged by the `ui_signals` system. Window sizes reach `backend.resize` from `Windows`
  (compared every frame in `ui_resize`).
- GPU: the overlay calls `gpu_init` / `gpu_resize` / `render` of the instance of `gpu.window`; an instance made after
  its window's surface runs `gpu_init` lazily on its first `render`.
- Web: an instance of a secondary window puts its layer element (`div[data-efui-window=N]`, absolutely positioned, pointer
  events pass through) next to the window's canvas. The `#ui` element and `uiLayerId` stay the main window's. Style sheets
  are shared (found by id).
- iced: `Model::build(tree, window)`, one `iced_wgpu` renderer per instance (a `Renderer` per window's surface format).
- The fallback backend (another work stream) must read `cx.window` in `init` and build its model from
  `tree.layers_in(window)` to take part; until then it shows its own idea of "all layers" in window 0 only.
- Scripts and mods cannot target a window yet (`ui.layer` from a script is window 0).

## Not supported / known limits

- SDL mode (see above). `CustomRenderer` only draws the main window. `Renderer::resize` is main-only.
- Debug draw, `DebugDraw2D`, is main-window only. Touch on native goes to the main window.
- No window positions, borderless / always-on-top flags or per-window `set_cursor_*` (they follow the focused window).
- Web `Body` windows are plain page boxes; there is no OS-style dragging or closing (use `close_window`).
- The WebGPU path of secondary canvases is compile-checked only. WebGL2 secondary windows pay a readback per frame.
- Resizing a native window below zero pixels (minimise) suspends only the main surface; a zero-sized secondary
  window is simply not acquired.
- A secondary window on the web is not auto-removed when its element disappears from the DOM (its canvas just stops
  being laid out); call `close_window`.

## Results (this machine, Wayland + Mesa)

- Native `timeout 8 cargo run --release -p multi_window`: two windows, labels in both, no panic or wgpu error, in the
  default Vulkan mode and with `EFE_GPU_MODE=wgpu-gl`; `EFE_GPU_MODE=sdl` runs the main window and logs that the map
  window was closed.
- Web (headless Firefox 159, WebGL2 copy path): both canvases non-blank (map canvas 320x240 with its UI label next
  to it, main canvas with its HUD), `C` closes the map window and `M` opens a new one in the same element, no
  console errors. Driver: a copy of `bidi_coin.py` that screenshots and checks canvas rectangles.
