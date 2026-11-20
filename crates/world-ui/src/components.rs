//! Components of world-space UI. Display only: nothing here takes input.
//!
//! A root element is any entity with an [`Anchor2D`]. It is drawn centred on its anchor point
//! (after `offset`), where the anchor is a world position projected through the 2D camera each
//! frame. Sizes and offsets are logical pixels, so labels keep their size when the camera zooms.
//! A [`Container2D`] stacks the entities it lists in `children`; those children are drawn by
//! their container and must not carry an `Anchor2D` of their own.

use efeengine_core::prelude::{Entity, TextureHandle, Vec2};

/// RGBA, 0..1, not premultiplied.
pub type Color = [f32; 4];

/// Where a root element is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Follows the world position of this entity (its `GlobalTransform`, else its `Transform`).
    /// Skipped while the entity has no transform.
    Entity(Entity),
    /// A fixed world point.
    Point(Vec2),
}

/// Places a root element. `offset` is in logical pixels from the anchor point, +y down
/// (so `Vec2::new(0.0, -40.0)` sits 40 px above the anchor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor2D {
    pub target: Anchor,
    pub offset: Vec2,
}

impl Anchor2D {
    pub fn entity(e: Entity) -> Self {
        Self { target: Anchor::Entity(e), offset: Vec2::ZERO }
    }

    pub fn point(p: Vec2) -> Self {
        Self { target: Anchor::Point(p), offset: Vec2::ZERO }
    }

    pub fn with_offset(mut self, offset: Vec2) -> Self {
        self.offset = offset;
        self
    }
}

/// A line of text, shaped with the bundled Inter faces. Size in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Text2D {
    pub text: String,
    pub size: f32,
    pub color: Color,
    pub bold: bool,
}

impl Default for Text2D {
    fn default() -> Self {
        Self { text: String::new(), size: 16.0, color: [1.0, 1.0, 1.0, 1.0], bold: false }
    }
}

/// A solid or rounded rectangle, optionally with a border drawn inside its edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Rect2D {
    pub size: Vec2,
    pub color: Color,
    /// Corner radius in logical pixels; `size.min_element() / 2` gives a circle or a pill.
    pub radius: f32,
    /// Border width in logical pixels; 0 = no border.
    pub border: f32,
    pub border_color: Color,
}

impl Default for Rect2D {
    fn default() -> Self {
        Self { size: Vec2::splat(16.0), color: [1.0, 1.0, 1.0, 1.0], radius: 0.0, border: 0.0, border_color: [0.0, 0.0, 0.0, 1.0] }
    }
}

/// A bar showing `fraction` (0..1) of its length. The displayed fill (`shown`) moves toward
/// `fraction` at `speed` fractions per second. A lost part, from the displayed fill up to the
/// old value (`ghost`), is drawn in `ghost_color` and shrinks more slowly, so a hit reads
/// as a flash. `shown` and `ghost` are owned by the engine's system; set only `fraction`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar2D {
    pub size: Vec2,
    pub fraction: f32,
    pub speed: f32,
    pub back: Color,
    pub fill: Color,
    pub ghost_color: Color,
    /// Grows upward from the bottom instead of rightward.
    pub vertical: bool,
    /// Displayed fill, set by the system. Starts at `fraction`.
    pub shown: f32,
    /// Displayed ghost edge, set by the system. Starts at `fraction`.
    pub ghost: f32,
}

impl Bar2D {
    /// A bar that starts full of `fraction` (no animation on spawn).
    pub fn new(size: Vec2, fraction: f32, fill: Color, back: Color) -> Self {
        let f = fraction.clamp(0.0, 1.0);
        Self {
            size,
            fraction: f,
            speed: 1.0,
            back,
            fill,
            ghost_color: [1.0, 1.0, 1.0, 0.85],
            vertical: false,
            shown: f,
            ghost: f,
        }
    }
}

/// An image from the engine's `Textures` store, drawn with `tint` multiplied in.
#[derive(Clone, Debug, PartialEq)]
pub struct Sprite2D {
    pub texture: TextureHandle,
    pub size: Vec2,
    pub tint: Color,
}

/// Stacks its `children` in a row or column with `gap` between them, inside `padding`, over an
/// optional `background`. Its size is that of the stack, so it is centred on its anchor like any
/// other root.
#[derive(Clone, Debug, PartialEq)]
pub struct Container2D {
    pub children: Vec<Entity>,
    pub vertical: bool,
    pub gap: f32,
    pub padding: f32,
    pub background: Option<Color>,
}

impl Default for Container2D {
    fn default() -> Self {
        Self { children: Vec::new(), vertical: false, gap: 0.0, padding: 0.0, background: None }
    }
}

/// Makes a root `Text2D` rise and fade, then despawns it: a damage number. The anchor's offset
/// moves up at `rise` px per second and the text fades out over `life` seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floating2D {
    pub rise: f32,
    pub life: f32,
    /// Seconds since spawn, advanced by the system.
    pub age: f32,
}

impl Floating2D {
    pub fn new(rise: f32, life: f32) -> Self {
        Self { rise, life: life.max(f32::EPSILON), age: 0.0 }
    }
}
