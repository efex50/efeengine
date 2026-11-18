//! The engine's renderer slots and the pass order.

use super::frame::RenderWarnings;
use super::{CustomRenderer, Gpu, RenderFrame, Renderer2D, Renderer3D, SpriteRenderer, Target};

/// The installed renderers. A [`CustomRenderer`], when set, replaces the 2D and 3D ones.
pub(crate) struct Renderers {
    r2: Box<dyn Renderer2D>,
    r3: Option<Box<dyn Renderer3D>>,
    custom: Option<Box<dyn CustomRenderer>>,
    /// A 2D renderer other than the default was installed (SDL mode cannot run it).
    r2_installed: bool,
}

impl Renderers {
    pub fn new() -> Self {
        Self { r2: Box::new(SpriteRenderer::new()), r3: None, custom: None, r2_installed: false }
    }

    /// Install a 2D renderer (drops a custom renderer). `gpu` is `Some` when the GPU is up.
    pub fn set_2d(&mut self, r: Box<dyn Renderer2D>, gpu: Option<&Gpu>) {
        self.custom = None;
        self.r2_installed = true;
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

    /// Draw one frame into `view`: custom renderer alone, or 3D (if there is a 3D camera),
    /// then 2D.
    pub fn render(&mut self, gpu: &Gpu, view: &wgpu::TextureView, frame: &RenderFrame, warnings: &mut RenderWarnings) {
        if let Some(c) = &mut self.custom {
            c.render(gpu, view, frame);
            return;
        }
        let mut clear = true;
        if frame.d3.camera.is_some() {
            match &mut self.r3 {
                Some(r3) => {
                    r3.render(gpu, &Target { view, clear }, &frame.d3, &frame.textures);
                    clear = false;
                }
                None if !warnings.no_renderer_3d => {
                    warnings.no_renderer_3d = true;
                    crate::global_warn("a 3D camera is active but no Renderer3D is installed (Engine::set_renderer_3d); nothing is drawn for it");
                }
                None => {}
            }
        }
        self.r2.render(gpu, &Target { view, clear }, &frame.d2, &frame.textures);
    }
}
