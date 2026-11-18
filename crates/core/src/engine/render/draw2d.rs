//! `Draw2D`: the shared 2D drawing surface (ROADMAP 4C.3).
//!
//! A small immediate API for 2D drawing in **target pixels** (physical, origin top-left, x right,
//! y down): textured / coloured triangle lists, clip rectangle, blend mode, texture filter. It is
//! what the debug draw ([`super::DebugDraw2D`]), the planned fallback UI and every 2D backend share.
//! It is deliberately no bigger than what SDL3's `SDL_Renderer` can do (`SDL_RenderGeometry` with
//! one texture per call, `SDL_SetRenderClipRect`, the SDL blend modes, per-texture scale mode),
//! so the SDL backend (4C.4) can implement it exactly. There is no transform and no shader state:
//! callers convert world to pixels on the CPU.
//!
//! * [`Draw2D`] is the trait. Its required methods are the four primitives; quads, rectangles,
//!   lines and outlines are provided on top of `triangles`.
//! * [`DrawList`] is the recorder: it implements [`Draw2D`] and keeps plain data (batched
//!   commands), so a frame's drawing can sit in [`Frame2D::draw`](super::Frame2D) and be replayed
//!   by any backend ([`DrawList::replay`] onto another `Draw2D`, or read `commands()` directly).
//! * [`WgpuDraw2D`](super::WgpuDraw2D) draws a list with wgpu.
//!
//! Colours are straight RGBA, **sRGB-encoded** 0..1 (what a UI author or SDL expects); the wgpu
//! backend converts for its sRGB surface. Textures are handles of the engine's
//! [`Textures`](super::Textures) store (images are straight-alpha RGBA8); a glyph atlas or a UI
//! nine-slice is simply a texture added to that store, and `Textures::replace` updates it.

use crate::engine::components::TextureHandle;

/// One vertex: pixel position, texture coordinate (0..1, (0,0) = image top-left) and colour
/// (multiplied with the texture sample; use white for "texture as is").
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex2D {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl Vertex2D {
    pub const fn new(pos: [f32; 2], uv: [f32; 2], color: [f32; 4]) -> Self {
        Self { pos, uv, color }
    }
}

/// An axis-aligned rectangle in pixels (`x`, `y` = top-left corner).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

/// An integer rectangle (clip rectangles; `SDL_Rect`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
}

/// How a draw combines with what is already in the target. Matches SDL's blend modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Blend {
    /// Overwrite (`SDL_BLENDMODE_NONE`).
    None,
    /// Straight alpha: `src * a + dst * (1 - a)` (`SDL_BLENDMODE_BLEND`). The default.
    #[default]
    Alpha,
    /// `src * a + dst` (`SDL_BLENDMODE_ADD`).
    Add,
    /// Colour already multiplied by alpha: `src + dst * (1 - a)` (`SDL_BLENDMODE_BLEND_PREMULTIPLIED`).
    /// For textures or vertex colours stored premultiplied; the sampled texture is straight alpha, so
    /// premultiply in the vertex colour only when the texture is opaque white or premultiplied itself.
    Premultiplied,
}

/// Texture sampling (`SDL_SCALEMODE_NEAREST` / `SDL_SCALEMODE_LINEAR`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    #[default]
    Nearest,
    Linear,
}

/// The 2D drawing calls. All coordinates are target pixels (see the module docs). State calls
/// (`set_*`) apply to the triangles drawn after them.
pub trait Draw2D {
    /// Clip to `rect` (target pixels, clamped to the target); `None` removes the clip.
    fn set_clip(&mut self, rect: Option<RectI>);
    /// Blend mode for the following draws (initially [`Blend::Alpha`]).
    fn set_blend(&mut self, blend: Blend);
    /// Sampling for the following textured draws (initially [`Filter::Nearest`]).
    fn set_filter(&mut self, filter: Filter);
    /// Draw a triangle list: `indices` (three per triangle) index `verts`. `texture` `None` draws
    /// the vertex colour only. A call with an index out of range, or an index count that is not a
    /// multiple of three, is ignored. A texture the backend does not know is drawn as white.
    fn triangles(&mut self, texture: Option<TextureHandle>, verts: &[Vertex2D], indices: &[u32]);

    /// A filled rectangle of one colour.
    fn fill_rect(&mut self, rect: Rect, color: [f32; 4]) {
        self.quad(None, rect, [0.0, 0.0, 1.0, 1.0], color);
    }

