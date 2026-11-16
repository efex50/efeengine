//! `MyRenderer`: the skeleton of a game renderer. It compiles and runs (it clears the screen
//! to the camera's colour) but draws NOTHING yet: every `TODO` below is yours.
//!
//! How the engine and the renderer fit together:
//!
//! * The engine owns the GPU: instance, adapter, device, queue, surface, resizing, acquiring
//!   the frame and presenting it. You only receive a [`Gpu`] (device, queue, surface format,
//!   size) and the frame's `target` view.
//! * Each frame the engine builds a plain-data [`RenderFrame`] (camera, sprites, meshes,
//!   texture changes) from the ECS world. You never touch the world.
//! * Calls: `init` once, `resize` when the window size changes, `render` every frame.
//!
//! Matrices in the frame are column-major `[[f32; 4]; 4]` (`m[column][row]`), the same memory
//! layout as WGSL `mat4x4<f32>`: copy them to a buffer with `bytemuck` and use them as-is.

use std::collections::HashMap;

use efeengine_core::prelude::*;
use efeengine_core::wgpu::{self, util::DeviceExt};

/// What the camera uniform buffer holds (matches `struct Camera` in `sprite.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
}

/// Per-sprite GPU data: one of these per `SpriteInstance`, in `frame.sprites` order.
///
/// TODO: decide the exact layout together with the vertex buffer layout in the pipeline and
/// the instance attributes in `sprite.wgsl` (the field list there mirrors this struct).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[allow(dead_code)]
struct SpriteGpu {
    world: [[f32; 4]; 4],
    size: [f32; 2],
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    color: [f32; 4],
    flip: u32,
    _pad: u32,
}

/// A texture living on the GPU, created from an engine `Image`.
#[allow(dead_code)]
struct GpuTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    // TODO: a `wgpu::BindGroup` (texture view + sampler) for drawing with this texture.
}

pub struct MyRenderer {
    /// Engine texture id -> GPU texture. Kept in sync by `sync_textures`.
    textures: HashMap<u32, GpuTexture>,
    camera_buffer: Option<wgpu::Buffer>,
    // TODO: the pipeline, bind group layouts, sampler, the 1x1 white texture for id 0, the
    // instance buffer (grow it when `frame.sprites.len()` exceeds its capacity), a depth
    // texture if you want one (recreate it in `resize`).
}

impl MyRenderer {
    pub fn new() -> Self {
        Self { textures: HashMap::new(), camera_buffer: None }
    }

    /// Mirror the engine's texture changes on the GPU. The engine delivers every change
    /// exactly once, so call this at the start of every `render`, before drawing.
    fn sync_textures(&mut self, gpu: &Gpu, changes: &TextureChanges) {
        for (handle, image) in changes.added.iter().chain(changes.updated.iter()) {
            // `updated` may change the size, so a changed texture is simply recreated.
            // (A same-size update could use `queue.write_texture` instead.)
            let texture = gpu.device.create_texture_with_data(
                gpu.queue,
                &wgpu::TextureDescriptor {
                    label: Some("sprite texture"),
                    size: wgpu::Extent3d { width: image.width(), height: image.height(), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    // The pixels are sRGB encoded RGBA8; the sampler decodes them.
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                image.rgba8(),
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            // TODO: create the bind group (view + sampler) here and store it in GpuTexture.
            self.textures.insert(handle.id, GpuTexture { texture, view, size: image.size() });
        }
        for handle in &changes.removed {
            self.textures.remove(&handle.id); // dropping the wgpu objects frees them
        }
    }

    /// Upload the camera. With no camera in the frame (nothing is active) there is nothing to
    /// draw with; this keeps the previous contents.
    fn upload_camera(&mut self, gpu: &Gpu, frame: &RenderFrame) {
        let (Some(camera), Some(buffer)) = (&frame.camera, &self.camera_buffer) else { return };
        let uniform = CameraUniform { view_projection: camera.view_projection };
        gpu.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// TODO: turn `frame.sprites` into instance data and batches.
    ///
    /// `frame.sprites` is already sorted back to front (layer, then entity id), so:
    ///   1. map every `SpriteInstance` to a `SpriteGpu` (same order) and `queue.write_buffer`
    ///      them into the instance buffer;
    ///   2. walk the list and cut it into runs of consecutive sprites with the same
    ///      `texture`; each run is one draw call (`draw(0..4, run_start..run_end)`).
    ///      Do NOT sort by texture: that would break the layer order.
    ///   3. `sprite.texture.id == 0` means "no texture": bind your 1x1 white texture.
    ///   4. `flip` bit 0 / bit 1 mirror the uv in x / y; `size` is in world units.
    #[allow(dead_code)]
    fn build_instances(&mut self, _gpu: &Gpu, _frame: &RenderFrame) {}
}

impl Renderer for MyRenderer {
    fn init(&mut self, gpu: &Gpu) {
        self.camera_buffer = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        // TODO: create the shader module from `include_str!("../shaders/sprite.wgsl")`, the
        // bind group layouts, the sampler, the render pipeline targeting `gpu.surface_format`
        // (alpha blending on), and the camera bind group over `camera_buffer`.
        efeengine_core::global_info(&format!("[custom_renderer] init: surface {:?} {}x{}", gpu.surface_format, gpu.size.0, gpu.size.1));
    }

    fn resize(&mut self, gpu: &Gpu) {
        // TODO: recreate size-dependent resources (depth buffer, offscreen targets).
        // The camera's projection already follows the new size (it comes with the frame).
        efeengine_core::global_info(&format!("[custom_renderer] resize: {}x{}", gpu.size.0, gpu.size.1));
    }

    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView, frame: &RenderFrame) {
        self.sync_textures(gpu, &frame.textures);
        self.upload_camera(gpu, frame);

        let [r, g, b, a] = frame.camera.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |c| c.clear_color);
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("my renderer") });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // TODO: draw the sprites here:
            //   pass.set_pipeline(..); pass.set_bind_group(0, camera, ..);
            //   for each batch: set_bind_group(1, texture of the batch, ..); pass.draw(..)
            // TODO: draw `frame.meshes` the same way once you have meshes.
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
        // The engine presents the frame after this returns.
    }
}
