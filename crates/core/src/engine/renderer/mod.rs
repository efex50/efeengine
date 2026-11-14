use std::sync::Arc;

use wgpu::{Instance, Surface, Adapter, Device, Queue, SurfaceConfiguration, SurfaceCapabilities};
use wgpu::util::DeviceExt; // create_buffer_init için gerekli
use crate::engine::window::GameWindowManager;
use crate::window::WindowManager;


// rewrite
pub struct RendererState{
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,
    window: Arc<GameWindowManager>,
}
impl RendererState {
    pub async fn new(window:Arc<GameWindowManager>) -> Self{
        let size = window.size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            #[cfg(not(target_arch = "wasm32"))]
            backends: wgpu::Backends::PRIMARY ,
            #[cfg(target_arch = "wasm32")]
            backends: wgpu::Backends::GL | wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        // The safe `create_surface` needs a `Send + Sync` window handle, which an
        // SDL window is not (it must stay on the main thread).
        // SAFETY: the window's raw handles stay valid for the whole life of the
        // surface: the `Arc<GameWindowManager>` is stored in `self.window`, which is
        // declared after `surface` and therefore dropped after it.
        let surface = unsafe {
            let target = wgpu::SurfaceTargetUnsafe::from_window(&*window).unwrap();
            instance.create_surface_unsafe(target).unwrap()
        };
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptionsBase {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }).await.unwrap();


        let (device , queue) = adapter.request_device(&wgpu::DeviceDescriptor{
            label: None,
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: if cfg!(target_arch  = "wasm32"){
                wgpu::Limits::downlevel_webgl2_defaults()
            }else{
                    wgpu::Limits::default()
            },
            memory_hints: Default::default(),
            trace: wgpu::Trace::Off,
        }).await.unwrap();

        let surface_caps = surface.get_capabilities(&adapter);


        // tutorial srgb üzerinden
        let surface_format = surface_caps.formats.iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.0,
            height: size.1,
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };


        
        Self{
            surface,
            device,
            queue,
            config,
            is_surface_configured: false,
            window,
        }
    }
    pub fn resize(&mut self,width:u32,height:u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            self.is_surface_configured = true;
        }
    }
}

















// ------------- ai gen code
// todo rewrite

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    position: [f32; 2],
    time: f32,
    _padding:u32,
}

impl Uniforms {
    fn new() -> Self {
        Self {
            position: [0.0, 0.0],
            time: 0.0,
            _padding:0,
        }
    }
}
pub struct GraphicsContext {
    instance: Instance,
    adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
}

pub struct SurfaceManager {
    surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    pub size: (u32, u32),
    render_pipeline: wgpu::RenderPipeline,
    
    // YENİ EKLENENLER:
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    uniforms: Uniforms, // CPU tarafındaki veriyi tutmak için
}

impl GraphicsContext {
    /// Create the GPU context and the window's surface.
    ///
    /// On the web this prefers WebGPU and falls back to WebGL2 when the browser
    /// has no working WebGPU adapter. The surface is created before the adapter
    /// because WebGL can only hand out an adapter for a given canvas.
    pub async fn new(window: &GameWindowManager) -> Result<(Self, SurfaceManager), String> {
        #[cfg(not(target_arch = "wasm32"))]
        let instance = Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        #[cfg(target_arch = "wasm32")]
        let instance = web_instance().await;

        let surface = create_surface(&instance, window)?;

        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.map_err(|e| format!("no suitable graphics adapter: {e}"))?;

        let info = adapter.get_info();
        crate::global_info(&format!("graphics backend: {:?} ({})", info.backend, info.name));

        // WebGL2 cannot meet wgpu's default limits. Take the max texture size from
        // the adapter, so large / HiDPI canvases are not capped at the defaults.
        let base_limits = if info.backend == wgpu::Backend::Gl {
            wgpu::Limits::downlevel_webgl2_defaults()
        } else {
            wgpu::Limits::default()
        };
        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("RealGameEngine Device"),
                required_features: wgpu::Features::empty(),
                required_limits: base_limits.using_resolution(adapter.limits()),
                ..Default::default()
            },
        ).await.map_err(|e| format!("could not create device: {e}"))?;

        let surface_manager = SurfaceManager::new(surface, &adapter, window.size(), &device);
        Ok((Self { instance, adapter, device, queue }, surface_manager))
    }
}

