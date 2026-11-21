//! Viewport tools (docs/IN_GAME_EDITOR.md, step 3): the editor camera, picking, gizmos and the grid.
//!
//! The camera is an entity with `Camera`, `Transform` and `EditorOnly` ([`EditorCamera`]): scene
//! capture, snapshots and the hierarchy never see it, and the render extract prefers it over the
//! game's cameras of its window. It renders the main window in Edit and Paused mode (layout A) or
//! the editor window (layout B, `separate_window`). Everything interactive happens only while it
//! is the camera that draws a window and the pointer is in that window and not over UI
//! (`InputState::game_input`).
//!
//! The pure functions at the top (camera math, hit tests, drag math) have no engine in them, so
//! the tests can check them directly.

use efeengine_core::input::{InputEventKind, WindowEvent, WindowId};
use efeengine_core::math::Affine2;
use efeengine_core::prelude::*;
use efeengine_core::window::WindowDesc;

use super::edit;
use super::{EditorMode, EditorSelection, EditorState, EditorUi, EditorView, GizmoTool};

/// Zoom limits (pixels per world unit) of the editor camera.
pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 64.0;
/// Zoom factor per wheel line.
pub const WHEEL_ZOOM: f32 = 1.1;
/// Gizmo sizes in screen pixels (they do not scale with the zoom).
pub const ARROW_PX: f32 = 72.0;
pub const CENTER_PX: f32 = 9.0;
pub const RING_PX: f32 = 64.0;
pub const HIT_PX: f32 = 8.0;
pub const BOX_PX: f32 = 40.0;
/// Pick radius of the handle of an entity without a sprite, in pixels.
pub const HANDLE_PX: f32 = 9.0;
/// Rotation step while Ctrl is held (radians).
pub const ROTATE_SNAP: f32 = std::f32::consts::PI / 12.0;
/// Scale step while Ctrl is held.
pub const SCALE_SNAP: f32 = 0.25;
/// The grid is hidden when its lines would be closer than this (pixels); it doubles its step until they are not.
const GRID_MIN_PX: f32 = 8.0;

// ---------------------------------------------------------------------------------------------
// camera math (the editor camera never rotates)

/// Screen pixel (origin top-left, y down) to world.
pub fn screen_to_world(cam: Vec2, zoom: f32, viewport: (u32, u32), p: Vec2) -> Vec2 {
    let (w, h) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    Vec2::new(cam.x + (p.x - w * 0.5) / zoom, cam.y + (h * 0.5 - p.y) / zoom)
}

/// World to screen pixel (inverse of [`screen_to_world`]).
pub fn world_to_screen(cam: Vec2, zoom: f32, viewport: (u32, u32), p: Vec2) -> Vec2 {
    let (w, h) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    Vec2::new((p.x - cam.x) * zoom + w * 0.5, h * 0.5 - (p.y - cam.y) * zoom)
}

/// The camera position after the content was dragged by `delta` pixels (screen, y down).
pub fn pan(cam: Vec2, zoom: f32, delta: Vec2) -> Vec2 {
    cam + Vec2::new(-delta.x, delta.y) / zoom
}

/// Camera position and zoom after `lines` of wheel at pixel `cursor`: the world point under the
/// cursor stays under it (unless the zoom hit a limit).
pub fn zoom_at(cam: Vec2, zoom: f32, viewport: (u32, u32), cursor: Vec2, lines: f32) -> (Vec2, f32) {
    let new = (zoom * WHEEL_ZOOM.powf(lines)).clamp(MIN_ZOOM, MAX_ZOOM);
    let anchor = screen_to_world(cam, zoom, viewport, cursor);
    let (w, h) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    (anchor - Vec2::new((cursor.x - w * 0.5) / new, (h * 0.5 - cursor.y) / new), new)
}

/// Camera position and zoom that show a box of `size` (world units) at `center` filling `fill`
/// (0..1) of the viewport.
pub fn frame_box(center: Vec2, size: Vec2, viewport: (u32, u32), fill: f32) -> (Vec2, f32) {
    let (w, h) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    let z = (w / size.x.max(1e-3)).min(h / size.y.max(1e-3)) * fill;
    (center, z.clamp(MIN_ZOOM, MAX_ZOOM))
}

/// `v` rounded to a multiple of `step` (`v` itself for a step that is not positive).
pub fn snap(v: f32, step: f32) -> f32 {
    if step > 0.0 && step.is_finite() { (v / step).round() * step } else { v }
}

// ---------------------------------------------------------------------------------------------
// transforms

/// The entity's world transform: its `Transform` times its ancestors' (a parent that is gone or
/// has no `Transform` is skipped, like the engine's propagation does; a cycle stops after 64 steps).
pub fn world_affine_of(world: &World, e: Entity) -> Affine2 {
    let mut m = world.get::<&Transform>(e).map_or(Affine2::IDENTITY, |t| t.affine());
    let mut cur = e;
    for _ in 0..64 {
        let Some(p) = world.get::<&Parent>(cur).ok().and_then(|p| p.entity()) else { break };
        let Ok(pt) = world.get::<&Transform>(p) else { break };
        m = pt.affine() * m;
        cur = p;
    }
    m
}

