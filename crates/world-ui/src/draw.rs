//! The overlay: projects each root through the camera of its window, measures and paints the
//! elements with the fallback backend's text and shape code, through `Draw2D`. Works in the wgpu
//! and the SDL rendering modes, and on web, since it only uses `Draw2D` and a texture.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use efeengine_core::prelude::*;
use efeengine_core::render::{Draw2D, Draw2DOverlay, Overlay2D, Rect, OVERLAY_TEXTURE_BASE};
use efeengine_ui::text_kit::shapes::Mesh;
use efeengine_ui::text_kit::text::{TextCell, TextSpec, TextSystem};
use efeengine_ui::style::TextAlign;

use crate::components::Color;
use crate::extract::{CamView, Item, Snapshot};

/// Texture ids for the glyph atlas: one per window, above the ids the fallback UI uses
/// (`OVERLAY_TEXTURE_BASE + window`).
const ATLAS_BASE: u32 = OVERLAY_TEXTURE_BASE + 0x100;

/// World point -> logical pixels on a window of `viewport` logical size. The camera's position
/// is the centre, its rotation turns the view, `zoom` is pixels per world unit, +y is down on
/// screen.
pub fn to_screen(cam: CamView, world: Vec2, viewport: Vec2) -> Vec2 {
    let d = world - cam.position;
    let (s, c) = (-cam.rotation).sin_cos();
    let local = Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c);
    Vec2::new(viewport.x * 0.5 + local.x * cam.zoom, viewport.y * 0.5 - local.y * cam.zoom)
}

/// The overlay's state: the text system and its glyph atlas, the shaped text per element, and
/// the snapshot the engine's system filled in this frame.
pub struct WorldUiOverlay {
    pub(crate) shared: Rc<RefCell<Snapshot>>,
    pub(crate) text: TextSystem,
    pub(crate) cells: HashMap<Entity, TextCell>,
    pub(crate) mesh: Mesh,
    atlas_tex: Option<TextureHandle>,
}

impl WorldUiOverlay {
    pub(crate) fn new(shared: Rc<RefCell<Snapshot>>) -> Self {
        Self { shared, text: TextSystem::new(false), cells: HashMap::new(), mesh: Mesh::new(), atlas_tex: None }
    }
}

impl Draw2DOverlay for WorldUiOverlay {
    fn render(&mut self, cx: &mut Overlay2D) {
        let atlas = *self.atlas_tex.get_or_insert(TextureHandle { id: ATLAS_BASE + cx.window });
        let scale = cx.scale_factor.max(0.01);
        self.mesh.feather = !cx.integer_vertices;
        self.text.begin_frame();

        let shared = self.shared.clone();
        let snap = shared.borrow();
        if let Some((_, cam)) = snap.cameras.iter().find(|(w, _)| *w == cx.window) {
            let viewport = Vec2::new(cx.size.0 as f32 / scale, cx.size.1 as f32 / scale);
            let mut used = HashSet::new();
            for root in &snap.roots {
                let anchor = to_screen(*cam, root.world, viewport) + root.offset;
                let size = self.measure(&root.item, &mut used);
                let origin = anchor - size * 0.5;
                if origin.x > viewport.x || origin.y > viewport.y || origin.x + size.x < 0.0 || origin.y + size.y < 0.0 {
                    continue;
                }
                self.paint(&root.item, origin, root.alpha, scale, atlas, cx.draw);
            }
            self.cells.retain(|k, _| used.contains(k));
        }
        drop(snap);

        if let Some(img) = self.text.atlas.take_image() {
            cx.textures.upload(atlas, &img);
        }
    }
}

impl WorldUiOverlay {
    /// Logical size of an element (shaping text as needed). Records the text keys it used.
    fn measure(&mut self, item: &Item, used: &mut HashSet<Entity>) -> Vec2 {
        match item {
            Item::Text { key, text, size, bold, .. } => {
                used.insert(*key);
                let spec = text_spec(text, *size, *bold);
                let cell = self.cells.entry(*key).or_default();
                let (w, h) = self.text.layout(cell, &spec, None);
                Vec2::new(w, h)
            }
            Item::Rect { size, .. } | Item::Bar { size, .. } | Item::Sprite { size, .. } => *size,
            Item::Container { vertical, gap, padding, children, .. } => {
                let mut main = 0.0f32;
                let mut cross = 0.0f32;
                for (i, child) in children.iter().enumerate() {
                    let s = self.measure(child, used);
                    let (m, c) = if *vertical { (s.y, s.x) } else { (s.x, s.y) };
                    main += m + if i > 0 { *gap } else { 0.0 };
                    cross = cross.max(c);
                }
                let inner = if *vertical { Vec2::new(cross, main) } else { Vec2::new(main, cross) };
                inner + Vec2::splat(*padding * 2.0)
            }
        }
    }

