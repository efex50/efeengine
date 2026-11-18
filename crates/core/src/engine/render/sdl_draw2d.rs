//! The SDL side of rendering (native only): [`SdlDraw2D`] implements [`Draw2D`] on an `SDL_Renderer`,
//! and [`SdlSpriteRenderer`] draws a [`Frame2D`] with it. This is "SDL mode" (ROADMAP 4C.4): no
//! wgpu context, for machines without a usable wgpu adapter.
//!
//! * [`SdlDraw2D`] owns the renderer, a texture table filled from [`TextureChanges`]
//!   ([`SdlDraw2D::sync`]) and implements [`Draw2D`] immediately: each `triangles` call is one
//!   `SDL_RenderGeometry` (one texture per call), the state calls set the clip rectangle, the
//!   blend mode and the texture scale mode.
//! * Colour space: SDL does no sRGB conversion and blends the encoded values. Vertex colours of
//!   [`Draw2D`] are sRGB-encoded already and are passed as is. Sprite tints and the clear colour are
//!   linear in the engine (the wgpu path converts on the way out), so [`SdlSpriteRenderer`] encodes
//!   them before use: an opaque sprite then matches the wgpu output. Blending of translucent
//!   pixels happens on encoded values here (linear in wgpu), so those edges differ a little.
//! * Targets: [`SdlDraw2D::for_window`] (a window's renderer, presented with vsync) or
//!   [`SdlDraw2D::offscreen`] (software renderer on a surface, works headless; used by tests).
//! * Unsafe code is confined to this file; every raw pointer is owned by `SdlDraw2D` and
//!   destroyed in `Drop` (textures first, then the renderer, then the surface).

use std::collections::HashMap;
use std::ffi::{c_int, c_void, CStr};
use std::ptr::{null, null_mut};

use sdl3::sys::blendmode::{SDL_BlendMode, SDL_BLENDMODE_ADD, SDL_BLENDMODE_BLEND, SDL_BLENDMODE_BLEND_PREMULTIPLIED, SDL_BLENDMODE_NONE};
use sdl3::sys::pixels::{SDL_FColor, SDL_PIXELFORMAT_RGBA32};
use sdl3::sys::rect::{SDL_FPoint, SDL_Rect};
use sdl3::sys::render::*;
use sdl3::sys::surface::{SDL_CreateSurface, SDL_DestroySurface, SDL_Surface, SDL_SCALEMODE_LINEAR, SDL_SCALEMODE_NEAREST};

use crate::engine::components::TextureHandle;
use super::draw2d::{Blend, Draw2D, Filter, RectI, Vertex2D};
use super::frame::{Frame2D, SpriteInstance};
use super::textures::TextureChanges;

fn sdl_error() -> String {
    // SAFETY: SDL_GetError returns a valid NUL-terminated string (possibly empty).
    unsafe { CStr::from_ptr(sdl3::sys::error::SDL_GetError()).to_string_lossy().into_owned() }
}

/// sRGB encode of one linear channel (0..1).
pub(crate) fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Linear RGBA (the engine's tint / clear colour space) to sRGB-encoded RGBA (alpha unchanged).
pub(crate) fn encode_color(c: [f32; 4]) -> [f32; 4] {
    [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2]), c[3]]
}

struct SdlTexture {
    ptr: *mut SDL_Texture,
    size: (u32, u32),
}

fn blend_mode(b: Blend) -> SDL_BlendMode {
    match b {
        Blend::None => SDL_BLENDMODE_NONE,
        Blend::Alpha => SDL_BLENDMODE_BLEND,
        Blend::Add => SDL_BLENDMODE_ADD,
        Blend::Premultiplied => SDL_BLENDMODE_BLEND_PREMULTIPLIED,
    }
}

/// [`Draw2D`] on an `SDL_Renderer`, plus its texture table. See the module docs.
pub struct SdlDraw2D {
    renderer: *mut SDL_Renderer,
    /// Owned surface of an offscreen (software) renderer, else null.
    surface: *mut SDL_Surface,
    textures: HashMap<u32, SdlTexture>,
    blend: Blend,
    filter: Filter,
    clip: Option<RectI>,
    /// The clip rectangle is empty after clamping: skip draws.
    clip_empty: bool,
    /// Scratch buffers for the vertex / index conversion.
    verts: Vec<SDL_Vertex>,
    indices: Vec<c_int>,
    name: String,
}

