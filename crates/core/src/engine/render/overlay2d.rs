//! The [`Draw2DOverlay`](super::Draw2DOverlay) runner: records the overlays of one window into a
//! [`DrawList`], applies the textures they uploaded, and draws the list with the active mode's
//! `Draw2D` (wgpu: [`WgpuDraw2D`] on the window's surface view; SDL: the window's `SdlDraw2D`).

use std::sync::Arc;

use super::draw2d::DrawList;
use super::draw2d_wgpu::{GpuTextures, WgpuDraw2D};
use super::textures::{Image, TextureChanges};
use super::{Draw2DOverlay, Gpu, Overlay2D, OverlayTextures};
use crate::engine::components::TextureHandle;

/// Uploads collected while the overlays record.
#[derive(Default)]
struct Pending(Vec<(TextureHandle, Arc<Image>)>);

impl OverlayTextures for Pending {
    fn upload(&mut self, handle: TextureHandle, image: &Image) {
        self.0.retain(|(h, _)| *h != handle);
        self.0.push((handle, Arc::new(image.clone())));
    }
}

#[derive(Default)]
pub(crate) struct Overlays2D {
    items: Vec<Box<dyn Draw2DOverlay>>,
    wgpu: Option<(GpuTextures, WgpuDraw2D)>,
    list: DrawList,
}

impl Overlays2D {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn add(&mut self, o: Box<dyn Draw2DOverlay>) {
        self.items.push(o);
    }

    fn record(&mut self, window: u32, size: (u32, u32), scale: f32, integer_vertices: bool) -> Pending {
        self.list.clear();
        let mut pending = Pending::default();
        for o in &mut self.items {
            let mut cx = Overlay2D { window, size, scale_factor: scale, integer_vertices, draw: &mut self.list, textures: &mut pending };
            o.render(&mut cx);
        }
        pending
    }

    fn wgpu(&mut self, gpu: &Gpu) -> &mut (GpuTextures, WgpuDraw2D) {
        self.wgpu.get_or_insert_with(|| {
            let t = GpuTextures::new(gpu);
            let d = WgpuDraw2D::new(gpu, &t);
            (t, d)
        })
    }

    /// Mirrors the frame's texture changes into the overlays' own GPU table (wgpu mode).
    pub fn sync_textures(&mut self, gpu: &Gpu, changes: &TextureChanges) {
        if self.items.is_empty() {
            return;
        }
        self.wgpu(gpu).0.sync(gpu, changes);
    }

    /// wgpu mode: run the overlays for `gpu.window` and draw them over `view`.
    pub fn render_wgpu(&mut self, gpu: &Gpu, view: &wgpu::TextureView) {
        if self.items.is_empty() {
            return;
        }
        let pending = self.record(gpu.window, gpu.size, gpu.scale_factor, false);
        let list = std::mem::take(&mut self.list);
        let (tex, draw) = self.wgpu(gpu);
        for (h, img) in &pending.0 {
            tex.upload(gpu, h.id, img.width(), img.height(), img.rgba8());
        }
        draw.render(gpu, tex, view, &list);
        self.list = list;
    }

    /// SDL mode: run the overlays for the main window and draw them (before present).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_sdl(&mut self, d: &mut super::SdlDraw2D, scale: f32) {
        if self.items.is_empty() {
            return;
        }
        let size = d.output_size();
        let software = d.renderer_name() == "software";
        let pending = self.record(0, size, scale, software);
        let changes = TextureChanges { updated: pending.0, ..Default::default() };
        d.sync(&changes);
        self.list.replay(d);
    }
}