    /// Draws an element whose top-left is `origin` (logical px) at `alpha`.
    fn paint(&mut self, item: &Item, origin: Vec2, alpha: f32, scale: f32, atlas: TextureHandle, out: &mut dyn Draw2D) {
        let phys = |p: Vec2, s: Vec2| Rect::new(p.x * scale, p.y * scale, s.x * scale, s.y * scale);
        match item {
            Item::Text { key, color, .. } => {
                // Shaped by `measure` earlier this frame.
                let Some(cell) = self.cells.get(key) else { return };
                self.mesh.flush(out);
                self.text.draw(&cell.buffer, (origin.x, origin.y), scale, faded(*color, alpha), None, atlas, out);
            }
            Item::Rect { size, color, radius, border, border_color } => {
                let r = phys(origin, *size);
                self.mesh.fill(r, *radius * scale, faded(*color, alpha));
                if *border > 0.0 {
                    self.mesh.stroke(r, *radius * scale, *border * scale, faded(*border_color, alpha));
                }
                self.mesh.flush(out);
            }
            Item::Bar { size, shown, ghost, back, fill, ghost_color, vertical } => {
                self.mesh.rect(phys(origin, *size), faded(*back, alpha));
                let frac = |v: f32| v.clamp(0.0, 1.0);
                let (hi, g) = (frac(*shown), frac(*ghost));
                let seg = |a: f32, b: f32| -> Rect {
                    if *vertical {
                        // Fills from the bottom: fraction 0 is the bottom edge.
                        let top = origin.y + size.y * (1.0 - b);
                        let h = size.y * (b - a);
                        phys(Vec2::new(origin.x, top), Vec2::new(size.x, h))
                    } else {
                        phys(Vec2::new(origin.x + size.x * a, origin.y), Vec2::new(size.x * (b - a), size.y))
                    }
                };
                if g > hi {
                    self.mesh.rect(seg(hi, g), faded(*ghost_color, alpha));
                }
                if hi > 0.0 {
                    self.mesh.rect(seg(0.0, hi), faded(*fill, alpha));
                }
                self.mesh.flush(out);
            }
            Item::Sprite { texture, size, tint } => {
                out.quad(Some(*texture), phys(origin, *size), [0.0, 0.0, 1.0, 1.0], faded(*tint, alpha));
            }
            Item::Container { vertical, gap, padding, background, children } => {
                let total = self.measure(item, &mut HashSet::new());
                if let Some(bg) = background {
                    self.mesh.fill(phys(origin, total), 0.0, faded(*bg, alpha));
                    self.mesh.flush(out);
                }
                let mut cursor = origin + Vec2::splat(*padding);
                for child in children {
                    let s = self.measure(child, &mut HashSet::new());
                    self.paint(child, cursor, alpha, scale, atlas, out);
                    if *vertical {
                        cursor.y += s.y + gap;
                    } else {
                        cursor.x += s.x + gap;
                    }
                }
            }
        }
    }
}

fn text_spec(text: &str, size: f32, bold: bool) -> TextSpec {
    TextSpec { text: text.to_string(), size, font: None, bold, align: TextAlign::Start, wrap: false }
}

fn faded(c: Color, alpha: f32) -> Color {
    [c[0], c[1], c[2], c[3] * alpha]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::approach;

    fn cam(x: f32, y: f32, zoom: f32, rotation: f32) -> CamView {
        CamView { position: Vec2::new(x, y), rotation, zoom }
    }

    #[test]
    fn projection_centres_the_camera_and_flips_y() {
        let vp = Vec2::new(200.0, 100.0);
        assert_eq!(to_screen(cam(0.0, 0.0, 1.0, 0.0), Vec2::ZERO, vp), Vec2::new(100.0, 50.0));
        // World +y is up, so it goes to smaller screen y.
        let up = to_screen(cam(0.0, 0.0, 2.0, 0.0), Vec2::new(10.0, 5.0), vp);
        assert!((up - Vec2::new(120.0, 40.0)).length() < 1e-4, "{up:?}");
    }

    #[test]
    fn projection_follows_camera_position_and_rotation() {
        let vp = Vec2::new(200.0, 100.0);
        // Camera at (10, 0): the world point (10, 0) is at the centre.
        assert_eq!(to_screen(cam(10.0, 0.0, 1.0, 0.0), Vec2::new(10.0, 0.0), vp), Vec2::new(100.0, 50.0));
        // Camera rotated 90 degrees counter-clockwise: world +y appears to the right of centre.
        let p = to_screen(cam(0.0, 0.0, 1.0, std::f32::consts::FRAC_PI_2), Vec2::new(0.0, 10.0), vp);
        assert!((p - Vec2::new(110.0, 50.0)).length() < 1e-4, "{p:?}");
    }

    #[test]
    fn approach_moves_by_at_most_the_step() {
        assert_eq!(approach(1.0, 0.0, 0.25), 0.75);
        assert_eq!(approach(1.0, 0.9, 0.25), 0.9);
        assert_eq!(approach(0.0, 0.5, 0.2), 0.2);
        assert_eq!(approach(0.4, 0.5, 1.0), 0.5);
    }

    #[test]
    fn text_measures_and_container_stacks() {
        let shared = Rc::new(RefCell::new(Snapshot::default()));
        let mut ov = WorldUiOverlay::new(shared);
        let mut used = HashSet::new();
        let mut w = efeengine_core::hecs::World::new();
        let a = w.spawn(());
        let text = |key| Item::Text { key, text: "Damage 42".into(), size: 16.0, color: [1.0; 4], bold: false };
        let one = ov.measure(&text(a), &mut used);
        assert!(one.x > 10.0 && one.y > 10.0, "text should have size, got {one:?}");
        let stack = Item::Container {
            vertical: true,
            gap: 4.0,
            padding: 2.0,
            background: None,
            children: vec![text(a), Item::Rect { size: Vec2::new(40.0, 6.0), color: [1.0; 4], radius: 0.0, border: 0.0, border_color: [0.0; 4] }],
        };
        let s = ov.measure(&stack, &mut used);
        assert!((s.y - (one.y + 4.0 + 6.0 + 4.0)).abs() < 1e-3, "{s:?}");
        assert!((s.x - one.x.max(40.0) - 4.0).abs() < 1e-3, "{s:?}");
    }
}
