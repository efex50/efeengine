//! Gizmos (docs/EDITOR.md E7.2): move, rotate and scale handles for the selected entity, drawn by
//! the game itself through the engine's `DebugDraw2D` (rendering stays in the game process).
//!
//! The editor only sends `select` / `clear_selection`; the link stores the result in the
//! [`Selection`] resource, and one call per frame ([`draw_selection`]) pushes the handles. They
//! are drawn in edit mode only. There is no mouse dragging in the game window (that needs input
//! ownership, which EDITOR.md defers): the editor changes the `Transform` with `set_field`, and
//! the handles follow it.
//!
//! Handles are sized in screen pixels: their world length is `HANDLE_PIXELS / camera zoom`
//! (zoom is pixels per world unit), so they stay readable at any zoom.

use efeengine_core::engine::resources::Resources;
use efeengine_core::prelude::*;

use crate::protocol::GizmoMode;

/// Length of an axis handle on screen, in pixels.
pub const HANDLE_PIXELS: f32 = 80.0;
const LINE_PIXELS: f32 = 3.0;
const RED: [f32; 4] = [1.0, 0.1, 0.1, 1.0];
const GREEN: [f32; 4] = [0.1, 1.0, 0.1, 1.0];
const YELLOW: [f32; 4] = [1.0, 0.9, 0.1, 1.0];

/// The entity the editor selected, and the handles to draw over it (a resource, always present
/// once the link is on; `entity` is `None` until the editor selects something).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Selection {
    pub entity: Option<Entity>,
    pub mode: GizmoMode,
}

/// One primitive of a gizmo, in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Part {
    Line { a: Vec2, b: Vec2, color: [f32; 4] },
    Circle { center: Vec2, radius: f32, color: [f32; 4] },
    FilledRect { min: Vec2, max: Vec2, color: [f32; 4] },
}

/// The primitives of the gizmo for `mode` at `pos`, where `unit` is the handle length in world
/// units. Move draws arrows along the world axes; Rotate a ring plus a tick on the current
/// rotation; Scale a line with a square along each local axis. Pure, so it is unit-tested.
pub fn parts(mode: GizmoMode, pos: Vec2, rotation: f32, unit: f32) -> Vec<Part> {
    let (s, c) = rotation.sin_cos();
    let (lx, ly) = (Vec2::new(c, s), Vec2::new(-s, c));
    match mode {
        GizmoMode::Move => {
            let mut out = Vec::new();
            for (dir, color) in [(Vec2::X, RED), (Vec2::Y, GREEN)] {
                let tip = pos + dir * unit;
                let side = Vec2::new(-dir.y, dir.x);
                out.push(Part::Line { a: pos, b: tip, color });
                out.push(Part::Line { a: tip, b: tip - dir * unit * 0.2 + side * unit * 0.1, color });
                out.push(Part::Line { a: tip, b: tip - dir * unit * 0.2 - side * unit * 0.1, color });
            }
            out
        }
        GizmoMode::Rotate => vec![
            Part::Circle { center: pos, radius: unit, color: YELLOW },
            Part::Line { a: pos, b: pos + lx * unit, color: RED },
        ],
        GizmoMode::Scale => {
            let half = unit * 0.07;
            let mut out = Vec::new();
            for (dir, color) in [(lx, RED), (ly, GREEN)] {
                let tip = pos + dir * unit;
                out.push(Part::Line { a: pos, b: tip, color });
                out.push(Part::FilledRect { min: tip - Vec2::splat(half), max: tip + Vec2::splat(half), color });
            }
            out
        }
    }
}

