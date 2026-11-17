//! The engine-owned GPU context: instance, adapter, device, queue, and the window's surface.

use wgpu::{Adapter, Device, Instance, Queue, Surface, SurfaceConfiguration};

use crate::engine::window::GameWindowManager;
use crate::window::WindowManager;
use super::Gpu;

/// Everything the engine needs to render to the main window. Declared in `Engine` BEFORE the
/// window so the surface drops before the window it was created from.
pub(crate) struct GpuContext {
    #[allow(dead_code)]
    instance: Instance,
    #[allow(dead_code)]
    adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
    pub surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    /// The window has a zero side (minimized): no texture is acquired and nothing is
    /// reconfigured until a non-zero size arrives.
    suspended: bool,
}

/// What a resize event asks of the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResizePlan {
    /// The new suspended state.
    pub suspended: bool,
    /// Configure the surface with the new size.
    pub reconfigure: bool,
}

/// Pure resize logic: a zero side suspends rendering (the old size is kept, nothing is
/// reconfigured); the next non-zero size resumes and always reconfigures, even at the old
/// size, because the surface may have been invalidated while minimized.
pub(crate) fn plan_resize(size: (u32, u32)) -> ResizePlan {
    let zero = size.0 == 0 || size.1 == 0;
    ResizePlan { suspended: zero, reconfigure: !zero }
}

impl GpuContext {
    /// Create the GPU context and configure the window's surface.
    ///
    /// On the web this prefers WebGPU and falls back to WebGL2 when the browser
    /// has no working WebGPU adapter. The surface is created before the adapter
    /// because WebGL can only hand out an adapter for a given canvas.
    pub async fn new(window: &GameWindowManager) -> Result<Self, String> {
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
                label: Some("efeengine device"),
                required_features: wgpu::Features::empty(),
                required_limits: base_limits.using_resolution(adapter.limits()),
                ..Default::default()
            },
        ).await.map_err(|e| format!("could not create device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).or_else(|| caps.formats.first().copied())
            .ok_or_else(|| "the surface supports no texture format".to_string())?;
        let size = window.size();
        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: caps.present_modes.first().copied().unwrap_or(wgpu::PresentMode::Fifo),
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        Ok(Self { instance, adapter, device, queue, surface, config, suspended: false })
    }

    /// The borrowed view handed to the renderer.
    pub fn gpu(&self) -> Gpu<'_> {
        Gpu {
            device: &self.device,
            queue: &self.queue,
            surface_format: self.config.format,
            size: (self.config.width, self.config.height),
        }
    }

    /// Reconfigure the surface for `size`. A zero side (minimized) suspends the context
    /// instead; the next non-zero size resumes it. Returns whether it was reconfigured.
    pub fn resize(&mut self, size: (u32, u32)) -> bool {
        let plan = plan_resize(size);
        self.suspended = plan.suspended;
        if !plan.reconfigure {
            return false;
        }
        self.config.width = size.0;
        self.config.height = size.1;
        self.surface.configure(&self.device, &self.config);
        true
    }

    /// `true` while the window has a zero size: nothing may be acquired or reconfigured.
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Reconfigure with the current size (after Lost / Outdated). No-op while suspended.
    pub fn reconfigure(&mut self) {
        if self.suspended {
            return;
        }
        self.surface.configure(&self.device, &self.config);
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
    // SAFETY: the returned surface is stored in `Engine::gpu`, which `Engine`
    // declares before (and so drops before) `Engine::windows`, whose inner window's raw
    // handles it uses. The window never moves out of
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_side_suspends_and_non_zero_resumes_with_reconfigure() {
        assert_eq!(plan_resize((0, 0)), ResizePlan { suspended: true, reconfigure: false });
        assert_eq!(plan_resize((0, 600)), ResizePlan { suspended: true, reconfigure: false });
        assert_eq!(plan_resize((800, 0)), ResizePlan { suspended: true, reconfigure: false });
        assert_eq!(plan_resize((800, 600)), ResizePlan { suspended: false, reconfigure: true });
    }
}
