//! Rendering: the engine side of it.
//!
//! The engine owns the GPU (instance, adapter, device, surface, configure, resize, acquire,
//! present) and builds a plain-data [`RenderFrame`] each frame in `Stage::RenderExtract`. A
//! game supplies the drawing as a [`Renderer`] (`Engine::set_renderer`); the default is
//! [`ClearRenderer`], which only clears. See `docs/RENDERING.md`.

mod extract;
mod frame;
pub(crate) mod gpu;
mod png;
mod sprite_renderer;
mod textures;

pub use extract::{camera_matrices, render_extract};
pub use frame::{CameraView, MeshInstance, RenderFrame, RenderWarnings, SpriteInstance};
pub use png::PngError;
pub use sprite_renderer::SpriteRenderer;
pub use textures::{Image, TextureChanges, TextureError, Textures};

/// Borrowed GPU handles for a [`Renderer`] call.
pub struct Gpu<'a> {
    /// The adapter the device was created from (renderers like `iced_wgpu` need it).
    pub adapter: &'a wgpu::Adapter,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// Format of the surface; render pipelines that draw to `target` must use it.
    pub surface_format: wgpu::TextureFormat,
    /// Size of the surface in physical pixels.
    pub size: (u32, u32),
    /// Window scale factor (physical pixels per logical pixel): logical = physical / scale.
    pub scale_factor: f32,
}

/// The game's drawing code. The engine calls it on the main thread only (no `Send` needed;
/// on the web there is one thread anyway).
pub trait Renderer: 'static {
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

/// Extra drawing on top of what the main [`Renderer`] drew (a UI layer, debug HUD, ...).
/// Added with `Engine::add_render_overlay`; main thread only (no `Send` needed).
pub trait RenderOverlay: 'static {
    /// Once when the GPU is ready (or when added, if it already is).
    fn init(&mut self, _gpu: &Gpu) {}
    /// The surface was resized or the scale factor changed (`gpu.size` / `gpu.scale_factor`
    /// are the new values).
    fn resize(&mut self, _gpu: &Gpu) {}
    /// Draw on top of what the main renderer drew into `target` this frame (use
    /// `LoadOp::Load`, never clear). Called every rendered frame after `Renderer::render` and
    /// before present, in the order the overlays were added.
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView);
}

/// The default renderer: clears to the camera's clear colour (black without a camera) and
/// draws nothing else.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClearRenderer;

impl Renderer for ClearRenderer {
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView, frame: &RenderFrame) {
        let [r, g, b, a] = frame.camera.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |c| c.clear_color);
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("clear") });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }),
                        store: wgpu::StoreOp::Store,
                    },
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
