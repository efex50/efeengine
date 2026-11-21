//! The render extract system: world -> [`RenderFrame`].

use glam::{Affine2, Mat4, Vec2};
use hecs::Entity;

use crate::engine::components::{Camera, EditorOnly, GlobalTransform, MeshHandle, Sprite, TextureHandle, Transform, Visibility};
use crate::engine::schedule::Ctx;
use super::frame::{CameraView, MeshInstance, RenderFrame, RenderWarnings, SpriteInstance, WindowFrame};
use crate::engine::input::WindowId;
use super::textures::Textures;

fn mat(m: Mat4) -> [[f32; 4]; 4] {
    m.to_cols_array_2d()
}

fn world_affine(g: Option<&GlobalTransform>, t: Option<&Transform>) -> Affine2 {
    match (g, t) {
        (Some(g), _) => g.affine(),
        (None, Some(t)) => t.affine(),
        (None, None) => Affine2::IDENTITY,
    }
}

fn lift(a: Affine2) -> [[f32; 4]; 4] {
    mat(GlobalTransform::from_affine(a).to_mat4())
}

/// Orthographic, centred, y up; view-space z in [-far, -near] (looking down -z) maps to the
/// clip depth [0, 1].
fn orthographic(hw: f32, hh: f32, near: f32, far: f32) -> Mat4 {
    let d = if (far - near).abs() < 1e-6 { 1e-6 } else { far - near };
    Mat4::from_cols(
        glam::vec4(1.0 / hw, 0.0, 0.0, 0.0),
        glam::vec4(0.0, 1.0 / hh, 0.0, 0.0),
        glam::vec4(0.0, 0.0, -1.0 / d, 0.0),
        glam::vec4(0.0, 0.0, -near / d, 1.0),
    )
}

/// Right-handed perspective, clip depth [0, 1].
fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    Mat4::from_cols(
        glam::vec4(f / aspect, 0.0, 0.0, 0.0),
        glam::vec4(0.0, f, 0.0, 0.0),
        glam::vec4(0.0, 0.0, far / (near - far), -1.0),
        glam::vec4(0.0, 0.0, near * far / (near - far), 0.0),
    )
}

/// View and projection of `cam` at `position`/`rotation` for a `viewport` in pixels.
/// Public so tests and tools can reproduce what the extract builds.
pub fn camera_matrices(cam: &Camera, position: Vec2, rotation: f32, viewport: (u32, u32)) -> (Mat4, Mat4) {
    let (w, h) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    let view = Mat4::from_rotation_z(-rotation) * Mat4::from_translation((-position).extend(0.0));
    let projection = if cam.kind == Camera::PERSPECTIVE {
        let near = cam.near.max(1e-3);
        perspective(cam.fov_y.clamp(1e-3, 3.1), w / h, near, cam.far.max(near + 1e-3))
    } else {
        let zoom = if cam.zoom.is_finite() && cam.zoom > 0.0 { cam.zoom } else { 1.0 };
        let (hw, hh) = (w * 0.5 / zoom, h * 0.5 / zoom);
        orthographic(hw, hh, cam.near, cam.far)
    };
    (view, projection)
}

