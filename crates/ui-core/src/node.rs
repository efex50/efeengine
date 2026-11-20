//! Node kinds, nodes and layers (API.md sections 3 and 4).

use std::sync::Arc;

use smallvec::SmallVec;

use crate::id::{Owner, WidgetId};
use crate::style::Style;

/// Interned class name (see [`UiTree::class_id`](crate::UiTree::class_id)).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClassId(pub u32);

/// Opaque key of an image the host provides (the adapter maps it to its own texture or asset
/// handle). The tree never looks inside.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ImageId(pub u32);

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
    Image { texture: ImageId, fit: ImageFit },
    /// A label, or children instead (icon + text).
    Button { label: String },
    Checkbox { label: String, checked: bool },
    Slider { min: f32, max: f32, step: f32, value: f32 },
    /// `max_len` counts chars, 0 = unlimited.
    TextInput { value: String, placeholder: String, max_len: u32, password: bool },
    /// Clips children, scrolls on the main axis.
    Scroll,
    /// A view the game draws with wgpu: the [`UiPainter`](crate::UiPainter) registered under
    /// `painter` (API.md section 17). Sized and clipped like an `Image`; an `Auto` size is
    /// empty (give it a size). `pointer`: send `Pointer` signals.
    GpuView { painter: Arc<str>, pointer: bool },
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
    GpuView,
}

impl KindTag {
    pub const ALL: [KindTag; 9] = [
        KindTag::Container,
        KindTag::Text,
        KindTag::Image,
        KindTag::Button,
        KindTag::Checkbox,
        KindTag::Slider,
        KindTag::TextInput,
        KindTag::Scroll,
        KindTag::GpuView,
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
            KindTag::GpuView => "gpu-view",
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
            NodeKind::GpuView { .. } => KindTag::GpuView,
        }
    }

    /// A slider kind; `min`/`max` are ordered and `value` clamped (see [`normalize`](Self::normalize)).
    pub fn slider(min: f32, max: f32, value: f32) -> NodeKind {
        let mut k = NodeKind::Slider { min, max, step: 0.0, value };
        k.normalize();
        k
    }

    /// Makes the kind data consistent, never panics: a slider gets ordered limits (a NaN limit
    /// takes the other one, both NaN give `0..=1`), a non-negative finite `step` (else 0) and a
    /// `value` clamped to the limits (NaN becomes `min`); a text input's value is cut to
    /// `max_len` chars. The tree applies this after `set_kind` and `edit_kind`.
    pub fn normalize(&mut self) {
        match self {
            NodeKind::Slider { min, max, step, value } => {
                let (lo, hi) = match (min.is_nan(), max.is_nan()) {
                    (true, true) => (0.0, 1.0),
                    (true, false) => (*max, *max),
                    (false, true) => (*min, *min),
                    (false, false) => (min.min(*max), min.max(*max)),
                };
                *min = lo;
                *max = hi;
                if step.is_nan() || *step < 0.0 {
                    *step = 0.0;
                }
                *value = clamp_slider(lo, hi, *value);
            }
            NodeKind::TextInput { value, max_len, .. } => {
                if *max_len > 0 && value.chars().count() > *max_len as usize {
                    *value = value.chars().take(*max_len as usize).collect();
                }
            }
            _ => {}
        }
    }

    /// A GPU view drawn by the painter registered as `painter`, without pointer signals.
    pub fn gpu_view(painter: &str) -> NodeKind {
        NodeKind::GpuView { painter: painter.into(), pointer: false }
    }

    pub fn text_input(value: impl Into<String>) -> NodeKind {
        NodeKind::TextInput { value: value.into(), placeholder: String::new(), max_len: 0, password: false }
    }
}

/// `x` clamped to `min..=max` without `f32::clamp`'s panics: NaN `x` gives `min`, and `min`
/// and `max` are ordered first.
pub(crate) fn clamp_slider(min: f32, max: f32, x: f32) -> f32 {
    let (lo, hi) = (min.min(max), min.max(max));
    if x.is_nan() { lo } else { x.max(lo).min(hi) }
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
    /// The window the layer is shown in (a core `WindowId`'s number; 0 = the main window).
    /// `z` and `modal` only act inside this window. See [`Layer::window`] and
    /// `UiTree::layer_in`.
    pub window: u32,
}

impl Layer {
    pub fn anchored(z: i32, anchor: Anchor) -> Self {
        Self { z, modal: false, anchor, offset: (0.0, 0.0), window: 0 }
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

    /// Show the layer in `window` (a `WindowId` or its raw `u32`) instead of the main window.
    pub fn window(mut self, window: impl Into<u32>) -> Self {
        self.window = window.into();
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
    /// Creation order (monotonic over the tree's life); ties between equal-z layers.
    pub(crate) seq: u64,
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
            seq: 0,
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
