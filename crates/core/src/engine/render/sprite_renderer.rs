//! `SpriteRenderer`: the built-in wgpu [`Renderer2D`] (and the engine's default): draws `frame.sprites`.
//!
//! Install a different one with `engine.set_renderer_2d(SpriteRenderer::new())` (it is already the default, so this is only
//! needed for another filter mode). It is meant to be read: the
//! same steps are what a hand-written renderer (`examples/custom_renderer`) has to do.
//!
//! * Textures: `textures` (added / updated / removed) is mirrored into GPU textures by
//!   [`GpuTextures`] (shared with the Draw2D pass), each with a bind group (texture + sampler).
//!   `TextureHandle { id: 0 }` is a 1x1 white texture.
//! * Camera: `frame.camera.view_projection` goes into a uniform; the target is cleared to
//!   the camera's `clear_color` (black, and nothing drawn, without a camera).
//! * Sprites: each becomes one 112-byte instance in a vertex buffer (the world matrix, size,
//!   uv rectangle, tint, flip). One instanced draw of a 6-vertex quad per *batch*, where a
//!   batch is a run of consecutive sprites with the same texture (the order is already
//!   back to front, so batching must not reorder).
//! * Draw2D: after the sprites, `frame.draw` (a [`DrawList`](super::DrawList) in screen pixels: debug
//!   draw, later UI) is drawn in the same pass by a [`WgpuDraw2D`].
//! * WebGL2: no storage buffers (instance data lives in a vertex buffer), no `first_instance`
//!   (each batch binds its own slice of the buffer and draws instances `0..n`), and the
//!   texture size is checked against the device limit.
//! * The instance buffer grows (power of two) when a frame has more sprites than it holds.

use std::ops::Range;

use crate::engine::components::TextureHandle;
use super::draw2d::Filter;
use super::draw2d_wgpu::{GpuTextures, WgpuDraw2D};
use super::frame::{Frame2D, SpriteInstance};
use super::textures::TextureChanges;
use super::{Gpu, Renderer2D, Target};

/// Per-instance vertex data; must match `Instance` in `sprite.wgsl` (location order).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteGpu {
    world: [[f32; 4]; 4],
    size: [f32; 2],
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    color: [f32; 4],
    flip: u32,
    _pad: u32,
}

const INSTANCE_BYTES: u64 = std::mem::size_of::<SpriteGpu>() as u64;

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, // world matrix columns
    4 => Float32x2, // size
    5 => Float32x2, // uv_min
    6 => Float32x2, // uv_max
    7 => Float32x4, // color
    8 => Uint32,    // flip
];

impl SpriteGpu {
    fn new(s: &SpriteInstance) -> Self {
        Self { world: s.world, size: s.size, uv_min: s.uv_min, uv_max: s.uv_max, color: s.color, flip: s.flip, _pad: 0 }
    }
}

/// Consecutive runs of sprites that share a texture, as `(texture, index range)`.
/// `resolve` maps a handle to the texture that will really be bound (unknown handles
/// become the white texture), so sprites that fall back to it batch together.
fn batches(sprites: &[SpriteInstance], resolve: impl Fn(TextureHandle) -> TextureHandle) -> Vec<(TextureHandle, Range<usize>)> {
    let mut out: Vec<(TextureHandle, Range<usize>)> = Vec::new();
    for (i, s) in sprites.iter().enumerate() {
        let tex = resolve(s.texture);
        match out.last_mut() {
            Some((t, r)) if *t == tex => r.end = i + 1,
            _ => out.push((tex, i..i + 1)),
        }
    }
    out
}

/// Everything created in `init`.
struct State {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    filter: Filter,
    /// GPU textures by handle id (shared with `draw2d`).
    textures: GpuTextures,
    draw2d: WgpuDraw2D,
    instance_buffer: wgpu::Buffer,
    /// Capacity of `instance_buffer` in sprites.
    capacity: usize,
    /// Reused CPU staging for the instances.
    staging: Vec<SpriteGpu>,
}

/// A built-in 2D sprite renderer. See the module docs.
pub struct SpriteRenderer {
    linear: bool,
    state: Option<State>,
}

impl Default for SpriteRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl SpriteRenderer {
    /// Nearest-neighbour texture sampling (crisp pixel art).
    pub fn new() -> Self {
        Self { linear: false, state: None }
    }

