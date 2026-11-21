//! 2D debug draw (ROADMAP 4A.17): lines, rectangles and circles in world or screen space, drawn
//! over the sprite pass.
//!
//! [`DebugDraw2D`] is a resource (always present). Systems push shapes during the frame
//! (`cx.resources.get_mut::<DebugDraw2D>()`); `Stage::RenderExtract` converts them to pixels with
//! the active 2D camera into [`Frame2D::draw`](super::Frame2D) (a [`DrawList`]) and empties the
//! resource, so shapes live for exactly one frame (immediate mode: push again next frame). The 2D
//! renderer draws that list through [`Draw2D`](super::Draw2D), so it works on every 2D backend.
//!
//! * World shapes use world units (y up) and need an active 2D camera; without one they are
//!   dropped. Screen shapes are physical pixels, origin top-left, y down.
//! * Rectangles and circles are outlines, `*_filled` variants are solid. The line width (default 1
//!   pixel) is set with [`DebugDraw2D::set_width`] and applies to the shapes pushed after it. It is
//!   always in screen pixels, whatever the zoom. Colours are straight RGBA, sRGB-encoded 0..1.
//! * Shapes are drawn in the order they were pushed, over everything the sprite pass drew.
//! * Shapes go to the main window unless [`DebugDraw2D::set_window`] named another one (then they
//!   use that window's 2D camera and size).

use glam::Vec2;

use super::draw2d::{Draw2D, DrawList, Rect};
use super::frame::CameraView;

/// Segments of a circle outline.
const CIRCLE_SEGMENTS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Line(Vec2, Vec2),
    /// Rectangle outline, min and max corners.
    Rect(Vec2, Vec2),
    FillRect(Vec2, Vec2),
    Circle(Vec2, f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    kind: Kind,
    world: bool,
    color: [f32; 4],
    width: f32,
    /// The window it is drawn in (0 = the main window).
    window: u32,
}

/// The debug draw queue (a resource). See the module docs.
#[derive(Clone, Debug)]
pub struct DebugDraw2D {
    shapes: Vec<Shape>,
    width: f32,
    window: u32,
}

impl Default for DebugDraw2D {
    fn default() -> Self {
        Self { shapes: Vec::new(), width: 1.0, window: 0 }
    }
}

impl DebugDraw2D {
    pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    pub const RED: [f32; 4] = [1.0, 0.1, 0.1, 1.0];
    pub const GREEN: [f32; 4] = [0.1, 1.0, 0.1, 1.0];
    pub const BLUE: [f32; 4] = [0.2, 0.4, 1.0, 1.0];
    pub const YELLOW: [f32; 4] = [1.0, 0.9, 0.1, 1.0];

    pub fn new() -> Self {
        Self::default()
    }

    /// Line width in pixels for the shapes pushed from now on (until the end of the frame; the
    /// width is back to 1 afterwards). Values below 1 are drawn 1 pixel wide.
    pub fn set_width(&mut self, pixels: f32) -> &mut Self {
        self.width = if pixels.is_finite() { pixels.max(1.0) } else { 1.0 };
        self
    }

    /// The window the shapes pushed from now on are drawn in (`WindowId` or its raw `u32`; 0 = the
    /// main window, the default). Shapes of a window that has no 2D camera are dropped. Back to
    /// 0 after the frame's extract.
    pub fn set_window(&mut self, window: impl Into<u32>) -> &mut Self {
        self.window = window.into();
        self
    }

    fn push(&mut self, kind: Kind, world: bool, color: [f32; 4]) -> &mut Self {
        self.shapes.push(Shape { kind, world, color, width: self.width, window: self.window });
        self
    }

    /// A line between two world points.
    pub fn line(&mut self, a: Vec2, b: Vec2, color: [f32; 4]) -> &mut Self {
        self.push(Kind::Line(a, b), true, color)
    }

    /// A rectangle outline between two world corners (any order).
    pub fn rect(&mut self, min: Vec2, max: Vec2, color: [f32; 4]) -> &mut Self {
        self.push(Kind::Rect(min, max), true, color)
    }

