//! Renders a `UiTree` through the fallback backend into an offscreen target and reads the pixels
//! back, for screenshots and tests: with SDL's software renderer (runs anywhere) or with wgpu
//! (needs an adapter).

use std::sync::Arc;

use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::{
    Blend, ClearRenderer, Draw2D, DrawList, Frame2D, GpuTextures, Image, OffscreenGpu, Rect, SdlDraw2D, TextureChanges, WgpuDraw2D, encode_png,
};

use super::FallbackBackend;
use crate::backend::{UiBackend, UiBackendCx};
use crate::journal::UiChange;
use crate::style::FontId;
use crate::tree::UiTree;

/// Pixels read back (RGBA8, sRGB encoded, rows from the top).
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Shot {
    pub fn png(&self) -> Vec<u8> {
        encode_png(self.width, self.height, &self.rgba)
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// One recorded frame and the textures it needs.
pub struct Recorded {
    pub list: DrawList,
    pub changes: TextureChanges,
}

/// Runs the backend on `tree` for one frame: `size` physical px, window `scale`, opaque
/// `background` drawn first. `images` are the textures `Image` nodes use. `antialias` feathers
/// the edges of rounded shapes (off for SDL's software renderer).
pub fn record(tree: &UiTree, images: &[(TextureHandle, Arc<Image>)], size: (u32, u32), scale: f32, background: [f32; 3], antialias: bool) -> Recorded {
    let mut b = FallbackBackend::new();
    b.set_caret_blink(false);
    b.set_antialias(antialias);
    let logical = (size.0 as f32 / scale, size.1 as f32 / scale);
    let mut cx = UiBackendCx::new(logical, scale);
    cx.images = images.to_vec();
    b.init(&mut cx);
    let fonts: Vec<UiChange> = (0..tree.fonts().len()).map(|i| UiChange::Font(FontId(i as u32))).collect();
    b.sync(tree, &fonts, tree.theme(), &mut cx);
    let atlas = TextureHandle { id: images.iter().map(|i| i.0.id).max().unwrap_or(0) + 1000 };
    b.set_atlas_texture(atlas);
    b.prepare();
    let mut list = DrawList::new();
    list.set_blend(Blend::None);
    list.fill_rect(Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32), [background[0], background[1], background[2], 1.0]);
    list.set_blend(Blend::Alpha);
    b.draw(&mut list);
    let mut changes = TextureChanges::default();
    changes.added.extend(images.iter().cloned());
    if let Some(img) = b.take_atlas_image() {
        changes.added.push((atlas, img));
    }
    Recorded { list, changes }
}

/// Renders with SDL's software renderer (`SdlDraw2D::offscreen`).
pub fn render_sdl(tree: &UiTree, images: &[(TextureHandle, Arc<Image>)], size: (u32, u32), scale: f32, background: [f32; 3]) -> Result<Shot, String> {
    let r = record(tree, images, size, scale, background, false);
    let mut d = SdlDraw2D::offscreen(size)?;
    d.sync(&r.changes);
    d.begin_frame([0.0, 0.0, 0.0, 1.0]);
    r.list.replay(&mut d);
    let rgba = d.read_rgba8().ok_or("no pixels")?;
    Ok(Shot { width: size.0, height: size.1, rgba })
}

/// Renders with wgpu on an offscreen target (needs a GPU adapter).
pub fn render_wgpu(tree: &UiTree, images: &[(TextureHandle, Arc<Image>)], size: (u32, u32), scale: f32, background: [f32; 3]) -> Result<Shot, String> {
    let r = record(tree, images, size, scale, background, true);
    let mut off = OffscreenGpu::new(size)?;
    off.render_2d(&mut ClearRenderer, &Frame2D::default(), &TextureChanges::default());
    {
        let gpu = off.gpu();
        let mut tex = GpuTextures::new(&gpu);
        tex.sync(&gpu, &r.changes);
        let mut d = WgpuDraw2D::new(&gpu, &tex);
        d.render(&gpu, &tex, off.view(), &r.list);
    }
    Ok(Shot { width: size.0, height: size.1, rgba: off.read_rgba8() })
}

/// Screenshots of a live backend (one that has scrolled, focused, resized, ...) through one SDL
/// software renderer that keeps the glyph atlas between shots.
pub struct SdlShooter {
    d: SdlDraw2D,
    size: (u32, u32),
}

impl SdlShooter {
    pub fn new(size: (u32, u32)) -> Result<Self, String> {
        Ok(Self { d: SdlDraw2D::offscreen(size)?, size })
    }

    /// Records the backend's current frame (it must have an atlas texture,
    /// `set_atlas_texture`) and reads the pixels back, `background` under it.
    pub fn shoot(&mut self, b: &mut FallbackBackend, background: [f32; 3]) -> Result<Shot, String> {
        b.set_antialias(false);
        b.prepare();
        let mut list = DrawList::new();
        list.set_blend(Blend::None);
        list.fill_rect(Rect::new(0.0, 0.0, self.size.0 as f32, self.size.1 as f32), [background[0], background[1], background[2], 1.0]);
        list.set_blend(Blend::Alpha);
        b.draw(&mut list);
        let mut changes = TextureChanges::default();
        if let (Some(h), Some(img)) = (b.atlas_texture(), b.take_atlas_image()) {
            changes.added.push((h, img));
        }
        self.d.sync(&changes);
        self.d.begin_frame([0.0, 0.0, 0.0, 1.0]);
        list.replay(&mut self.d);
        let rgba = self.d.read_rgba8().ok_or("no pixels")?;
        Ok(Shot { width: self.size.0, height: self.size.1, rgba })
    }
}
