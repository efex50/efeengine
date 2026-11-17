//! The demo's GPU-view painter: a rotating triangle drawn with wgpu directly (premultiplied,
//! transparent background, so the panel shows through the view's corners).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use efeengine_core::render::Gpu;
use efeengine_core::wgpu;
use efeengine_ui::{UiPainter, ViewTarget};

/// The shape colors as sRGB (what the screen shows); a click on the view steps through them.
/// View targets are sRGB formats and painters output linear colour (API.md 17.2), so they are
/// converted before they reach the shader.
pub const PALETTE: [[f32; 3]; 4] = [[1.0, 0.5, 0.0], [0.2, 0.9, 0.3], [0.9, 0.2, 0.8], [0.3, 0.6, 1.0]];

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Index into [`PALETTE`], shared by the update system (writer) and the painter (reader).
#[derive(Clone, Default)]
pub struct ShapeColor(pub Arc<AtomicU32>);

const WGSL: &str = r#"
struct U { color: vec4<f32>, angle: f32, aspect: f32, _p0: f32, _p1: f32 };
@group(0) @binding(0) var<uniform> u: U;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let a = u.angle + f32(i) * 2.0943951;
    let p = vec2<f32>(cos(a), sin(a)) * 0.8;
    return vec4<f32>(p.x / u.aspect, p.y, 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> {
    // Premultiplied: the triangle is opaque.
    return vec4<f32>(u.color.rgb, 1.0);
}
"#;

struct Gfx {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

pub struct ShapePainter {
    color: ShapeColor,
    gfx: Option<Gfx>,
    #[cfg(not(target_arch = "wasm32"))]
    t0: std::time::Instant,
    #[cfg(target_arch = "wasm32")]
    frames: u32,
}

impl ShapePainter {
    pub fn new(color: ShapeColor) -> Self {
        Self {
            color,
            gfx: None,
            #[cfg(not(target_arch = "wasm32"))]
            t0: std::time::Instant::now(),
            #[cfg(target_arch = "wasm32")]
            frames: 0,
        }
    }

    fn angle(&mut self) -> f32 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.t0.elapsed().as_secs_f32()
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.frames += 1;
            self.frames as f32 / 60.0
        }
    }
}

impl UiPainter for ShapePainter {
    fn resize(&mut self, gpu: &Gpu, target: &ViewTarget) {
        // The pipeline depends on the target format only, but `resize` also runs for a new
        // format, so rebuild here.
        let device = gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui_demo shape"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui_demo shape"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: target.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui_demo shape uniform"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        self.gfx = Some(Gfx { pipeline, buffer, group });
    }

    fn paint(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, target: &ViewTarget) {
        let angle = self.angle();
        let Some(g) = &self.gfx else { return };
        let c = PALETTE[self.color.0.load(Ordering::Relaxed) as usize % PALETTE.len()].map(srgb_to_linear);
        let aspect = target.size.0 as f32 / target.size.1.max(1) as f32;
        let data: [f32; 8] = [c[0], c[1], c[2], 1.0, angle, aspect, 0.0, 0.0];
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        gpu.queue.write_buffer(&g.buffer, 0, &bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui_demo shape"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&g.pipeline);
        pass.set_bind_group(0, &g.group, &[]);
        pass.draw(0..3, 0..1);
    }
}
