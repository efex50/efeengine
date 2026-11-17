//! `RenderFrame`: the plain-data description of one frame, built by the render extract
//! (`Stage::RenderExtract`) and handed to the game's [`Renderer`](super::Renderer).
//!
//! Matrix convention (everywhere in this module): a 4x4 matrix is `[[f32; 4]; 4]` in
//! COLUMN-MAJOR order, `m[column][row]`, exactly the memory layout of a WGSL `mat4x4<f32>`,
//! so it can be uploaded as-is. Vectors are columns (`p' = M * p`). World space is y up,
//! x right, rotation counter-clockwise. Clip space is wgpu's: x, y in [-1, 1] (y up), z in [0, 1].

use glam::Vec2;
use hecs::Entity;

use crate::engine::components::{MeshHandle, TextureHandle};
use super::textures::TextureChanges;

/// The one camera of the frame, ready to use.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraView {
    /// The camera entity.
    pub entity: Entity,
    /// World position (from the entity's `GlobalTransform`, else `Transform`).
    pub position: Vec2,
    /// World rotation in radians, counter-clockwise.
    pub rotation: f32,
    pub zoom: f32,
    /// `Camera::kind`: 0 = orthographic 2D, 1 = perspective.
    pub kind: u32,
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

/// Everything a renderer needs for one frame. A resource, rebuilt every frame in
/// `Stage::RenderExtract`.
#[derive(Clone, Debug, Default)]
pub struct RenderFrame {
    /// The camera used, or `None` when no entity has an active `Camera`.
    pub camera: Option<CameraView>,
    /// How many further active cameras were ignored this frame (see [`RenderFrame::camera`]).
    pub ignored_cameras: usize,
    /// Visible sprites ordered by `layer` (ascending: first drawn first), ties by entity id.
    pub sprites: Vec<SpriteInstance>,
    /// Mesh entities, by entity id.
    pub meshes: Vec<MeshInstance>,
    /// Texture store changes since the previous frame; delivered once.
    pub textures: TextureChanges,
    /// `Time::frame` of this frame.
    pub frame: u64,
    /// `Time::elapsed` in seconds.
    pub time: f64,
}

/// Latched "already warned" flags: each problem is logged once, not every frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderWarnings {
    /// More than one active camera was found.
    pub multiple_cameras: bool,
    /// A `Parent` cycle was found (and broken).
    pub hierarchy_cycle: bool,
}
