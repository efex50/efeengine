//! The style subset (API.md section 5). Every field is an `Option`: `None` inherits from the
//! classes, then the theme.

/// Opaque font handle from the font registry ([`UiTree::load_font`](crate::UiTree::load_font)).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Direction {
    Row,
    Column,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Justify {
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Cross-axis alignment.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextAlign {
    Start,
    Center,
    End,
}

/// Logical-pixel edge sizes (padding).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const ZERO: Edges = Edges { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 };

    pub fn all(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }

    /// `h` for left and right, `v` for top and bottom.
    pub fn symmetric(h: f32, v: f32) -> Self {
        Self { top: v, right: h, bottom: v, left: h }
    }

    pub fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self { top, right, bottom, left }
    }
}

impl From<f32> for Edges {
    fn from(v: f32) -> Self {
        Edges::all(v)
    }
}

impl From<(f32, f32)> for Edges {
    /// `(horizontal, vertical)`.
    fn from((h, v): (f32, f32)) -> Self {
        Edges::symmetric(h, v)
    }
}

/// Width or height.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Size {
    Auto,
    Px(f32),
    /// Take the remaining space.
    Fill,
    /// Share the remaining space n:m between siblings.
    Portion(u16),
}

impl From<f32> for Size {
    fn from(v: f32) -> Self {
        Size::Px(v)
    }
}

/// Linear RGBA in 0..=1 (sRGB values as written; backends do not convert).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// From `0xRRGGBB`.
    pub const fn hex(rgb: u32) -> Self {
        Self::rgb(((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0)
    }

    /// `[r, g, b, a]` as 8-bit values.
    pub fn to_rgba8(self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
}

/// Border width, color and one radius for all corners.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Border {
    pub width: f32,
    pub color: Color,
    pub radius: f32,
}

impl Border {
    pub const fn new(width: f32, color: Color, radius: f32) -> Self {
        Self { width, color, radius }
    }
}

macro_rules! style_fields {
    ($cb:ident) => {
        $cb! {
            direction: Direction = impl Into<Direction>;
            justify: Justify = impl Into<Justify>;
            align: Align = impl Into<Align>;
            gap: f32 = f32;
            padding: Edges = impl Into<Edges>;
            width: Size = impl Into<Size>;
            height: Size = impl Into<Size>;
            min_width: f32 = f32;
            max_width: f32 = f32;
            min_height: f32 = f32;
            max_height: f32 = f32;
            background: Color = impl Into<Color>;
            border: Border = impl Into<Border>;
            text_color: Color = impl Into<Color>;
            font_size: f32 = f32;
            font: FontId = impl Into<FontId>;
            bold: bool = bool;
            text_align: TextAlign = impl Into<TextAlign>;
            opacity: f32 = f32;
        }
    };
}
pub(crate) use style_fields;

macro_rules! def_style {
    ($($f:ident: $t:ty = $a:ty;)*) => {
        /// One struct, rendered as CSS, iced layout and style, or taffy. A property exists only
        /// if all backends implement it.
        #[derive(Clone, Copy, PartialEq, Debug, Default)]
        pub struct Style {
            $(pub $f: Option<$t>,)*
        }

        impl Style {
            pub fn new() -> Self {
                Self::default()
            }

            /// Puts every field that is set in `over` on top of `self`.
            pub fn merge(&mut self, over: &Style) {
                $(if over.$f.is_some() { self.$f = over.$f; })*
            }

            /// `self` with `over` on top.
            pub fn overlaid(mut self, over: &Style) -> Style {
                self.merge(over);
                self
            }

            /// `true` if no field is set.
            pub fn is_empty(&self) -> bool {
                true $(&& self.$f.is_none())*
            }

            $(pub fn $f(mut self, v: $a) -> Self {
                self.$f = Some(v.into());
                self
            })*
        }
    };
}
style_fields!(def_style);
