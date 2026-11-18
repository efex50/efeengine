//! The wgpu side of [`Draw2D`](super::Draw2D): the GPU texture table and the triangle pipeline.
//!
//! * [`GpuTextures`] mirrors the engine's texture store into GPU textures (from
//!   [`TextureChanges`]), one bind group per sampling filter. [`SpriteRenderer`](super::SpriteRenderer)
//!   and [`WgpuDraw2D`] share one table, so a texture is uploaded once.
//! * [`WgpuDraw2D`] draws a [`DrawList`]: `prepare` uploads the vertices / indices and the target
//!   size, `draw` records the commands into a render pass (one pipeline per blend mode, one
//!   `draw_indexed` per command, scissor rectangle per command), `render` does both in a pass of
//!   its own. Textures the table does not hold are drawn as white.
//! * Colours: the vertex shader converts the sRGB-encoded vertex colours to linear when the
//!   surface format is sRGB (the sample of the sRGB texture is already linear).
//! * WebGL2: indices are made absolute on the CPU (no `base_vertex`), one pipeline per blend.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use super::draw2d::{Blend, DrawList, Filter, RectI, Vertex2D};
use super::textures::TextureChanges;
use super::Gpu;

fn filter_index(f: Filter) -> usize {
    match f {
        Filter::Nearest => 0,
        Filter::Linear => 1,
    }
}

struct GpuTexture {
    /// Kept to own the GPU resource that the bind groups view.
    _texture: wgpu::Texture,
    /// Bind group (texture + sampler) per [`Filter`].
    binds: [wgpu::BindGroup; 2],
}

/// GPU textures by handle id, filled from [`TextureChanges`]. Id 0 is a 1x1 white texture.
pub struct GpuTextures {
    layout: wgpu::BindGroupLayout,
    samplers: [wgpu::Sampler; 2],
    map: HashMap<u32, GpuTexture>,
}

impl GpuTextures {
    pub fn new(gpu: &Gpu) -> Self {
        let device = gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = |f: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("texture sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: f,
                min_filter: f,
                ..Default::default()
            })
        };
        let mut t = Self { layout, samplers: [sampler(wgpu::FilterMode::Nearest), sampler(wgpu::FilterMode::Linear)], map: HashMap::new() };
        t.upload(gpu, 0, 1, 1, &[255, 255, 255, 255]);
        t
    }

    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Create (or replace) the GPU texture of handle `id` from straight-alpha sRGB RGBA8 pixels.
    pub fn upload(&mut self, gpu: &Gpu, id: u32, width: u32, height: u32, rgba8: &[u8]) {
        let max = gpu.device.limits().max_texture_dimension_2d;
        if width > max || height > max {
            crate::global_warn(&format!("texture {id} is {width}x{height}, over this device's limit of {max}; not uploaded"));
            self.map.remove(&id);
            return;
        }
        let texture = gpu.device.create_texture_with_data(
            gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("texture"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // sRGB: the sampler converts to linear, the sRGB surface converts back
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba8,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = |s: &wgpu::Sampler| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("texture"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(s) },
                ],
            })
        };
        let binds = [bind(&self.samplers[0]), bind(&self.samplers[1])];
        self.map.insert(id, GpuTexture { _texture: texture, binds });
    }

    /// Mirror one frame's texture changes. (They are delivered once, so call this every frame.)
    pub fn sync(&mut self, gpu: &Gpu, changes: &TextureChanges) {
        for (h, img) in changes.added.iter().chain(&changes.updated) {
            self.upload(gpu, h.id, img.width(), img.height(), img.rgba8());
        }
        for h in &changes.removed {
            self.map.remove(&h.id);
        }
    }

    pub fn contains(&self, id: u32) -> bool {
        self.map.contains_key(&id)
    }

    /// The bind group to draw `id` with `filter`; unknown ids give the white texture.
    pub(crate) fn bind(&self, id: u32, filter: Filter) -> &wgpu::BindGroup {
        let t = self.map.get(&id).unwrap_or_else(|| &self.map[&0]);
        &t.binds[filter_index(filter)]
    }

    /// The sprite renderer's lookup: a bind group for a texture that exists.
    pub(crate) fn get_bind(&self, id: u32, filter: Filter) -> Option<&wgpu::BindGroup> {
        self.map.get(&id).map(|t| &t.binds[filter_index(filter)])
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    size: [f32; 2],
    srgb: u32,
    _pad: u32,
}

