//! Rendering: the engine side of it.
//!
//! The engine owns the GPU (instance, adapter, device, surface, configure, resize, acquire,
//! present) and builds a plain-data [`RenderFrame`] each frame in `Stage::RenderExtract`. The
//! frame is split into [`Frame2D`] and [`Frame3D`], drawn by a [`Renderer2D`] (default:
//! [`SpriteRenderer`]) and an optional [`Renderer3D`]; pass order is 3D, then 2D, then the
//! [`RenderOverlay`]s. A game that wants the whole frame and the whole surface installs a
//! [`CustomRenderer`] instead. See `docs/RENDERER_PLAN.md` and `docs/RENDERING.md`.

mod debug_draw;
mod draw2d;
mod draw2d_wgpu;
mod extra_surface;
mod extract;
mod frame;
pub(crate) mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod offscreen;
mod overlay2d;
mod png;
mod renderers;
#[cfg(not(target_arch = "wasm32"))]
mod sdl_draw2d;
mod sprite_renderer;
mod textures;

pub use debug_draw::DebugDraw2D;
pub use draw2d::{Blend, Draw2D, DrawCommand, DrawList, DrawState, Filter, Rect, RectI, Vertex2D};
pub use draw2d_wgpu::{GpuTextures, WgpuDraw2D};
pub use extra_surface::{ExtraSurface, SurfaceFrame};
pub use extract::{camera_matrices, render_extract};
pub use frame::{CameraView, Frame2D, Frame3D, MeshInstance, RenderFrame, RenderWarnings, SpriteInstance, WindowFrame};
#[cfg(not(target_arch = "wasm32"))]
pub use offscreen::{encode_png, write_png, OffscreenGpu};
pub use png::PngError;
pub(crate) use overlay2d::Overlays2D;
pub(crate) use renderers::Renderers;
#[cfg(not(target_arch = "wasm32"))]
pub use sdl_draw2d::{SdlDraw2D, SdlSpriteRenderer};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use sdl_draw2d::SdlMode;
pub use sprite_renderer::SpriteRenderer;
pub use textures::{Image, TextureChanges, TextureError, Textures};

/// Borrowed GPU handles for a renderer call.
pub struct Gpu<'a> {
    /// The adapter the device was created from (renderers like `iced_wgpu` need it).
    pub adapter: &'a wgpu::Adapter,
    /// The instance the device came from (create extra surfaces on it).
    pub instance: &'a wgpu::Instance,
    /// The backend in use (`Gl` = WebGL2 on the web).
    pub backend: wgpu::Backend,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// Format of the surface; render pipelines that draw to `target` must use it.
    pub surface_format: wgpu::TextureFormat,
    /// Size of the surface in physical pixels.
    pub size: (u32, u32),
    /// Window scale factor (physical pixels per logical pixel): logical = physical / scale.
    pub scale_factor: f32,
    /// The window this call draws to: 0 = the main window. Size, scale and `surface_format` above are
    /// that window's. A renderer with size-dependent resources (depth buffer, ...) keys them by this
    /// and checks `size` on every `render`.
    pub window: u32,
}

/// The surface view a [`Renderer2D`] / [`Renderer3D`] draws into.
pub struct Target<'a> {
    /// The acquired surface view.
    pub view: &'a wgpu::TextureView,
    /// `true` for the first pass of the frame: clear to the camera's colour. `false` when an
    /// earlier pass already drew (a 3D background under 2D sprites): use `LoadOp::Load`.
    pub clear: bool,
}

impl Target<'_> {
    /// The colour attachment load op for this pass (`clear_color` is linear RGBA).
    pub fn load_op(&self, clear_color: [f32; 4]) -> wgpu::LoadOp<wgpu::Color> {
        if self.clear {
            let [r, g, b, a] = clear_color;
            wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 })
        } else {
            wgpu::LoadOp::Load
        }
    }
}

/// Draws [`Frame2D`]. The engine calls it on the main thread only (no `Send` needed; on the
/// web there is one thread anyway), every rendered frame, also without a 2D camera (then it
/// should clear and draw nothing). Install with `Engine::set_renderer_2d`.
pub trait Renderer2D: 'static {
    /// Once, when the GPU is ready (or at `set_renderer_2d` if it already is): create
    /// pipelines, buffers, samplers.
    fn init(&mut self, _gpu: &Gpu) {}

    /// The MAIN window's surface was resized (`gpu.size` is the new size): recreate
    /// size-dependent resources. Not called for the initial size (see `init`), nor for secondary
    /// windows: those draw through the same `render` with their own `gpu.window` / `gpu.size`,
    /// so keep size-dependent resources per `gpu.window` and check `gpu.size` in `render`.
    fn resize(&mut self, _gpu: &Gpu) {}

    /// Record and submit the 2D drawing into `target` (the window `gpu.window`; `frame` is that
    /// window's part of the frame). `textures` lists what to create,
    /// replace and drop in your GPU texture table; it is delivered once, so process it every
    /// call.
    fn render(&mut self, gpu: &Gpu, target: &Target, frame: &Frame2D, textures: &TextureChanges);
}

