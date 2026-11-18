//! Fonts, shaping (cosmic-text) and the CPU glyph atlas.
//!
//! One [`FontSystem`] with only the bundled Inter faces and the fonts the game loads (system
//! fonts are opt-in), one [`SwashCache`] rasterising glyphs on the CPU, and an [`Atlas`]: an RGBA8
//! image (white with the glyph coverage as alpha; colour glyphs keep their colours) that the host
//! uploads as a texture of the engine's store. Glyphs are drawn as textured quads at integer
//! pixel positions with nearest sampling, tinted by the vertex colour.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{Align, Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, Weight, Wrap, fontdb};
use efeengine_core::render::{Draw2D, Image, Vertex2D};
use efeengine_core::prelude::TextureHandle;

use crate::style::{FontId, TextAlign};

/// The family name of the bundled default font (Inter, SIL OFL 1.1).
pub const DEFAULT_FAMILY: &str = "Inter";
/// iced's default relative line height, so both native backends break lines alike.
pub const LINE_HEIGHT: f32 = 1.3;

static INTER_REGULAR: &[u8] = include_bytes!("../../assets/Inter-Regular.ttf");
static INTER_BOLD: &[u8] = include_bytes!("../../assets/Inter-Bold.ttf");

/// What a text cell shows; a change re-shapes it.
#[derive(Clone, PartialEq, Debug)]
pub struct TextSpec {
    pub text: String,
    /// Logical px.
    pub size: f32,
    pub font: Option<FontId>,
    pub bold: bool,
    pub align: TextAlign,
    /// Wrap at the layout width (false: one line, never wraps).
    pub wrap: bool,
}

/// A shaped text: the cosmic-text buffer plus what it was shaped from.
pub struct TextCell {
    pub buffer: Buffer,
    spec: Option<TextSpec>,
    /// The width the buffer is currently laid out for (`None` = unbounded), as bits.
    width: Option<u32>,
    shaped: bool,
}

impl TextCell {
    pub fn new() -> Self {
        Self { buffer: Buffer::new_empty(Metrics::new(16.0, 16.0 * LINE_HEIGHT)), spec: None, width: None, shaped: false }
    }
}

impl Default for TextCell {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a glyph sits in the atlas.
#[derive(Clone, Copy, Debug)]
struct Entry {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    /// Pixel offset of the bitmap's left edge from the pen position, and of its top edge above
    /// the baseline.
    left: i32,
    top: i32,
    /// Colour glyph (emoji): drawn untinted.
    color: bool,
}

struct Shelf {
    y: u32,
    h: u32,
    x: u32,
}

/// Shelf-packed glyph atlas.
pub struct Atlas {
    size: u32,
    pixels: Vec<u8>,
    shelves: Vec<Shelf>,
    map: HashMap<CacheKey, Option<Entry>>,
    dirty: bool,
    /// A glyph did not fit: the atlas is cleared at the start of the next frame.
    full: bool,
    used: u64,
}

const ATLAS_SIZE: u32 = 1024;

impl Atlas {
    fn new() -> Self {
        Self {
            size: ATLAS_SIZE,
            pixels: vec![0; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
            shelves: Vec::new(),
            map: HashMap::new(),
            dirty: true,
            full: false,
            used: 0,
        }
    }

    fn clear(&mut self) {
        self.pixels.iter_mut().for_each(|p| *p = 0);
        self.shelves.clear();
        self.map.clear();
        self.dirty = true;
        self.full = false;
        self.used = 0;
    }

    /// Finds room for a `w` x `h` bitmap (plus one pixel of padding).
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (pw, ph) = (w + 1, h + 1);
        if pw > self.size || ph > self.size {
            return None;
        }
        for s in &mut self.shelves {
            if s.h >= ph && s.h <= ph + ph / 2 + 2 && s.x + pw <= self.size {
                let at = (s.x, s.y);
                s.x += pw;
                return Some(at);
            }
        }
        let y = self.shelves.last().map_or(0, |s| s.y + s.h);
        if y + ph > self.size {
            return None;
        }
        self.shelves.push(Shelf { y, h: ph, x: pw });
        Some((0, y))
    }

    /// Entries are used by about this share of the atlas pixels (for the clear-when-full policy).
    pub fn usage(&self) -> f32 {
        self.used as f32 / (self.size as f32 * self.size as f32)
    }

    /// The pixels as an image when something changed since the last call.
    pub fn take_image(&mut self) -> Option<Arc<Image>> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(Arc::new(Image::new(self.size, self.size, self.pixels.clone()).expect("atlas image")))
    }