const BLENDS: [Blend; 4] = [Blend::None, Blend::Alpha, Blend::Add, Blend::Premultiplied];

fn blend_index(b: Blend) -> usize {
    BLENDS.iter().position(|x| *x == b).expect("known blend")
}

fn blend_state(b: Blend) -> Option<wgpu::BlendState> {
    use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation::Add};
    let c = |s, d| C { src_factor: s, dst_factor: d, operation: Add };
    match b {
        Blend::None => None,
        Blend::Alpha => Some(wgpu::BlendState { color: c(F::SrcAlpha, F::OneMinusSrcAlpha), alpha: c(F::One, F::OneMinusSrcAlpha) }),
        Blend::Add => Some(wgpu::BlendState { color: c(F::SrcAlpha, F::One), alpha: c(F::Zero, F::One) }),
        Blend::Premultiplied => Some(wgpu::BlendState { color: c(F::One, F::OneMinusSrcAlpha), alpha: c(F::One, F::OneMinusSrcAlpha) }),
    }
}

/// Clamp a clip rectangle to a `size` target: `(x, y, w, h)`, or `None` if nothing is left.
pub(crate) fn clamp_clip(r: RectI, size: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let x0 = (r.x as i64).clamp(0, size.0 as i64);
    let y0 = (r.y as i64).clamp(0, size.1 as i64);
    let x1 = (r.x as i64 + r.w.max(0) as i64).clamp(0, size.0 as i64);
    let y1 = (r.y as i64 + r.h.max(0) as i64).clamp(0, size.1 as i64);
    (x1 > x0 && y1 > y0).then(|| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Draws [`DrawList`]s with wgpu. See the module docs.
pub struct WgpuDraw2D {
    pipelines: [wgpu::RenderPipeline; 4],
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    index_capacity: usize,
    /// Reused CPU staging for the absolute indices.
    index_staging: Vec<u32>,
    /// Size of the target the last `prepare` was for (clip clamping).
    size: (u32, u32),
}

const VERTEX_BYTES: u64 = std::mem::size_of::<Vertex2D>() as u64;
const VERTEX_ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];

fn buffer(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

impl WgpuDraw2D {
    pub fn new(gpu: &Gpu, textures: &GpuTextures) -> Self {
        let device = gpu.device;
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw2d view layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let view_buffer = buffer(device, "draw2d view", 16, wgpu::BufferUsages::UNIFORM);
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draw2d view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: view_buffer.as_entire_binding() }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("draw2d.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("draw2d.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("draw2d pipeline layout"),
            bind_group_layouts: &[Some(&view_layout), Some(textures.layout())],
            immediate_size: 0,
        });
        let pipeline = |b: Blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("draw2d pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_BYTES,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &VERTEX_ATTRS,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: gpu.surface_format, blend: blend_state(b), write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let (vertex_capacity, index_capacity) = (1024, 1536);
        Self {
            pipelines: BLENDS.map(pipeline),
            view_buffer,
            view_bind_group,
            vertex_buffer: buffer(device, "draw2d vertices", vertex_capacity as u64 * VERTEX_BYTES, wgpu::BufferUsages::VERTEX),
            index_buffer: buffer(device, "draw2d indices", index_capacity as u64 * 4, wgpu::BufferUsages::INDEX),
            vertex_capacity,
            index_capacity,
            index_staging: Vec::new(),
            size: gpu.size,
        }
    }

    /// Upload `list` and the target size (`gpu.size`). Call before [`Self::draw`], outside the pass.
    pub fn prepare(&mut self, gpu: &Gpu, list: &DrawList) {
        self.size = gpu.size;
        let view = ViewUniform {
            size: [gpu.size.0.max(1) as f32, gpu.size.1.max(1) as f32],
            srgb: u32::from(gpu.surface_format.is_srgb()),
            _pad: 0,
        };
        gpu.queue.write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(&view));
        if list.is_empty() {
            return;
        }
        let verts = list.vertices();
        if verts.len() > self.vertex_capacity {
            self.vertex_capacity = verts.len().next_power_of_two();
            self.vertex_buffer = buffer(gpu.device, "draw2d vertices", self.vertex_capacity as u64 * VERTEX_BYTES, wgpu::BufferUsages::VERTEX);
        }
        // absolute indices: every command's are relative to its first vertex
        self.index_staging.clear();
        self.index_staging.resize(list.indices().len(), 0);
        for c in list.commands() {
            for k in c.indices.start..c.indices.end {
                self.index_staging[k as usize] = list.indices()[k as usize] + c.verts.start;
            }
        }
        if self.index_staging.len() > self.index_capacity {
            self.index_capacity = self.index_staging.len().next_power_of_two();
            self.index_buffer = buffer(gpu.device, "draw2d indices", self.index_capacity as u64 * 4, wgpu::BufferUsages::INDEX);
        }
        gpu.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(verts));
        gpu.queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&self.index_staging));
    }

    /// Record `list` (already `prepare`d) into `pass`. The scissor rectangle is reset to the
    /// whole target afterwards.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, textures: &GpuTextures, list: &DrawList) {
        if list.is_empty() {
            return;
        }
        let (w, h) = self.size;
        pass.set_bind_group(0, &self.view_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        let mut last_blend = None;
        for c in list.commands() {
            let scissor = match c.state.clip {
                Some(r) => match clamp_clip(r, self.size) {
                    Some(s) => s,
                    None => continue, // clipped away entirely
                },
                None => (0, 0, w, h),
            };
            if last_blend != Some(c.state.blend) {
                pass.set_pipeline(&self.pipelines[blend_index(c.state.blend)]);
                last_blend = Some(c.state.blend);
            }
            pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
            pass.set_bind_group(1, textures.bind(c.state.texture, c.state.filter), &[]);
            pass.draw_indexed(c.indices.start..c.indices.end, 0, 0..1);
        }
        pass.set_scissor_rect(0, 0, w, h);
    }

    /// Draw `list` into `target` in a pass of its own (loads the target, never clears): for
    /// overlays that draw with `Draw2D` after the renderers.
    pub fn render(&mut self, gpu: &Gpu, textures: &GpuTextures, target: &wgpu::TextureView, list: &DrawList) {
        if list.is_empty() {
            return;
        }
        self.prepare(gpu, list);
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("draw2d") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("draw2d pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw(&mut pass, textures, list);
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_is_clamped_to_the_target() {
        let s = (100, 50);
        assert_eq!(clamp_clip(RectI::new(10, 10, 20, 20), s), Some((10, 10, 20, 20)));
        assert_eq!(clamp_clip(RectI::new(-5, -5, 20, 20), s), Some((0, 0, 15, 15)));
        assert_eq!(clamp_clip(RectI::new(90, 40, 50, 50), s), Some((90, 40, 10, 10)));
        assert_eq!(clamp_clip(RectI::new(200, 0, 10, 10), s), None);
        assert_eq!(clamp_clip(RectI::new(0, 0, 0, 10), s), None);
        assert_eq!(clamp_clip(RectI::new(0, 0, -4, 10), s), None);
    }

    #[test]
    fn vertex_layout() {
        assert_eq!(VERTEX_BYTES, 32);
        assert_eq!(VERTEX_ATTRS[2].offset, 16);
    }
}
