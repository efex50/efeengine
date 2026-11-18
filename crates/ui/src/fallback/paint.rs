//! Painting: the laid out model recorded into a `Draw2D`, in tree order (layers by z), all in
//! physical pixels. Backgrounds and borders are triangle meshes (`shapes`), text is textured
//! glyph quads from the atlas, images are textured quads; `Scroll` nodes clip with the clip
//! rectangle.

use std::collections::HashMap;

use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::{Draw2D, DrawList, Filter, Rect, RectI};

use super::edit::{InputState, caret_x};
use super::layout::{checkbox_box, text_spec};
use super::model::{DISABLED, FOCUSED, HOVERED, LRect, NORMAL, PRESSED, VNode};
use super::shapes::{Mesh, check_mark};
use super::text::{LINE_HEIGHT, TextCell, TextSystem};
use crate::id::WidgetId;
use crate::node::{ImageFit, NodeKind};
use crate::style::{Border, Color, Edges, Style};

/// Handle radius of a slider, logical px.
pub const SLIDER_HANDLE: f32 = 8.0;
/// Width of the scrollbar thumb, logical px.
pub const THUMB_W: f32 = 6.0;

/// Interaction state the painter reads.
pub struct PaintState<'a> {
    pub hovered: Option<WidgetId>,
    pub pressed: Option<WidgetId>,
    pub focus: Option<WidgetId>,
    pub focus_visible: bool,
    pub scroll: &'a HashMap<WidgetId, (f32, f32)>,
    pub caret_on: bool,
}

pub struct Painter<'a> {
    pub out: &'a mut DrawList,
    pub text: &'a mut TextSystem,
    pub cells: &'a mut HashMap<WidgetId, TextCell>,
    pub inputs: &'a mut HashMap<WidgetId, InputState>,
    pub images: &'a HashMap<TextureHandle, (u32, u32)>,
    pub mesh: &'a mut Mesh,
    pub st: PaintState<'a>,
    pub scale: f32,
    pub atlas: Option<TextureHandle>,
}

fn col(c: Color, opacity: f32) -> [f32; 4] {
    [c.r, c.g, c.b, c.a * opacity.clamp(0.0, 1.0)]
}

pub fn padding(st: &Style) -> Edges {
    st.padding.unwrap_or(Edges::ZERO)
}

/// The slider track: `(x_start, x_end, y_center)` of the handle centre, logical px.
pub fn slider_track(r: LRect) -> (f32, f32, f32) {
    (r.x + SLIDER_HANDLE, r.x + r.w - SLIDER_HANDLE, r.y + r.h * 0.5)
}

/// The scrollbar thumb of a scroll node at `r` with `content` height and `offset`, if the
/// content overflows: `(x, y, w, h)` logical px.
pub fn thumb_rect(r: LRect, content_h: f32, offset: f32) -> Option<LRect> {
    if content_h <= r.h + 0.5 || r.h <= 0.0 {
        return None;
    }
    let th = (r.h * r.h / content_h).max(20.0).min(r.h);
    let max_off = content_h - r.h;
    let y = r.y + (offset / max_off).clamp(0.0, 1.0) * (r.h - th);
    Some(LRect { x: r.x + r.w - THUMB_W - 2.0, y, w: THUMB_W, h: th })
}

