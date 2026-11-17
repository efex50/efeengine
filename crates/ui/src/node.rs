//! Node kinds, nodes and layers (API.md sections 3 and 4).

use std::sync::Arc;

use efeengine_core::prelude::TextureHandle;
use smallvec::SmallVec;

use crate::id::{Owner, WidgetId};
use crate::style::Style;

/// Interned class name (see [`UiTree::class_id`](crate::UiTree::class_id)).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClassId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ImageFit {
    Contain,
    Cover,
    Fill,
    None,
}

/// The node kinds every backend implements.
#[derive(Clone, PartialEq, Debug)]
pub enum NodeKind {
    /// Flex box: row or column (`Style::direction`).
    Container,
    Text { text: String },
    Image { texture: TextureHandle, fit: ImageFit },
    /// A label, or children instead (icon + text).
    Button { label: String },
    Checkbox { label: String, checked: bool },
    Slider { min: f32, max: f32, step: f32, value: f32 },
    /// `max_len` counts chars, 0 = unlimited.
    TextInput { value: String, placeholder: String, max_len: u32, password: bool },
    /// Clips children, scrolls on the main axis.
    Scroll,
}

/// [`NodeKind`] without its data: the key of per-kind theme styles.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KindTag {
    Container,
    Text,
    Image,
    Button,
    Checkbox,
    Slider,
    TextInput,
    Scroll,
}

impl KindTag {
    pub const ALL: [KindTag; 8] = [
        KindTag::Container,
        KindTag::Text,
        KindTag::Image,
        KindTag::Button,
        KindTag::Checkbox,
        KindTag::Slider,
        KindTag::TextInput,
        KindTag::Scroll,
    ];

    /// Lowercase name, as used in the web stylesheet (`.efui-k-<name>`).
    pub fn name(self) -> &'static str {
        match self {
            KindTag::Container => "container",
            KindTag::Text => "text",
            KindTag::Image => "image",
            KindTag::Button => "button",
            KindTag::Checkbox => "checkbox",
            KindTag::Slider => "slider",
            KindTag::TextInput => "text-input",
            KindTag::Scroll => "scroll",
        }
    }
}

impl NodeKind {
    pub fn tag(&self) -> KindTag {
        match self {
            NodeKind::Container => KindTag::Container,
            NodeKind::Text { .. } => KindTag::Text,
            NodeKind::Image { .. } => KindTag::Image,
            NodeKind::Button { .. } => KindTag::Button,
            NodeKind::Checkbox { .. } => KindTag::Checkbox,
            NodeKind::Slider { .. } => KindTag::Slider,
            NodeKind::TextInput { .. } => KindTag::TextInput,
            NodeKind::Scroll => KindTag::Scroll,
        }
    }

    /// A slider kind; `min`/`max` are ordered and `value` clamped.
    pub fn slider(min: f32, max: f32, value: f32) -> NodeKind {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        NodeKind::Slider { min, max, step: 0.0, value: value.clamp(min, max) }
    }

    pub fn text_input(value: impl Into<String>) -> NodeKind {
        NodeKind::TextInput { value: value.into(), placeholder: String::new(), max_len: 0, password: false }
    }
}

/// Where a layer's root sits in the window.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Anchor {
    Fill,
    Center,
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

/// A named root of the tree, drawn in `z` order.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Layer {
    pub z: i32,
    /// A visible modal layer captures all pointer and keyboard input below it.
    pub modal: bool,
    pub anchor: Anchor,
    /// Logical-pixel offset from the anchor.
    pub offset: (f32, f32),
}

impl Layer {
    pub fn anchored(z: i32, anchor: Anchor) -> Self {
        Self { z, modal: false, anchor, offset: (0.0, 0.0) }
    }

    pub fn fill(z: i32) -> Self {
        Self::anchored(z, Anchor::Fill)
    }

    pub fn centered(z: i32) -> Self {
        Self::anchored(z, Anchor::Center)
    }

    pub fn top_left(z: i32) -> Self {
        Self::anchored(z, Anchor::TopLeft)
    }

    pub fn top_right(z: i32) -> Self {
        Self::anchored(z, Anchor::TopRight)
    }

    pub fn bottom_left(z: i32) -> Self {
        Self::anchored(z, Anchor::BottomLeft)
    }

    pub fn bottom_right(z: i32) -> Self {
        Self::anchored(z, Anchor::BottomRight)
    }

    pub fn modal(mut self) -> Self {
        self.modal = true;
        self
    }

    pub fn offset(mut self, x: f32, y: f32) -> Self {
        self.offset = (x, y);
        self
    }
}

/// One node of the tree. Read through [`UiTree::get`](crate::UiTree::get); changes go through
/// the tree so they reach the journal.
#[derive(Clone, Debug)]
pub struct Node {
    pub(crate) kind: NodeKind,
    pub(crate) parent: Option<WidgetId>,
    pub(crate) children: Vec<WidgetId>,
    pub(crate) classes: SmallVec<[ClassId; 2]>,
    pub(crate) style: Style,
    pub(crate) visible: bool,
    pub(crate) enabled: bool,
    pub(crate) key: Option<Arc<str>>,
    pub(crate) owner: Owner,
    pub(crate) layer: Option<Layer>,
    pub(crate) hover_signals: bool,
}

impl Node {
    pub(crate) fn new(kind: NodeKind, parent: Option<WidgetId>, owner: Owner) -> Self {
        Self {
            kind,
            parent,
            children: Vec::new(),
            classes: SmallVec::new(),
            style: Style::default(),
            visible: true,
            enabled: true,
            key: None,
            owner,
            layer: None,
            hover_signals: false,
        }
    }

    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    pub fn tag(&self) -> KindTag {
        self.kind.tag()
    }

    pub fn parent(&self) -> Option<WidgetId> {
        self.parent
    }

    pub fn children(&self) -> &[WidgetId] {
        &self.children
    }

    pub fn classes(&self) -> &[ClassId] {
        &self.classes
    }

    /// Inline style overrides.
    pub fn style(&self) -> &Style {
        &self.style
    }

    /// The node's own flag (an invisible ancestor hides it too: see
    /// [`UiTree::is_shown`](crate::UiTree::is_shown)).
    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn owner(&self) -> &Owner {
        &self.owner
    }

    /// `Some` for a layer root.
    pub fn layer(&self) -> Option<&Layer> {
        self.layer.as_ref()
    }

    /// Whether the node sends `Hover` signals.
    pub fn hover_signals(&self) -> bool {
        self.hover_signals
    }
}