    pub fn rect_filled(&mut self, min: Vec2, max: Vec2, color: [f32; 4]) -> &mut Self {
        self.push(Kind::FillRect(min, max), true, color)
    }

    /// A circle outline; `radius` in world units.
    pub fn circle(&mut self, center: Vec2, radius: f32, color: [f32; 4]) -> &mut Self {
        self.push(Kind::Circle(center, radius), true, color)
    }

    /// A line between two screen pixels.
    pub fn screen_line(&mut self, a: Vec2, b: Vec2, color: [f32; 4]) -> &mut Self {
        self.push(Kind::Line(a, b), false, color)
    }

    pub fn screen_rect(&mut self, min: Vec2, max: Vec2, color: [f32; 4]) -> &mut Self {
        self.push(Kind::Rect(min, max), false, color)
    }

    pub fn screen_rect_filled(&mut self, min: Vec2, max: Vec2, color: [f32; 4]) -> &mut Self {
        self.push(Kind::FillRect(min, max), false, color)
    }

    /// A circle outline; `radius` in pixels.
    pub fn screen_circle(&mut self, center: Vec2, radius: f32, color: [f32; 4]) -> &mut Self {
        self.push(Kind::Circle(center, radius), false, color)
    }

    /// Drop everything still queued (shapes of windows without a camera); the extract calls it
    /// after all windows were flushed.
    pub fn discard(&mut self) {
        self.shapes.clear();
        self.width = 1.0;
        self.window = 0;
    }

    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Convert the queued shapes of the MAIN window into `out` (screen pixels) with `camera`
    /// (world shapes are dropped without one) and remove them from the queue (the width and the
    /// target window reset). Shapes of other windows stay for [`flush_window_into`](Self::flush_window_into).
    pub fn flush_into(&mut self, camera: Option<&CameraView>, out: &mut DrawList) {
        self.flush_window_into(0, camera, out);
        self.width = 1.0;
        self.window = 0;
    }