impl Painter<'_> {
    fn flush(&mut self) {
        self.mesh.flush(self.out);
    }

    fn ph(&self, r: LRect) -> Rect {
        Rect::new(r.x * self.scale, r.y * self.scale, r.w * self.scale, r.h * self.scale)
    }

    fn set_clip(&mut self, clip: Option<LRect>) {
        self.flush();
        self.out.set_clip(clip.map(|c| {
            let s = self.scale;
            let (x0, y0) = ((c.x * s).floor(), (c.y * s).floor());
            let (x1, y1) = (((c.x + c.w) * s).ceil(), ((c.y + c.h) * s).ceil());
            RectI::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32)
        }));
    }

    fn clip_px(&self, clip: Option<LRect>) -> Option<[f32; 4]> {
        clip.map(|c| [c.x * self.scale, c.y * self.scale, (c.x + c.w) * self.scale, (c.y + c.h) * self.scale])
    }

    fn state_index(&self, n: &VNode) -> usize {
        if !n.enabled {
            DISABLED
        } else if self.st.pressed == Some(n.id) {
            PRESSED
        } else if self.st.focus == Some(n.id) && (self.st.focus_visible || matches!(n.kind, NodeKind::TextInput { .. })) {
            FOCUSED
        } else if self.st.hovered == Some(n.id) {
            HOVERED
        } else {
            NORMAL
        }
    }

    /// Background and border of a box.
    fn boxed(&mut self, r: LRect, st: &Style) {
        let op = st.opacity.unwrap_or(1.0);
        let radius = st.border.map_or(0.0, |b| b.radius) * self.scale;
        let rect = self.ph(r);
        if let Some(bg) = st.background
            && bg.a > 0.0
        {
            self.mesh.fill(rect, radius, col(bg, op));
        }
        if let Some(Border { width, color, .. }) = st.border
            && width > 0.0
        {
            self.mesh.stroke(rect, radius, width * self.scale, col(color, op));
        }
    }

    fn draw_buffer(&mut self, id: WidgetId, origin: (f32, f32), color: [f32; 4], clip: Option<LRect>) {
        let Some(atlas) = self.atlas else { return };
        self.flush();
        let clip = self.clip_px(clip);
        if let Some(cell) = self.cells.get(&id) {
            self.text.draw(&cell.buffer, origin, self.scale, color, clip, atlas, self.out);
        }
    }

    /// Lays `cell` out for `width` and returns its height.
    fn shape(&mut self, id: WidgetId, st: &Style, text: &str, wrap: bool, width: Option<f32>) -> (f32, f32) {
        let spec = text_spec(st, text, wrap);
        let cell = self.cells.entry(id).or_default();
        self.text.layout(cell, &spec, width)
    }

    pub fn node(&mut self, n: &VNode, origin: (f32, f32), clip: Option<LRect>) {
        let r = n.rect.offset(origin.0, origin.1);
        let is_container = matches!(n.kind, NodeKind::Container | NodeKind::Scroll) || (matches!(n.kind, NodeKind::Button { .. }) && !n.children.is_empty());
        if !is_container
            && clip.is_some_and(|c| {
                let i = c.intersect(&r);
                i.w <= 0.0 || i.h <= 0.0
            })
        {
            return;
        }
        let base = *n.style();
        let st = n.styles[self.state_index(n)];
        let pad = padding(&base);
        let content = LRect { x: r.x + pad.left, y: r.y + pad.top, w: (r.w - pad.left - pad.right).max(0.0), h: (r.h - pad.top - pad.bottom).max(0.0) };
        let op = st.opacity.unwrap_or(1.0);
        let tcol = col(st.text_color.unwrap_or(Color::WHITE), op);
        match &n.kind {
            NodeKind::Container => {
                self.boxed(r, &st);
                self.children(n, (r.x, r.y), clip);
            }
            NodeKind::Scroll => {
                self.boxed(r, &st);
                let off = self.st.scroll.get(&n.id).copied().unwrap_or((0.0, 0.0));
                let inner = clip.map_or(r, |c| c.intersect(&r));
                self.set_clip(Some(inner));
                self.children(n, (r.x - off.0, r.y - off.1), Some(inner));
                self.flush();
                if let Some(t) = thumb_rect(r, n.content.1, off.1) {
                    let hot = self.st.pressed == Some(n.id) || self.st.hovered == Some(n.id);
                    let a = if hot { 0.55 } else { 0.35 };
                    self.mesh.fill(self.ph(t), THUMB_W * 0.5 * self.scale, [0.8, 0.82, 0.88, a * op]);
                }
                self.set_clip(clip);
            }
            NodeKind::Text { text } => {
                self.boxed(r, &st);
                let (_, th) = self.shape(n.id, &base, text, true, Some(content.w));
                let _ = th;
                self.draw_buffer(n.id, (content.x, content.y), tcol, clip);
            }
            NodeKind::Button { label } => {
                self.boxed(r, &st);
                if n.children.is_empty() {
                    let (_, th) = self.shape(n.id, &base, label, true, Some(content.w));
                    let y = content.y + (content.h - th) * 0.5;
                    self.draw_buffer(n.id, (content.x, y), tcol, clip);
                } else {
                    self.children(n, (r.x, r.y), clip);
                }
            }
            NodeKind::Checkbox { label, checked } => {
                let b = checkbox_box(base.font_size.unwrap_or(16.0));
                let gap = base.gap.unwrap_or(8.0);
                let by = content.y + (content.h - b) * 0.5;
                let bx = LRect { x: content.x, y: by, w: b, h: b };
                let (mut border_w, mut border_c, mut radius) = st.border.map_or((0.0, Color::TRANSPARENT, 3.0), |b| (b.width, b.color, b.radius));
                if border_w <= 0.0 {
                    let tc = st.text_color.unwrap_or(Color::WHITE);
                    (border_w, border_c, radius) = (1.0, tc.with_alpha(tc.a * 0.6), 3.0);
                }
                let rect = self.ph(bx);
                if let Some(bg) = st.background
                    && bg.a > 0.0
                {
                    self.mesh.fill(rect, radius * self.scale, col(bg, op));
                }
                self.mesh.stroke(rect, radius * self.scale, border_w * self.scale, col(border_c, op));
                self.flush();
                if *checked {
                    check_mark(self.out, rect, tcol);
                }
                let lx = content.x + b + gap;
                let (_, th) = self.shape(n.id, &base, label, true, Some((content.w - b - gap).max(0.0)));
                self.draw_buffer(n.id, (lx, content.y + (content.h - th) * 0.5), tcol, clip);
            }
            NodeKind::Slider { min, max, value, .. } => {
                let accent = col(st.text_color.unwrap_or(Color::WHITE), op);
                let rail = st.background.map_or([0.23, 0.25, 0.29, op], |c| col(c, op));
                let (x0, x1, cy) = slider_track(r);
                let t = if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
                let hx = x0 + t * (x1 - x0);
                let rr = st.border.map_or(2.0, |b| b.radius).min(2.0) * self.scale;
                let s = self.scale;
                let rail_r = |x: f32, w: f32| Rect::new(x * s, (cy - 2.0) * s, w * s, 4.0 * s);
                self.mesh.fill(rail_r(r.x, r.w), rr, rail);
                self.mesh.fill(rail_r(r.x, hx - r.x), rr, accent);
                let hr = SLIDER_HANDLE * s;
                self.mesh.fill(Rect::new(hx * s - hr, cy * s - hr, 2.0 * hr, 2.0 * hr), hr, accent);
                if self.state_index(n) == FOCUSED
                    && let Some(b) = st.border
                    && b.width > 0.0
                {
                    self.mesh.stroke(self.ph(r), b.radius * s, b.width * s, col(b.color, op));
                }
            }
            NodeKind::TextInput { value, placeholder, max_len: _, password } => {
                self.boxed(r, &st);
                self.text_input(n, &base, &st, value, placeholder, *password, content, clip);
            }
            NodeKind::Image { texture, fit } => {
                let Some((iw, ih)) = self.images.get(texture).copied() else { return };
                let (dest, uv) = fit_image(*fit, r, (iw as f32, ih as f32));
                self.flush();
                self.set_clip(clip.map_or(Some(r), |c| Some(c.intersect(&r))));
                self.out.set_filter(Filter::Linear);
                self.out.quad(Some(*texture), self.ph(dest), uv, [1.0, 1.0, 1.0, op.clamp(0.0, 1.0)]);
                self.out.set_filter(Filter::Nearest);
                self.set_clip(clip);
            }
            NodeKind::GpuView { .. } => {
                // A GPU view needs a wgpu pass of its own; this backend only marks the spot.
                self.mesh.stroke(self.ph(r), 0.0, 1.0 * self.scale, [0.6, 0.65, 0.75, 0.35 * op]);
            }
        }
    }

    fn children(&mut self, n: &VNode, origin: (f32, f32), clip: Option<LRect>) {
        for c in &n.children {
            self.node(c, origin, clip);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text_input(&mut self, n: &VNode, base: &Style, st: &Style, value: &str, placeholder: &str, password: bool, inner: LRect, clip: Option<LRect>) {
        let op = st.opacity.unwrap_or(1.0);
        let tc = st.text_color.unwrap_or(Color::WHITE);
        let tcol = col(tc, op);
        let line_h = base.font_size.unwrap_or(16.0) * LINE_HEIGHT;
        let y = inner.y + (inner.h - line_h) * 0.5;
        let focused = self.st.focus == Some(n.id);
        let spec = text_spec(base, value, false);
        let inputs = &mut *self.inputs;
        let ts = &mut *self.text;
        let state = inputs.entry(n.id).or_insert_with(|| InputState::new(ts, &spec, value));
        if state.value != value {
            state.set_value(ts, &spec, value);
        }
        state.refresh_display(ts, &spec, password);
        // Keep the caret inside the field.
        let cur_d = state.display_offset(state.cursor(), password);
        let cx = caret_x(&state.display.buffer, cur_d + state.preedit.len());
        let room = (inner.w - 2.0).max(1.0);
        if cx - state.scroll_x > room {
            state.scroll_x = cx - room;
        }
        if cx < state.scroll_x {
            state.scroll_x = cx;
        }
        if state.scroll_x < 0.0 {
            state.scroll_x = 0.0;
        }
        let sx = state.scroll_x;
        let origin = (inner.x - sx, y);
        let clip_box = clip.map_or(inner, |c| c.intersect(&inner));
        let s = self.scale;
        self.flush();
        self.set_clip(Some(clip_box));
        // selection
        let state = &*self.inputs.get(&n.id).expect("input state");
        let mut sel_rect = None;
        if focused && let Some((a, b)) = state.selection() {
            let (da, db) = (state.display_offset(a, password), state.display_offset(b, password));
            let (xa, xb) = (caret_x(&state.display.buffer, da), caret_x(&state.display.buffer, db));
            sel_rect = Some(Rect::new((origin.0 + xa) * s, y * s, (xb - xa) * s, line_h * s));
        }
        let show_placeholder = value.is_empty() && state.preedit.is_empty();
        let preedit_span = (!state.preedit.is_empty()).then(|| {
            let (a, b) = (cur_d, cur_d + state.preedit.len());
            (caret_x(&state.display.buffer, a), caret_x(&state.display.buffer, b))
        });
        let caret = (focused && self.st.caret_on).then(|| origin.0 + cx);
        if let Some(r) = sel_rect {
            self.mesh.fill(r, 0.0, [0.36, 0.62, 1.0, 0.4 * op]);
        }
        self.flush();
        if show_placeholder {
            if !placeholder.is_empty() {
                let pspec = text_spec(base, placeholder, false);
                let cell = self.cells.entry(n.id).or_default();
                self.text.layout(cell, &pspec, None);
                self.draw_buffer(n.id, origin, [tcol[0], tcol[1], tcol[2], tcol[3] * 0.5], Some(clip_box));
            }
        } else if let Some(atlas) = self.atlas {
            let clip_px = self.clip_px(Some(clip_box));
            let state = self.inputs.get(&n.id).expect("input state");
            self.text.draw(&state.display.buffer, origin, s, tcol, clip_px, atlas, self.out);
        }
        if let Some((a, b)) = preedit_span {
            self.mesh.rect(Rect::new((origin.0 + a) * s, (y + line_h - 1.0) * s, (b - a) * s, s.max(1.0)), tcol);
        }
        if let Some(x) = caret {
            self.mesh.rect(Rect::new(x * s, (y + 1.0) * s, s.max(1.0), (line_h - 2.0) * s), tcol);
        }
        self.flush();
        self.set_clip(clip);
    }

    pub fn finish(&mut self) {
        self.flush();
    }
}

/// Destination rectangle and uv bounds for an image of `size` shown in `r` with `fit`; the
/// destination is cut to `r` (with the uv cut to match).
pub fn fit_image(fit: ImageFit, r: LRect, size: (f32, f32)) -> (LRect, [f32; 4]) {
    let (iw, ih) = size;
    if iw <= 0.0 || ih <= 0.0 || r.w <= 0.0 || r.h <= 0.0 {
        return (LRect { w: 0.0, h: 0.0, ..r }, [0.0, 0.0, 1.0, 1.0]);
    }
    let (dw, dh) = match fit {
        ImageFit::Fill => (r.w, r.h),
        ImageFit::Contain => {
            let k = (r.w / iw).min(r.h / ih);
            (iw * k, ih * k)
        }
        ImageFit::Cover => {
            let k = (r.w / iw).max(r.h / ih);
            (iw * k, ih * k)
        }
        ImageFit::None => (iw, ih),
    };
    let dest = LRect { x: r.x + (r.w - dw) * 0.5, y: r.y + (r.h - dh) * 0.5, w: dw, h: dh };
    let vis = dest.intersect(&r);
    let uv = [(vis.x - dest.x) / dw, (vis.y - dest.y) / dh, (vis.x + vis.w - dest.x) / dw, (vis.y + vis.h - dest.y) / dh];
    (vis, uv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_letterboxes_and_cover_crops() {
        let r = LRect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        let (d, uv) = fit_image(ImageFit::Contain, r, (50.0, 50.0));
        assert_eq!((d.x, d.w, d.h), (25.0, 50.0, 50.0));
        assert_eq!(uv, [0.0, 0.0, 1.0, 1.0]);
        let (d, uv) = fit_image(ImageFit::Cover, r, (50.0, 50.0));
        assert_eq!((d.w, d.h), (100.0, 50.0));
        assert_eq!((uv[0], uv[2]), (0.0, 1.0));
        assert!((uv[1] - 0.25).abs() < 1e-6 && (uv[3] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn thumb_only_when_overflowing() {
        let r = LRect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        assert!(thumb_rect(r, 100.0, 0.0).is_none());
        let t = thumb_rect(r, 400.0, 0.0).unwrap();
        assert_eq!(t.h, 25.0);
        let t = thumb_rect(r, 400.0, 300.0).unwrap();
        assert!((t.y + t.h - 100.0).abs() < 1e-3);
    }
}
