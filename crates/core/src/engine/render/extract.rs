//! The render extract system: world -> [`RenderFrame`].

use glam::{Affine2, Mat4, Vec2};
use hecs::Entity;

use crate::engine::components::{Camera, GlobalTransform, MeshHandle, Sprite, TextureHandle, Transform, Visibility};
use crate::engine::schedule::Ctx;
use super::frame::{CameraView, MeshInstance, RenderFrame, RenderWarnings, SpriteInstance};
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
    frame.sprites.clear();
    frame.meshes.clear();
    frame.frame = cx.time.frame();
    frame.time = cx.time.elapsed();

    // ---- camera: the active one with the lowest entity id ----
    let mut chosen: Option<(Entity, Camera, Affine2)> = None;
    let mut active = 0usize;
    for (e, cam, g, t) in cx.world.query::<(Entity, &Camera, Option<&GlobalTransform>, Option<&Transform>)>().iter() {
        if cam.active == 0 {
            continue;
        }
        active += 1;
        if chosen.as_ref().is_none_or(|(c, _, _)| e.id() < c.id()) {
            chosen = Some((e, *cam, world_affine(g, t)));
        }
    }
    frame.ignored_cameras = active.saturating_sub(1);
    if active > 1 && !warnings.multiple_cameras {
        warnings.multiple_cameras = true;
        crate::global_warn(&format!(
            "{active} cameras are active; only the one with the lowest entity id is used (the rest are ignored)"
        ));
    }
    let viewport = cx.window.size;
    frame.camera = chosen.map(|(entity, cam, a)| {
        let g = GlobalTransform::from_affine(a);
        let (position, rotation) = (g.position(), g.rotation());
        let (view, projection) = camera_matrices(&cam, position, rotation, viewport);
        CameraView {
            entity,
            position,
            rotation,
            zoom: cam.zoom,
            kind: cam.kind,
            near: cam.near,
            far: cam.far,
            fov_y: cam.fov_y,
            clear_color: cam.clear_color,
            viewport,
            view: mat(view),
            projection: mat(projection),
            view_projection: mat(projection * view),
        }
    });

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
        let mut size = s.size;
        if size.x == 0.0 || size.y == 0.0 {
            let px = tex_size(s.texture).map_or(Vec2::ONE, |(w, h)| Vec2::new(w as f32, h as f32));
            if size.x == 0.0 {
                size.x = px.x;
            }
            if size.y == 0.0 {
                size.y = px.y;
            }
        }
        frame.sprites.push(SpriteInstance {
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
    frame.sprites.sort_by_key(|s| (s.layer, s.entity.id()));

    // ---- meshes ----
    for (e, m, g, t, v) in cx
        .world
        .query::<(Entity, &MeshHandle, Option<&GlobalTransform>, Option<&Transform>, Option<&Visibility>)>()
        .iter()
    {
        if v.is_some_and(|v| v.visible == 0) {
            continue;
        }
        frame.meshes.push(MeshInstance { entity: e, mesh: *m, world: lift(world_affine(g, t)) });
    }
    frame.meshes.sort_by_key(|m| m.entity.id());

    // ---- textures: drained, delivered once ----
    frame.textures = match cx.resources.get_mut::<Textures>() {
        Some(t) => t.drain_changes(),
        None => Default::default(),
    };

    cx.resources.insert(warnings);
    cx.resources.insert(frame);
}
