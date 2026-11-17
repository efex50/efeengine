//! GPU views on iced (API.md section 17): `GpuView` nodes become iced `shader` widgets.
//!
//! The painters cannot run inside iced's `Primitive` (those must be `Send + Sync`, painters
//! are main-thread objects), so the work is split around `Renderer::present`:
//! 1. `ui.draw` runs [`ViewProgram::draw`] for every shown view: it records a [`Request`] (node,
//!    painter name, logical bounds) in the [`Shared`] state and returns a [`ViewPrimitive`].
//! 2. [`ViewPainter::paint`] (backend `render`, before `present`) goes through the requests,
//!    makes sure each view has an offscreen texture of its physical size, calls the painters
//!    (`resize` first on a new size, scale or format) and submits their commands.
//! 3. `present` calls `ViewPrimitive::prepare` / `draw`: a fullscreen triangle with the view
//!    texture, drawn with iced's viewport = the view's bounds and scissor = its clip, with
//!    premultiplied blending, in UI order (iced layers).
//! Idle frames (nothing changed) repaint the textures from the last requests and present again.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use efeengine_core::render::Gpu;
use iced_core::{Event, Rectangle, mouse};
use iced_widget::Action;
use iced_widget::shader::{self, Pipeline, Primitive, Viewport};
use iced_wgpu::wgpu;

use crate::id::WidgetId;
use crate::painter::{UiPainters, ViewTarget};
use crate::signal::PointerEvent;

use super::view::Msg;

/// A view the last `ui.draw` showed.
#[derive(Clone)]
struct Request {
    id: WidgetId,
    painter: Arc<str>,
    /// Logical px.
    bounds: Rectangle,
}

/// The format of every view texture: painters output linear colour, the hardware encodes.
const VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct ViewTex {
    // Keeps the texture alive; the view is what painters and the compositor use.
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

#[derive(Default)]
pub struct Shared {
    requests: Vec<Request>,
    textures: HashMap<WidgetId, ViewTex>,
}

/// The state iced's widgets and the backend share. Cheap to clone.
#[derive(Clone, Default)]
pub struct GpuViews(Arc<Mutex<Shared>>);

impl GpuViews {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Forget the views of the last draw (call right before `ui.draw`).
    pub fn begin_draw(&self) {
        self.lock().requests.clear();
    }
}

/// Paints the views and tracks what each painter last saw.
#[derive(Default)]
pub struct ViewPainter {
    warned: HashSet<Arc<str>>,
    /// Per node: the `(size, scale bits, format)` its painter's `resize` last got.
    seen: HashMap<WidgetId, ((u32, u32), u32, wgpu::TextureFormat)>,
}

impl ViewPainter {
    /// Paints every view of the last draw once. `scale` is the window scale factor.
    pub fn paint(&mut self, views: &GpuViews, painters: Option<&UiPainters>, gpu: &Gpu) {
        let mut sh = views.lock();
        if sh.requests.is_empty() && sh.textures.is_empty() {
            return;
        }
        let scale = gpu.scale_factor.max(0.01);
        // View targets are always sRGB (API.md 17.2), whatever the window surface is.
        let format = VIEW_FORMAT;
        let reqs = sh.requests.clone();
        let mut live: HashSet<WidgetId> = HashSet::new();
        let mut encoder: Option<wgpu::CommandEncoder> = None;
        for r in &reqs {
            if live.contains(&r.id) {
                continue;
            }
            let size = ((r.bounds.width * scale).round() as u32, (r.bounds.height * scale).round() as u32);
            if size.0 == 0 || size.1 == 0 {
                continue;
            }
            let Some(painters) = painters.filter(|p| p.contains(&r.painter)) else {
                if self.warned.insert(r.painter.clone()) {
                    efeengine_core::prelude::global_warn(&format!(
                        "[efeengine-ui] GPU view uses painter \"{}\" but none is registered (add_ui_painter); the view stays transparent",
                        r.painter
                    ));
                }
                continue;
            };
            let _ = live.insert(r.id);
            if sh.textures.get(&r.id).is_none_or(|t| t.size != size) {
                let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("efeengine-ui gpu view"),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let _ = sh.textures.insert(r.id, ViewTex { _texture: texture, view, size });
            }
            let tex = sh.textures.get(&r.id).expect("inserted above");
            let target = ViewTarget { view: &tex.view, format, size, scale, widget: r.id };
            let key = (size, scale.to_bits(), format);
            let changed = self.seen.insert(r.id, key) != Some(key);
            let enc = encoder.get_or_insert_with(|| gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default()));
            let _ = painters.with(&r.painter, |p| {
                if changed {
                    p.resize(gpu, &target);
                }
                p.paint(gpu, enc, &target);
            });
        }
        sh.textures.retain(|id, _| live.contains(id));
        self.seen.retain(|id, _| live.contains(id));
        drop(sh);
        if let Some(enc) = encoder {
            let _ = gpu.queue.submit([enc.finish()]);
        }
    }
}

// ----- the widget ------------------------------------------------------------------------------

