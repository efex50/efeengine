//! The engine's renderer slots and the pass order.

use super::frame::RenderWarnings;
use super::{CustomRenderer, Gpu, RenderFrame, Renderer2D, Renderer3D, SpriteRenderer, Target, TextureChanges};

/// The installed renderers. A [`CustomRenderer`], when set, replaces the 2D and 3D ones.
pub(crate) struct Renderers {
    r2: Box<dyn Renderer2D>,
    r3: Option<Box<dyn Renderer3D>>,
    custom: Option<Box<dyn CustomRenderer>>,
    /// A 2D renderer other than the default was installed (SDL mode cannot run it).
    r2_installed: bool,
    /// Clears secondary windows while a `CustomRenderer` (main window only) is installed.
    window_clear: super::ClearRenderer,
    warned_custom_windows: bool,
}

impl Renderers {
    pub fn new() -> Self {
        Self { r2: Box::new(SpriteRenderer::new()), r3: None, custom: None, r2_installed: false, window_clear: super::ClearRenderer, warned_custom_windows: false }
    }

    /// Install a 2D renderer (drops a custom renderer). `builtin` is true for `SpriteRenderer` (SDL mode
    /// has an equivalent, so it is not reported as unsupported). `gpu` is `Some` when the GPU is up.
    pub fn set_2d(&mut self, r: Box<dyn Renderer2D>, builtin: bool, gpu: Option<&Gpu>) {
        self.custom = None;
        self.r2_installed = !builtin;
        self.r2 = r;
        if let Some(g) = gpu {
            self.r2.init(g);
        }
    }

    /// A [`CustomRenderer`] is installed.
    pub fn has_custom(&self) -> bool {
        self.custom.is_some()
    }

    /// A 2D renderer was installed with `set_2d` (not the built-in default).
    pub fn has_installed_2d(&self) -> bool {
        self.r2_installed
    }

    pub fn set_3d(&mut self, r: Box<dyn Renderer3D>, gpu: Option<&Gpu>) {
        self.custom = None;
        self.r3 = Some(r);
        if let (Some(g), Some(r3)) = (gpu, &mut self.r3) {
            r3.init(g);
        }
    }

    pub fn set_custom(&mut self, r: Box<dyn CustomRenderer>, gpu: Option<&Gpu>) {
        self.custom = Some(r);
        if let (Some(g), Some(c)) = (gpu, &mut self.custom) {
            c.init(g);
        }
    }

    pub fn init(&mut self, gpu: &Gpu) {
        if let Some(c) = &mut self.custom {
            c.init(gpu);
            return;
        }
        self.r2.init(gpu);
        if let Some(r3) = &mut self.r3 {
            r3.init(gpu);
        }
    }

    pub fn resize(&mut self, gpu: &Gpu) {
        if let Some(c) = &mut self.custom {
            c.resize(gpu);
            return;
        }
        self.r2.resize(gpu);
        if let Some(r3) = &mut self.r3 {
            r3.resize(gpu);
        }
    }

    /// Draw `window` of the frame into `view`: custom renderer alone (main window only), or 3D
    /// (if the window has a 3D camera), then 2D. `textures` is what the renderers get as the
    /// texture changes (the frame's list for the first window drawn, empty for later ones; a
    /// custom renderer reads `frame.textures` itself).
    pub fn render(&mut self, gpu: &Gpu, view: &wgpu::TextureView, frame: &RenderFrame, textures: &TextureChanges, warnings: &mut RenderWarnings) {
        let window = gpu.window;
        let Some((d2, d3)) = frame.window(window) else { return };
        if let Some(c) = &mut self.custom {
            if window == 0 {
                c.render(gpu, view, frame);
                return;
            }
            // A custom renderer draws the main window only: the others are just cleared.
            if !self.warned_custom_windows {
                self.warned_custom_windows = true;
                crate::global_warn("a CustomRenderer draws the main window only; secondary windows are cleared and nothing else");
            }
            self.window_clear.render(gpu, &Target { view, clear: true }, d2, textures);
            return;
        }
        let mut clear = true;
        if d3.camera.is_some() {
            match &mut self.r3 {
                Some(r3) => {
                    r3.render(gpu, &Target { view, clear }, d3, textures);
                    clear = false;
                }
                None if !warnings.no_renderer_3d => {
                    warnings.no_renderer_3d = true;
                    crate::global_warn("a 3D camera is active but no Renderer3D is installed (Engine::set_renderer_3d); nothing is drawn for it");
                }
                None => {}
            }
        }
        self.r2.render(gpu, &Target { view, clear }, d2, textures);
    }
}