    /// A rectangle showing `uv = [u_min, v_min, u_max, v_max]` of `texture`, multiplied by `color`.
    /// Swap the uv bounds to mirror.
    fn quad(&mut self, texture: Option<TextureHandle>, rect: Rect, uv: [f32; 4], color: [f32; 4]) {
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
        let v = [
            Vertex2D::new([x0, y0], [uv[0], uv[1]], color),
            Vertex2D::new([x1, y0], [uv[2], uv[1]], color),
            Vertex2D::new([x1, y1], [uv[2], uv[3]], color),
            Vertex2D::new([x0, y1], [uv[0], uv[3]], color),
        ];
        self.triangles(texture, &v, &[0, 1, 2, 0, 2, 3]);
    }

    /// The 1-`width` pixel outline of a rectangle, inside `rect`, drawn as four filled rectangles.
    fn rect_outline(&mut self, rect: Rect, width: f32, color: [f32; 4]) {
        let t = width.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
        if t <= 0.0 {
            return;
        }
        let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
        self.fill_rect(Rect::new(x, y, w, t), color);
        self.fill_rect(Rect::new(x, y + h - t, w, t), color);
        self.fill_rect(Rect::new(x, y + t, t, h - 2.0 * t), color);
        self.fill_rect(Rect::new(x + w - t, y + t, t, h - 2.0 * t), color);
    }

    /// A line segment `width` pixels thick (a quad; flat ends).
    fn line(&mut self, a: [f32; 2], b: [f32; 2], width: f32, color: [f32; 4]) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len <= f32::EPSILON || width <= 0.0 {
            return;
        }
        let (nx, ny) = (-dy / len * width * 0.5, dx / len * width * 0.5);
        let c = |p: [f32; 2], s: f32| Vertex2D::new([p[0] + nx * s, p[1] + ny * s], [0.0, 0.0], color);
        let v = [c(a, 1.0), c(b, 1.0), c(b, -1.0), c(a, -1.0)];
        self.triangles(None, &v, &[0, 1, 2, 0, 2, 3]);
    }

    /// A polyline of connected segments, `width` thick, square caps half a width long (so
    /// corners join without gaps); `closed` joins the last point to the first.
    fn polyline(&mut self, points: &[[f32; 2]], width: f32, color: [f32; 4], closed: bool) {
        let n = points.len();
        if n < 2 {
            return;
        }
        let segs = if closed && n > 2 { n } else { n - 1 };
        let h = width * 0.5;
        for i in 0..segs {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len <= f32::EPSILON {
                continue;
            }
            let (ux, uy) = (dx / len * h, dy / len * h);
            self.line([a[0] - ux, a[1] - uy], [b[0] + ux, b[1] + uy], width, color);
        }
    }
}

/// The state a command was recorded with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawState {
    /// 0 = no texture.
    pub texture: u32,
    pub blend: Blend,
    pub filter: Filter,
    pub clip: Option<RectI>,
}

impl Default for DrawState {
    fn default() -> Self {
        Self { texture: 0, blend: Blend::Alpha, filter: Filter::Nearest, clip: None }
    }
}

/// One batch of triangles drawn with one [`DrawState`]. Its vertices are
/// `DrawList::vertices()[verts.clone()]`; its indices (`DrawList::indices()[indices.clone()]`) are
/// relative to `verts.start`, so a backend can pass the two slices straight to e.g.
/// `SDL_RenderGeometry`.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawCommand {
    pub state: DrawState,
    pub verts: std::ops::Range<u32>,
    pub indices: std::ops::Range<u32>,
}

/// A recorded drawing: plain data, backend-neutral. Consecutive calls with the same state merge
/// into one command.
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    state: DrawState,
    verts: Vec<Vertex2D>,
    indices: Vec<u32>,
    commands: Vec<DrawCommand>,
}