/// iced `shader` program of one `GpuView` node.
pub struct ViewProgram {
    pub id: WidgetId,
    pub painter: Arc<str>,
    pub pointer: bool,
    pub views: GpuViews,
}

/// Pressed buttons seen over the view (for the `buttons` mask and release outside).
#[derive(Default)]
pub struct PointerState {
    buttons: u8,
}

fn bit(b: mouse::Button) -> u8 {
    match b {
        mouse::Button::Left => PointerEvent::LEFT,
        mouse::Button::Right => PointerEvent::RIGHT,
        mouse::Button::Middle => PointerEvent::MIDDLE,
        _ => 0,
    }
}

impl shader::Program<Msg> for ViewProgram {
    type State = PointerState;
    type Primitive = ViewPrimitive;

    fn update(&self, state: &mut PointerState, event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Msg>> {
        if !self.pointer {
            return None;
        }
        let Event::Mouse(ev) = event else { return None };
        let emit = |pos: (f32, f32), buttons: u8, wheel: (f32, f32), capture: bool| {
            let a = Action::publish(Msg::Pointer(self.id, PointerEvent { pos, buttons, wheel }));
            Some(if capture { a.and_capture() } else { a })
        };
        let rel = |p: iced_core::Point| (p.x - bounds.x, p.y - bounds.y);
        match ev {
            mouse::Event::CursorMoved { position } if bounds.contains(*position) => emit(rel(*position), state.buttons, (0.0, 0.0), false),
            mouse::Event::ButtonPressed(b) => {
                let p = cursor.position_over(bounds)?;
                state.buttons |= bit(*b);
                emit(rel(p), state.buttons, (0.0, 0.0), true)
            }
            mouse::Event::ButtonReleased(b) if state.buttons & bit(*b) != 0 => {
                state.buttons &= !bit(*b);
                let p = cursor.position()?;
                emit(rel(p), state.buttons, (0.0, 0.0), cursor.is_over(bounds))
            }
            mouse::Event::WheelScrolled { delta } => {
                let p = cursor.position_over(bounds)?;
                let w = match *delta {
                    mouse::ScrollDelta::Lines { x, y } => (x, y),
                    mouse::ScrollDelta::Pixels { x, y } => (x, y),
                };
                emit(rel(p), state.buttons, w, true)
            }
            _ => None,
        }
    }

    fn draw(&self, _state: &PointerState, _cursor: mouse::Cursor, bounds: Rectangle) -> ViewPrimitive {
        self.views.lock().requests.push(Request { id: self.id, painter: self.painter.clone(), bounds });
        ViewPrimitive { id: self.id, views: self.views.clone() }
    }

    fn mouse_interaction(&self, _state: &PointerState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if self.pointer && cursor.is_over(bounds) { mouse::Interaction::Crosshair } else { mouse::Interaction::None }
    }
}

/// Composites one view texture.
pub struct ViewPrimitive {
    id: WidgetId,
    views: GpuViews,
}

impl std::fmt::Debug for ViewPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewPrimitive").field("id", &self.id).finish()
    }
}

/// Shared by every view of a renderer: the composite pipeline and this frame's bind groups.
pub struct Compositor {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    groups: HashMap<WidgetId, wgpu::BindGroup>,
}

const SHADER: &str = r#"
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
// Sampling the sRGB view texture gives linear premultiplied colour: right for an sRGB
// target (the hardware encodes on write).
@fragment fn fs(i: VOut) -> @location(0) vec4<f32> { return textureSample(t, s, i.uv); }
// Target without sRGB encoding: encode here (premultiplied: un-premultiply, encode, redo).
fn enc(c: f32) -> f32 {
    return select(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, c <= 0.0031308);
}
@fragment fn fs_encode(i: VOut) -> @location(0) vec4<f32> {
    let v = textureSample(t, s, i.uv);
    if (v.a <= 0.0) { return v; }
    let c = v.rgb / v.a;
    return vec4<f32>(vec3<f32>(enc(c.r), enc(c.g), enc(c.b)) * v.a, v.a);
}
"#;

impl Pipeline for Compositor {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("efeengine-ui gpu view composite"),
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
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("efeengine-ui gpu view composite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("efeengine-ui gpu view composite"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if format.is_srgb() { "fs" } else { "fs_encode" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(premultiplied), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, layout, sampler, groups: HashMap::new() }
    }

    fn trim(&mut self) {
        self.groups.clear();
    }
}

impl Primitive for ViewPrimitive {
    type Pipeline = Compositor;

    fn prepare(&self, pipeline: &mut Compositor, device: &wgpu::Device, _queue: &wgpu::Queue, _bounds: &Rectangle, _viewport: &Viewport) {
        let sh = self.views.lock();
        let Some(tex) = sh.textures.get(&self.id) else {
            let _ = pipeline.groups.remove(&self.id);
            return;
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("efeengine-ui gpu view"),
            layout: &pipeline.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tex.view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&pipeline.sampler) },
            ],
        });
        let _ = pipeline.groups.insert(self.id, group);
    }

    fn draw(&self, pipeline: &Compositor, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if let Some(group) = pipeline.groups.get(&self.id) {
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        true
    }
}