/// Build `RenderFrame` from the world and the texture store. Registered by the engine in
/// `Stage::RenderExtract`.
pub fn render_extract(cx: &mut Ctx<'_>) {
    let mut frame = cx.resources.remove::<RenderFrame>().unwrap_or_default();
    let mut warnings = cx.resources.remove::<RenderWarnings>().unwrap_or_default();
    frame.d2.sprites.clear();
    frame.d2.draw.clear();
    frame.d3.meshes.clear();
    frame.frame = cx.time.frame();
    frame.time = cx.time.elapsed();

    // ---- cameras: per window and kind, the active one with the lowest entity id ----
    // Index 0 = the main window, then the secondary windows in id order.
    let ids: Vec<WindowId> = cx.windows.iter().map(|(id, _)| id).collect();
    let mut chosen: Vec<[Option<(Entity, Camera, Affine2)>; 2]> = ids.iter().map(|_| [None, None]).collect();
    let mut active: Vec<[usize; 2]> = ids.iter().map(|_| [0, 0]).collect();
    // An `EditorOnly` camera (the in-game editor's) beats the game's cameras of its window.
    let mut editor_won: Vec<[bool; 2]> = ids.iter().map(|_| [false, false]).collect();
    for (e, cam, g, t, tool) in cx.world.query::<(Entity, &Camera, Option<&GlobalTransform>, Option<&Transform>, Option<&EditorOnly>)>().iter() {
        if cam.active == 0 {
            continue;
        }
        // A camera aimed at a window that does not exist draws nowhere.
        let Some(w) = ids.iter().position(|id| id.0 == cam.target) else { continue };
        let k = usize::from(cam.kind == Camera::PERSPECTIVE);
        active[w][k] += 1;
        let tool = tool.is_some();
        let better = chosen[w][k].as_ref().is_none_or(|(c, _, _)| (tool && !editor_won[w][k]) || (tool == editor_won[w][k] && e.id() < c.id()));
        if better {
            chosen[w][k] = Some((e, *cam, world_affine(g, t)));
            editor_won[w][k] = tool;
        }
    }
    for (w, a) in active.iter().enumerate() {
        for (k, n) in a.iter().enumerate() {
            if editor_won[w][k] {
                continue;
            }
            let flag = if k == 0 { &mut warnings.multiple_cameras } else { &mut warnings.multiple_cameras_3d };
            if *n > 1 && !*flag {
                *flag = true;
                crate::global_warn(&format!(
                    "{n} {} cameras are active for window {}; only the one with the lowest entity id is used (the rest are ignored)",
                    if k == 0 { "2D" } else { "3D" },
                    ids[w].0
                ));
            }
        }
    }
    let view_of = |viewport: (u32, u32), (entity, cam, a): (Entity, Camera, Affine2)| {
        let g = GlobalTransform::from_affine(a);
        let (position, rotation) = (g.position(), g.rotation());
        let (view, projection) = camera_matrices(&cam, position, rotation, viewport);
        CameraView {
            entity,
            position,
            rotation,
            zoom: cam.zoom,
            kind: cam.kind,
            target: cam.target,
            near: cam.near,
            far: cam.far,
            fov_y: cam.fov_y,
            clear_color: cam.clear_color,
            viewport,
            view: mat(view),
            projection: mat(projection),
            view_projection: mat(projection * view),
        }
    };
    let mut per_window: Vec<(Option<CameraView>, usize, Option<CameraView>, usize)> = Vec::with_capacity(ids.len());
    for (w, [c2, c3]) in chosen.into_iter().enumerate() {
        let viewport = cx.windows.get(ids[w]).map_or((0, 0), |s| s.size);
        per_window.push((c2.map(|c| view_of(viewport, c)), active[w][0].saturating_sub(1), c3.map(|c| view_of(viewport, c)), active[w][1].saturating_sub(1)));
    }
    {
        let (c2, i2, c3, i3) = per_window.remove(0);
        frame.d2.camera = c2;
        frame.d2.ignored_cameras = i2;
        frame.d3.camera = c3;
        frame.d3.ignored_cameras = i3;
    }
    // One entry per open secondary window, in id order (rebuilt every frame).
    frame.windows.clear();
    for (id, (c2, i2, c3, i3)) in ids.iter().skip(1).zip(per_window) {
        let mut wf = WindowFrame { window: id.0, ..Default::default() };
        wf.d2.camera = c2;
        wf.d2.ignored_cameras = i2;
        wf.d3.camera = c3;
        wf.d3.ignored_cameras = i3;
        frame.windows.push(wf);
    }

    // ---- sprites ----
    {
    let textures = cx.resources.get::<Textures>();
    let tex_size = |h: TextureHandle| -> Option<(u32, u32)> { textures.as_ref().and_then(|t| t.size(h)) };
    for (e, s, g, t, v) in cx
        .world
        .query::<(Entity, &Sprite, Option<&GlobalTransform>, Option<&Transform>, Option<&Visibility>)>()
        .iter()
    {
        if v.is_some_and(|v| v.visible == 0) {
            continue;
        }
        let size = if s.auto_size != 0 {
            tex_size(s.texture).map_or(Vec2::ONE, |(w, h)| Vec2::new(w as f32, h as f32))
        } else {
            s.size
        };
        frame.d2.sprites.push(SpriteInstance {
            entity: e,
            world: lift(world_affine(g, t)),
            size: size.to_array(),
            uv_min: s.uv_min.to_array(),
            uv_max: s.uv_max.to_array(),
            color: s.color,
            texture: s.texture,
            layer: s.layer,
            flip: s.flip,
        });
    }
    }
    frame.d2.sprites.sort_by_key(|s| (s.layer, s.entity.id()));
    for w in &mut frame.windows {
        if w.d2.camera.is_some() {
            w.d2.sprites.clone_from(&frame.d2.sprites);
        }
    }

    // ---- debug draw: queued shapes -> screen pixels (one frame of life) ----
    if let Some(dd) = cx.resources.get_mut::<super::DebugDraw2D>() {
        dd.flush_into(frame.d2.camera.as_ref(), &mut frame.d2.draw);
        for w in &mut frame.windows {
            w.d2.draw.clear();
            if w.d2.camera.is_some() {
                dd.flush_window_into(w.window, w.d2.camera.as_ref(), &mut w.d2.draw);
            }
        }
        dd.discard();
    }

    // ---- meshes ----
    for (e, m, g, t, v) in cx
        .world
        .query::<(Entity, &MeshHandle, Option<&GlobalTransform>, Option<&Transform>, Option<&Visibility>)>()
        .iter()
    {
        if v.is_some_and(|v| v.visible == 0) {
            continue;
        }
        frame.d3.meshes.push(MeshInstance { entity: e, mesh: *m, world: lift(world_affine(g, t)) });
    }
    frame.d3.meshes.sort_by_key(|m| m.entity.id());
    for w in &mut frame.windows {
        if w.d3.camera.is_some() {
            w.d3.meshes.clone_from(&frame.d3.meshes);
        }
    }

    // ---- textures: drained, delivered once ----
    frame.textures = match cx.resources.get_mut::<Textures>() {
        Some(t) => t.drain_changes(),
        None => Default::default(),
    };

    cx.resources.insert(warnings);
    cx.resources.insert(frame);
}