    /// Number of glyph bitmaps in the atlas.
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Fonts, shaping and glyph drawing.
pub struct TextSystem {
    pub fs: FontSystem,
    swash: SwashCache,
    /// Family name per loaded font id.
    families: HashMap<FontId, Arc<str>>,
    pub atlas: Atlas,
}

impl TextSystem {
    /// The bundled Inter faces only; `system_fonts` also scans the installed fonts (slow start,
    /// but covers scripts Inter lacks).
    pub fn new(system_fonts: bool) -> Self {
        let sources = [INTER_REGULAR, INTER_BOLD].map(|b| fontdb::Source::Binary(Arc::new(b)));
        let mut fs = FontSystem::new_with_fonts(sources);
        if system_fonts {
            fs.db_mut().load_system_fonts();
        }
        Self { fs, swash: SwashCache::new(), families: HashMap::new(), atlas: Atlas::new() }
    }

    /// Registers font bytes under `id`; returns the family name, `None` when the bytes hold no face.
    pub fn load_font(&mut self, id: FontId, bytes: &Arc<[u8]>) -> Option<Arc<str>> {
        let before: Vec<_> = self.fs.db().faces().map(|f| f.id).collect();
        self.fs.db_mut().load_font_source(fontdb::Source::Binary(Arc::new(bytes.to_vec())));
        let family: Arc<str> = self.fs.db().faces().find(|f| !before.contains(&f.id)).and_then(|f| f.families.first().map(|(n, _)| n.as_str().into()))?;
        self.families.insert(id, family.clone());
        Some(family)
    }