impl SdlDraw2D {
    /// The renderer of a window (`window` is the `SDL_Window*` of the engine's main window) with vsync
    /// on. The `SDL_RENDER_DRIVER` environment variable / hint picks the driver.
    ///
    /// # Safety
    /// `window` must be a valid SDL window that outlives the returned value.
    pub unsafe fn for_window(window: *mut sdl3::sys::video::SDL_Window) -> Result<Self, String> {
        if window.is_null() {
            return Err("this window has no SDL window (headless)".to_string());
        }
        // SAFETY: the caller guarantees `window` is valid.
        let renderer = unsafe { SDL_CreateRenderer(window, null()) };
        if renderer.is_null() {
            return Err(format!("SDL_CreateRenderer failed: {}", sdl_error()));
        }
        // SAFETY: `renderer` is valid.
        if !unsafe { SDL_SetRenderVSync(renderer, 1) } {
            crate::global_warn(&format!("SDL renderer: vsync not available ({})", sdl_error()));
        }
        Ok(Self::from_renderer(renderer, null_mut()))
    }

    /// A software renderer on a `size` RGBA32 surface: no window, no video driver needed.
    pub fn offscreen(size: (u32, u32)) -> Result<Self, String> {
        // SAFETY: plain creation calls; the surface is owned by the result and freed in `Drop`.
        unsafe {
            let surface = SDL_CreateSurface(size.0 as c_int, size.1 as c_int, SDL_PIXELFORMAT_RGBA32);
            if surface.is_null() {
                return Err(format!("SDL_CreateSurface failed: {}", sdl_error()));
            }
            let renderer = SDL_CreateSoftwareRenderer(surface);
            if renderer.is_null() {
                let e = format!("SDL_CreateSoftwareRenderer failed: {}", sdl_error());
                SDL_DestroySurface(surface);
                return Err(e);
            }
            Ok(Self::from_renderer(renderer, surface))
        }
    }

