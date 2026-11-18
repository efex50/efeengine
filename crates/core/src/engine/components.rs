//! Built-in components (ROADMAP 3.1). The script-visible ones are `#[repr(C)]` and `Pod`,
//! so they can be copied to and from VM memory as bytes; they are registered in the
//! [`TypeRegistry`](crate::engine::types::TypeRegistry) by `Engine::with_config`.

use efeengine_macros::script_component;
use glam::{Affine2, Mat4, Vec2};

use crate::engine::types::ScriptField;

/// Position, rotation and scale of an entity in 2D world space.
#[script_component(tracked)]
#[derive(Debug, PartialEq)]
pub struct Transform {
    /// World position.
    pub position: Vec2,
    /// Rotation in radians, counter-clockwise.
    pub rotation: f32,
    /// Scale per axis; 1 is the natural size.
    pub scale: Vec2,
}

impl Default for Transform {
    fn default() -> Self {
        Self { position: Vec2::ZERO, rotation: 0.0, scale: Vec2::ONE }
    }
}

impl Transform {
    /// A transform at `position` with no rotation and scale 1.
    pub fn from_position(position: Vec2) -> Self {
        Self { position, ..Self::default() }
    }
}

/// Linear and angular velocity.
#[script_component]
#[derive(Debug, Default, PartialEq)]
pub struct Velocity {
    /// World units per second.
    pub linear: Vec2,
    /// Radians per second, counter-clockwise.
    pub angular: f32,
}

/// Handle of the mesh an entity is drawn with. Meshes are collected into
/// `RenderFrame::meshes`; what the id means is up to the game's renderer.
#[script_component]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MeshHandle {
    /// Mesh id (0 = none; the engine gives it no further meaning).
    pub id: u32,
}

/// Handle of a texture in the [`Textures`](crate::engine::render::Textures) store.
#[script_component]
#[derive(Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureHandle {
    /// Texture id; 0 is "no texture" (draw plain white, so the tint is the colour).
    pub id: u32,
}

impl TextureHandle {
    /// "No texture": a sprite with it is drawn as a flat white quad times its tint.
    pub const NONE: TextureHandle = TextureHandle { id: 0 };

    pub fn is_none(self) -> bool {
        self.id == 0
    }
}

// A `TextureHandle` inside another script-visible struct is one `u32`.
impl ScriptField for TextureHandle {
    const PRIM: efevm::Prim = efevm::Prim::U32;
    const COUNT: u32 = 1;
}

/// A camera. Its position and rotation are those of its entity's `Transform` (or
/// `GlobalTransform`, once propagated); its scale is ignored. One camera of each kind (2D, 3D) is used per
/// frame: the active one with the lowest entity id (see `RenderFrame`).
#[script_component]
#[derive(Debug, PartialEq)]
pub struct Camera {
    /// 0 = orthographic 2D (drawn by the `Renderer2D`), 1 = perspective 3D (drawn by the `Renderer3D`).
    pub kind: u32,
    /// Orthographic magnification: pixels per world unit. 1 = one world unit is one pixel.
    pub zoom: f32,
    /// Near plane (view-space distance). The 2D default is negative so z = 0 is inside.
    pub near: f32,
    /// Far plane.
    pub far: f32,
    /// Vertical field of view in radians (perspective only).
    pub fov_y: f32,
    /// Colour the target is cleared to (linear RGBA).
    pub clear_color: [f32; 4],
    /// Non-zero: a candidate for the one camera of its kind the frame uses.
    pub active: u32,
    /// Where this camera renders: a window id, 0 = the main window. Reserved for the
    /// multi-window phase; ignored today (carried into `CameraView::target`).
    pub target: u32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            kind: 0,
            zoom: 1.0,
            near: -1000.0,
            far: 1000.0,
            fov_y: 60.0_f32.to_radians(),
            clear_color: [0.05, 0.05, 0.08, 1.0],
            active: 1,
            target: 0,
        }
    }
}

impl Camera {
    pub const ORTHOGRAPHIC: u32 = 0;
    pub const PERSPECTIVE: u32 = 1;

    /// An active orthographic 2D camera at zoom 1 (same as `Camera::default()`).
    pub fn orthographic_2d() -> Self {
        Self::default()
    }

    /// An active perspective 3D camera (default near 0.1, far 1000), drawn by the `Renderer3D`.
    pub fn perspective_3d() -> Self {
        Self { kind: Self::PERSPECTIVE, near: 0.1, far: 1000.0, ..Self::default() }
    }
}

