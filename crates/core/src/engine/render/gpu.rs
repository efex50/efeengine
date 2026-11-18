//! The engine-owned GPU context: instance, adapter, device, queue, and the window's surface.

use wgpu::{Adapter, Device, Instance, Queue, Surface, SurfaceConfiguration};

use crate::engine::window::GameWindowManager;
use crate::engine::input::WindowId;
use super::{ExtraSurface, Gpu};

/// Everything the engine needs to render to the main window. Declared in `Engine` BEFORE the
/// window so the surface drops before the window it was created from.
pub(crate) struct GpuContext {
    instance: Instance,
    adapter: Adapter,
    backend: wgpu::Backend,
    pub device: Device,
    pub queue: Queue,
    pub surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    /// The window has a zero side (minimized): no texture is acquired and nothing is
    /// reconfigured until a non-zero size arrives.
    suspended: bool,
    /// Window scale factor (physical per logical pixel).
    pub scale_factor: f32,
    /// Surfaces of the secondary windows, on the same device (`docs/MULTI_WINDOW.md`).
    pub windows: std::collections::BTreeMap<u32, WindowSurface>,
}

/// A secondary window's surface and the size / scale the engine last gave it.
pub(crate) struct WindowSurface {
    pub surface: ExtraSurface,
    pub size: (u32, u32),
    pub scale_factor: f32,
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
        let (instance, surface, adapter) = native_adapter(window).await?;
        #[cfg(target_arch = "wasm32")]
        let (instance, surface, adapter) = {
            let instance = web_instance().await;
            let surface = create_surface(&instance, window)?;
            let adapter = request_adapter(&instance, &surface).await?;
            (instance, surface, adapter)
        };

        let info = adapter.get_info();
        crate::global_info(&format!("graphics backend: {:?} ({})", info.backend, info.name));

        // WebGL2 cannot meet wgpu's default limits. Take the max texture size from
        // the adapter, so large / HiDPI canvases are not capped at the defaults.
        // Native GL (3.3+ / GLES 3.0) gets the downlevel limits, WebGL2 the stricter ones.
        let base_limits = if info.backend == wgpu::Backend::Gl {
            if cfg!(target_arch = "wasm32") { wgpu::Limits::downlevel_webgl2_defaults() } else { wgpu::Limits::downlevel_defaults() }
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
        let size = window.size(WindowId::MAIN);
        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: caps.present_modes.first().copied().unwrap_or(wgpu::PresentMode::Fifo),
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        let backend = info.backend;
        Ok(Self { instance, adapter, backend, device, queue, surface, config, suspended: false, scale_factor: sanitize_scale(window.scale_factor(WindowId::MAIN)), windows: Default::default() })
    }

    /// The borrowed view handed to the renderer.
    pub fn gpu(&self) -> Gpu<'_> {
        Gpu {
            adapter: &self.adapter,
            instance: &self.instance,
            backend: self.backend,
            device: &self.device,
            queue: &self.queue,
            surface_format: self.config.format,
            size: (self.config.width, self.config.height),
            scale_factor: self.scale_factor,
            window: 0,
        }
    }

    /// The handles for drawing into secondary window `id`: its size and scale, the format of
    /// its surface. `None` if it has no surface.
    pub fn gpu_for(&self, id: u32) -> Option<Gpu<'_>> {
        if id == 0 {
            return Some(self.gpu());
        }
        let w = self.windows.get(&id)?;
        Some(Gpu { surface_format: w.surface.format(), size: w.size, scale_factor: w.scale_factor, window: id, ..self.gpu() })
    }

    /// Give secondary window `id` a surface (native: from its raw handles; web: its canvas).
    /// `size` / `scale` are what the engine knows now.
    pub fn add_window(&mut self, windows: &GameWindowManager, id: WindowId, size: (u32, u32), scale: f64) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        let surface = {
            let (rw, rd) = windows.raw_handles(id).ok_or_else(|| format!("window {} has no native handles", id.0))?;
            // SAFETY: the surface lives in `self.windows`, which `Engine` drops before the
            // window manager (declared earlier in `Engine`); a closed window's surface is
            // removed (`remove_window`) before the window is destroyed.
            let target = wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(rd), raw_window_handle: rw };
            let s = unsafe { self.instance.create_surface_unsafe(target) }.map_err(|e| format!("could not create surface: {e}"))?;
            let mut es = ExtraSurface::from_surface(&self.gpu(), s)?;
            es.resize(&self.gpu(), size);
            es
        };
        #[cfg(target_arch = "wasm32")]
        let surface = {
            let canvas = windows.canvas(id).ok_or_else(|| format!("window {} has no canvas", id.0))?;
            let mut es = ExtraSurface::for_canvas(&self.gpu(), canvas)?;
            es.resize(&self.gpu(), size);
            es
        };
        self.windows.insert(id.0, WindowSurface { surface, size, scale_factor: sanitize_scale(scale) });
        Ok(())
    }

    /// Drop the surface of a closed window (before the window itself goes away).
    pub fn remove_window(&mut self, id: WindowId) {
        self.windows.remove(&id.0);
    }

    /// A secondary window's size changed.
    pub fn resize_window(&mut self, id: WindowId, size: (u32, u32)) -> bool {
        let Some(mut w) = self.windows.remove(&id.0) else { return false };
        w.size = size;
        w.surface.resize(&self.gpu(), size);
        self.windows.insert(id.0, w);
        true
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

async fn request_adapter(instance: &Instance, surface: &Surface<'static>) -> Result<Adapter, String> {
    instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: Some(surface),
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }).await.map_err(|e| format!("no suitable graphics adapter: {e}"))
}