    fn from_renderer(renderer: *mut SDL_Renderer, surface: *mut SDL_Surface) -> Self {
        // SAFETY: `renderer` is valid; the name is a static string owned by SDL.
        let name = unsafe {
            let p = SDL_GetRendererName(renderer);
            if p.is_null() { "unknown".to_string() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        Self {
            renderer,
            surface,
            textures: HashMap::new(),
            blend: Blend::Alpha,
            filter: Filter::Nearest,
            clip: None,
            clip_empty: false,
            verts: Vec::new(),
            indices: Vec::new(),
            name,
        }
    }

    /// The SDL renderer driver in use (`"opengl"`, `"vulkan"`, `"software"`, ...).
    pub fn renderer_name(&self) -> &str {
        &self.name
    }

    /// The output size in pixels (0, 0 when it cannot be queried).
    pub fn output_size(&self) -> (u32, u32) {
        let (mut w, mut h) = (0 as c_int, 0 as c_int);
        // SAFETY: valid renderer and out pointers.
        if unsafe { SDL_GetCurrentRenderOutputSize(self.renderer, &mut w, &mut h) } {
            (w.max(0) as u32, h.max(0) as u32)
        } else {
            (0, 0)
        }
    }

    /// Mirror one frame's texture changes into the SDL texture table. (They are delivered once,
    /// so call this every frame.)
    pub fn sync(&mut self, changes: &TextureChanges) {
        for (h, img) in changes.added.iter().chain(&changes.updated) {
            self.remove_texture(h.id);
            let (w, hh) = (img.width(), img.height());
            // SAFETY: valid renderer; the pixel slice is `w * h * 4` bytes (Image invariant).
            let tex = unsafe {
                let t = SDL_CreateTexture(self.renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_STATIC, w as c_int, hh as c_int);
                if t.is_null() {
                    crate::global_warn(&format!("SDL texture {} ({w}x{hh}) could not be created: {}", h.id, sdl_error()));
                    continue;
                }
                if !SDL_UpdateTexture(t, null(), img.rgba8().as_ptr() as *const c_void, (w * 4) as c_int) {
                    crate::global_warn(&format!("SDL texture {} could not be uploaded: {}", h.id, sdl_error()));
                    SDL_DestroyTexture(t);
                    continue;
                }
                t
            };
            self.textures.insert(h.id, SdlTexture { ptr: tex, size: (w, hh) });
        }
        for h in &changes.removed {
            self.remove_texture(h.id);
        }
    }

    fn remove_texture(&mut self, id: u32) {
        if let Some(t) = self.textures.remove(&id) {
            // SAFETY: the pointer came from SDL_CreateTexture and is removed from the table.
            unsafe { SDL_DestroyTexture(t.ptr) };
        }
    }

    pub fn contains(&self, id: u32) -> bool {
        self.textures.contains_key(&id)
    }

    /// Size of the texture `id`, if it exists.
    pub fn texture_size(&self, id: u32) -> Option<(u32, u32)> {
        self.textures.get(&id).map(|t| t.size)
    }

    /// Reset the per-frame state (no clip, alpha blend, nearest) and clear the whole target to
    /// `color` (sRGB-encoded RGBA).
    pub fn begin_frame(&mut self, color: [f32; 4]) {
        self.blend = Blend::Alpha;
        self.filter = Filter::Nearest;
        self.set_clip(None);
        // SAFETY: valid renderer.
        unsafe {
            SDL_SetRenderDrawBlendMode(self.renderer, SDL_BLENDMODE_NONE);
            SDL_SetRenderDrawColorFloat(self.renderer, color[0], color[1], color[2], 1.0);
            SDL_RenderClear(self.renderer);
        }
    }

    /// Present a window renderer (no-op visually for an offscreen one).
    pub fn present(&mut self) {
        // SAFETY: valid renderer.
        if !unsafe { SDL_RenderPresent(self.renderer) } {
            crate::global_warn(&format!("SDL_RenderPresent failed: {}", sdl_error()));
        }
    }

    /// The offscreen target's pixels as RGBA8 (sRGB-encoded bytes as drawn), rows from the top, no
    /// padding. `None` for a window renderer.
    pub fn read_rgba8(&mut self) -> Option<Vec<u8>> {
        if self.surface.is_null() {
            return None;
        }
        // SAFETY: the surface is owned and valid; RGBA32 has 4 bytes per pixel; the renderer is
        // flushed so all queued draws have landed in the surface.
        unsafe {
            SDL_FlushRenderer(self.renderer);
            let s = &*self.surface;
            let (w, h, pitch) = (s.w as usize, s.h as usize, s.pitch as usize);
            let base = s.pixels as *const u8;
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                out.extend_from_slice(std::slice::from_raw_parts(base.add(y * pitch), w * 4));
            }
            Some(out)
        }
    }

    fn apply_clip(&mut self) {
        let (w, h) = self.output_size();
        self.clip_empty = false;
        let rect = self.clip.map(|c| {
            // clamp to the target, like the wgpu scissor
            let x0 = c.x.clamp(0, w as i32);
            let y0 = c.y.clamp(0, h as i32);
            let x1 = (c.x.saturating_add(c.w)).clamp(0, w as i32);
            let y1 = (c.y.saturating_add(c.h)).clamp(0, h as i32);
            SDL_Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
        });
        match rect {
            Some(r) if r.w <= 0 || r.h <= 0 => {
                self.clip_empty = true;
            }
            Some(r) => {
                // SAFETY: valid renderer and rect.
                unsafe { SDL_SetRenderClipRect(self.renderer, &r) };
            }
            None => {
                // SAFETY: NULL disables clipping.
                unsafe { SDL_SetRenderClipRect(self.renderer, null()) };
            }
        }
    }
}

impl Draw2D for SdlDraw2D {
    fn set_clip(&mut self, rect: Option<RectI>) {
        self.clip = rect;
        self.apply_clip();
    }

    fn set_blend(&mut self, blend: Blend) {
        self.blend = blend;
    }

    fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
    }

