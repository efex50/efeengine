//! Anti-aliased rounded rectangles and rings as triangle meshes (no shader, no texture).
//!
//! All coordinates are physical pixels. A shape is a convex polygon given by its outline points
//! and outward normals; the edge is feathered by a one pixel wide ring whose outer vertices have
//! alpha 0, so pixels on the edge get their coverage under `Blend::Alpha`. Rectangles without
//! rounding are pixel-snapped and drawn without feathering (crisp edges).

use efeengine_core::render::{Draw2D, Rect, Vertex2D};

/// Radii below this draw as plain rectangles.
const MIN_RADIUS: f32 = 1.0;

pub struct Mesh {
    pub verts: Vec<Vertex2D>,
    pub idx: Vec<u32>,
    /// Feather the edges (anti-aliasing). Off: whole-pixel coordinates and hard edges, for
    /// rasterisers that work on integer vertices (SDL's software renderer).
    pub feather: bool,
}

impl Default for Mesh {
    fn default() -> Self {
        Self { verts: Vec::new(), idx: Vec::new(), feather: true }
    }
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.verts.clear();
        self.idx.clear();
    }

    pub fn flush(&mut self, out: &mut dyn Draw2D) {
        if !self.idx.is_empty() {
            out.triangles(None, &self.verts, &self.idx);
        }
        self.clear();
    }

    fn v(&mut self, p: [f32; 2], c: [f32; 4]) -> u32 {
        self.verts.push(Vertex2D::new(p, [0.0, 0.0], c));
        (self.verts.len() - 1) as u32
    }

    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.idx.extend([a, b, c]);
    }

    /// An axis-aligned rectangle, edges snapped to whole pixels.
    pub fn rect(&mut self, r: Rect, c: [f32; 4]) {
        let (x0, y0, x1, y1) = (r.x.round(), r.y.round(), (r.x + r.w).round(), (r.y + r.h).round());
        if x1 <= x0 || y1 <= y0 || c[3] <= 0.0 {
            return;
        }
        let a = self.v([x0, y0], c);
        let b = self.v([x1, y0], c);
        let d = self.v([x1, y1], c);
        let e = self.v([x0, y1], c);
        self.tri(a, b, d);
        self.tri(a, d, e);
    }

    /// A filled rounded rectangle, feathered edge.
    pub fn fill(&mut self, r: Rect, radius: f32, c: [f32; 4]) {
        if r.w <= 0.0 || r.h <= 0.0 || c[3] <= 0.0 {
            return;
        }
        let radius = radius.min(r.w * 0.5).min(r.h * 0.5);
        if radius < MIN_RADIUS {
            return self.rect(r, c);
        }
        let ring = outline(r, radius, segments(radius));
        if !self.feather {
            let centre = self.v([(r.x + r.w * 0.5).round(), (r.y + r.h * 0.5).round()], c);
            let pts: Vec<u32> = ring.iter().map(|p| self.v([p.0.round(), p.1.round()], c)).collect();
            for i in 0..pts.len() {
                self.tri(centre, pts[i], pts[(i + 1) % pts.len()]);
            }
            return;
        }
        let clear = [c[0], c[1], c[2], 0.0];
        let centre = self.v([r.x + r.w * 0.5, r.y + r.h * 0.5], c);
        let core: Vec<u32> = ring.iter().map(|p| self.v([p.0 - p.2 * 0.5, p.1 - p.3 * 0.5], c)).collect();
        let fringe: Vec<u32> = ring.iter().map(|p| self.v([p.0 + p.2 * 0.5, p.1 + p.3 * 0.5], clear)).collect();
        let n = ring.len();
        for i in 0..n {
            let j = (i + 1) % n;
            self.tri(centre, core[i], core[j]);
            self.tri(core[i], fringe[i], fringe[j]);
            self.tri(core[i], fringe[j], core[j]);
        }
    }

    /// The border ring of a rounded rectangle: `width` pixels inside `r`.
    pub fn stroke(&mut self, r: Rect, radius: f32, width: f32, c: [f32; 4]) {
        if r.w <= 0.0 || r.h <= 0.0 || width <= 0.0 || c[3] <= 0.0 {
            return;
        }
        let width = width.min(r.w * 0.5).min(r.h * 0.5);
        let radius = radius.min(r.w * 0.5).min(r.h * 0.5);
        if radius < MIN_RADIUS {
            let w = width.round().max(1.0);
            let c = if width < 1.0 { [c[0], c[1], c[2], c[3] * width] } else { c };
            self.rect(Rect::new(r.x, r.y, r.w, w), c);
            self.rect(Rect::new(r.x, r.y + r.h - w, r.w, w), c);
            self.rect(Rect::new(r.x, r.y + w, w, r.h - 2.0 * w), c);
            self.rect(Rect::new(r.x + r.w - w, r.y + w, w, r.h - 2.0 * w), c);
            return;
        }
        if !self.feather {
            let w = width.round().max(1.0);
            let inner_r = Rect::new(r.x + w, r.y + w, r.w - 2.0 * w, r.h - 2.0 * w);
            let segs = segments(radius);
            let outer = outline(r, radius, segs);
            let inner = outline(inner_r, (radius - w).max(0.0), segs);
            let rows: Vec<[u32; 2]> = outer
                .iter()
                .zip(&inner)
                .map(|(o, i)| [self.v([o.0.round(), o.1.round()], c), self.v([i.0.round(), i.1.round()], c)])
                .collect();
            for k in 0..rows.len() {
                let (a, b) = (rows[k], rows[(k + 1) % rows.len()]);
                self.tri(a[0], a[1], b[0]);
                self.tri(a[1], b[1], b[0]);
            }
            return;
        }
        // Thin borders keep one pixel of width and fade instead.
        let (width, c) = if width < 1.0 { (1.0, [c[0], c[1], c[2], c[3] * width]) } else { (width, c) };
        let clear = [c[0], c[1], c[2], 0.0];
        let segs = segments(radius);
        let outer = outline(r, radius, segs);
        let inner_r = Rect::new(r.x + width, r.y + width, r.w - 2.0 * width, r.h - 2.0 * width);
        let inner = outline(inner_r, (radius - width).max(0.0), segs);
        let n = outer.len();
        let mut rows: Vec<[u32; 4]> = Vec::with_capacity(n);
        for (o, i) in outer.iter().zip(&inner) {
            rows.push([
                self.v([o.0 + o.2 * 0.5, o.1 + o.3 * 0.5], clear),
                self.v([o.0 - o.2 * 0.5, o.1 - o.3 * 0.5], c),
                self.v([i.0 + i.2 * 0.5, i.1 + i.3 * 0.5], c),
                self.v([i.0 - i.2 * 0.5, i.1 - i.3 * 0.5], clear),
            ]);
        }
        for k in 0..n {
            let (a, b) = (rows[k], rows[(k + 1) % n]);
            for s in 0..3 {
                self.tri(a[s], a[s + 1], b[s]);
                self.tri(a[s + 1], b[s + 1], b[s]);
            }
        }
    }
}