    pub(super) fn attrs<'a>(&'a self, spec: &TextSpec) -> Attrs<'a> {
        let family = spec.font.and_then(|f| self.families.get(&f)).map_or(DEFAULT_FAMILY, |f| &**f);
        Attrs::new().family(Family::Name(family)).weight(if spec.bold { Weight::BOLD } else { Weight::NORMAL })
    }

    /// Makes `cell` show `spec` laid out for `width` (logical px; `None` = unbounded) and returns
    /// its size. Re-shapes only when the spec or the width changed.
    pub fn layout(&mut self, cell: &mut TextCell, spec: &TextSpec, width: Option<f32>) -> (f32, f32) {
        let respec = cell.spec.as_ref() != Some(spec);
        if respec {
            let m = Metrics::new(spec.size, spec.size * LINE_HEIGHT);
            cell.buffer.set_metrics(m);
            cell.buffer.set_wrap(if spec.wrap { Wrap::Word } else { Wrap::None });
            let attrs = self.attrs(spec);
            let align = match spec.align {
                TextAlign::Start => None,
                TextAlign::Center => Some(Align::Center),
                TextAlign::End => Some(Align::End),
            };
            cell.buffer.set_text(&spec.text, &attrs, Shaping::Advanced, align);
            cell.spec = Some(spec.clone());
            cell.shaped = false;
        }
        let w = if spec.wrap || spec.align != TextAlign::Start { width } else { None };
        let wbits = w.map(f32::to_bits);
        if !cell.shaped || cell.width != wbits {
            cell.buffer.set_size(w, None);
            cell.buffer.shape_until_scroll(&mut self.fs, false);
            cell.width = wbits;
            cell.shaped = true;
        }
        let line_h = spec.size * LINE_HEIGHT;
        let (mut tw, mut bottom) = (0.0f32, 0.0f32);
        for run in cell.buffer.layout_runs() {
            tw = tw.max(run.line_w);
            bottom = bottom.max(run.line_top + run.line_height);
        }
        (tw, bottom.max(line_h))
    }

    /// Draws a shaped cell with its top-left at `origin` (logical px), scaled by `scale` to
    /// physical pixels, tinted `color` (sRGB 0..1). Glyphs outside `clip` (physical px) are
    /// skipped. Needs `atlas_tex`, the texture that holds the atlas.
    pub fn draw(&mut self, buffer: &Buffer, origin: (f32, f32), scale: f32, color: [f32; 4], clip: Option<[f32; 4]>, atlas_tex: TextureHandle, out: &mut dyn Draw2D) {
        if color[3] <= 0.0 {
            return;
        }
        let (ox, oy) = ((origin.0 * scale).round(), (origin.1 * scale).round());
        let inv = 1.0 / self.atlas.size as f32;
        let mut verts: Vec<Vertex2D> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        let mut colored: Vec<(Vertex2D, Vertex2D, Vertex2D, Vertex2D)> = Vec::new();
        for run in buffer.layout_runs() {
            let by = oy + (run.line_y * scale).round();
            if let Some(c) = clip
                && (by + 4.0 * scale < c[1] || by - run.line_height * scale > c[3])
            {
                continue;
            }
            for g in run.glyphs {
                let p = g.physical((ox, by), scale);
                let Some(e) = self.entry(p.cache_key) else { continue };
                if e.w == 0 || e.h == 0 {
                    continue;
                }
                let (x0, y0) = ((p.x + e.left) as f32, (p.y - e.top) as f32);
                let (x1, y1) = (x0 + e.w as f32, y0 + e.h as f32);
                if let Some(c) = clip
                    && (x1 <= c[0] || x0 >= c[2] || y1 <= c[1] || y0 >= c[3])
                {
                    continue;
                }
                let (u0, v0, u1, v1) = (e.x as f32 * inv, e.y as f32 * inv, (e.x + e.w) as f32 * inv, (e.y + e.h) as f32 * inv);
                if e.color {
                    let tint = [1.0, 1.0, 1.0, color[3]];
                    colored.push((
                        Vertex2D::new([x0, y0], [u0, v0], tint),
                        Vertex2D::new([x1, y0], [u1, v0], tint),
                        Vertex2D::new([x1, y1], [u1, v1], tint),
                        Vertex2D::new([x0, y1], [u0, v1], tint),
                    ));
                } else {
                    let base = verts.len() as u32;
                    verts.extend([
                        Vertex2D::new([x0, y0], [u0, v0], color),
                        Vertex2D::new([x1, y0], [u1, v0], color),
                        Vertex2D::new([x1, y1], [u1, v1], color),
                        Vertex2D::new([x0, y1], [u0, v1], color),
                    ]);
                    idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
        }
        for (a, b, c, d) in colored {
            let base = verts.len() as u32;
            verts.extend([a, b, c, d]);
            idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        if !idx.is_empty() {
            out.triangles(Some(atlas_tex), &verts, &idx);
        }
    }

    /// The atlas entry of a glyph, rasterising it on first use.
    fn entry(&mut self, key: CacheKey) -> Option<Entry> {
        if let Some(e) = self.atlas.map.get(&key) {
            return *e;
        }
        let img = self.swash.get_image(&mut self.fs, key).as_ref();
        let entry = match img {
            None => None,
            Some(img) => {
                let (w, h) = (img.placement.width, img.placement.height);
                if w == 0 || h == 0 {
                    Some(Entry { x: 0, y: 0, w: 0, h: 0, left: 0, top: 0, color: false })
                } else {
                    match self.atlas.alloc(w, h) {
                        None => {
                            // No room: skip the glyph (not cached) and clear the atlas next frame.
                            self.atlas.full = true;
                            return None;
                        }
                        Some((x, y)) => {
                            let size = self.atlas.size as usize;
                            let color = matches!(img.content, SwashContent::Color);
                            for row in 0..h as usize {
                                for col in 0..w as usize {
                                    let dst = ((y as usize + row) * size + x as usize + col) * 4;
                                    let px: [u8; 4] = match img.content {
                                        SwashContent::Mask => [255, 255, 255, img.data[row * w as usize + col]],
                                        SwashContent::SubpixelMask => {
                                            let s = (row * w as usize + col) * 4;
                                            let a = ((img.data[s] as u32 + img.data[s + 1] as u32 + img.data[s + 2] as u32) / 3) as u8;
                                            [255, 255, 255, a]
                                        }
                                        SwashContent::Color => {
                                            let s = (row * w as usize + col) * 4;
                                            [img.data[s], img.data[s + 1], img.data[s + 2], img.data[s + 3]]
                                        }
                                    };
                                    self.atlas.pixels[dst..dst + 4].copy_from_slice(&px);
                                }
                            }
                            self.atlas.dirty = true;
                            self.atlas.used += (w as u64 + 1) * (h as u64 + 1);
                            Some(Entry { x, y, w, h, left: img.placement.left, top: img.placement.top, color })
                        }
                    }
                }
            }
        };
        self.atlas.map.insert(key, entry);
        entry
    }

    /// Frame start: clears the atlas when a glyph did not fit last frame, or it is nearly full.
    /// Returns whether it cleared (every recorded frame is stale then).
    pub fn begin_frame(&mut self) -> bool {
        if self.atlas.full || self.atlas.usage() > 0.9 {
            self.atlas.clear();
            return true;
        }
        false
    }

    /// A glyph did not fit into the atlas since the last clear.
    pub fn atlas_full(&self) -> bool {
        self.atlas.full
    }

    /// Whether the atlas holds this glyph, for tests.
    pub fn cached_glyphs(&self) -> usize {
        self.atlas.len()
    }
}