    fn triangles(&mut self, texture: Option<TextureHandle>, verts: &[Vertex2D], indices: &[u32]) {
        if self.clip_empty || indices.is_empty() || indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= verts.len()) {
            return;
        }
        let mode = blend_mode(self.blend);
        // Unknown handles draw as white (untextured), like the wgpu backend.
        let tex = texture.and_then(|t| self.textures.get(&t.id)).map_or(null_mut(), |t| t.ptr);
        self.verts.clear();
        self.verts.extend(verts.iter().map(|v| SDL_Vertex {
            position: SDL_FPoint { x: v.pos[0], y: v.pos[1] },
            color: SDL_FColor { r: v.color[0], g: v.color[1], b: v.color[2], a: v.color[3] },
            tex_coord: SDL_FPoint { x: v.uv[0], y: v.uv[1] },
        }));
        self.indices.clear();
        self.indices.extend(indices.iter().map(|&i| i as c_int));
        // SAFETY: valid renderer / texture; the buffers hold the counts passed.
        unsafe {
            if tex.is_null() {
                SDL_SetRenderDrawBlendMode(self.renderer, mode);
            } else {
                SDL_SetTextureBlendMode(tex, mode);
                SDL_SetTextureScaleMode(tex, if self.filter == Filter::Linear { SDL_SCALEMODE_LINEAR } else { SDL_SCALEMODE_NEAREST });
            }
            SDL_RenderGeometry(self.renderer, tex, self.verts.as_ptr(), self.verts.len() as c_int, self.indices.as_ptr(), self.indices.len() as c_int);
        }
    }
}

impl Drop for SdlDraw2D {
    fn drop(&mut self) {
        // SAFETY: every pointer is owned by `self`; textures go before their renderer, the
        // renderer before its surface.
        unsafe {
            for (_, t) in self.textures.drain() {
                SDL_DestroyTexture(t.ptr);
            }
            SDL_DestroyRenderer(self.renderer);
            if !self.surface.is_null() {
                SDL_DestroySurface(self.surface);
            }
        }
    }
}

/// Draws [`Frame2D`] with a [`SdlDraw2D`]: the SDL mode's built-in 2D renderer. Sprites become
/// textured quads (four vertices through `triangles`), converted on the CPU with the camera
/// matrix exactly as `sprite.wgsl` does, then `frame.draw` (the debug draw) is replayed.
#[derive(Debug, Default, Clone, Copy)]
pub struct SdlSpriteRenderer {
    /// Bilinear sampling instead of nearest.
    pub linear: bool,
}

impl SdlSpriteRenderer {
    pub fn new() -> Self {
        Self { linear: false }
    }

    /// Draw one frame (clear, sprites, debug draw). Does not present; textures must be synced.
    pub fn render(&self, d: &mut SdlDraw2D, frame: &Frame2D) {
        let clear = frame.camera.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |c| c.clear_color);
        d.begin_frame(encode_color(clear));
        let (w, h) = d.output_size();
        if let (Some(cam), true) = (&frame.camera, w > 0 && h > 0) {
            d.set_filter(if self.linear { Filter::Linear } else { Filter::Nearest });
            let vp = cam.view_projection;
            let mut quad = DrawList4::default();
            for s in &frame.sprites {
                quad.fill(s, &vp, (w as f32, h as f32), |id| d.contains(id));
                d.triangles(quad.texture, &quad.verts, &[0, 1, 2, 0, 2, 3]);
            }
        }
        d.set_filter(Filter::Nearest);
        frame.draw.replay(d);
    }
}

/// What SDL mode cannot do: logged once each, never a panic.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SdlWarned {
    custom: bool,
    renderer_2d: bool,
    camera_3d: bool,
    overlays: bool,
}

/// The engine's SDL mode: the window's renderer and the built-in 2D renderer.
pub(crate) struct SdlMode {
    pub draw: SdlDraw2D,
    pub sprites: SdlSpriteRenderer,
    warned: SdlWarned,
}

impl SdlMode {
    pub fn new(draw: SdlDraw2D) -> Self {
        Self { draw, sprites: SdlSpriteRenderer::new(), warned: SdlWarned::default() }
    }

    /// Draw and present one frame. `custom`, `renderer_2d` (a user-installed wgpu renderer) and
    /// `overlays` say what the game installed that this mode cannot run (warned once each).
    pub fn render(&mut self, frame: &super::RenderFrame, custom: bool, renderer_2d: bool, overlays: bool, overlays_2d: &mut super::Overlays2D, scale: f32) {
        let w = &mut self.warned;
        let once = |flag: &mut bool, on: bool, msg: &str| {
            if on && !*flag {
                *flag = true;
                crate::global_warn(msg);
            }
        };
        once(&mut w.custom, custom, "SDL mode: the CustomRenderer (wgpu) is not used; drawing with the built-in SDL 2D renderer (3D and custom shaders need wgpu)");
        once(&mut w.renderer_2d, renderer_2d && !custom, "SDL mode: the installed Renderer2D (wgpu) is not used; drawing with the built-in SDL 2D renderer");
        once(&mut w.camera_3d, frame.d3.camera.is_some(), "SDL mode: a 3D camera is active, but 3D needs wgpu; 3D cameras are ignored");
        once(&mut w.overlays, overlays, "SDL mode: render overlays (the iced UI) need wgpu and are not drawn; use the fallback UI (UiPlugin::fallback, feature `fallback`), which draws through Draw2D");
        self.draw.sync(&frame.textures);
        self.sprites.render(&mut self.draw, &frame.d2);
        overlays_2d.render_sdl(&mut self.draw, scale);
        self.draw.present();
    }
}

