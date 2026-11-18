# Rendering

The engine does the plumbing; the game draws. Each frame the engine turns the ECS world into a
plain-data `RenderFrame` (split into `d2: Frame2D` and `d3: Frame3D`) and hands it to the renderers: a
`Renderer2D` (default `SpriteRenderer`), an optional `Renderer3D`, or one `CustomRenderer` for everything. Renderers never
touch the world. Design and plan: [`RENDERER_PLAN.md`](RENDERER_PLAN.md).

## Data flow

```
components ──► systems (per stage) ──► RenderFrame (resource) ──► Renderer3D, Renderer2D, overlays
```

| Stage | System | What it does |
|---|---|---|
| `FixedUpdate` | `velocity_system` | `Transform.position += Velocity.linear * dt`, `rotation += angular * dt` (fixed delta) |
| `PostUpdate` | `propagate_transforms` | `GlobalTransform = parent_global * local` along the `Parent` chain; inserts `GlobalTransform` where missing; rebuilds `Children`. A missing/despawned parent (or one without `Transform`) makes the entity a root. A cycle is broken (the first entity of it in id order becomes a root) and warned once. |
| `RenderExtract` | `render_extract` | builds `RenderFrame` (below) |

Then the engine acquires the surface, draws the passes (3D if there is a 3D camera, then 2D, then `RenderOverlay`s) and presents.
Headless engines (and a failed GPU init) skip drawing but still build the `RenderFrame`, so it can be tested.

Components (`engine/components.rs`): `Camera`, `Sprite`, `TextureHandle`, `Visibility`, `Parent`, `GlobalTransform`
are script-visible (registered, in the API index); `Children` is Rust-only. A missing `Visibility` means visible;
it is not inherited by children.

