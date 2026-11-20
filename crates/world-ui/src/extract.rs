//! The per-frame system. It advances bar and floating animations, despawns finished labels, and
//! copies what the overlay needs into a [`Snapshot`]: owned data, no world borrows, so drawing
//! does not touch the ECS.

use std::collections::HashSet;

use efeengine_core::prelude::*;

use crate::components::{Anchor, Anchor2D, Bar2D, Color, Container2D, Floating2D, Rect2D, Sprite2D, Text2D};

/// A camera the overlay projects through: the active 2D camera of one window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamView {
    pub position: Vec2,
    pub rotation: f32,
    /// Logical pixels per world unit.
    pub zoom: f32,
}

/// One element with its colours and sizes copied out of the world.
#[derive(Clone, Debug)]
pub enum Item {
    Text { key: Entity, text: String, size: f32, color: Color, bold: bool },
    Rect { size: Vec2, color: Color, radius: f32, border: f32, border_color: Color },
    Bar { size: Vec2, shown: f32, ghost: f32, back: Color, fill: Color, ghost_color: Color, vertical: bool },
    Sprite { texture: TextureHandle, size: Vec2, tint: Color },
    Container { vertical: bool, gap: f32, padding: f32, background: Option<Color>, children: Vec<Item> },
}

/// A root element: where it is anchored (already a world point), its offset and its fade.
#[derive(Clone, Debug)]
pub struct Root {
    pub world: Vec2,
    pub offset: Vec2,
    /// 1 unless the element is floating and fading out.
    pub alpha: f32,
    pub item: Item,
}

/// Everything one frame of the overlay needs.
#[derive(Default, Debug)]
pub struct Snapshot {
    /// One entry per window that has an active 2D camera.
    pub cameras: Vec<(u32, CamView)>,
    pub roots: Vec<Root>,
}

/// Moves `value` toward `target` by at most `step`.
pub fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

/// Advances bars and floating labels by `dt` seconds, despawns expired labels, and refreshes the
/// snapshot. Runs in `PostUpdate`, after the game has moved things.
pub fn run(cx: &mut Ctx<'_>, snap: &mut Snapshot) {
    let dt = cx.time.delta().max(0.0);

    for bar in cx.world.query_mut::<&mut Bar2D>() {
        bar.shown = approach(bar.shown, bar.fraction, bar.speed * dt);
        if bar.ghost < bar.shown {
            bar.ghost = bar.shown;
        } else {
            bar.ghost = approach(bar.ghost, bar.shown, bar.speed * 0.5 * dt);
        }
    }

    let mut expired = Vec::new();
    for (e, anchor, float) in cx.world.query_mut::<(Entity, &mut Anchor2D, &mut Floating2D)>() {
        float.age += dt;
        anchor.offset.y -= float.rise * dt;
        if float.age >= float.life {
            expired.push(e);
        }
    }
    for e in expired {
        cx.commands.despawn(e);
    }

    snap.cameras = active_cameras(cx.world);

    // Children of a container are drawn by it, never as roots of their own.
    let mut nested: HashSet<Entity> = HashSet::new();
    for container in cx.world.query::<&Container2D>().iter() {
        nested.extend(container.children.iter().copied());
    }

    snap.roots.clear();
    let roots: Vec<(Entity, Anchor2D)> = cx.world.query::<(Entity, &Anchor2D)>().iter().map(|(e, a)| (e, *a)).collect();
    for (e, anchor) in roots {
        if nested.contains(&e) {
            continue;
        }
        let world_pos = match anchor.target {
            Anchor::Point(p) => p,
            Anchor::Entity(t) => match pose(cx.world, t) {
                Some((p, _)) => p,
                None => continue,
            },
        };
        let alpha = match cx.world.get::<&Floating2D>(e) {
            Ok(f) => (1.0 - f.age / f.life).clamp(0.0, 1.0),
            Err(_) => 1.0,
        };
        let Some(item) = item_of(cx.world, e, 0) else { continue };
        snap.roots.push(Root { world: world_pos, offset: anchor.offset, alpha, item });
    }
}

/// World position and rotation of an entity: the global transform when there is one, else the
/// local `Transform`.
pub fn pose(world: &World, e: Entity) -> Option<(Vec2, f32)> {
    if let Ok(g) = world.get::<&GlobalTransform>(e) {
        return Some((g.position(), g.rotation()));
    }
    world.get::<&Transform>(e).ok().map(|t| (t.position, t.rotation))
}

/// The active 2D camera of each window: among the cameras with `kind` 0 and `active` set that
/// target that window, the one with the lowest entity id.
fn active_cameras(world: &World) -> Vec<(u32, CamView)> {
    let mut best: Vec<(u32, u32, CamView)> = Vec::new();
    for (e, cam) in world.query::<(Entity, &Camera)>().iter() {
        if cam.kind != Camera::ORTHOGRAPHIC || cam.active == 0 {
            continue;
        }
        let Some((position, rotation)) = pose(world, e) else { continue };
        let view = CamView { position, rotation, zoom: cam.zoom };
        match best.iter_mut().find(|(w, _, _)| *w == cam.target) {
            Some(slot) if e.id() < slot.1 => *slot = (cam.target, e.id(), view),
            Some(_) => {}
            None => best.push((cam.target, e.id(), view)),
        }
    }
    best.into_iter().map(|(w, _, v)| (w, v)).collect()
}

/// Copies an entity's element into an [`Item`]. A container copies its children too; `depth`
/// stops a cycle of containers.
fn item_of(world: &World, e: Entity, depth: u32) -> Option<Item> {
    if depth > 16 {
        return None;
    }
    if let Ok(t) = world.get::<&Text2D>(e) {
        return Some(Item::Text { key: e, text: t.text.clone(), size: t.size, color: t.color, bold: t.bold });
    }
    if let Ok(r) = world.get::<&Rect2D>(e) {
        return Some(Item::Rect { size: r.size, color: r.color, radius: r.radius, border: r.border, border_color: r.border_color });
    }
    if let Ok(b) = world.get::<&Bar2D>(e) {
        return Some(Item::Bar {
            size: b.size,
            shown: b.shown,
            ghost: b.ghost,
            back: b.back,
            fill: b.fill,
            ghost_color: b.ghost_color,
            vertical: b.vertical,
        });
    }
    if let Ok(s) = world.get::<&Sprite2D>(e) {
        return Some(Item::Sprite { texture: s.texture, size: s.size, tint: s.tint });
    }
    if let Ok(c) = world.get::<&Container2D>(e) {
        let children = c.children.iter().filter_map(|&k| item_of(world, k, depth + 1)).collect();
        return Some(Item::Container { vertical: c.vertical, gap: c.gap, padding: c.padding, background: c.background, children });
    }
    None
}