/// A textured, tinted quad drawn at its entity's transform, centred on the origin.
#[script_component]
#[derive(Debug, PartialEq)]
pub struct Sprite {
    /// The texture; id 0 = none (flat white).
    pub texture: TextureHandle,
    /// RGBA tint multiplied with the texture.
    pub color: [f32; 4],
    /// Size in world units. Used literally (zero = nothing visible) when `auto_size` is 0.
    pub size: Vec2,
    /// Texture coordinates of the quad's top-left corner (0,0 = top-left of the image).
    pub uv_min: Vec2,
    /// Texture coordinates of the quad's bottom-right corner.
    pub uv_max: Vec2,
    /// Draw order: higher layers are drawn later (on top).
    pub layer: i32,
    /// Bit 0 mirrors horizontally, bit 1 vertically.
    pub flip: u32,
    /// Non-zero: the size is the texture's pixel size and `size` is ignored. Zero: `size` is
    /// used as it is, so a `size` of 0 draws a zero-size (invisible) quad.
    pub auto_size: u32,
}

impl Sprite {
    pub const FLIP_X: u32 = 1;
    pub const FLIP_Y: u32 = 2;

    /// A white, full-texture sprite with `texture`, drawn at the texture's pixel size.
    pub fn new(texture: TextureHandle) -> Self {
        Self { texture, ..Self::default() }
    }

    /// Use `size` (world units) instead of the texture's pixel size.
    pub fn with_size(mut self, size: Vec2) -> Self {
        self.size = size;
        self.auto_size = 0;
        self
    }
}

impl Default for Sprite {
    fn default() -> Self {
        Self {
            texture: TextureHandle::NONE,
            color: [1.0; 4],
            size: Vec2::ZERO,
            uv_min: Vec2::ZERO,
            uv_max: Vec2::ONE,
            layer: 0,
            flip: 0,
            auto_size: 1,
        }
    }
}

/// Whether an entity is drawn. A missing component means visible. Not inherited by children.
#[script_component]
#[derive(Debug, PartialEq, Eq)]
pub struct Visibility {
    /// Non-zero: drawn.
    pub visible: u32,
}

impl Default for Visibility {
    fn default() -> Self {
        Self { visible: 1 }
    }
}

impl Visibility {
    pub const VISIBLE: Visibility = Visibility { visible: 1 };
    pub const HIDDEN: Visibility = Visibility { visible: 0 };
}

/// The entity this one is attached to: its `Transform` is relative to the parent's. A parent
/// that is gone (or has no `Transform`) makes this entity a root.
#[script_component]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parent {
    /// The parent, as `entity_to_bits` encodes it (0 = none).
    #[script(entity)]
    pub entity: u64,
}

impl Parent {
    pub fn new(e: hecs::Entity) -> Self {
        Self { entity: crate::engine::types::entity_to_bits(e) }
    }

    pub fn entity(&self) -> Option<hecs::Entity> {
        crate::engine::types::entity_from_bits(self.entity)
    }
}

/// The entities whose `Parent` is this one, in entity-id order. Rust-only; rebuilt every
/// frame by the transform propagation system. Read-only: the list is private, game code reads
/// it with [`entities`](Self::entities) and cannot build or change one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Children(Vec<hecs::Entity>);

impl Children {
    pub(crate) fn new(list: Vec<hecs::Entity>) -> Self {
        Self(list)
    }

    pub(crate) fn set(&mut self, list: Vec<hecs::Entity>) {
        self.0 = list;
    }

    /// The children, in entity-id order.
    pub fn entities(&self) -> &[hecs::Entity] {
        &self.0
    }

    pub fn iter(&self) -> impl Iterator<Item = hecs::Entity> + '_ {
        self.0.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// World-space 2D affine transform, computed every frame in `PostUpdate` from `Transform`
/// and the `Parent` chain. Read-only: the fields are private (read them with the getters) and
/// scripts get status 5 for `set`/`insert`/`remove`.
///
/// The matrix columns are `x_axis` / `y_axis` (the local axes, scale and rotation included)
/// and `translation`; it maps local points to world as `p' = x_axis * p.x + y_axis * p.y + translation`.
#[script_component(tracked, readonly)]
#[derive(Debug, PartialEq)]
pub struct GlobalTransform {
    /// World direction (and length) of the local x axis.
    x_axis: Vec2,
    /// World direction (and length) of the local y axis.
    y_axis: Vec2,
    /// World position of the origin.
    translation: Vec2,
}

impl GlobalTransform {
    pub(crate) const IDENTITY: GlobalTransform = GlobalTransform { x_axis: Vec2::X, y_axis: Vec2::Y, translation: Vec2::ZERO };