/// The window system's display connection as an owned value for `InstanceDescriptor::display`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct DisplayOwner(raw_window_handle::RawDisplayHandle);

// SAFETY: the raw display handle (wl_display / X11 Display pointer) is only ever handed to
// wgpu's EGL setup, and stays valid for as long as the SDL video subsystem, which outlives the
// GPU context (`Engine` drops `gpu` before `windows`). The engine uses it from the main thread.
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Send for DisplayOwner {}
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Sync for DisplayOwner {}

#[cfg(not(target_arch = "wasm32"))]
impl raw_window_handle::HasDisplayHandle for DisplayOwner {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: see the `Send` impl above.
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(self.0) })
    }
}

/// Native instance, surface and adapter for the GPU mode. The backend sets are tried in order
/// and each gets its own instance (wgpu picks among every adapter of one instance by device
/// type, not by backend, so one `PRIMARY | GL` instance could not guarantee GL comes last):
/// `PRIMARY` (Vulkan / Metal / DX12), then `GL`. `wgpu-gl` tries only `GL`.
#[cfg(not(target_arch = "wasm32"))]
async fn native_adapter(window: &GameWindowManager) -> Result<(Instance, Surface<'static>, Adapter), String> {
    use crate::config::GpuMode;
    // Test hook: pretend no adapter exists (this is how the `auto` -> SDL fallback is tested on
    // a machine that has a GPU). `wgpu` / `wgpu-gl` then fail, `auto` falls back to SDL mode.
    if std::env::var_os("EFE_TEST_NO_WGPU").is_some_and(|v| !v.is_empty() && v != "0") {
        return Err("no wgpu adapter (forced by EFE_TEST_NO_WGPU)".to_string());
    }
    let sets: &[(wgpu::Backends, &str)] = match GpuMode::effective() {
        GpuMode::Auto | GpuMode::Wgpu => &[(wgpu::Backends::PRIMARY, "Vulkan/Metal/DX12"), (wgpu::Backends::GL, "GL")],
        GpuMode::WgpuGl => &[(wgpu::Backends::GL, "GL")],
        GpuMode::Sdl => return Err("gpu mode \"sdl\" has no wgpu context".to_string()),
    };
    let mut errors = Vec::new();
    for &(backends, name) in sets {
        // The GL backend needs the platform display (Wayland / X11) when the instance is
        // created, to make its EGL display; the others do not use it.
        let desc = if backends.contains(wgpu::Backends::GL) {
            use raw_window_handle::HasDisplayHandle;
            match window.display_handle() {
                Ok(h) => wgpu::InstanceDescriptor::new_with_display_handle(Box::new(DisplayOwner(h.as_raw()))),
                Err(e) => return Err(format!("no display handle for GL: {e}")),
            }
        } else {
            wgpu::InstanceDescriptor::new_without_display_handle()
        };
        let instance = Instance::new(wgpu::InstanceDescriptor { backends, ..desc });
        let attempt = match create_surface(&instance, window) {
            Ok(surface) => request_adapter(&instance, &surface).await.map(|a| (surface, a)),
            Err(e) => Err(e),
        };
        match attempt {
            Ok((surface, adapter)) => return Ok((instance, surface, adapter)),
            Err(e) => {
                crate::global_warn(&format!("graphics backend set {name} unavailable: {e}"));
                errors.push(format!("{name}: {e}"));
            }
        }
    }
    Err(format!("no usable graphics backend ({})", errors.join("; ")))
}

/// Pick WebGPU or WebGL2 per the startup config's `graphicsBackend`.
/// This must be decided before the canvas gets a context: a canvas that has a
/// WebGPU context can never get a WebGL one, so a fallback after the fact is impossible.
#[cfg(target_arch = "wasm32")]
async fn web_instance() -> Instance {
    use crate::config::{GraphicsBackend, startup_config};

    let gl_only = || Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let choice = startup_config().graphics_backend;
    if choice == GraphicsBackend::Webgl
        || (choice == GraphicsBackend::Auto && is_desktop_linux_browser())
    {
        return gl_only();
    }
    // Drops BROWSER_WEBGPU when `navigator.gpu` is missing or gives no adapter.
    wgpu::util::new_instance_with_webgpu_detection(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
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
        let canvas = window.canvas(WindowId::MAIN).ok_or_else(|| {
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
        // wgpu 30 wants the display handle either on the instance or on the surface target
        // (`SurfaceTargetUnsafe::from_window` leaves it empty, which fails on Wayland / X11).
        // The SDL display is not `Send + Sync + 'static`, so it is passed per surface.
        use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
        let raw_window_handle = window.window_handle()
            .map_err(|e| format!("no window handle: {e}"))?.as_raw();
        let raw_display_handle = window.display_handle()
            .map_err(|e| format!("no display handle: {e}"))?.as_raw();
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(raw_display_handle),
            raw_window_handle,
        };
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

/// Scale factors that are not finite and positive count as 1.0.
pub(crate) fn sanitize_scale(s: f64) -> f32 {
    if s.is_finite() && s > 0.0 { s as f32 } else { 1.0 }
}
