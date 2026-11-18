# Renderer plan: separate 2D and 3D renderers, GPU modes, SDL fallback

> **Status:** 4C.1 to 4C.5 built (see sections 8, 8.1, 8.2); 4C.6 onward not started\
> **Date:** 2026-11-18\
> **Current design:** [`RENDERING.md`](RENDERING.md) (one `Renderer` slot, `RenderFrame`, built-in `SpriteRenderer`)\
> **Roadmap:** `ROADMAP.md` Phase 4 (4A 2D, 4B 3D) and the new Phase 4C below

## 1. Goal

- 2D and 3D are **two renderers with their own data**, not one renderer that does both.
- 2D gets a **second backend on SDL3's `SDL_Renderer`**, so 2D games run where wgpu cannot: hardware
  below GL 3.3 / GLES 3.0 / WebGL2, and homebrew targets like the PS Vita (SDL3 has a GXM backend for
  `SDL_Renderer` there).
- 3D stays **wgpu only**. `SDL_Renderer` has no depth buffer and no shaders.
- Games that only use components (`Sprite`, `Camera`, meshes) and the built-in renderers never see
  which backend runs. This is the same idea as the window backends (`WindowManager`) and the UI
  backends (`UiBackend`).

## 2. Today

- The engine owns the wgpu context (`render::gpu::GpuContext`) and one `Box<dyn Renderer>`
  (`Engine::set_renderer`, default `ClearRenderer`; built-in opt-in `SpriteRenderer`).
- `Renderer::render(&Gpu, &wgpu::TextureView, &RenderFrame)`: `Gpu` hands out wgpu types, so every
  renderer is a wgpu renderer.
- `RenderFrame` is plain data (camera, sprites, meshes, texture changes): already backend-neutral.
- `RenderOverlay` (added for the UI) draws after the renderer, before present, on wgpu.
- Native asks wgpu for `Backends::PRIMARY` only (Vulkan, Metal, DX12): no GL path on native yet. The
  web uses WebGPU or WebGL2.

## 3. Target design

```text
ECS world ──extract──► RenderFrame
                        ├── d2: Frame2D   sprites, shapes, text, tilemaps   (per 2D camera)
                        └── d3: Frame3D   meshes, lights, materials         (per 3D camera)

GPU mode (chosen once at startup, one per window):

  wgpu mode (default)                         SDL mode (fallback)
  ─────────────────────                       ─────────────────────
  Renderer3D   (wgpu)                         (none: a game that needs 3D fails at startup)
  Renderer2D   (wgpu: today's SpriteRenderer)  Renderer2D (SDL_Renderer)
  overlays     (UI: iced or fallback UI)       overlays   (fallback UI through Draw2D)
```

### 3.1 Frame data