/// The world transform of the entity's parent (identity for a root).
pub fn parent_affine_of(world: &World, e: Entity) -> Affine2 {
    match world.get::<&Parent>(e).ok().and_then(|p| p.entity()) {
        Some(p) if world.contains(p) && world.get::<&Transform>(p).is_ok() => world_affine_of(world, p),
        _ => Affine2::IDENTITY,
    }
}

/// A transform (position, rotation, scale) that gives the affine `a`.
pub fn decompose(a: Affine2) -> Transform {
    let (x, y) = (a.matrix2.x_axis, a.matrix2.y_axis);
    let det = x.x * y.y - x.y * y.x;
    Transform { position: a.translation, rotation: x.y.atan2(x.x), scale: Vec2::new(x.length(), if det < 0.0 { -y.length() } else { y.length() }) }
}

fn invertible(a: &Affine2) -> bool {
    a.matrix2.determinant().abs() > 1e-12 && a.translation.is_finite()
}

/// The size a sprite is drawn with (texture size when it is auto-sized, like the render extract).
pub fn sprite_size(s: &Sprite, textures: Option<&Textures>) -> Vec2 {
    if s.auto_size != 0 {
        textures.and_then(|t| t.size(s.texture)).map_or(Vec2::ONE, |(w, h)| Vec2::new(w as f32, h as f32))
    } else {
        s.size
    }
}

// ---------------------------------------------------------------------------------------------
// gizmo geometry and drag math

/// What a gizmo drag changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    MoveFree,
    MoveX,
    MoveY,
    Rotate,
    /// A corner of the scale box (the signs of its local position).
    Scale { sx: f32, sy: f32 },
}

/// Where the gizmo of the selected entity is.
#[derive(Clone, Copy, Debug)]
pub struct Geom {
    pub center: Vec2,
    /// World units per screen pixel (1 / zoom).
    pub px: f32,
    /// Maps the scale box's local coordinates to the world.
    pub frame: Affine2,
    /// Half size of the scale box in `frame` units.
    pub half: Vec2,
}

impl Geom {
    /// The corners of the scale box in the world, with their signs.
    pub fn corners(&self) -> [(Vec2, f32, f32); 4] {
        let c = |sx: f32, sy: f32| (self.frame.transform_point2(Vec2::new(sx * self.half.x, sy * self.half.y)), sx, sy);
        [c(-1.0, -1.0), c(1.0, -1.0), c(1.0, 1.0), c(-1.0, 1.0)]
    }
}

/// The gizmo geometry of `entity` at `px` world units per pixel.
pub fn geometry(world: &World, textures: Option<&Textures>, entity: Entity, px: f32) -> Geom {
    let aff = world_affine_of(world, entity);
    match world.get::<&Sprite>(entity) {
        Ok(s) => Geom { center: aff.translation, px, frame: aff, half: sprite_size(&s, textures) * 0.5 },
        Err(_) => Geom { center: aff.translation, px, frame: Affine2::from_angle_translation(aff.matrix2.x_axis.to_angle(), aff.translation), half: Vec2::splat(BOX_PX * px) },
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// The handle of `tool` under the world point `ptr`, if any.
pub fn hit(tool: GizmoTool, g: &Geom, ptr: Vec2) -> Option<Handle> {
    let (px, d) = (g.px, ptr - g.center);
    match tool {
        GizmoTool::Move => {
            if d.x.abs() <= CENTER_PX * px && d.y.abs() <= CENTER_PX * px {
                return Some(Handle::MoveFree);
            }
            let l = ARROW_PX * px;
            let (dx, dy) = (seg_dist(ptr, g.center, g.center + Vec2::new(l, 0.0)), seg_dist(ptr, g.center, g.center + Vec2::new(0.0, l)));
            match (dx <= HIT_PX * px, dy <= HIT_PX * px) {
                (true, true) => Some(if dx <= dy { Handle::MoveX } else { Handle::MoveY }),
                (true, false) => Some(Handle::MoveX),
                (false, true) => Some(Handle::MoveY),
                _ => None,
            }
        }
        GizmoTool::Rotate => ((d.length() - RING_PX * px).abs() <= HIT_PX * px).then_some(Handle::Rotate),
        GizmoTool::Scale => g
            .corners()
            .iter()
            .map(|(p, sx, sy)| (p.distance(ptr), *sx, *sy))
            .filter(|(dist, _, _)| *dist <= HIT_PX * px)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, sx, sy)| Handle::Scale { sx, sy }),
    }
}

/// A gizmo drag in progress.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub entity: Entity,
    pub handle: Handle,
    /// The entity's local `Transform` when the drag started.
    pub start: Transform,
    /// The pointer in the world when the drag started.
    pub start_ptr: Vec2,
    /// The entity's world transform at the start.
    pub start_world: Affine2,
    /// The parent's world transform (identity for a root).
    pub parent: Affine2,
    /// The scale box frame at the start ([`Geom::frame`]).
    pub frame: Affine2,
}