/// Draws [`Frame3D`] (wgpu only). Called before the 2D pass, only on frames that have an
/// active 3D camera. Install with `Engine::set_renderer_3d`; without one, 3D cameras log a
/// warning once and draw nothing.
pub trait Renderer3D: 'static {
    /// Once, when the GPU is ready (or at `set_renderer_3d` if it already is).
    fn init(&mut self, _gpu: &Gpu) {}

    /// The surface was resized: recreate size-dependent resources (depth buffer, ...).
    fn resize(&mut self, _gpu: &Gpu) {}

    /// Record and submit the 3D drawing into `target` (it is the first pass, so `target.clear`
    /// is set). `textures` as in [`Renderer2D::render`].
    fn render(&mut self, gpu: &Gpu, target: &Target, frame: &Frame3D, textures: &TextureChanges);
}

/// A full wgpu renderer: gets the whole [`RenderFrame`] and the whole surface and replaces
/// both the 2D and the 3D renderer (`Engine::set_renderer`). Unavailable in the planned SDL
/// mode. `examples/custom_renderer` is one.
pub trait CustomRenderer: 'static {
    /// Once, when the GPU is ready (or at `set_renderer` if it already is): create pipelines,
    /// buffers, samplers.
    fn init(&mut self, _gpu: &Gpu) {}

    /// The surface was resized (`gpu.size` is the new size): recreate size-dependent
    /// resources (depth buffers, ...). Not called for the initial size (see `init`).
    fn resize(&mut self, _gpu: &Gpu) {}

    /// Record and submit this frame's drawing into `target` (the acquired surface view).
    /// The engine presents it afterwards. `frame.textures` lists what to create, replace
    /// and drop in your GPU texture table; it is delivered once, so process it every call.
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView, frame: &RenderFrame);
}

/// Extra drawing on top of what the renderers drew (a UI layer, debug HUD, ...).
/// Added with `Engine::add_render_overlay`; main thread only (no `Send` needed).
pub trait RenderOverlay: 'static {
    /// Once per window when its surface is ready (or when added, if it already is): `gpu.window`
    /// is the window (0 = main). Secondary windows call it when they are created.
    fn init(&mut self, _gpu: &Gpu) {}
    /// The window's surface was resized or its scale factor changed (`gpu.window`, `gpu.size`,
    /// `gpu.scale_factor` are the new values).
    fn resize(&mut self, _gpu: &Gpu) {}
    /// Draw on top of what the renderers drew into `target` this frame (use `LoadOp::Load`,
    /// never clear). Called for every rendered window (`gpu.window`) after the renderers and
    /// before present, in the order the overlays were added.
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView);
}

/// Where an overlay puts textures it owns (a glyph atlas, ...). The handle is chosen by the overlay
/// and must not collide with the engine's `Textures` store, whose ids count up from 1: use ids from
/// [`OVERLAY_TEXTURE_BASE`] up. The upload takes effect at once for the draws of this call, in both
/// rendering modes. Calling it again with the same handle replaces the texture.
pub trait OverlayTextures {
    fn upload(&mut self, handle: crate::engine::components::TextureHandle, image: &Image);
}

/// First texture id an overlay may use for its own textures.
pub const OVERLAY_TEXTURE_BASE: u32 = 0xFFFF_0000;

/// What a [`Draw2DOverlay`] gets for one window and frame.
pub struct Overlay2D<'a> {
    /// The window (0 = main).
    pub window: u32,
    /// Target size in physical pixels.
    pub size: (u32, u32),
    pub scale_factor: f32,
    /// The target rasterises on integer vertices (SDL's software renderer): feathered edges do
    /// not work there.
    pub integer_vertices: bool,
    /// Draw here (physical pixels, origin top-left). The textures are those of the engine's
    /// `Textures` store plus the overlay's own (see [`OverlayTextures`]).
    pub draw: &'a mut dyn Draw2D,
    pub textures: &'a mut dyn OverlayTextures,
}

/// Extra 2D drawing on top of the frame that works in the wgpu and in the SDL rendering mode (the
/// fallback UI is one). Added with `Engine::add_draw2d_overlay`; called for every rendered window
/// after the renderers and the [`RenderOverlay`]s, in add order.
pub trait Draw2DOverlay: 'static {
    fn render(&mut self, cx: &mut Overlay2D);
}

/// A 2D renderer that only clears to the 2D camera's colour (black without a camera) and
/// draws nothing else. [`SpriteRenderer`] is the default; this is for games that draw
/// everything in an overlay or a 3D renderer.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClearRenderer;

impl Renderer2D for ClearRenderer {
    fn render(&mut self, gpu: &Gpu, target: &Target, frame: &Frame2D, _textures: &TextureChanges) {
        let clear = frame.camera.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |c| c.clear_color);
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("clear") });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: target.load_op(clear), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
    }
}