/// Segments per corner for a radius.
fn segments(radius: f32) -> usize {
    ((radius / 3.0).ceil() as usize + 2).clamp(3, 16)
}

/// Outline points `(x, y, nx, ny)` of a rounded rectangle, clockwise on screen (y down), starting
/// at the top-left corner's start; the normals point outwards.
fn outline(r: Rect, radius: f32, segs: usize) -> Vec<(f32, f32, f32, f32)> {
    let corners = [
        (r.x + r.w - radius, r.y + radius, -90.0f32),
        (r.x + r.w - radius, r.y + r.h - radius, 0.0),
        (r.x + radius, r.y + r.h - radius, 90.0),
        (r.x + radius, r.y + radius, 180.0),
    ];
    let mut pts = Vec::with_capacity(4 * (segs + 1));
    for (cx, cy, start) in corners {
        for s in 0..=segs {
            let a = (start + 90.0 * s as f32 / segs as f32).to_radians();
            let (nx, ny) = (a.cos(), a.sin());
            pts.push((cx + nx * radius, cy + ny * radius, nx, ny));
        }
    }
    pts
}

/// A pixel-snapped checkmark of two strokes, `size` pixels wide, drawn inside `r`.
pub fn check_mark(out: &mut dyn Draw2D, r: Rect, color: [f32; 4]) {
    let s = r.w.min(r.h);
    let (cx, cy) = (r.x + r.w * 0.5, r.y + r.h * 0.5);
    let w = (s * 0.14).max(1.5);
    let a = [cx - s * 0.26, cy + s * 0.02];
    let b = [cx - s * 0.06, cy + s * 0.22];
    let c = [cx + s * 0.28, cy - s * 0.2];
    out.polyline(&[a, b, c], w, color, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_rect_is_snapped_and_has_no_fringe() {
        let mut m = Mesh::new();
        m.rect(Rect::new(0.4, 0.6, 10.0, 5.0), [1.0; 4]);
        assert_eq!(m.verts.len(), 4);
        assert_eq!(m.verts[0].pos, [0.0, 1.0]);
        assert_eq!(m.verts[2].pos, [10.0, 6.0]);
    }

    #[test]
    fn rounded_fill_has_a_transparent_fringe_ring() {
        let mut m = Mesh::new();
        m.fill(Rect::new(0.0, 0.0, 40.0, 20.0), 6.0, [1.0, 0.0, 0.0, 1.0]);
        assert!(m.verts.iter().any(|v| v.color[3] == 0.0));
        assert!(m.verts.iter().any(|v| v.color[3] == 1.0));
        // every vertex stays within half a pixel of the rectangle
        for v in &m.verts {
            assert!(v.pos[0] >= -0.51 && v.pos[0] <= 40.51 && v.pos[1] >= -0.51 && v.pos[1] <= 20.51, "{:?}", v.pos);
        }
        assert!(m.idx.iter().all(|&i| (i as usize) < m.verts.len()));
        assert_eq!(m.idx.len() % 3, 0);
    }

    #[test]
    fn small_radius_falls_back_to_a_rect() {
        let mut m = Mesh::new();
        m.fill(Rect::new(0.0, 0.0, 10.0, 10.0), 0.5, [1.0; 4]);
        assert_eq!(m.verts.len(), 4);
    }

    #[test]
    fn stroke_is_a_ring() {
        let mut m = Mesh::new();
        m.stroke(Rect::new(0.0, 0.0, 40.0, 20.0), 6.0, 2.0, [1.0; 4]);
        assert!(m.idx.len() > 0 && m.idx.len() % 3 == 0);
        let mut m = Mesh::new();
        m.stroke(Rect::new(0.0, 0.0, 40.0, 20.0), 0.0, 2.0, [1.0; 4]);
        assert_eq!(m.verts.len(), 16);
    }
}