impl Drag {
    pub fn new(world: &World, textures: Option<&Textures>, entity: Entity, handle: Handle, ptr: Vec2, px: f32) -> Option<Drag> {
        let start = *world.get::<&Transform>(entity).ok()?;
        let g = geometry(world, textures, entity, px);
        Some(Drag { entity, handle, start, start_ptr: ptr, start_world: world_affine_of(world, entity), parent: parent_affine_of(world, entity), frame: g.frame })
    }
}

fn ratio(a: f32, b: f32) -> f32 {
    if b.abs() < 1e-4 { 1.0 } else { a / b }
}

/// The local `Transform` the drag gives with the pointer at `ptr` (world). `grid` is the snap step
/// of positions, `snap` (Ctrl) turns snapping on, `uniform` (Shift) keeps the scale's proportions.
/// Position drags work in world space and are converted through the parent's world transform.
pub fn drag_transform(d: &Drag, ptr: Vec2, grid: f32, snap_on: bool, uniform: bool) -> Transform {
    match d.handle {
        Handle::MoveFree | Handle::MoveX | Handle::MoveY => {
            let mut delta = ptr - d.start_ptr;
            if d.handle == Handle::MoveX {
                delta.y = 0.0;
            }
            if d.handle == Handle::MoveY {
                delta.x = 0.0;
            }
            let mut w = d.start_world.translation + delta;
            if snap_on {
                if d.handle != Handle::MoveY {
                    w.x = snap(w.x, grid);
                }
                if d.handle != Handle::MoveX {
                    w.y = snap(w.y, grid);
                }
            }
            if !invertible(&d.parent) {
                return d.start;
            }
            let local = d.parent.inverse().transform_point2(w);
            if local.is_finite() { Transform { position: local, ..d.start } } else { d.start }
        }
        Handle::Rotate => {
            let c = d.start_world.translation;
            let delta = ((ptr - c).to_angle() - (d.start_ptr - c).to_angle() + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            // A mirroring parent turns the other way.
            let delta = if d.parent.matrix2.determinant() < 0.0 { -delta } else { delta };
            let mut rot = d.start.rotation + delta;
            if snap_on {
                rot = snap(rot, ROTATE_SNAP);
            }
            Transform { rotation: rot, ..d.start }
        }
        Handle::Scale { .. } => {
            if !invertible(&d.frame) {
                return d.start;
            }
            let inv = d.frame.inverse();
            let (p0, p) = (inv.transform_point2(d.start_ptr), inv.transform_point2(ptr));
            let mut r = Vec2::new(ratio(p.x, p0.x), ratio(p.y, p0.y));
            if uniform {
                let k = if p0.length() > 1e-4 { p.length() / p0.length() * if p.dot(p0) < 0.0 { -1.0 } else { 1.0 } } else { 1.0 };
                r = Vec2::splat(k);
            }
            let mut s = d.start.scale * r;
            if snap_on {
                let fix = |v: f32| {
                    let q = snap(v, SCALE_SNAP);
                    if q == 0.0 { SCALE_SNAP * v.signum() } else { q }
                };
                s = Vec2::new(fix(s.x), fix(s.y));
            }
            if s.is_finite() { Transform { scale: s, ..d.start } } else { d.start }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// picking

/// The entity under the world point `p` (`px` = world units per pixel): the handle of an entity
/// without a sprite (nearest first; handles are drawn over sprites), else the topmost sprite
/// whose quad contains `p` (render order: layer, then entity id; invisible sprites are skipped).
/// Only entities the hierarchy lists can be picked.
pub fn pick(e: &Engine, p: Vec2, px: f32) -> Option<Entity> {
    let world = e.world();
    let textures = e.resources().get::<Textures>();
    let rows = e.editable_entities();
    let handles_on = e.resources().get::<EditorView>().is_none_or(|v| v.handles);
    let mut handle: Option<(f32, Entity)> = None;
    let mut sprite: Option<((i32, u32), Entity)> = None;
    for r in &rows {
        let ent = r.entity;
        if let Ok(s) = world.get::<&Sprite>(ent) {
            if world.get::<&Visibility>(ent).is_ok_and(|v| v.visible == 0) {
                continue;
            }
            let half = sprite_size(&s, textures.as_deref()) * 0.5;
            let aff = world_affine_of(world, ent);
            if half.x <= 0.0 || half.y <= 0.0 || !invertible(&aff) {
                continue;
            }
            let l = aff.inverse().transform_point2(p);
            if l.x.abs() <= half.x && l.y.abs() <= half.y {
                let key = (s.layer, ent.id());
                if sprite.is_none_or(|(k, _)| key > k) {
                    sprite = Some((key, ent));
                }
            }
        } else if handles_on && world.get::<&Transform>(ent).is_ok() {
            let d = world_affine_of(world, ent).translation.distance(p);
            if d <= HANDLE_PX * px && handle.is_none_or(|(b, _)| d < b) {
                handle = Some((d, ent));
            }
        }
    }
    handle.map(|(_, e)| e).or(sprite.map(|(_, e)| e))
}

// ---------------------------------------------------------------------------------------------
// the runtime

/// The editor camera entity. A resource; the camera lives as long as the plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorCamera {
    pub entity: Entity,
}

/// Private state of the viewport systems.
#[derive(Default)]
pub(super) struct ViewportRt {
    pub drag: Option<Drag>,
    /// The editor camera's position and zoom. This is the truth: `camera_apply` writes it to the
    /// entity every frame, so a game system that moves every `Transform` cannot move the camera.
    pub cam: (Vec2, f32),
    /// The camera position and zoom `Home` goes back to (the game camera's at the first frame).
    pub home: (Vec2, f32),
    /// Open the editor window at boot (`EditorPlugin::separate_window`).
    pub want_detach: bool,
}

/// The editor camera's window when it is the camera that draws there now.
fn live_window(world: &World, res: &efeengine_core::engine::resources::Resources) -> Option<u32> {
    let cam = res.get::<EditorCamera>()?;
    let c = world.get::<&Camera>(cam.entity).ok()?;
    (c.active != 0).then_some(c.target)
}

fn cam_state(res: &efeengine_core::engine::resources::Resources) -> Option<(Vec2, f32)> {
    res.get::<ViewportRt>().map(|r| r.cam)
}

/// Stores the camera position and zoom and writes them to the entity.
fn set_cam(world: &mut World, res: &mut efeengine_core::engine::resources::Resources, cam: EditorCamera, pos: Vec2, zoom: f32) {
    if let Some(rt) = res.get_mut::<ViewportRt>() {
        rt.cam = (pos, zoom);
    }
    write_cam(world, cam, pos, zoom);
}

fn write_cam(world: &mut World, cam: EditorCamera, pos: Vec2, zoom: f32) {
    if let Ok(mut t) = world.get::<&mut Transform>(cam.entity) {
        if t.position != pos {
            t.position = pos;
        }
    }
    if let Ok(mut c) = world.get::<&mut Camera>(cam.entity) {
        if c.zoom != zoom {
            c.zoom = zoom;
        }
    }
}

/// Spawns the editor camera (called from `EditorPlugin::build`).
pub(super) fn spawn_camera(engine: &mut Engine) {
    let entity = engine.world_mut().spawn((Camera { active: 0, clear_color: [0.10, 0.11, 0.14, 1.0], ..Camera::orthographic_2d() }, Transform::default(), EditorOnly));
    engine.insert_resource(EditorCamera { entity });
}

/// First frame: the editor camera starts where the game's camera is (position, zoom, clear colour).
pub(super) fn boot(cx: &mut Ctx, want_detach: bool) {
    let Some(cam) = cx.resources.get::<EditorCamera>().map(|c| *c) else { return };
    let game = cx
        .world
        .query::<(Entity, &Camera, Option<&Transform>)>()
        .without::<&EditorOnly>()
        .iter()
        .filter(|(_, c, _)| c.active != 0 && c.target == 0 && c.kind == Camera::ORTHOGRAPHIC)
        .min_by_key(|(e, _, _)| e.id())
        .map(|(_, c, t)| (t.map_or(Vec2::ZERO, |t| t.position), c.zoom, c.clear_color));
    if let Some((pos, zoom, clear)) = game {
        set_cam(cx.world, cx.resources, cam, pos, zoom);
        if let Ok(mut c) = cx.world.get::<&mut Camera>(cam.entity) {
            c.clear_color = clear;
        }
    }
    let home = cam_state(cx.resources).unwrap_or((Vec2::ZERO, 1.0));
    if let Some(rt) = cx.resources.get_mut::<ViewportRt>() {
        rt.home = home;
        rt.want_detach = want_detach;
    }
    if want_detach {
        cx.commands.engine_task(|e| set_detached(e, true));
    }
}

/// Keeps the editor camera's `active` and `target` right: layout A, Edit / Paused: active on the
/// main window, Play: off (the game's camera draws); layout B: active on the editor window.
pub(super) fn camera_apply(cx: &mut Ctx) {
    let Some(cam) = cx.resources.get::<EditorCamera>().map(|c| *c) else { return };
    let mode = cx.resources.get::<EditorState>().map_or(EditorMode::Edit, |s| s.mode);
    let win = cx.resources.get::<EditorView>().and_then(|v| v.window);
    let (active, target) = match win {
        Some(w) => (1, w),
        None => (u32::from(mode != EditorMode::Play), 0),
    };
    if let Ok(mut c) = cx.world.get::<&mut Camera>(cam.entity) {
        if c.active != active {
            c.active = active;
        }
        if c.target != target {
            c.target = target;
        }
    }
    if let Some((pos, zoom)) = cam_state(cx.resources) {
        write_cam(cx.world, cam, pos, zoom);
    }
}

fn set_view(e: &mut Engine, f: impl FnOnce(&mut EditorView)) {
    let Some(cur) = e.resources().get::<EditorView>().map(|v| (*v).clone()) else { return };
    let mut next = cur.clone();
    f(&mut next);
    if next != cur {
        if let Some(v) = e.resources_mut().get_mut::<EditorView>() {
            *v = next;
        }
    }
}

/// The world position at the centre of the editor camera (`None` without the camera).
pub(super) fn camera_center(e: &Engine) -> Option<Vec2> {
    cam_state(e.resources()).map(|(p, _)| p)
}

/// Opens (`true`) or closes (`false`) the editor window (layout B / A). Layout B needs a second
/// window: when it cannot be had (SDL mode, a custom renderer, a backend that cannot open one) a
/// warning is logged, the message line says why and the layout stays A.
pub(crate) fn set_detached(e: &mut Engine, on: bool) {
    let cur = e.resources().get::<EditorView>().and_then(|v| v.window);
    if on {
        if cur.is_some() {
            return;
        }
        if e.has_custom_renderer() {
            return edit::say(e, "No separate editor window: a custom renderer draws the main window only (staying in one window)", true);
        }
        match e.create_window(WindowDesc::new("Editor").size(1100, 720)) {
            Ok(id) => {
                set_view(e, |v| {
                    v.window_size = (1100, 720);
                    v.detached = true;
                    v.window = Some(id.0);
                });
                if let Some(views) = e.resources_mut().get_mut::<crate::UiViews>() {
                    views.set_enabled(super::PANEL_VIEW, true);
                }
                super::request_refresh(e);
            }
            Err(err) => edit::say(e, format!("No separate editor window: {err} (staying in one window)"), true),
        }
    } else {
        let Some(w) = cur else { return };
        e.close_window(WindowId(w));
        set_view(e, |v| {
            v.detached = false;
            v.window_size = (0, 0);
            v.window = None;
        });
        if let Some(views) = e.resources_mut().get_mut::<crate::UiViews>() {
            views.set_enabled(super::PANEL_VIEW, false);
        }
        if let Some(rt) = e.resources_mut().get_mut::<ViewportRt>() {
            rt.drag = None;
        }
        super::request_refresh(e);
    }
}

/// Closing the editor window attaches the panels again.
pub(super) fn window_watch(cx: &mut Ctx) {
    let Some(w) = cx.resources.get::<EditorView>().and_then(|v| v.window) else { return };
    // The panels are sized to the window: a resize changes the view (a write only on change).
    if let Some(size) = cx.windows.get(WindowId(w)).map(|win| win.size) {
        if cx.resources.get::<EditorView>().is_some_and(|v| v.window_size != size) {
            if let Some(v) = cx.resources.get_mut::<EditorView>() {
                v.window_size = size;
            }
        }
    }
    let closed = cx.input.events().iter().any(|ev| ev.window.0 == w && matches!(ev.kind, InputEventKind::Window(WindowEvent::Closed)));
    // A window the engine no longer has (closed behind our back) counts too.
    if closed || cx.windows.get(WindowId(w)).is_none() {
        cx.commands.engine_task(|e| set_detached(e, false));
    }
}

/// Places the panel layer in the editor window (the view rebuilds its layer every frame).
pub(super) fn place_panel(cx: &mut Ctx) {
    let Some(w) = cx.resources.get::<EditorView>().and_then(|v| v.window) else { return };
    if let Some(tree) = cx.resources.get_mut::<crate::UiTree>() {
        tree.layer_in(w, super::PANEL_VIEW, crate::Layer::top_left(40));
    }
}

/// Whether the keyboard shortcuts of the viewport apply: the editor camera draws a window that has keyboard focus.
fn keys_window(cx: &Ctx) -> Option<u32> {
    let w = live_window(cx.world, cx.resources)?;
    (cx.input.focus_window().0 == w).then_some(w)
}

/// The editor window's undo shortcut condition (layout B).
pub(super) fn detached_focused(cx: &Ctx) -> bool {
    cx.resources.get::<EditorView>().and_then(|v| v.window).is_some_and(|w| cx.input.focus_window().0 == w)
}

/// Camera controls, picking and gizmo drags.
pub(super) fn input(cx: &mut Ctx) {
    let Some(cam) = cx.resources.get::<EditorCamera>().map(|c| *c) else { return };
    let Some(win) = live_window(cx.world, cx.resources) else {
        if let Some(rt) = cx.resources.get_mut::<ViewportRt>() {
            rt.drag = None;
        }
        return;
    };
    let wid = WindowId(win);
    let Some(viewport) = cx.windows.get(wid).map(|w| w.size) else { return };
    let Some((cam_pos, zoom)) = cam_state(cx.resources) else { return };
    let gi = cx.input.game_input();
    let typing = cx.input.capture().keyboard || cx.resources.get::<EditorUi>().is_none_or(|u| u.focus.is_some());
    let mods = cx.input.modifiers();
    let view = cx.resources.get::<EditorView>().map(|v| (*v).clone()).unwrap_or_default();
    let selected = cx.resources.get::<EditorSelection>().and_then(|s| s.entity).filter(|e| cx.world.contains(*e));
    let pointer_here = cx.input.pointer_window().unwrap_or(WindowId::MAIN) == wid;
    let cursor = cx.input.mouse_pos_in(wid).map(|(x, y)| Vec2::new(x, y));
    let px = 1.0 / zoom;

    // ---- keys ----
    let keys = keys_window(cx).is_some() && !typing && !mods.ctrl() && !mods.alt();
    let mut new_view = view.clone();
    let mut new_cam = (cam_pos, zoom);
    if keys {
        if gi.just_pressed(KeyCode::W) {
            new_view.tool = GizmoTool::Move;
        }
        if gi.just_pressed(KeyCode::E) {
            new_view.tool = GizmoTool::Rotate;
        }
        if gi.just_pressed(KeyCode::R) {
            new_view.tool = GizmoTool::Scale;
        }
        if gi.just_pressed(KeyCode::G) {
            new_view.grid = !new_view.grid;
        }
        if gi.just_pressed(KeyCode::H) {
            new_view.handles = !new_view.handles;
        }
        if gi.just_pressed(KeyCode::Home) {
            if let Some(rt) = cx.resources.get::<ViewportRt>() {
                new_cam = rt.home;
            }
        }
        if gi.just_pressed(KeyCode::F) {
            if let Some(sel) = selected {
                let textures = cx.resources.get::<Textures>();
                let g = geometry(cx.world, textures.as_deref(), sel, px);
                let corners = g.corners().map(|c| c.0);
                let (lo, hi) = corners.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), c| (lo.min(*c), hi.max(*c)));
                new_cam = if cx.world.get::<&Sprite>(sel).is_ok() { frame_box((lo + hi) * 0.5, hi - lo, viewport, 0.5) } else { (g.center, zoom) };
            }
        }
    }

    // ---- pan and zoom ----
    let space = !typing && cx.input.pressed(KeyCode::Space);
    if pointer_here {
        if gi.mouse_pressed(MouseButton::Middle) || (space && gi.mouse_pressed(MouseButton::Left)) {
            let d = gi.mouse_delta();
            if d != (0.0, 0.0) {
                new_cam.0 = pan(new_cam.0, new_cam.1, Vec2::new(d.0, d.1));
            }
        }
        let wheel = gi.wheel().1;
        if wheel != 0.0 {
            if let Some(c) = cursor {
                new_cam = zoom_at(new_cam.0, new_cam.1, viewport, c, wheel);
            }
        }
    }
    if new_cam != (cam_pos, zoom) {
        set_cam(cx.world, cx.resources, cam, new_cam.0, new_cam.1);
    }
    if new_view != view {
        if let Some(v) = cx.resources.get_mut::<EditorView>() {
            *v = new_view.clone();
        }
    }
    let (cam_pos, zoom) = new_cam;
    let px = 1.0 / zoom;
    let to_world = |c: Vec2| screen_to_world(cam_pos, zoom, viewport, c);

    // ---- a drag in progress ----
    let drag = cx.resources.get::<ViewportRt>().and_then(|r| r.drag);
    if let Some(d) = drag {
        if !cx.world.contains(d.entity) || cx.input.just_pressed(KeyCode::Escape) {
            // Gone, or cancelled: put the start back.
            if cx.world.contains(d.entity) {
                if let Ok(mut t) = cx.world.get::<&mut Transform>(d.entity) {
                    *t = d.start;
                }
            }
            if let Some(rt) = cx.resources.get_mut::<ViewportRt>() {
                rt.drag = None;
            }
        } else {
            if let Some(c) = cursor {
                let t = drag_transform(&d, to_world(c), new_view.grid_step, mods.ctrl(), mods.shift());
                if let Ok(mut cur) = cx.world.get::<&mut Transform>(d.entity) {
                    if *cur != t {
                        *cur = t;
                    }
                }
            }
            if !cx.input.mouse_pressed(MouseButton::Left) {
                let (entity, start) = (d.entity, d.start);
                let end = cx.world.get::<&Transform>(entity).ok().map(|t| *t);
                if let Some(rt) = cx.resources.get_mut::<ViewportRt>() {
                    rt.drag = None;
                }
                if let Some(end) = end.filter(|t| *t != start) {
                    cx.commands.engine_task(move |e| edit::record_transform(e, entity, start, end));
                }
            }
        }
        return;
    }

    // ---- a click ----
    if pointer_here && !space && gi.mouse_just_pressed(MouseButton::Left) {
        let Some(c) = cursor else { return };
        let ptr = to_world(c);
        if let Some(sel) = selected {
            let textures = cx.resources.get::<Textures>();
            let g = geometry(cx.world, textures.as_deref(), sel, px);
            if let Some(h) = hit(new_view.tool, &g, ptr) {
                let d = Drag::new(cx.world, textures.as_deref(), sel, h, ptr, px);
                drop(textures);
                if let Some(rt) = cx.resources.get_mut::<ViewportRt>() {
                    rt.drag = d;
                }
                return;
            }
        }
        cx.commands.engine_task(move |e| {
            let hit = pick(e, ptr, px);
            edit::select(e, hit);
            super::request_refresh(e);
        });
    }
}

// ---------------------------------------------------------------------------------------------
// drawing

const COL_X: [f32; 4] = [1.0, 0.25, 0.25, 1.0];
const COL_Y: [f32; 4] = [0.3, 0.9, 0.3, 1.0];
const COL_FREE: [f32; 4] = [1.0, 0.9, 0.2, 1.0];
const COL_ROT: [f32; 4] = [0.4, 0.7, 1.0, 1.0];
const COL_SCALE: [f32; 4] = [1.0, 0.6, 0.2, 1.0];
const COL_OUTLINE: [f32; 4] = [1.0, 0.8, 0.1, 1.0];
const COL_HANDLE: [f32; 4] = [0.8, 0.85, 1.0, 0.9];
const COL_GRID: [f32; 4] = [1.0, 1.0, 1.0, 0.10];
const COL_AXIS: [f32; 4] = [1.0, 1.0, 1.0, 0.30];

fn arrow(d: &mut DebugDraw2D, from: Vec2, dir: Vec2, len: f32, head: f32, color: [f32; 4]) {
    let tip = from + dir * len;
    let side = Vec2::new(-dir.y, dir.x);
    d.line(from, tip, color);
    d.line(tip, tip - dir * head + side * head * 0.5, color);
    d.line(tip, tip - dir * head - side * head * 0.5, color);
}

fn draw_grid(d: &mut DebugDraw2D, cam: Vec2, zoom: f32, viewport: (u32, u32), step: f32) {
    if !(step > 0.0 && step.is_finite()) {
        return;
    }
    let mut s = step;
    while s * zoom < GRID_MIN_PX && s.is_finite() && s < 1e9 {
        s *= 2.0;
    }
    let lo = screen_to_world(cam, zoom, viewport, Vec2::new(0.0, viewport.1 as f32));
    let hi = screen_to_world(cam, zoom, viewport, Vec2::new(viewport.0 as f32, 0.0));
    if (hi.x - lo.x) / s > 400.0 || (hi.y - lo.y) / s > 400.0 {
        return;
    }
    d.set_width(1.0);
    let mut x = (lo.x / s).floor() * s;
    while x <= hi.x {
        d.line(Vec2::new(x, lo.y), Vec2::new(x, hi.y), if x == 0.0 { COL_AXIS } else { COL_GRID });
        x += s;
    }
    let mut y = (lo.y / s).floor() * s;
    while y <= hi.y {
        d.line(Vec2::new(lo.x, y), Vec2::new(hi.x, y), if y == 0.0 { COL_AXIS } else { COL_GRID });
        y += s;
    }
}

/// The grid, the handles of entities without a sprite, the selection outline and the gizmo.
pub(super) fn overlay(cx: &mut Ctx) {
    if cx.resources.get::<EditorCamera>().is_none() {
        return;
    }
    let live = live_window(cx.world, cx.resources);
    let view = cx.resources.get::<EditorView>().map(|v| (*v).clone()).unwrap_or_default();
    let selected = cx.resources.get::<EditorSelection>().and_then(|s| s.entity).filter(|e| cx.world.contains(*e));
    // The outline follows the camera that draws; without the editor camera it is the game's (window 0).
    let win = live.unwrap_or(0);
    let Some((cam_pos, zoom)) = cam_state(cx.resources) else { return };
    let zoom = if live.is_some() { zoom } else { 1.0 };
    let px = 1.0 / zoom;
    let viewport = cx.windows.get(WindowId(win)).map_or((0, 0), |w| w.size);
    let textures = cx.resources.get::<Textures>();
    let drag = cx.resources.get::<ViewportRt>().and_then(|r| r.drag);
    let mut handles: Vec<Vec2> = Vec::new();
    if live.is_some() && view.handles {
        for (e, t) in cx.world.query::<(Entity, &Transform)>().without::<&Sprite>().without::<&EditorOnly>().iter() {
            handles.push(if cx.world.get::<&Parent>(e).is_ok() { world_affine_of(cx.world, e).translation } else { t.position });
        }
    }
    let geom = selected.map(|s| (s, geometry(cx.world, textures.as_deref(), s, px)));
    let is_sprite = selected.is_some_and(|s| cx.world.get::<&Sprite>(s).is_ok());
    drop(textures);
    let Some(d) = cx.resources.get_mut::<DebugDraw2D>() else { return };
    d.set_window(win);
    if live.is_some() {
        if view.grid {
            draw_grid(d, cam_pos, zoom, viewport, view.grid_step);
        }
        for h in handles {
            d.circle(h, 4.0 * px, COL_HANDLE);
        }
    }
    if let Some((_, g)) = geom {
        // The outline of a sprite (always, also under the game's camera in Play).
        if is_sprite {
            let c = g.corners();
            for i in 0..4 {
                d.line(c[i].0, c[(i + 1) % 4].0, COL_OUTLINE);
            }
        }
        if live.is_some() {
            d.set_width(2.0);
            let active = drag.map(|dr| dr.handle);
            let on = |h: Handle| active == Some(h);
            match view.tool {
                GizmoTool::Move => {
                    let l = ARROW_PX * px;
                    arrow(d, g.center, Vec2::X, l, 12.0 * px, if on(Handle::MoveX) { COL_FREE } else { COL_X });
                    arrow(d, g.center, Vec2::Y, l, 12.0 * px, if on(Handle::MoveY) { COL_FREE } else { COL_Y });
                    let c = CENTER_PX * px;
                    let col = if on(Handle::MoveFree) { [1.0, 1.0, 1.0, 1.0] } else { COL_FREE };
                    d.rect(g.center - Vec2::splat(c), g.center + Vec2::splat(c), col);
                }
                GizmoTool::Rotate => {
                    d.circle(g.center, RING_PX * px, if on(Handle::Rotate) { COL_FREE } else { COL_ROT });
                    let a = g.frame.matrix2.x_axis.normalize_or_zero();
                    if a != Vec2::ZERO {
                        d.line(g.center, g.center + a * RING_PX * px, COL_ROT);
                    }
                }
                GizmoTool::Scale => {
                    let c = g.corners();
                    for i in 0..4 {
                        d.line(c[i].0, c[(i + 1) % 4].0, COL_SCALE);
                    }
                    for (p, sx, sy) in c {
                        let on_it = matches!(active, Some(Handle::Scale { sx: a, sy: b }) if a == sx && b == sy);
                        d.rect_filled(p - Vec2::splat(5.0 * px), p + Vec2::splat(5.0 * px), if on_it { [1.0, 1.0, 1.0, 1.0] } else { COL_SCALE });
                    }
                }
            }
        }
    }
    d.set_width(1.0);
    d.set_window(0u32);
}

#[cfg(test)]
mod math_tests {
    use super::*;

    fn near(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn screen_and_world_are_inverse_and_y_is_flipped() {
        let (cam, z, vp) = (Vec2::new(10.0, -5.0), 2.0, (800u32, 600u32));
        assert!(near(screen_to_world(cam, z, vp, Vec2::new(400.0, 300.0)), cam), "the centre is the camera");
        assert!(near(screen_to_world(cam, z, vp, Vec2::new(500.0, 300.0)), cam + Vec2::new(50.0, 0.0)));
        assert!(near(screen_to_world(cam, z, vp, Vec2::new(400.0, 200.0)), cam + Vec2::new(0.0, 50.0)), "up on screen is +y");
        let p = Vec2::new(3.0, 7.0);
        assert!(near(screen_to_world(cam, z, vp, world_to_screen(cam, z, vp, p)), p));
    }

    #[test]
    fn panning_moves_the_content_with_the_pointer() {
        let (cam, z, vp) = (Vec2::new(1.0, 2.0), 4.0, (800u32, 600u32));
        let before = Vec2::new(300.0, 250.0);
        let world_pt = screen_to_world(cam, z, vp, before);
        let moved = pan(cam, z, Vec2::new(40.0, -20.0));
        // The same world point is now 40 px right and 20 px up of where it was.
        assert!(near(world_to_screen(moved, z, vp, world_pt), before + Vec2::new(40.0, -20.0)));
    }

    #[test]
    fn zooming_keeps_the_world_point_under_the_cursor() {
        let (cam, z, vp) = (Vec2::new(-30.0, 12.0), 1.5, (1000u32, 700u32));
        for (cursor, lines) in [(Vec2::new(100.0, 50.0), 3.0), (Vec2::new(900.0, 600.0), -4.0), (Vec2::new(500.0, 350.0), 2.0)] {
            let under = screen_to_world(cam, z, vp, cursor);
            let (c2, z2) = zoom_at(cam, z, vp, cursor, lines);
            assert!((z2 - z * WHEEL_ZOOM.powf(lines)).abs() < 1e-3);
            assert!(near(screen_to_world(c2, z2, vp, cursor), under), "cursor {cursor:?}");
        }
        // zooming at the centre does not move the camera
        let (c3, _) = zoom_at(cam, z, vp, Vec2::new(500.0, 350.0), 5.0);
        assert!(near(c3, cam));
    }

    #[test]
    fn zoom_is_clamped() {
        let (_, z) = zoom_at(Vec2::ZERO, 60.0, (100, 100), Vec2::ZERO, 50.0);
        assert_eq!(z, MAX_ZOOM);
        let (_, z) = zoom_at(Vec2::ZERO, 0.03, (100, 100), Vec2::ZERO, -50.0);
        assert_eq!(z, MIN_ZOOM);
    }

    #[test]
    fn framing_fits_the_box() {
        let (pos, z) = frame_box(Vec2::new(5.0, 5.0), Vec2::new(100.0, 50.0), (800, 600), 0.5);
        assert_eq!(pos, Vec2::new(5.0, 5.0));
        assert!((z - 4.0).abs() < 1e-4, "{z}");
    }

    #[test]
    fn decompose_round_trips() {
        let t = Transform { position: Vec2::new(3.0, -4.0), rotation: 0.7, scale: Vec2::new(2.0, 0.5) };
        let d = decompose(t.affine());
        assert!(near(d.position, t.position) && (d.rotation - t.rotation).abs() < 1e-4 && near(d.scale, t.scale), "{d:?}");
        let m = Transform { scale: Vec2::new(2.0, -1.0), ..t };
        assert!(near(decompose(m.affine()).scale, m.scale));
    }
}
