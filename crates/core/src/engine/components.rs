//! Built-in components (ROADMAP 3.1). The script-visible ones are `#[repr(C)]` and `Pod`,
//! so they can be copied to and from VM memory as bytes; they are registered in the
//! [`TypeRegistry`](crate::engine::types::TypeRegistry) by `Engine::with_config`.

use efeengine_macros::ScriptType;
use glam::Vec2;

/// Position, rotation and scale of an entity in 2D world space.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
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
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
pub struct Velocity {
    /// World units per second.
    pub linear: Vec2,
    /// Radians per second, counter-clockwise.
    pub angular: f32,
}

/// Placeholder handle of the mesh an entity is drawn with.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
pub struct MeshHandle {
    /// Mesh id (the placeholder renderer draws a triangle for any id).
    pub id: u32,
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
