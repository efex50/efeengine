//! `RenderFrame`: the plain-data description of one frame, built by the render extract
//! (`Stage::RenderExtract`) and handed to the renderers ([`Renderer2D`](super::Renderer2D) with `d2`, [`Renderer3D`](super::Renderer3D) with `d3`, or a [`CustomRenderer`](super::CustomRenderer) with the whole frame).
//!
//! Matrix convention (everywhere in this module): a 4x4 matrix is `[[f32; 4]; 4]` in
//! COLUMN-MAJOR order, `m[column][row]`, exactly the memory layout of a WGSL `mat4x4<f32>`,
//! so it can be uploaded as-is. Vectors are columns (`p' = M * p`). World space is y up,
//! x right, rotation counter-clockwise. Clip space is wgpu's: x, y in [-1, 1] (y up), z in [0, 1].

use glam::Vec2;
use hecs::Entity;

use crate::engine::components::{MeshHandle, TextureHandle};
use super::draw2d::DrawList;
use super::textures::TextureChanges;

/// One camera of the frame, ready to use (the active one of its kind).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraView {
    /// The camera entity.
    pub entity: Entity,
    /// World position (from the entity's `GlobalTransform`, else `Transform`).
    pub position: Vec2,
    /// World rotation in radians, counter-clockwise.
    pub rotation: f32,
    pub zoom: f32,
    /// `Camera::kind`: 0 = orthographic 2D, 1 = perspective. A 2D camera lands in
    /// [`Frame2D::camera`], a perspective one in [`Frame3D::camera`].
    pub kind: u32,
    /// `Camera::target`: which window the camera renders to (0 = the main window).
    pub target: u32,
    pub near: f32,
    pub far: f32,
    pub fov_y: f32,
    pub clear_color: [f32; 4],
    /// Size of the window's drawable area in physical pixels (0 in a headless engine that
    /// has had no resize event; matrices then use a 1x1 viewport).
    pub viewport: (u32, u32),
    /// World -> view: inverse of the camera's transform. Column-major.
    pub view: [[f32; 4]; 4],
    /// View -> clip. Orthographic: `zoom` pixels per world unit, centred on the camera, y up.
    /// Column-major.
    pub projection: [[f32; 4]; 4],
    /// `projection * view`, column-major: world -> clip in one upload.
    pub view_projection: [[f32; 4]; 4],
}

/// One sprite to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteInstance {
    pub entity: Entity,
    /// Quad -> world, column-major. The entity's world transform (translation, rotation,
    /// scale); the quad itself is the unit square centred on the origin, scaled by `size`.
    pub world: [[f32; 4]; 4],
    /// Size in world units: `Sprite::size`, or the texture's pixel size (1x1 for no texture)
    /// when `Sprite::auto_size` is set. May be zero for a sprite sized 0 on purpose.
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub color: [f32; 4],
    pub texture: TextureHandle,
    pub layer: i32,
    /// Bit 0 = mirror x, bit 1 = mirror y (not applied to `uv_*`: the renderer does it).
    pub flip: u32,
}

/// One mesh entity.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshInstance {
    pub entity: Entity,
    pub mesh: MeshHandle,
    /// Local -> world, column-major (the 2D transform lifted to 4x4).
    pub world: [[f32; 4]; 4],
}

/// What a [`Renderer2D`](super::Renderer2D) draws: the active 2D (orthographic) camera and the
/// sprites. Later also shapes, text runs and tilemap chunks in the same layer-sorted list.
#[derive(Clone, Debug, Default)]
pub struct Frame2D {
    /// The 2D camera used, or `None` when no entity has an active orthographic `Camera`.
    pub camera: Option<CameraView>,
    /// How many further active 2D cameras were ignored this frame.
    pub ignored_cameras: usize,
    /// Visible sprites ordered by `layer` (ascending: first drawn first), ties by entity id.
    pub sprites: Vec<SpriteInstance>,
    /// Extra 2D drawing in **screen pixels**, drawn after (over) the sprites: the debug draw
    /// (`DebugDraw2D`) is converted into it by the extract. Cleared and refilled every frame.
    pub draw: DrawList,
}

/// What a [`Renderer3D`](super::Renderer3D) draws: the active perspective camera and the meshes.
/// Later also lights and materials.
#[derive(Clone, Debug, Default)]
pub struct Frame3D {
    /// The 3D camera used, or `None` when no entity has an active perspective `Camera`.
    pub camera: Option<CameraView>,
    /// How many further active 3D cameras were ignored this frame.
    pub ignored_cameras: usize,
    /// Mesh entities, by entity id.
    pub meshes: Vec<MeshInstance>,
}

/// The 2D and 3D parts of one SECONDARY window (see [`RenderFrame::windows`]).
#[derive(Clone, Debug, Default)]
pub struct WindowFrame {
    /// The window id (never 0; the main window's parts are `RenderFrame::d2` / `d3`).
    pub window: u32,
    pub d2: Frame2D,
    pub d3: Frame3D,
}

/// Everything a renderer needs for one frame. A resource, rebuilt every frame in
/// `Stage::RenderExtract`.
#[derive(Clone, Debug, Default)]
pub struct RenderFrame {
    /// The 2D part of the MAIN window: drawn after the 3D part.
    pub d2: Frame2D,
    /// The 3D part of the MAIN window: drawn first.
    pub d3: Frame3D,
    /// One entry per open secondary window, by window id. Each has its own camera (an active
    /// `Camera` whose `target` is that window; none = the window is cleared) and shares the
    /// sprites / meshes of the world. No debug draw in these (it belongs to the main window).
    pub windows: Vec<WindowFrame>,
    /// Texture store changes since the previous frame; delivered once. Every renderer of the
    /// frame gets the same list.
    pub textures: TextureChanges,
    /// `Time::frame` of this frame.
    pub frame: u64,
    /// `Time::elapsed` in seconds.
    pub time: f64,
}

impl RenderFrame {
    /// The 2D and 3D parts of a window (`0` = the main window).
    pub fn window(&self, window: u32) -> Option<(&Frame2D, &Frame3D)> {
        if window == 0 {
            Some((&self.d2, &self.d3))
        } else {
            self.windows.iter().find(|w| w.window == window).map(|w| (&w.d2, &w.d3))
        }
    }
}

/// Latched "already warned" flags: each problem is logged once, not every frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderWarnings {
    /// More than one active 2D camera was found.
    pub multiple_cameras: bool,
    /// More than one active 3D camera was found.
    pub multiple_cameras_3d: bool,
    /// A 3D camera is active but no `Renderer3D` is installed.
    pub no_renderer_3d: bool,
    /// A `Parent` cycle was found (and broken).
    pub hierarchy_cycle: bool,
    /// A secondary window's surface format differs from the main window's (it is not drawn).
    pub window_format_mismatch: bool,
}