    /// Bilinear texture sampling (smooth scaling).
    pub fn with_linear_filtering() -> Self {
        Self { linear: true, state: None }
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sprite instances"),
        size: capacity as u64 * INSTANCE_BYTES,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl State {
    fn new(gpu: &Gpu, linear: bool) -> Self {
        let device = gpu.device;

        // group 0: camera
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() }],
        });

        // group 1: texture + sampler (layout owned by the shared texture table)
        let textures = GpuTextures::new(gpu);
        let draw2d = WgpuDraw2D::new(gpu, &textures);

        // pipeline: straight-alpha blending, no depth (the order of the draws is the order)
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(textures.layout())],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRS,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.surface_format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let capacity = 256;
        Self {
            pipeline,
            camera_buffer,
            camera_bind_group,
            filter: if linear { Filter::Linear } else { Filter::Nearest },
            textures,
            draw2d,
            instance_buffer: create_instance_buffer(device, capacity),
            capacity,
            staging: Vec::new(),
        }
    }

    fn render(&mut self, gpu: &Gpu, target: &Target, frame: &Frame2D, textures: &TextureChanges) {
        self.textures.sync(gpu, textures);

        let clear_color = frame.camera.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |c| c.clear_color);
        // Without a camera there is no projection: clear and draw nothing.
        let sprites: &[SpriteInstance] = if frame.camera.is_some() { &frame.sprites } else { &[] };

        // camera uniform
        if let Some(cam) = &frame.camera {
            gpu.queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&cam.view_projection));
        }

        // instances: pack, grow the buffer if needed, upload once
        let max_sprites = (gpu.device.limits().max_buffer_size / INSTANCE_BYTES) as usize;
        let sprites = &sprites[..sprites.len().min(max_sprites)];
        self.staging.clear();
        self.staging.extend(sprites.iter().map(SpriteGpu::new));
        if self.staging.len() > self.capacity {
            self.capacity = self.staging.len().next_power_of_two().min(max_sprites.max(1));
            self.instance_buffer = create_instance_buffer(gpu.device, self.capacity);
        }
        if !self.staging.is_empty() {
            gpu.queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.staging));
        }

        let runs = batches(sprites, |t| if self.textures.contains(t.id) { t } else { TextureHandle::NONE });

        self.draw2d.prepare(gpu, &frame.draw);

        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sprites") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprite pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: target.load_op(clear_color), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !runs.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                for (tex, range) in &runs {
                    let Some(bind_group) = self.textures.get_bind(tex.id, self.filter) else { continue };
                    pass.set_bind_group(1, bind_group, &[]);
                    // bind just this batch's slice, so every draw starts at instance 0
                    let bytes = range.start as u64 * INSTANCE_BYTES..range.end as u64 * INSTANCE_BYTES;
                    pass.set_vertex_buffer(0, self.instance_buffer.slice(bytes));
                    pass.draw(0..6, 0..(range.end - range.start) as u32);
                }
            }
            self.draw2d.draw(&mut pass, &self.textures, &frame.draw);
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
    }
}

impl Renderer2D for SpriteRenderer {
    fn init(&mut self, gpu: &Gpu) {
        self.state = Some(State::new(gpu, self.linear));
    }

    fn render(&mut self, gpu: &Gpu, target: &Target, frame: &Frame2D, textures: &TextureChanges) {
        let linear = self.linear;
        self.state.get_or_insert_with(|| State::new(gpu, linear)).render(gpu, target, frame, textures);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hecs::Entity;

    fn sprite(tex: u32) -> SpriteInstance {
        SpriteInstance {
            entity: Entity::DANGLING,
            world: [[0.0; 4]; 4],
            size: [1.0, 1.0],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            texture: TextureHandle { id: tex },
            layer: 0,
            flip: 0,
        }
    }

    #[test]
    fn batches_are_runs_of_one_texture() {
        let s: Vec<_> = [1, 1, 2, 1, 1, 1].into_iter().map(sprite).collect();
        let b = batches(&s, |t| t);
        assert_eq!(b.iter().map(|(t, r)| (t.id, r.clone())).collect::<Vec<_>>(), vec![(1, 0..2), (2, 2..3), (1, 3..6)]);
        assert!(batches(&[], |t| t).is_empty());
        // unknown textures fall back to white and batch with it
        let s: Vec<_> = [0, 7, 8].into_iter().map(sprite).collect();
        assert_eq!(batches(&s, |_| TextureHandle::NONE).len(), 1);
    }

    #[test]
    fn instance_layout_matches_the_shader() {
        assert_eq!(INSTANCE_BYTES, 112);
        let last = INSTANCE_ATTRS[8];
        assert_eq!((last.offset, last.shader_location), (104, 8));
    }
}