`RenderFrame`: `d2: Frame2D { camera, ignored_cameras, sprites }`, `d3: Frame3D { camera, ignored_cameras, meshes }`,
`textures: TextureChanges`, `frame`, `time`. `d2.sprites` is a `Vec<SpriteInstance>` (sorted by
`layer`, then entity id; `auto_size != 0` resolves the size to the texture's pixel size, 1x1 without a texture; with `auto_size == 0` `size` is used as is, so 0 means zero-size),
`d3.meshes` is a `Vec<MeshInstance>`.

## One camera per kind

One 2D (`Camera::kind == ORTHOGRAPHIC`, goes to `d2.camera`) and one 3D (`PERSPECTIVE`, `d3.camera`, `Camera::perspective_3d()`)
camera is used per frame: per kind, the entity with `Camera.active != 0` and the lowest entity id. If more than
one of a kind is active, the rest are ignored (`RenderFrame::ignored_cameras`) and a warning is logged once
(`RenderWarnings::multiple_cameras`). No active camera: `camera` is `None`. Position and rotation come from the
camera entity's `GlobalTransform` (else `Transform`); scale is ignored.
`Camera.target` (0 = the main window) is reserved: later, cameras will be assigned to screen panels or windows
(several cameras per frame). It is ignored today.

## Textures

`Textures` (resource; `engine.textures_mut()`, or `ctx.resources.get_mut::<Textures>()` in a system) stores CPU
images (`Image`: width, height, RGBA8 sRGB, length checked) under `TextureHandle`s (id 0 = none; ids never reused).
`add` / `replace` / `remove` queue changes; the extract drains them into `RenderFrame::textures`
(`added`, `updated`, `removed`) exactly once, collapsed to the net effect (add then remove never appears; two
replaces are one update). If a frame could not be drawn, its changes are queued again. The renderer mirrors them in
its own GPU texture map.

### PNG

`Image::from_png(&[u8])`, `Textures::add_png(&[u8])` (all targets: use `include_bytes!` for assets of a web build) and,
native only, `Textures::load_png(path)`. The decoder (`render/png.rs`) is pure Rust on `miniz_oxide` + `crc32fast`.
It reads non-interlaced PNGs: 8-bit gray, gray+alpha, RGB, RGBA and palette (with `tRNS` alpha), 1/2/4-bit gray and
palette, all five filters, many `IDAT` chunks; output is RGBA8. It rejects, with a `PngError`: Adam7 interlacing,
16-bit samples, a bad signature, a CRC mismatch, truncated data, and sizes over 16384 per side / 16384 x 16384 pixels.
Gray/RGB `tRNS` colour keys are ignored. Garbage input returns an error and never panics (a mutation test checks it).

## Matrix conventions

- 4x4 matrices are `[[f32; 4]; 4]`, column-major, `m[column][row]`: the memory layout of WGSL `mat4x4<f32>`.
  Upload them as-is (`bytemuck::bytes_of`). Vectors are columns, `p' = M * p`.
- World: y up, x right, rotation counter-clockwise, 1 unit = 1 pixel at zoom 1.
- `CameraView::view`: world -> view (inverse camera transform). `projection`: view -> clip, orthographic for
  `kind` 0: half-extents `viewport / 2 / zoom`, centred, y up, so the camera position is the screen centre and
  there is no stretching after a resize (the extract uses the window's current size). `view_projection = projection * view`.
- Clip space is wgpu's: x, y in [-1, 1], z in [0, 1]. The 2D default `near = -1000`, `far = 1000` puts z = 0 inside.
- `SpriteInstance::world`: quad -> world (the entity's world transform lifted to 4x4). The quad is the unit square
  centred on the origin, scaled by `size` (world units). `uv_min` is the top-left of the image, `flip` bit 0/1 mirror x/y
  (applied by the renderer).
- Sprites arrive back to front; draw in order, batching only consecutive runs of one texture.

## The renderer traits

```rust
pub struct Target<'a> { pub view: &'a wgpu::TextureView, pub clear: bool }  // clear: first pass of the frame
pub trait Renderer2D: 'static {
    fn init(&mut self, gpu: &Gpu) {}     // once, when the GPU is ready (or at set_renderer_2d if it is)
    fn resize(&mut self, gpu: &Gpu) {}   // surface size changed
    fn render(&mut self, gpu: &Gpu, target: &Target, frame: &Frame2D, textures: &TextureChanges);
}
pub trait Renderer3D: 'static { /* same, with frame: &Frame3D */ }
pub trait CustomRenderer: 'static {      // whole frame, whole surface; replaces 2D + 3D
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView, frame: &RenderFrame);
    /* init, resize as above */
}
```

`Engine::set_renderer_2d(r)`, `set_renderer_3d(r)`, `set_renderer(custom)`. Setting a 2D or 3D renderer drops a custom one;
setting a custom one replaces both until a 2D/3D renderer is set again. Defaults: `SpriteRenderer` as 2D, no 3D renderer
(a 3D camera then logs a warning once and draws nothing). Pass order: 3D (only with an active 3D camera; `Target::clear`
is true), 2D (always, also without a camera; clears unless the 3D pass drew; use `Target::load_op(clear_color)`),
then overlays. `ClearRenderer` is a 2D renderer that only clears. Texture changes are the same list for every renderer
of the frame. Custom renderers read `frame.d2` / `frame.d3`.
No `Send` bound: the engine and the renderers stay on the main thread (the web has one thread).
`wgpu` and `bytemuck` are re-exported from the prelude (`efeengine_core::wgpu`, `render::wgpu`).

## GPU mode

Startup config `gpuMode` (`StartupConfig::gpu_mode`, `GpuMode`) or, on native when the config says `auto`, the
environment variable `EFE_GPU_MODE`: `auto` (default), `wgpu`, `wgpu-gl`, `sdl`. Native `auto` / `wgpu` try wgpu on
Vulkan / Metal / DX12 first and then on GL (one instance per backend set, so GL is really last); `wgpu-gl` uses GL only
(GL 3.3+ / GLES 3.0, downlevel limits, display handle given to the instance, needed on Wayland). `sdl` is reserved
for Phase 4C.4 and currently fails graphics init with a clear error. The web ignores it (`graphicsBackend` picks
WebGPU or WebGL2). The log line `graphics backend: Gl (...)` shows what was chosen.

## `SpriteRenderer` (built in)

The default 2D renderer (`engine.set_renderer_2d(SpriteRenderer::new())` to choose it or its filter explicitly) draws `frame.d2.sprites` with no further code (`SpriteRenderer::with_linear_filtering()`
for smooth scaling; the default is nearest-neighbour, for pixel art). Source: `render/sprite_renderer.rs` and
`render/sprite.wgsl`, written to be read; it does what the template below asks for.

- Textures: `frame.textures` is mirrored into `Rgba8UnormSrgb` GPU textures, each with a bind group; handle id 0 is a 1x1
  white texture (also used for a handle the renderer does not know). A texture larger than the device limit is skipped
  with a warning.
- Camera: `view_projection` goes into a 64-byte uniform; the target is cleared to `clear_color`. Without a camera it
  clears to black and draws nothing.
- Instances: every sprite is one 112-byte instance in a vertex buffer (the 4x4 `world` matrix, size, `uv_min`/`uv_max`,
  colour, flip) drawn as a 6-vertex quad built from `vertex_index`. The buffer is written once per frame and grows
  (powers of two) when the frame has more sprites.
- Batching: a batch is a run of consecutive sprites with the same texture (draw order is never changed), one
  `draw(0..6, 0..n)` each. A frame with one texture is one draw call.
- Blending: straight alpha (`SrcAlpha`, `OneMinusSrcAlpha`), no depth buffer: the order of the sprites is the order of
  the draws, so layers and transparency work by sorting alone.
- Flip is done in the vertex shader by swapping which UV corner a vertex gets.
- WebGL2: no storage buffers (instance data is in a vertex buffer), no `first_instance` (each batch binds its own
  slice of the instance buffer and draws from instance 0), 9 vertex attributes (limit 16), sRGB texture format.
- Colour: with an sRGB surface (the engine prefers one) the output is correct; on a non-sRGB fallback surface colours
  look darker (no gamma step in the shader).
- Not done: text, debug lines, meshes, several cameras, render-to-texture.

Example: `examples/coin_dash` (tile ground from an atlas, an animated sprite sheet, a parented shadow, PNG assets).

## Template

`examples/custom_renderer`: `src/renderer.rs` is `MyRenderer`, a commented skeleton with TODOs (pipelines in
`init`, texture map sync from `frame.textures`, camera uniform upload, instance buffer and per-texture batching,
`resize`); it clears the screen and draws nothing. `shaders/sprite.wgsl` is a stub that documents the expected
bindings and instance inputs. `src/lib.rs` spawns a camera, three generated textures and sprites (one parented, one
moving with `Velocity`) and logs a `RenderFrame` summary once a second.
Run: `cargo xtask run custom_renderer native`.