    pub(crate) fn from_affine(a: Affine2) -> Self {
        Self { x_axis: a.matrix2.x_axis, y_axis: a.matrix2.y_axis, translation: a.translation }
    }

    /// An identity value for tests that need to spawn one. Not for game code: the engine
    /// computes every `GlobalTransform`.
    #[doc(hidden)]
    pub fn __identity_for_tests() -> Self {
        Self::IDENTITY
    }

    /// World direction (and length) of the local x axis.
    pub fn x_axis(&self) -> Vec2 {
        self.x_axis
    }

    /// World direction (and length) of the local y axis.
    pub fn y_axis(&self) -> Vec2 {
        self.y_axis
    }

    /// World position of the origin.
    pub fn translation(&self) -> Vec2 {
        self.translation
    }

    pub fn affine(&self) -> Affine2 {
        Affine2::from_cols(self.x_axis, self.y_axis, self.translation)
    }

    /// World position of the origin.
    pub fn position(&self) -> Vec2 {
        self.translation
    }

    /// World rotation in radians (of the x axis).
    pub fn rotation(&self) -> f32 {
        self.x_axis.y.atan2(self.x_axis.x)
    }

    /// As a 4x4 matrix (z untouched), column-major when flattened with `to_cols_array_2d`.
    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_cols(
            glam::vec4(self.x_axis.x, self.x_axis.y, 0.0, 0.0),
            glam::vec4(self.y_axis.x, self.y_axis.y, 0.0, 0.0),
            glam::Vec4::Z,
            glam::vec4(self.translation.x, self.translation.y, 0.0, 1.0),
        )
    }
}

impl Transform {
    /// The local matrix: scale, then rotate, then translate.
    pub fn affine(&self) -> Affine2 {
        Affine2::from_scale_angle_translation(self.scale, self.rotation, self.position)
    }
}

/// A display name. Rust-only: not `Pod`, not visible to scripts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Name(pub String);

impl Name {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::ScriptType;
    use efevm::Prim;
    use std::mem::offset_of;

    #[test]
    fn layouts_match_offset_of() {
        let t = Transform::layout();
        assert_eq!((t.name.as_str(), t.size, t.align), ("Transform", size_of::<Transform>() as u32, align_of::<Transform>() as u32));
        let f: Vec<_> = t.fields.iter().map(|f| (f.name.as_str(), f.offset, f.ty, f.count)).collect();
        assert_eq!(f, [
            ("position", offset_of!(Transform, position) as u32, Prim::F32, 2),
            ("rotation", offset_of!(Transform, rotation) as u32, Prim::F32, 1),
            ("scale", offset_of!(Transform, scale) as u32, Prim::F32, 2),
        ]);
        assert_eq!(t.doc, "Position, rotation and scale of an entity in 2D world space.");
        assert_eq!(t.fields[0].doc, "World position.");
        t.validate().unwrap();

        let v = Velocity::layout();
        let f: Vec<_> = v.fields.iter().map(|f| (f.name.as_str(), f.offset, f.ty, f.count)).collect();
        assert_eq!(f, [
            ("linear", offset_of!(Velocity, linear) as u32, Prim::F32, 2),
            ("angular", offset_of!(Velocity, angular) as u32, Prim::F32, 1),
        ]);
        assert_eq!(v.size, size_of::<Velocity>() as u32);
        v.validate().unwrap();

        let m = MeshHandle::layout();
        assert_eq!((m.size, m.align), (4, 4));
        assert_eq!((m.fields[0].name.as_str(), m.fields[0].offset, m.fields[0].ty, m.fields[0].count), ("id", 0, Prim::U32, 1));
        m.validate().unwrap();
    }

    #[test]
    fn transform_default_scale_is_one() {
        let t = Transform::default();
        assert_eq!(t.scale, Vec2::ONE);
        assert_eq!(Transform::from_position(Vec2::new(1.0, 2.0)).position, Vec2::new(1.0, 2.0));
    }
}