impl DrawList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything (keeps the allocations) and reset the state to the defaults.
    pub fn clear(&mut self) {
        self.state = DrawState::default();
        self.verts.clear();
        self.indices.clear();
        self.commands.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    pub fn vertices(&self) -> &[Vertex2D] {
        &self.verts
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Append `other` (its commands keep their own states; this list's current state is unchanged).
    pub fn append(&mut self, other: &DrawList) {
        for c in &other.commands {
            let verts = &other.verts[c.verts.start as usize..c.verts.end as usize];
            let idx = &other.indices[c.indices.start as usize..c.indices.end as usize];
            self.push(c.state, verts, idx);
        }
    }

    /// Replay onto another `Draw2D` (state calls and triangles); the target's state afterwards
    /// is that of the last command.
    pub fn replay(&self, to: &mut dyn Draw2D) {
        for c in &self.commands {
            to.set_clip(c.state.clip);
            to.set_blend(c.state.blend);
            to.set_filter(c.state.filter);
            let tex = (c.state.texture != 0).then_some(TextureHandle { id: c.state.texture });
            to.triangles(
                tex,
                &self.verts[c.verts.start as usize..c.verts.end as usize],
                &self.indices[c.indices.start as usize..c.indices.end as usize],
            );
        }
    }

    fn push(&mut self, state: DrawState, verts: &[Vertex2D], indices: &[u32]) {
        if indices.is_empty() || indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= verts.len()) {
            return;
        }
        let mergeable = self.commands.last().is_some_and(|c| c.state == state);
        if !mergeable {
            let (v, i) = (self.verts.len() as u32, self.indices.len() as u32);
            self.commands.push(DrawCommand { state, verts: v..v, indices: i..i });
        }
        let cmd = self.commands.last_mut().expect("a command exists");
        let base = cmd.verts.end - cmd.verts.start;
        self.verts.extend_from_slice(verts);
        self.indices.extend(indices.iter().map(|&i| i + base));
        cmd.verts.end = self.verts.len() as u32;
        cmd.indices.end = self.indices.len() as u32;
    }
}

impl Draw2D for DrawList {
    fn set_clip(&mut self, rect: Option<RectI>) {
        self.state.clip = rect;
    }

    fn set_blend(&mut self, blend: Blend) {
        self.state.blend = blend;
    }

    fn set_filter(&mut self, filter: Filter) {
        self.state.filter = filter;
    }

    fn triangles(&mut self, texture: Option<TextureHandle>, verts: &[Vertex2D], indices: &[u32]) {
        let state = DrawState { texture: texture.map_or(0, |t| t.id), ..self.state };
        self.push(state, verts, indices);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: [f32; 4] = [1.0; 4];

    #[test]
    fn same_state_merges_and_indices_are_relative() {
        let mut l = DrawList::new();
        l.fill_rect(Rect::new(0.0, 0.0, 2.0, 2.0), W);
        l.fill_rect(Rect::new(5.0, 5.0, 2.0, 2.0), W);
        assert_eq!(l.commands().len(), 1);
        let c = &l.commands()[0];
        assert_eq!((c.verts.clone(), c.indices.clone()), (0..8, 0..12));
        assert_eq!(&l.indices()[6..], &[4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn state_changes_split_commands_with_relative_indices() {
        let mut l = DrawList::new();
        l.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), W);
        l.set_blend(Blend::Add);
        l.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), W);
        l.set_clip(Some(RectI::new(1, 2, 3, 4)));
        l.quad(Some(TextureHandle { id: 7 }), Rect::new(0.0, 0.0, 1.0, 1.0), [0.0, 0.0, 1.0, 1.0], W);
        let c = l.commands();
        assert_eq!(c.len(), 3);
        assert_eq!(c[1].state.blend, Blend::Add);
        assert_eq!(c[2].state.clip, Some(RectI::new(1, 2, 3, 4)));
        assert_eq!(c[2].state.texture, 7);
        assert_eq!(c[2].verts, 8..12);
        // indices restart at 0 in every command
        assert_eq!(&l.indices()[c[2].indices.start as usize..c[2].indices.end as usize], &[0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn bad_calls_are_ignored() {
        let mut l = DrawList::new();
        let v = [Vertex2D::default(); 3];
        l.triangles(None, &v, &[0, 1, 3]);
        l.triangles(None, &v, &[0, 1]);
        l.triangles(None, &v, &[]);
        assert!(l.is_empty());
        l.line([1.0, 1.0], [1.0, 1.0], 2.0, W);
        l.line([0.0, 0.0], [4.0, 0.0], 0.0, W);
        assert!(l.is_empty());
    }

    #[test]
    fn line_is_a_quad_of_the_given_width() {
        let mut l = DrawList::new();
        l.line([0.0, 10.0], [4.0, 10.0], 2.0, W);
        let ys: Vec<f32> = l.vertices().iter().map(|v| v.pos[1]).collect();
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 9.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 11.0);
    }

    #[test]
    fn replay_reproduces_the_list() {
        let mut a = DrawList::new();
        a.set_blend(Blend::Premultiplied);
        a.set_filter(Filter::Linear);
        a.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), W);
        a.set_clip(Some(RectI::new(0, 0, 5, 5)));
        a.rect_outline(Rect::new(0.0, 0.0, 10.0, 10.0), 1.0, W);
        let mut b = DrawList::new();
        a.replay(&mut b);
        assert_eq!(a.commands(), b.commands());
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.indices(), b.indices());
        let mut c = DrawList::new();
        c.append(&a);
        assert_eq!(c.commands(), a.commands());
    }

    #[test]
    fn outline_and_polyline_counts() {
        let mut l = DrawList::new();
        l.rect_outline(Rect::new(0.0, 0.0, 10.0, 10.0), 2.0, W);
        assert_eq!(l.vertices().len(), 16);
        let mut l = DrawList::new();
        l.polyline(&[[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]], 1.0, W, true);
        assert_eq!(l.vertices().len(), 12);
    }
}