    /// Like [`flush_into`](Self::flush_into) for the shapes queued for `window`.
    pub fn flush_window_into(&mut self, window: u32, camera: Option<&CameraView>, out: &mut DrawList) {
        let (shapes, rest): (Vec<Shape>, Vec<Shape>) = std::mem::take(&mut self.shapes).into_iter().partition(|s| s.window == window);
        self.shapes = rest;
        let project = |p: Vec2| -> Option<[f32; 2]> {
            let cam = camera?;
            let m = &cam.view_projection;
            // clip = M * (x, y, 0, 1); column-major, m[column][row]
            let x = m[0][0] * p.x + m[1][0] * p.y + m[3][0];
            let y = m[0][1] * p.x + m[1][1] * p.y + m[3][1];
            let w = m[0][3] * p.x + m[1][3] * p.y + m[3][3];
            let w = if w.abs() < 1e-8 { 1.0 } else { w };
            let (vw, vh) = (cam.viewport.0.max(1) as f32, cam.viewport.1.max(1) as f32);
            Some([(x / w * 0.5 + 0.5) * vw, (0.5 - y / w * 0.5) * vh])
        };
        for s in shapes {
            let pt = |p: Vec2| if s.world { project(p) } else { Some(p.to_array()) };
            let corners = |a: Vec2, b: Vec2| -> Option<[[f32; 2]; 4]> {
                let (lo, hi) = (a.min(b), a.max(b));
                Some([pt(lo)?, pt(Vec2::new(hi.x, lo.y))?, pt(hi)?, pt(Vec2::new(lo.x, hi.y))?])
            };
            match s.kind {
                Kind::Line(a, b) => {
                    if let (Some(a), Some(b)) = (pt(a), pt(b)) {
                        out.line(a, b, s.width, s.color);
                    }
                }
                Kind::Rect(a, b) => {
                    if let Some(c) = corners(a, b) {
                        out.polyline(&c, s.width, s.color, true);
                    }
                }
                Kind::FillRect(a, b) => {
                    if !s.world {
                        let (lo, hi) = (a.min(b), a.max(b));
                        out.fill_rect(Rect::new(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y), s.color);
                    } else if let Some(c) = corners(a, b) {
                        let v: Vec<_> = c.iter().map(|p| super::Vertex2D::new(*p, [0.0, 0.0], s.color)).collect();
                        out.triangles(None, &v, &[0, 1, 2, 0, 2, 3]);
                    }
                }
                Kind::Circle(c, r) => {
                    let pts: Option<Vec<[f32; 2]>> = (0..CIRCLE_SEGMENTS)
                        .map(|i| {
                            let t = i as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
                            pt(c + Vec2::new(t.cos(), t.sin()) * r)
                        })
                        .collect();
                    if let Some(p) = pts {
                        out.polyline(&p, s.width, s.color, true);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(viewport: (u32, u32), zoom: f32) -> CameraView {
        // orthographic, centred on the origin, `zoom` pixels per world unit, y up
        let (hw, hh) = (viewport.0 as f32 * 0.5 / zoom, viewport.1 as f32 * 0.5 / zoom);
        let m = [[1.0 / hw, 0.0, 0.0, 0.0], [0.0, 1.0 / hh, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        CameraView {
            entity: hecs::Entity::DANGLING,
            position: Vec2::ZERO,
            rotation: 0.0,
            zoom,
            kind: 0,
            target: 0,
            near: -1.0,
            far: 1.0,
            fov_y: 1.0,
            clear_color: [0.0; 4],
            viewport,
            view: m,
            projection: m,
            view_projection: m,
        }
    }

    fn bounds(l: &DrawList) -> ([f32; 2], [f32; 2]) {
        let mut lo = [f32::MAX; 2];
        let mut hi = [f32::MIN; 2];
        for v in l.vertices() {
            for i in 0..2 {
                lo[i] = lo[i].min(v.pos[i]);
                hi[i] = hi[i].max(v.pos[i]);
            }
        }
        let r = |a: [f32; 2]| [(a[0] * 100.0).round() / 100.0, (a[1] * 100.0).round() / 100.0];
        (r(lo), r(hi))
    }

    #[test]
    fn world_rect_lands_on_the_right_pixels() {
        let mut d = DebugDraw2D::new();
        d.set_width(2.0).rect(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0), DebugDraw2D::RED);
        let mut l = DrawList::new();
        // 100x100 viewport, 10 px per unit: the rect spans pixels 40..60 (centred line of width 2)
        d.flush_into(Some(&camera((100, 100), 10.0)), &mut l);
        let (lo, hi) = bounds(&l);
        assert_eq!((lo, hi), ([39.0, 39.0], [61.0, 61.0]));
        assert!(d.is_empty());
    }

    #[test]
    fn world_y_is_up_and_screen_y_is_down() {
        let mut d = DebugDraw2D::new();
        d.rect_filled(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0), DebugDraw2D::GREEN);
        let mut l = DrawList::new();
        d.flush_into(Some(&camera((100, 100), 10.0)), &mut l);
        assert_eq!(bounds(&l), ([50.0, 40.0], [60.0, 50.0]));
        d.screen_rect_filled(Vec2::new(5.0, 6.0), Vec2::new(15.0, 26.0), DebugDraw2D::GREEN);
        let mut l = DrawList::new();
        d.flush_into(None, &mut l);
        assert_eq!(bounds(&l), ([5.0, 6.0], [15.0, 26.0]));
    }

    #[test]
    fn world_shapes_need_a_camera_and_width_resets() {
        let mut d = DebugDraw2D::new();
        d.set_width(5.0).line(Vec2::ZERO, Vec2::ONE, DebugDraw2D::WHITE).circle(Vec2::ZERO, 1.0, DebugDraw2D::WHITE);
        assert_eq!(d.len(), 2);
        let mut l = DrawList::new();
        d.flush_into(None, &mut l);
        assert!(l.is_empty() && d.is_empty());
        d.screen_line(Vec2::ZERO, Vec2::new(10.0, 0.0), DebugDraw2D::WHITE);
        d.flush_into(None, &mut l);
        // width back to 1: the line quad is 1 pixel thick
        assert_eq!(bounds(&l), ([0.0, -0.5], [10.0, 0.5]));
    }
}