/// The four vertices of one sprite quad, reused.
#[derive(Default)]
struct DrawList4 {
    verts: [Vertex2D; 4],
    texture: Option<TextureHandle>,
}

impl DrawList4 {
    /// Corners in the order top-left, top-right, bottom-right, bottom-left (the shader's
    /// `CORNERS`: x right, y down in image space; local y is flipped, world is y up).
    fn fill(&mut self, s: &SpriteInstance, vp: &[[f32; 4]; 4], viewport: (f32, f32), has_texture: impl Fn(u32) -> bool) {
        let tint = encode_color(s.color);
        const CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (v, c) in self.verts.iter_mut().zip(CORNERS) {
            let local = [(c[0] - 0.5) * s.size[0], (0.5 - c[1]) * s.size[1], 0.0, 1.0];
            // vp * (world * local), in the shader's order, so shared edges of neighbouring sprites
            // come out bit-identical (a product matrix would leave hairline gaps).
            let world = mul_vec(&s.world, local);
            let clip = mul_vec(vp, world);
            let inv_w = if clip[3].abs() > f32::EPSILON { 1.0 / clip[3] } else { 1.0 };
            let (ndc_x, ndc_y) = (clip[0] * inv_w, clip[1] * inv_w);
            let mut t = c;
            if s.flip & 1 != 0 { t[0] = 1.0 - t[0]; }
            if s.flip & 2 != 0 { t[1] = 1.0 - t[1]; }
            *v = Vertex2D {
                pos: [snap((ndc_x * 0.5 + 0.5) * viewport.0), snap((0.5 - ndc_y * 0.5) * viewport.1)],
                uv: [s.uv_min[0] + (s.uv_max[0] - s.uv_min[0]) * t[0], s.uv_min[1] + (s.uv_max[1] - s.uv_min[1]) * t[1]],
                color: tint,
            };
        }
        self.texture = (s.texture.id != 0 && has_texture(s.texture.id)).then_some(s.texture);
    }
}

/// Round to 1/256 pixel. Float noise puts a shared tile edge a hair beside a pixel boundary, and the
/// SDL rasteriser then leaves one-pixel seams between neighbouring tiles; snapping makes edges that
/// should coincide identical (and exact integers stay exact).
fn snap(v: f32) -> f32 {
    (v * 256.0).round() / 256.0
}

/// `m * v` for a column-major 4x4 matrix (`m[column][row]`).
fn mul_vec(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2] + m[3][r] * v[3];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_encoding_roundtrip_points() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
        assert!((linear_to_srgb(0.214_041_14) - 0.5).abs() < 1e-4);
        assert_eq!(encode_color([0.0, 1.0, 0.5, 0.25])[3], 0.25);
    }

    #[test]
    fn matrix_vector_product() {
        let t = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [2.0, 3.0, 0.0, 1.0]];
        assert_eq!(mul_vec(&t, [1.0, 1.0, 0.0, 1.0]), [3.0, 4.0, 0.0, 1.0]);
    }

    /// The software renderer works without a window or a video driver.
    #[test]
    fn offscreen_draws_a_rect() {
        let mut d = match SdlDraw2D::offscreen((8, 8)) {
            Ok(d) => d,
            Err(e) => panic!("offscreen SDL renderer: {e}"),
        };
        assert_eq!(d.output_size(), (8, 8));
        d.begin_frame([0.0, 0.0, 1.0, 1.0]);
        d.fill_rect(super::super::Rect::new(0.0, 0.0, 4.0, 8.0), [1.0, 0.0, 0.0, 1.0]);
        let px = d.read_rgba8().expect("pixels");
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[(7 * 4)..(7 * 4 + 4)], &[0, 0, 255, 255]);
    }
}