/// Push the selected entity's gizmo into `DebugDraw2D`. Does nothing without a selection, when
/// the entity is gone or has no `Transform`.
pub fn draw_selection(world: &World, resources: &mut Resources) {
    let Some(sel) = resources.get::<Selection>().map(|s| *s) else { return };
    let Some(entity) = sel.entity.filter(|e| world.contains(*e)) else { return };
    if world.get::<&Transform>(entity).is_err() {
        return;
    }
    // The world pose when the engine has computed it (parents), else the local `Transform`.
    let (pos, rotation) = match world.get::<&GlobalTransform>(entity) {
        Ok(g) => (g.translation(), g.x_axis().y.atan2(g.x_axis().x)),
        Err(_) => match world.get::<&Transform>(entity) {
            Ok(t) => (t.position, t.rotation),
            Err(_) => return,
        },
    };
    let zoom = world
        .query::<&Camera>()
        .iter()
        .find(|c| c.kind == Camera::ORTHOGRAPHIC && c.active != 0)
        .map_or(1.0, |c| c.zoom)
        .max(1e-6);
    let unit = HANDLE_PIXELS / zoom;
    let Some(draw) = resources.get_mut::<DebugDraw2D>() else { return };
    draw.set_width(LINE_PIXELS);
    for p in parts(sel.mode, pos, rotation, unit) {
        match p {
            Part::Line { a, b, color } => draw.line(a, b, color),
            Part::Circle { center, radius, color } => draw.circle(center, radius, color),
            Part::FilledRect { min, max, color } => draw.rect_filled(min, max, color),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(p: &[Part]) -> usize {
        p.iter().filter(|x| matches!(x, Part::Line { .. })).count()
    }

    #[test]
    fn each_mode_draws_its_own_handles() {
        let mv = parts(GizmoMode::Move, Vec2::new(1.0, 2.0), 0.0, 10.0);
        assert_eq!(mv.len(), 6);
        assert_eq!(lines(&mv), 6);
        assert!(matches!(mv[0], Part::Line { a, b, .. } if a == Vec2::new(1.0, 2.0) && b == Vec2::new(11.0, 2.0)));
        let rot = parts(GizmoMode::Rotate, Vec2::ZERO, 0.0, 10.0);
        assert!(rot.iter().any(|p| matches!(p, Part::Circle { radius, .. } if *radius == 10.0)));
        let sc = parts(GizmoMode::Scale, Vec2::ZERO, 0.0, 10.0);
        assert_eq!(sc.len(), 4);
        assert!(sc.iter().any(|p| matches!(p, Part::FilledRect { .. })));
    }

    #[test]
    fn rotation_turns_local_axes_but_not_move_arrows() {
        let q = std::f32::consts::FRAC_PI_2;
        let mv = parts(GizmoMode::Move, Vec2::ZERO, q, 10.0);
        assert!(matches!(mv[0], Part::Line { b, .. } if (b - Vec2::new(10.0, 0.0)).length() < 1e-4));
        let sc = parts(GizmoMode::Scale, Vec2::ZERO, q, 10.0);
        assert!(matches!(sc[0], Part::Line { b, .. } if (b - Vec2::new(0.0, 10.0)).length() < 1e-4));
    }

    fn setup(mode: GizmoMode, with_transform: bool) -> (World, Resources) {
        let mut world = World::new();
        let e = if with_transform { world.spawn((Transform::from_position(Vec2::new(3.0, 4.0)),)) } else { world.spawn(()) };
        let mut res = Resources::new();
        res.insert(DebugDraw2D::new());
        res.insert(Selection { entity: Some(e), mode });
        (world, res)
    }

    #[test]
    fn draws_only_for_a_live_entity_with_a_transform() {
        let (world, mut res) = setup(GizmoMode::Move, true);
        draw_selection(&world, &mut res);
        assert_eq!(res.get::<DebugDraw2D>().unwrap().len(), 6);

        let (world, mut res) = setup(GizmoMode::Scale, false);
        draw_selection(&world, &mut res);
        assert!(res.get::<DebugDraw2D>().unwrap().is_empty(), "no Transform, no handles");

        let (mut world, mut res) = setup(GizmoMode::Move, true);
        let e = res.get::<Selection>().unwrap().entity.unwrap();
        world.despawn(e).unwrap();
        draw_selection(&world, &mut res);
        assert!(res.get::<DebugDraw2D>().unwrap().is_empty(), "despawned entity");

        let mut res = Resources::new();
        res.insert(DebugDraw2D::new());
        draw_selection(&World::new(), &mut res);
        assert!(res.get::<DebugDraw2D>().unwrap().is_empty(), "no selection");
    }
}
