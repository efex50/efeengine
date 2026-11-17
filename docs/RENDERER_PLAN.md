# Renderer plan: separate 2D and 3D renderers, GPU modes, SDL fallback

> **Status:** Accepted plan, not started\
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

- [ ] **4C.1 Split the frame and traits.** `Frame2D`/`Frame3D`, camera kinds, `Renderer2D`/`Renderer3D`,
  `CustomRenderer` (old trait), `GpuCx` with only the wgpu variant. The current `SpriteRenderer`
  becomes the wgpu `Renderer2D` and the default. _Done when_ coin_dash, custom_renderer and all tests
  work unchanged in behaviour.
- [ ] **4C.2 Native GL fallback for wgpu.** `PRIMARY | GL` with GL last, downlevel limits on GL, config
  switch. _Done when_ coin_dash runs with the GL backend forced natively.
- [ ] **4C.3 Draw2D.** The trait, the wgpu implementation, 2D debug draw (ROADMAP 4A.17) on top of it.
- [ ] **4C.4 SDL mode.** `graphics = sdl` startup path (SDL creates an `SDL_Renderer` for the window,
  no wgpu context), the SDL `Renderer2D` and `Draw2D`, texture table from `TextureChanges`, the
  overlay hook. _Done when_ coin_dash runs in both modes and screenshots of the two match closely
  (same sprites, layers, alpha; filtering may differ).
- [ ] **4C.5 `auto` mode fallback.** No wgpu adapter → SDL mode, with a log line saying so.
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
