//! Rendering data path, headless: camera selection, matrices, sprite gathering, transform
//! propagation, velocity, texture changes, component layouts. No window, no GPU.

use efeengine_core::glam::{Mat4, Vec4};
use efeengine_core::prelude::*;
use efeengine_core::render::{Image, RenderFrame, RenderWarnings, TextureError};

const DT: f64 = 1.0 / 60.0;

fn engine() -> Engine {
    Engine::with_config(EngineConfig { fixed_dt: DT, ..EngineConfig::headless() }).expect("headless engine")
}

fn frame_of(e: &mut Engine) -> RenderFrame {
    e.run_frames(2, DT);
    e.render_frame().clone()
}

fn m4(m: [[f32; 4]; 4]) -> Mat4 {
    Mat4::from_cols_array_2d(&m)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// ---- camera ----

#[test]
fn no_camera_gives_none() {
    let mut e = engine();
    let f = frame_of(&mut e);
    assert!(f.d2.camera.is_none());
    assert_eq!(f.d2.ignored_cameras, 0);
}

#[test]
fn one_camera_is_used() {
    let mut e = engine();
    let cam = e.world_mut().spawn((Camera { clear_color: [1.0, 0.5, 0.25, 1.0], ..Camera::default() }, Transform::from_position(Vec2::new(3.0, 4.0))));
    let f = frame_of(&mut e);
    let c = f.d2.camera.expect("camera");
    assert_eq!(c.entity, cam);
    assert_eq!(c.position, Vec2::new(3.0, 4.0));
    assert_eq!(c.clear_color, [1.0, 0.5, 0.25, 1.0]);
    assert_eq!(f.d2.ignored_cameras, 0);
    assert!(!e.resources().get::<RenderWarnings>().unwrap().multiple_cameras);
}

#[test]
fn inactive_cameras_are_ignored() {
    let mut e = engine();
    e.world_mut().spawn((Camera { active: 0, ..Camera::default() }, Transform::default()));
    let f = frame_of(&mut e);
    assert!(f.d2.camera.is_none());
    let on = e.world_mut().spawn((Camera::default(), Transform::default()));
    // spawned after the inactive one but the only active one
    assert_eq!(frame_of(&mut e).d2.camera.unwrap().entity, on);
}

#[test]
fn two_active_cameras_use_the_lowest_id_and_warn_once() {
    let mut e = engine();
    let first = e.world_mut().spawn((Camera::default(), Transform::from_position(Vec2::new(1.0, 0.0))));
    let _second = e.world_mut().spawn((Camera::default(), Transform::from_position(Vec2::new(2.0, 0.0))));
    let f = frame_of(&mut e);
    assert_eq!(f.d2.camera.as_ref().unwrap().entity, first);
    assert_eq!(f.d2.camera.unwrap().position.x, 1.0);
    assert_eq!(f.d2.ignored_cameras, 1);
    assert!(e.resources().get::<RenderWarnings>().unwrap().multiple_cameras, "the warning was latched");
    // still one camera per frame afterwards
    assert_eq!(frame_of(&mut e).d2.ignored_cameras, 1);
}

#[test]
fn orthographic_view_and_projection() {
    let mut e = engine();
    e.world_mut().spawn((Camera { zoom: 2.0, ..Camera::default() }, Transform { position: Vec2::new(100.0, 50.0), rotation: 0.0, scale: Vec2::ONE }));
    let c = frame_of(&mut e).d2.camera.unwrap();
    let (w, h) = c.viewport;
    assert_eq!((w, h), (800, 600), "headless window size");
    let (view, proj, vp) = (m4(c.view), m4(c.projection), m4(c.view_projection));
    // the camera position is the centre of the screen
    let centre = vp * Vec4::new(100.0, 50.0, 0.0, 1.0);
    assert!(close(centre.x, 0.0) && close(centre.y, 0.0) && centre.w == 1.0, "{centre:?}");
    // half the viewport / zoom to the right and up reaches the clip edges (y up)
    let right = vp * Vec4::new(100.0 + w as f32 / 4.0, 50.0, 0.0, 1.0);
    let up = vp * Vec4::new(100.0, 50.0 + h as f32 / 4.0, 0.0, 1.0);
    assert!(close(right.x, 1.0) && close(up.y, 1.0), "{right:?} {up:?}");
    // z = 0 is inside the depth range [0, 1]
    assert!(centre.z > 0.0 && centre.z < 1.0, "{centre:?}");
    // view is a pure translation here; vp = projection * view
    assert_eq!(view * Vec4::new(100.0, 50.0, 0.0, 1.0), Vec4::new(0.0, 0.0, 0.0, 1.0));
    assert!((vp - proj * view).abs_diff_eq(Mat4::ZERO, 1e-5));
    // column-major: the translation of `view` is in the last column
    assert_eq!(c.view[3], [-100.0, -50.0, 0.0, 1.0]);
}

#[test]
fn rotated_camera_view() {
    let mut e = engine();
    e.world_mut().spawn((Camera::default(), Transform { position: Vec2::ZERO, rotation: std::f32::consts::FRAC_PI_2, scale: Vec2::ONE }));
    let c = frame_of(&mut e).d2.camera.unwrap();
    // a camera turned 90 degrees counter-clockwise sees world +y along its own +x
    let p = m4(c.view) * Vec4::new(0.0, 10.0, 0.0, 1.0);
    assert!(close(p.x, 10.0) && close(p.y, 0.0), "{p:?}");
}

// ---- 2D / 3D split ----

#[test]
fn cameras_are_chosen_per_kind() {
    let mut e = engine();
    let c2 = e.world_mut().spawn((Transform::default(), Camera::orthographic_2d()));
    // a camera aimed at a window that does not exist is not a candidate
    e.world_mut().spawn((Transform::default(), Camera { target: 7, ..Camera::perspective_3d() }));
    let c3 = e.world_mut().spawn((Transform::default(), Camera::perspective_3d()));
    e.world_mut().spawn((Transform::default(), Camera::perspective_3d()));
    let f = frame_of(&mut e);
    assert_eq!(f.d2.camera.as_ref().unwrap().entity, c2);
    assert_eq!(f.d2.ignored_cameras, 0);
    let c = f.d3.camera.as_ref().unwrap();
    assert_eq!((c.entity, c.target, c.kind), (c3, 0, Camera::PERSPECTIVE));
    assert_eq!(f.d3.ignored_cameras, 1);
}

// ---- sprites ----

#[test]
fn sprites_are_gathered_sorted_and_filtered() {
    let mut e = engine();
    let tex = e.textures_mut().add(Image::solid(16, 8, [255; 4]));
    let high = e.world_mut().spawn((Transform::default(), Sprite { layer: 5, ..Sprite::new(tex) }));
    let low_a = e.world_mut().spawn((Transform::default(), Sprite { layer: -1, ..Sprite::default() }));
    let low_b = e.world_mut().spawn((Transform::default(), Sprite { layer: -1, ..Sprite::default() }.with_size(Vec2::new(3.0, 4.0))));
    let mid = e.world_mut().spawn((Transform::from_position(Vec2::new(7.0, 9.0)), Sprite { layer: 2, ..Sprite::new(tex) }, Visibility::VISIBLE));
    let hidden = e.world_mut().spawn((Transform::default(), Sprite::default(), Visibility::HIDDEN));
    let f = frame_of(&mut e);
    let order: Vec<_> = f.d2.sprites.iter().map(|s| s.entity).collect();
    assert_eq!(order, [low_a, low_b, mid, high], "by layer, ties by entity id");
    assert!(!order.contains(&hidden));
    // auto size -> the texture's pixels, or 1x1 without a texture
    assert_eq!(f.d2.sprites[0].size, [1.0, 1.0]);
    assert_eq!(f.d2.sprites[1].size, [3.0, 4.0]);
    assert_eq!(f.d2.sprites[2].size, [16.0, 8.0]);
    assert_eq!(f.d2.sprites[2].texture, tex);
    // world matrix, column-major: translation in the last column
    assert_eq!(f.d2.sprites[2].world[3], [7.0, 9.0, 0.0, 1.0]);
    assert_eq!((f.d2.sprites[0].uv_min, f.d2.sprites[0].uv_max), ([0.0, 0.0], [1.0, 1.0]));
}

#[test]
fn sprite_size_zero_is_literal_unless_auto_size() {
    let mut e = engine();
    let tex = e.textures_mut().add(Image::solid(16, 8, [255; 4]));
    let zero = e.world_mut().spawn((Transform::default(), Sprite { layer: 0, ..Sprite::new(tex) }.with_size(Vec2::ZERO)));
    let auto = e.world_mut().spawn((Transform::default(), Sprite { layer: 1, size: Vec2::ZERO, ..Sprite::new(tex) }));
    let f = frame_of(&mut e);
    let size_of_entity = |en| f.d2.sprites.iter().find(|s| s.entity == en).unwrap().size;
    assert_eq!(size_of_entity(zero), [0.0, 0.0]);
    assert_eq!(size_of_entity(auto), [16.0, 8.0]);
}

#[test]
fn sprite_uses_global_transform_and_meshes_are_listed() {
    let mut e = engine();
    let parent = e.world_mut().spawn((Transform::from_position(Vec2::new(10.0, 0.0)),));
    let kid = e.world_mut().spawn((Transform::from_position(Vec2::new(0.0, 5.0)), Parent::new(parent), Sprite::default(), MeshHandle { id: 3 }));
    let f = frame_of(&mut e);
    assert_eq!(f.d2.sprites[0].entity, kid);
    assert_eq!(f.d2.sprites[0].world[3], [10.0, 5.0, 0.0, 1.0]);
    assert_eq!(f.d3.meshes.len(), 1);
    assert_eq!(f.d3.meshes[0].mesh.id, 3);
    assert_eq!(f.d3.meshes[0].world[3], [10.0, 5.0, 0.0, 1.0]);
}

// ---- hierarchy ----

#[test]
fn parent_propagation_and_children() {
    let mut e = engine();
    let root = e.world_mut().spawn((Transform { position: Vec2::new(10.0, 0.0), rotation: std::f32::consts::FRAC_PI_2, scale: Vec2::splat(2.0) },));
    let kid = e.world_mut().spawn((Transform::from_position(Vec2::new(5.0, 0.0)), Parent::new(root)));
    let grandkid = e.world_mut().spawn((Transform::from_position(Vec2::new(1.0, 0.0)), Parent::new(kid)));
    e.run_frames(2, DT);
    let g = |en| *e.world().get::<&GlobalTransform>(en).unwrap();
    // root: scale 2, rotated 90 deg: its local x axis points along world +y with length 2
    assert!(close(g(root).translation().x, 10.0));
    // kid at local (5,0) -> world (10, 10)
    let k = g(kid);
    assert!(close(k.translation().x, 10.0) && close(k.translation().y, 10.0), "{k:?}");
    // grandkid at local (1,0) of kid (scale 2, rotated) -> kid + (0, 2)
    let gk = g(grandkid);
    assert!(close(gk.translation().x, 10.0) && close(gk.translation().y, 12.0), "{gk:?}");
    assert!(close(gk.rotation(), std::f32::consts::FRAC_PI_2));
    assert_eq!(e.world().get::<&Children>(root).unwrap().entities(), &[kid]);
    assert_eq!(e.world().get::<&Children>(kid).unwrap().entities(), &[grandkid]);
    assert!(e.world().get::<&Children>(grandkid).is_err());
    // re-parenting updates Children
    e.world_mut().insert_one(grandkid, Parent::new(root)).unwrap();
    e.run_frames(1, DT);
    assert_eq!(e.world().get::<&Children>(root).unwrap().entities(), &[kid, grandkid]);
    assert!(e.world().get::<&Children>(kid).is_err(), "emptied Children is removed");
}

#[test]
fn stale_parent_makes_a_root() {
    let mut e = engine();
    let parent = e.world_mut().spawn((Transform::from_position(Vec2::new(100.0, 0.0)),));
    let kid = e.world_mut().spawn((Transform::from_position(Vec2::new(1.0, 2.0)), Parent::new(parent)));
    e.run_frames(2, DT);
    assert!(close(e.world().get::<&GlobalTransform>(kid).unwrap().translation().x, 101.0));
    e.world_mut().despawn(parent).unwrap();
    e.run_frames(1, DT);
    let g = *e.world().get::<&GlobalTransform>(kid).unwrap();
    assert_eq!(g.translation(), Vec2::new(1.0, 2.0));
    // a forged / zero parent is a root too
    let forged = e.world_mut().spawn((Transform::from_position(Vec2::new(4.0, 0.0)), Parent { entity: 0 }));
    let forged2 = e.world_mut().spawn((Transform::from_position(Vec2::new(5.0, 0.0)), Parent { entity: u64::MAX }));
    e.run_frames(1, DT);
    assert_eq!(e.world().get::<&GlobalTransform>(forged).unwrap().translation().x, 4.0);
    assert_eq!(e.world().get::<&GlobalTransform>(forged2).unwrap().translation().x, 5.0);
}

#[test]
fn parent_cycles_are_broken_and_warned_once() {
    let mut e = engine();
    let a = e.world_mut().spawn((Transform::from_position(Vec2::new(1.0, 0.0)),));
    let b = e.world_mut().spawn((Transform::from_position(Vec2::new(10.0, 0.0)), Parent::new(a)));
    e.world_mut().insert_one(a, Parent::new(b)).unwrap();
    let me = e.world_mut().spawn((Transform::from_position(Vec2::new(7.0, 0.0)),));
    e.world_mut().insert_one(me, Parent::new(me)).unwrap();
    assert!(!e.resources().get::<RenderWarnings>().unwrap().hierarchy_cycle);
    e.run_frames(3, DT); // must terminate
    assert!(e.resources().get::<RenderWarnings>().unwrap().hierarchy_cycle);
    let ga = e.world().get::<&GlobalTransform>(a).unwrap().translation().x;
    let gb = e.world().get::<&GlobalTransform>(b).unwrap().translation().x;
    // a is the lowest id: the walk breaks the cycle at a, so a is a root and b hangs below it
    assert_eq!((ga, gb), (1.0, 11.0));
    assert_eq!(e.world().get::<&GlobalTransform>(me).unwrap().translation().x, 7.0);
    assert!(e.world().get::<&Children>(me).is_err(), "a self-parent has no children");
}

// ---- velocity ----

#[test]
fn velocity_integrates_with_the_fixed_delta() {
    let mut e = engine();
    let en = e.world_mut().spawn((Transform::default(), Velocity { linear: Vec2::new(60.0, -30.0), angular: 3.0 }));
    let still = e.world_mut().spawn((Transform::from_position(Vec2::new(1.0, 1.0)),));
    // N frames at dt == fixed_dt give N-1 fixed steps
    e.run_frames(61, DT);
    let t = *e.world().get::<&Transform>(en).unwrap();
    assert!((t.position.x - 60.0).abs() < 0.05, "{t:?}");
    assert!((t.position.y + 30.0).abs() < 0.05, "{t:?}");
    assert!((t.rotation - 3.0).abs() < 0.01, "{t:?}");
    assert_eq!(e.world().get::<&Transform>(still).unwrap().position, Vec2::new(1.0, 1.0));
}

// ---- textures ----

#[test]
fn texture_changes_are_delivered_exactly_once() {
    let mut e = engine();
    let a = e.textures_mut().add(Image::solid(2, 2, [1, 2, 3, 4]));
    let b = e.textures_mut().add(Image::solid(1, 1, [9; 4]));
    let temp = e.textures_mut().add(Image::solid(1, 1, [0; 4]));
    assert!(e.textures_mut().remove(temp)); // added and removed before a frame: never seen
    e.run_frames(1, DT);
    let t = e.render_frame().textures.clone();
    assert_eq!(t.added.iter().map(|(h, _)| *h).collect::<Vec<_>>(), [a, b]);
    assert_eq!(t.added[0].1.rgba8(), &[1, 2, 3, 4].repeat(4)[..]);
    assert!(t.updated.is_empty() && t.removed.is_empty());
    // the next frame has nothing
    e.run_frames(1, DT);
    assert!(e.render_frame().textures.is_empty());
    // replace (new size) and remove
    e.textures_mut().replace(a, Image::solid(4, 4, [5; 4])).unwrap();
    e.textures_mut().replace(a, Image::solid(8, 8, [6; 4])).unwrap();
    e.textures_mut().remove(b);
    e.run_frames(1, DT);
    let t = e.render_frame().textures.clone();
    assert!(t.added.is_empty());
    assert_eq!(t.updated.len(), 1, "two replaces are one update with the latest image");
    assert_eq!(t.updated[0].1.size(), (8, 8));
    assert_eq!(t.removed, [b]);
    e.run_frames(1, DT);
    assert!(e.render_frame().textures.is_empty());
    // store queries
    assert_eq!(e.textures().size(a), Some((8, 8)));
    assert!(e.textures().get(b).is_none());
    assert!(e.textures_mut().replace(b, Image::solid(1, 1, [0; 4])).is_err());
    assert!(!e.textures_mut().remove(b));
}

#[test]
fn image_length_is_checked() {
    assert!(Image::new(2, 2, vec![0; 16]).is_ok());
    assert!(matches!(Image::new(2, 2, vec![0; 15]), Err(TextureError::WrongLength { expected: 16, got: 15, .. })));
    assert!(matches!(Image::new(0, 2, vec![]), Err(TextureError::Empty(0, 2))));
}

// ---- registration ----

#[test]
fn rendering_components_are_registered_and_indexed() {
    let e = engine();
    let idx = e.api_index();
    let check = |name: &str, size: usize| {
        let d = idx.struct_type(name).unwrap_or_else(|| panic!("{name} is not in the API index"));
        assert_eq!(d.size as usize, size, "{name}");
        d.validate().unwrap();
        assert!(e.types().by_name(name).is_some(), "{name} is not in the type registry");
    };
    check("Camera", size_of::<Camera>());
    check("Sprite", size_of::<Sprite>());
    check("TextureHandle", 4);
    check("Visibility", 4);
    check("Parent", 8);
    check("GlobalTransform", 24);
    assert_eq!(size_of::<Camera>(), 44);
    assert_eq!(size_of::<Sprite>(), 56);
    let cam = idx.struct_type("Camera").unwrap();
    let names: Vec<_> = cam.fields.iter().map(|f| (f.name.as_str(), f.offset)).collect();
    assert_eq!(names, [("kind", 0), ("zoom", 4), ("near", 8), ("far", 12), ("fov_y", 16), ("clear_color", 20), ("active", 36), ("target", 40)]);
    let parent = idx.struct_type("Parent").unwrap();
    assert_eq!(parent.fields[0].ty, efeengine_core::efevm::Prim::Entity);
}

// ---- 2D debug draw ----

#[test]
fn debug_draw_reaches_the_frame_for_one_frame() {
    use efeengine_core::render::DebugDraw2D;
    let mut e = engine();
    e.world_mut().spawn((Camera::default(), Transform::default()));
    e.world_mut().spawn((Sprite { size: Vec2::new(10.0, 10.0), ..Sprite::default() }, Transform::from_position(Vec2::new(4.0, 0.0))));
    {
        let dd = e.resources_mut().get_mut::<DebugDraw2D>().expect("DebugDraw2D resource");
        // a bounding box around the sprite (world) and a screen-space marker
        dd.rect(Vec2::new(-1.0, -5.0), Vec2::new(9.0, 5.0), DebugDraw2D::YELLOW);
        dd.screen_rect(Vec2::new(1.0, 1.0), Vec2::new(5.0, 5.0), DebugDraw2D::RED);
    }
    e.run_frames(1, DT);
    let f = e.render_frame().clone();
    assert_eq!(f.d2.sprites.len(), 1);
    assert!(!f.d2.draw.is_empty());
    assert_eq!(f.d2.draw.commands().len(), 1);
    assert!(e.resources().get::<DebugDraw2D>().unwrap().is_empty());
    // not repeated: shapes live one frame
    e.run_frames(1, DT);
    assert!(e.render_frame().d2.draw.is_empty());
}