/// Pick WebGPU or WebGL2 per the startup config's `graphicsBackend`.
/// This must be decided before the canvas gets a context: a canvas that has a
/// WebGPU context can never get a WebGL one, so a fallback after the fact is impossible.
#[cfg(target_arch = "wasm32")]
async fn web_instance() -> Instance {
    use crate::config::{GraphicsBackend, startup_config};

    let gl_only = || Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        ..Default::default()
    });
    let choice = startup_config().graphics_backend;
    if choice == GraphicsBackend::Webgl
        || (choice == GraphicsBackend::Auto && is_desktop_linux_browser())
    {
        return gl_only();
    }
    // Drops BROWSER_WEBGPU when `navigator.gpu` is missing or gives no adapter.
    wgpu::util::new_instance_with_webgpu_detection(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
        ..Default::default()
    }).await
}

/// Desktop Linux (not Android / ChromeOS), from the user agent.
#[cfg(target_arch = "wasm32")]
fn is_desktop_linux_browser() -> bool {
    let Some(ua) = web_sys::window().and_then(|w| w.navigator().user_agent().ok()) else {
        return false;
    };
    ua.contains("Linux") && !ua.contains("Android") && !ua.contains("CrOS")
}

fn create_surface(instance: &Instance, window: &GameWindowManager) -> Result<Surface<'static>, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let canvas = window.canvas().ok_or_else(|| {
            format!("canvas element with id \"{}\" not found (or not a <canvas>)", window.canvas_id())
        })?;
        instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| format!("could not create surface: {e}"))
    }
    // SAFETY: the returned surface is stored in `window.surface_manager`,
    // which `GameWindowManager` declares before (and so drops before) the
    // inner window whose raw handles it uses. The window never moves out of
    // its owner while the surface exists, and is not `Send`, so both stay on
    // the main thread.
    #[cfg(not(target_arch = "wasm32"))]
    unsafe {
        let target = wgpu::SurfaceTargetUnsafe::from_window(window)
            .map_err(|e| format!("no window handle: {e}"))?;
        instance.create_surface_unsafe(target)
            .map_err(|e| format!("could not create surface: {e}"))
    }
}

impl SurfaceManager {
    fn new(surface: Surface<'static>, adapter: &Adapter, size: (u32, u32), device: &Device) -> Self {
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps.formats.iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(device, &config);

        // --- UNIFORM HAZIRLIĞI ---
        let mut uniforms = Uniforms::new();
        // İstersen burada başlangıç pozisyonu verebilirsin
        uniforms.position = [0.0, 0.0]; 

        let uniform_buffer = device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Uniform Buffer"),
                contents: bytemuck::cast_slice(&[uniforms]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX, // Hem vertex hem fragment kullanıyorsa VERTEX | FRAGMENT
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }
            ],
            label: Some("uniform_bind_group_layout"),
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                }
            ],
            label: Some("uniform_bind_group"),
        });

        // --- PIPELINE HAZIRLIĞI ---
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Render Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout], // Layout burada ekleniyor
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"), // Option<str> oldu yeni versiyonlarda
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },

            multiview: None,
            cache: None,
        });

        Self {
            surface,
            config,
            size,
            render_pipeline,
            uniform_buffer,
            bind_group,
            uniforms,
        }
    }

    pub fn resize(&mut self, new_size: (u32, u32), device: &Device) {
        if new_size.0 > 0 && new_size.1 > 0 {
            self.size = new_size;
            self.config.width = new_size.0;
            self.config.height = new_size.1;
            self.surface.configure(device, &self.config); 
        }
    }

    pub fn render(&mut self, device: &Device, queue: &Queue, engine: &crate::engine::EngineState) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());


        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK ),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.render_pipeline);
            // world objeleri ekle
            {
                for obj in &engine.world.objects{
                    
                    self.uniforms.position = obj.position;
                    self.uniforms.time += 0.005;

                    queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[self.uniforms]));

                    // Binding'i (Group 0) pipeline'a bağla
                    render_pass.set_bind_group(0, &self.bind_group, &[]);
                    render_pass.draw(0..3, 0..1);
                }
            }
        }

        queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
const SHADER_SOURCE: &str = include_str!("../../../shaders/triangle_anim.wgsl");