- `RenderFrame` splits into `d2: Frame2D` and `d3: Frame3D`; `textures`, `frame` and `time` stay shared.
- `Camera` gets a kind: `Camera2D` (orthographic, today's camera) or `Camera3D` (perspective). The
  single-camera rule of `RENDERING.md` becomes "one active camera per kind" first, several cameras
  per kind later (with `Camera.target` and an `order`).
- 2D items in `Frame2D`: sprites (today's `SpriteInstance`), later shapes (lines, rects, circles),
  text runs, tilemap chunks. All in one list sorted by layer, so items of different types still
  draw in layer order.

### 3.2 Traits

```rust
/// Draws Frame2D. Two implementations: wgpu (built in) and SDL_Renderer (built in).
pub trait Renderer2D: 'static {
    fn init(&mut self, cx: &mut GpuCx) {}
    fn resize(&mut self, cx: &mut GpuCx) {}
    fn render(&mut self, cx: &mut GpuCx, target: &Target, frame: &Frame2D, textures: &TextureChanges);
}

/// Draws Frame3D. wgpu only.
pub trait Renderer3D: 'static {
    fn init(&mut self, gpu: &Gpu) {}
    fn resize(&mut self, gpu: &Gpu) {}
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView, frame: &Frame3D, textures: &TextureChanges);
}
```

- `GpuCx` / `Target` are enums over the mode: `Wgpu { gpu, view }` or `Sdl { canvas }`. A backend
  only matches its own variant; the engine never calls a renderer of the other mode.
- `Engine::set_renderer_2d`, `Engine::set_renderer_3d`. Defaults: the built-in 2D renderer for the
  current mode, no 3D renderer (3D cameras then log a warning once).
- **Custom wgpu renderers stay possible:** `Engine::set_renderer` (today's trait, renamed
  `CustomRenderer`) replaces both 2D and 3D in wgpu mode and gets the whole `RenderFrame`.
  `examples/custom_renderer` keeps working this way. It is unavailable in SDL mode.
- Pass order each frame: 3D cameras, then 2D cameras, then overlays (UI). This covers a 2D HUD over
  3D and 3D backgrounds behind 2D sprites. It replaces ROADMAP 4B.8's pass list.

### 3.3 Draw2D: the shared 2D drawing surface

> Built differently from this sketch: see section 8.1 (pixel coordinates, no transform, `DrawList`).

A small immediate API both 2D backends implement, used by the 2D renderers themselves and by
anything else that draws 2D on top (debug draw, the fallback UI):

```rust
pub trait Draw2D {
    fn quad(&mut self, rect: Rect, color: Color, texture: Option<(TextureId, UvRect)>, transform: Affine2);
    fn triangles(&mut self, verts: &[Vertex2D], indices: &[u32], texture: Option<TextureId>);
    fn set_clip(&mut self, rect: Option<RectI>);
    fn set_blend(&mut self, blend: Blend);   // alpha, additive, premultiplied
}
```

- wgpu: batched instanced quads (today's sprite pipeline) plus a triangle pipeline.
- SDL: `SDL_RenderTexture` / `SDL_RenderTextureRotated`, `SDL_RenderGeometry`, `SDL_SetRenderClipRect`,
  blend modes. Batching is SDL's job (SDL3 batches internally).
- Textures: both backends keep their own texture table, filled from the same `TextureChanges`.

### 3.4 GPU modes

- Startup config `graphics`: `auto` (default: wgpu, fall back to SDL mode if no wgpu adapter can be
  created), `wgpu`, `sdl`. On the web only wgpu exists (WebGPU or WebGL2).
- The mode is fixed for the process: wgpu and `SDL_Renderer` cannot share one window surface.
- Before any of this: add `Backends::GL` as a native wgpu fallback (prefer Vulkan/Metal/DX12, use GL
  3.3+ only when none works, downlevel limits on GL), plus a `wgpu-gl` choice in the config. Most old
  PCs have GL 3.3, so SDL mode is for what is below that.
- Shaders: WGSL by default. naga's `glsl` feature can be turned on for games that prefer GLSL; naga
  translates to every backend.

### 3.5 UI in each mode

| Mode | Game UI backend |
|---|---|
| wgpu, native | iced (default) or the fallback UI |
| wgpu, web | DOM |
| SDL | the fallback UI, drawn through `Draw2D` |

The fallback UI ([`ui/FALLBACK_NATIVE.md`](ui/FALLBACK_NATIVE.md)) is changed to draw through `Draw2D`
instead of its own WGSL pipeline: rounded rectangles become triangle meshes (or a nine-slice
texture), glyphs are rasterised on the CPU (cosmic-text + swash) into an atlas texture. That makes it
work in both modes, with one implementation.

## 4. What does not change

- Components, the extract systems' inputs, the `Textures` store and its PNG loading.
- `RenderOverlay` in wgpu mode (the iced UI uses it). SDL mode gets an overlay hook with `Draw2D`.
- Headless engines: no renderer is called; `RenderFrame` is still built and testable.

## 5. Phases (ROADMAP Phase 4C)

- [x] **4C.1 Split the frame and traits.** `Frame2D`/`Frame3D`, camera kinds, `Renderer2D`/`Renderer3D`,
  `CustomRenderer` (old trait), `GpuCx` with only the wgpu variant. The current `SpriteRenderer`
  becomes the wgpu `Renderer2D` and the default. _Done when_ coin_dash, custom_renderer and all tests
  work unchanged in behaviour.
- [x] **4C.2 Native GL fallback for wgpu.** `PRIMARY | GL` with GL last, downlevel limits on GL, config
  switch. _Done when_ coin_dash runs with the GL backend forced natively.
- [x] **4C.3 Draw2D.** The trait, the wgpu implementation, 2D debug draw (ROADMAP 4A.17) on top of it.
- [x] **4C.4 SDL mode.** `graphics = sdl` startup path (SDL creates an `SDL_Renderer` for the window,
  no wgpu context), the SDL `Renderer2D` and `Draw2D`, texture table from `TextureChanges`, the
  overlay hook. _Done when_ coin_dash runs in both modes and screenshots of the two match closely
  (same sprites, layers, alpha; filtering may differ).
- [x] **4C.5 `auto` mode fallback.** No wgpu adapter → SDL mode, with a log line saying so.
- [ ] **4C.6 Fallback UI on Draw2D.** Revise `FALLBACK_NATIVE.md`, then implement its phases on `Draw2D`.
- [ ] **4C.7 PS Vita (homebrew).** Rust target `armv7-sony-vita-newlibeabihf` (tier 3, nightly
  `build-std`), VitaSDK, `cargo-vita` for `.vpk`, SDL3's Vita port. Pre-built `.efvm` scripts (no `.efa`
  compile on 444 MHz). _Done when_ coin_dash runs in the Vita3K emulator, then on hardware.
- 3D (4B) continues on wgpu independently once 4C.1 exists.

## 6. Risks

| Risk | Plan |
|---|---|
| Two 2D backends drift apart | One `Draw2D` contract, shared tests that draw the same `Frame2D` in both and compare screenshots |
| SDL_Renderer lacks something 2D wants (custom shaders, 2D lights, post effects) | Those features are wgpu-mode only and documented as such; SDL mode is the "runs anywhere" subset |
| Texture memory twice (CPU store + GPU table) on small devices | Same as today; later an option to drop CPU copies after upload |
| The `Renderer` trait rename breaks games | No backward compatibility needed; examples are updated in 4C.1 |
| Vita toolchain is unofficial and tier 3 | It is a stretch target; 4C.1-4C.6 are useful without it |

## 7. Decision summary

| Area | Choice |
|---|---|
| 2D vs 3D | Separate renderers and frame data, shared GPU layer per mode |
| 2D backends | wgpu (default) and SDL_Renderer (fallback) |
| 3D backends | wgpu only |
| Mode switch | Per process at startup (`graphics = auto / wgpu / wgpu-gl / sdl`) |
| Legacy GL | wgpu's GL backend (GL 3.3 / GLES 3.0 / WebGL2); below that, SDL mode |
| Shared 2D drawing | `Draw2D`, used by both 2D renderers, debug draw and the fallback UI |
| Custom renderers | `CustomRenderer` (wgpu mode only), replaces 2D and 3D |

## 8. As built (4C.1, 4C.2, 4C.3)

Deviations and decisions:

- **Frame:** `RenderFrame { d2: Frame2D { camera, ignored_cameras, sprites }, d3: Frame3D { camera, ignored_cameras, meshes },
  textures, frame, time }`. `CameraView` gained `target` (copied from `Camera::target`; 0 = main window; used since the multi-window phase, see
  `docs/MULTI_WINDOW.md`: `RenderFrame::windows` holds the other windows' parts). One active camera per kind: `Camera::kind` 0 (orthographic) feeds `d2`, 1 (perspective,
  `Camera::perspective_3d()`) feeds `d3`; `RenderWarnings` has a flag per kind plus `no_renderer_3d`.
- **Traits:** `Renderer2D`, `Renderer3D`, `CustomRenderer`. `GpuCx` was not introduced: the traits take the existing
  `&Gpu` (wgpu handles) and a `Target { view, clear }`; 4C.4 will make both enums over the mode. `Target::clear` is how the
  pass order works: the first pass clears, later ones load.
- **Textures:** `&TextureChanges` (the frame's list) is passed to both renderers; the engine does not take it.
- **Defaults:** the default 2D renderer is `SpriteRenderer` (nearest filtering), not `ClearRenderer` (kept as a 2D renderer that
  only clears). A 3D renderer is called only on frames with an active 3D camera; the 2D renderer is called every frame.
- **Engine API:** `set_renderer_2d`, `set_renderer_3d`, `set_renderer` (custom). `render/renderers.rs` holds the slots and the
  pass order. `examples/custom_renderer` only changed the trait name and `frame.d2` / `frame.d3` field paths.
- **GPU mode (4C.2):** `GpuMode { Auto, Wgpu, WgpuGl, Sdl }` in `StartupConfig::gpu_mode` (JS `gpuMode`), plus env
  `EFE_GPU_MODE` on native when the config is `auto`. Native tries `PRIMARY` then `GL` with a separate wgpu instance each
  (a single `PRIMARY | GL` instance picks by device type, so it cannot guarantee GL last); `wgpu-gl` only `GL`. `Auto` and `Wgpu`
  behave the same until 4C.5 (auto will end in SDL mode). `Sdl` errors at graphics init until 4C.4. Native GL uses
  `Limits::downlevel_defaults()` (WebGL2 keeps `downlevel_webgl2_defaults()`). The GL instance needs the display handle at
  creation (`InstanceDescriptor::display`, a small `Send + Sync` wrapper around SDL's raw display handle), otherwise Mesa EGL
  on Wayland reports "gl not compatible with provided surface".
- **Result:** coin_dash runs natively in Vulkan (default) and with `EFE_GPU_MODE=wgpu-gl` (log: `graphics backend: Gl (Mesa
  Intel UHD 620)`), on Wayland and with `SDL_VIDEODRIVER=x11`, no panics or wgpu errors. No pixel comparison was made (the game
  has no offscreen screenshot path).

### 8.1 Draw2D (4C.3)

Files: `render/draw2d.rs` (API, `DrawList`), `draw2d_wgpu.rs` + `draw2d.wgsl` (wgpu backend), `debug_draw.rs`,
`offscreen.rs`. Deviations from the sketch in 3.3:

```rust
pub trait Draw2D {
    fn set_clip(&mut self, rect: Option<RectI>);
    fn set_blend(&mut self, blend: Blend);          // None | Alpha | Add | Premultiplied
    fn set_filter(&mut self, filter: Filter);       // Nearest | Linear
    fn triangles(&mut self, texture: Option<TextureHandle>, verts: &[Vertex2D], indices: &[u32]);
    // provided on top of `triangles`: fill_rect, quad(texture, rect, uv, color), rect_outline, line, polyline
}
pub struct Vertex2D { pub pos: [f32; 2], pub uv: [f32; 2], pub color: [f32; 4] }
```

- **No transform and no `Affine2` on quads.** Everything is in **target pixels** (physical, origin top-left, y down); callers
  convert world to pixels on the CPU (the debug draw does, in the extract). `SDL_Renderer` has no vertex transform either, so
  this is the common subset. Rotated quads are just four vertices through `triangles`.
- **`DrawList`** (plain data, `Clone`) implements `Draw2D` and records batched commands: consecutive calls with the same
  state (texture, blend, filter, clip) merge, each command has a vertex range and indices relative to its first vertex (so an
  SDL backend passes the slices straight to `SDL_RenderGeometry`). `replay(&mut dyn Draw2D)` and `append` exist. A backend
  either consumes a `DrawList` (wgpu does: `WgpuDraw2D::prepare` + `draw` inside a pass, or `render` for a pass of its own) or
  implements `Draw2D` directly (immediate; SDL can). Invalid calls (index out of range, count not a multiple of 3) are ignored.
- **Colours** are straight RGBA, sRGB-encoded (what UI authors and SDL use); the wgpu vertex shader converts to linear when the
  surface is sRGB. (`Camera::clear_color` and sprite tints stay linear as before.) Blending on an sRGB surface is therefore
  done in linear space in wgpu; SDL blends the encoded values, so semi-transparent edges differ slightly between modes.
- **Textures** are handles of the engine `Textures` store (straight-alpha RGBA8). A glyph atlas or nine-slice texture is added
  with `Textures::add` and updated with `Textures::replace` (whole image; a sub-rectangle update is a later addition, SDL has
  `SDL_UpdateTexture` with a rect). Unknown handles draw as white. `GpuTextures` is the shared wgpu table (one bind group per
  filter), used by `SpriteRenderer` and `WgpuDraw2D`, so a texture is uploaded once.
- **SpriteRenderer keeps its instanced sprite pipeline** (a world-matrix instanced quad is cheaper than CPU triangles and keeps
  coin_dash byte-identical); only the texture table is shared. After the sprites, `frame.d2.draw` (a `DrawList`, new `Frame2D`
  field, cleared and refilled every frame) is drawn in the same pass. SDL's `Renderer2D` will draw sprites as
  `SDL_RenderGeometry` quads, converted on the CPU with the camera matrix.
- **Debug draw (4A.17):** `DebugDraw2D` resource (also in the prelude): `line`, `rect`, `rect_filled`, `circle` in world units and
  `screen_*` in pixels, `set_width` in pixels (default 1). Shapes live one frame: `render_extract` converts them with the 2D
  camera (world shapes are dropped without one) into `frame.d2.draw` and empties the queue. Drawn over the sprites, in push order.
- **Clip** is clamped to the target and becomes one scissor rectangle per command (SDL: `SDL_SetRenderClipRect`).
- **Fallback UI (4C.6)** needs only this: rounded rects as triangle fans or nine-slice quads with per-vertex colour, glyph quads
  from an atlas texture with `Filter::Linear`/`Nearest`, `set_clip` for scroll areas, `Premultiplied` for glyph atlases stored
  premultiplied. Missing and wanted later: sub-rect texture update, a `RenderOverlay` hook that hands out a `Draw2D`.
- **Tests:** unit tests for `DrawList`, clip clamping and the debug draw projection; `tests/render.rs` checks the extract; the
  `#[ignore]`d GPU test `draw2d_pixels` (`cargo test -p efeengine-core --lib draw2d_pixels -- --ignored`) draws rects, a
  texture, clip, additive blend, a line, a vertex-coloured triangle and a sprite into an offscreen target and checks pixels.
  `EFE_SCREENSHOT_DIR=dir` also writes `draw2d.png`.
- **Offscreen screenshots:** `OffscreenGpu` (native only; own device, `Rgba8UnormSrgb` target) with `render_2d(renderer, frame,
  changes)`, `read_rgba8`, and `write_png` / `encode_png` (uncompressed, no deps). `examples/coin_dash/tests/sprite_screenshot.rs`
  (`#[ignore]`) runs coin_dash headless at 960x600, renders frame 32 through `SpriteRenderer` with a debug bounding box, asserts
  determinism and writes a PNG (`EFE_SCREENSHOT=path`). That PNG is the reference for the 4C.4 SDL parity check.

#### SDL_Renderer mapping (checked against the vendored SDL 3.4 headers)

| Draw2D | SDL3 |
|---|---|
| `triangles` | `SDL_RenderGeometry(renderer, texture-or-NULL, SDL_Vertex[] {position, SDL_FColor color, tex_coord}, n, indices, m)`; one texture per call, matching `DrawCommand` |
| `set_clip` | `SDL_SetRenderClipRect` (integer rect; `None` = `NULL`) |
| `Blend::None / Alpha / Add / Premultiplied` | `SDL_BLENDMODE_NONE / BLEND / ADD / BLEND_PREMULTIPLIED` (set per texture with `SDL_SetTextureBlendMode`, for untextured geometry with `SDL_SetRenderDrawBlendMode`) |
| `Filter` | `SDL_SetTextureScaleMode` (`NEAREST` / `LINEAR`), a property of the texture, set before the call |
| textures | `SDL_CreateTexture(RGBA32, STATIC)` + `SDL_UpdateTexture` from `TextureChanges`; straight alpha |
| lines | quads through `triangles` (not `SDL_RenderLines`, which is 1 px): same pixels in both modes |

Not expressible in SDL and therefore not in Draw2D: custom shaders, per-draw transforms, non-RGBA formats, render-to-texture
with depth. Colour-space differences: SDL does no sRGB conversion, so for equal output the SDL backend draws vertex colours as is and
the sprite tint conversion must match (tints are linear in wgpu; SDL must convert them to sRGB). Plan the parity test around
opaque sprites first.

## 8.2 SDL mode and the `auto` fallback (4C.4, 4C.5)

Files: `render/sdl_draw2d.rs`, `window/*` (`sdl_window_ptr`), `engine.rs` (`init_graphics`, `init_sdl_mode`,
`render_frame_to_surface`), `render/gpu.rs` (test hook). User docs: `RENDERING.md` ("GPU mode", "SDL mode").

Decisions (deviations from sections 3.2 / 8):

- **No `GpuCx` / `Target` enum.** The wgpu traits (`Renderer2D`, `Renderer3D`, `CustomRenderer`, `RenderOverlay`) keep taking
  `&Gpu` and stay wgpu-only. Making them enums would have forced every renderer (and `examples/custom_renderer`) to match a
  variant it can never see. SDL mode is a second, engine-owned path instead: `Engine` has `gpu: Option<GpuContext>` and
  `sdl: Option<SdlMode>`, exactly one is set after `init_graphics`. `SdlMode` = `SdlDraw2D` (Draw2D on the window's
  `SDL_Renderer` plus the texture table) + `SdlSpriteRenderer` (the built-in 2D renderer for this mode).
- **Sprites in SDL mode go through `Draw2D`** (four vertices through `triangles`, CPU transform in the shader's order, 1/256 px
  snap), not through an instanced pipeline; the wgpu `SpriteRenderer` keeps its instanced path unchanged.
- **Unsupported things warn once and are skipped**: `CustomRenderer`, a user-installed wgpu `Renderer2D` (the built-in
  `SpriteRenderer` installed explicitly is recognised by `TypeId` and not warned about), 3D cameras, overlays. The UI crate needed
  no change: its overlay is only reached through `RenderOverlay::init`, which SDL mode never calls.
- **Colour**: tints and the clear colour are encoded linear -> sRGB in the SDL renderer; vertex colours and texture bytes are
  already encoded. Blending of translucent pixels is in encoded space (wgpu: linear).
- **`auto`**: `GpuContext::new` fails (no adapter / surface for `PRIMARY` and `GL`) -> `init_sdl_mode` with a warning log line;
  only for non-headless windows, native only. `wgpu` / `wgpu-gl` fail instead (error logged, engine keeps ticking without
  rendering, as before). `sdl` goes straight to SDL mode. On the web `sdl` warns and uses wgpu.
- **Test hook**: `EFE_TEST_NO_WGPU=1` makes the native adapter search fail.
- **Window**: `InnerWindow::sdl_window_ptr()` (null for headless) lets the engine create the `SDL_Renderer`; the renderer is
  declared before `windows` in `Engine`, so it drops first. Vsync is on (`SDL_SetRenderVSync(1)`); SDL picks the driver
  (opengl on the dev machine; `SDL_RENDER_DRIVER` overrides).
- **Offscreen**: `SdlDraw2D::offscreen(size)` = `SDL_CreateSoftwareRenderer` on an RGBA32 `SDL_Surface`; verified to work with no
  window and no video driver. Parity test `coin_dash_wgpu_and_sdl_match` (`#[ignore]`, needs a GPU): 960x600 frame 32,
  0.107% of the pixels differ by more than 4/255 (all around animated sprites at fractional positions), mean error 0.12/255;
  PNGs with `EFE_SCREENSHOT_DIR`. `coin_dash_frame_through_sdl_renderer` runs everywhere. Before the 1/256 snap the SDL
  rasteriser left hairline seams between tiles (5.4% of pixels off).
- **Not done / next**: fallback UI on Draw2D (4C.6), so UI-only screens (the coin_dash menu) show nothing in SDL mode yet;
  `with_linear_filtering` is ignored in SDL mode; sub-rect texture updates; runtime window screenshot of SDL mode was not taken
  (headless offscreen parity only).
