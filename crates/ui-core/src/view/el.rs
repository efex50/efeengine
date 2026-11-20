//! `El<Msg>`: a description of one node and its children (API.md section 16).

use std::ops::RangeInclusive;
use std::sync::Arc;


use crate::node::{ImageFit, ImageId, NodeKind};
use crate::signal::{PointerEvent, UiSignalKind, UiValue};
#[allow(unused_imports)]
use crate::style::{Align, Border, Color, Direction, Edges, FontId, Justify, Size, Style, TextAlign, style_fields};

type Map<A, Msg> = Arc<dyn Fn(A) -> Msg + Send + Sync>;

/// The message constructors of one element.
pub struct Handlers<Msg> {
    pub click: Option<Msg>,
    pub change: Option<Map<f32, Msg>>,
    pub toggle: Option<Map<bool, Msg>>,
    pub input: Option<Map<String, Msg>>,
    pub submit: Option<Map<String, Msg>>,
    pub focus: Option<Map<bool, Msg>>,
    pub pointer: Option<Map<PointerEvent, Msg>>,
}

impl<Msg> Default for Handlers<Msg> {
    fn default() -> Self {
        Self { click: None, change: None, toggle: None, input: None, submit: None, focus: None, pointer: None }
    }
}

impl<Msg: Clone> Handlers<Msg> {
    pub fn is_empty(&self) -> bool {
        self.click.is_none()
            && self.change.is_none()
            && self.toggle.is_none()
            && self.input.is_none()
            && self.submit.is_none()
            && self.focus.is_none()
            && self.pointer.is_none()
    }

    /// The message for a signal of the widget these handlers belong to.
    pub fn dispatch(&self, kind: &UiSignalKind) -> Option<Msg> {
        match kind {
            UiSignalKind::Click => self.click.clone(),
            UiSignalKind::Changed(UiValue::F32(v)) => self.change.as_ref().map(|f| f(*v)),
            UiSignalKind::Changed(UiValue::Bool(b)) => self.toggle.as_ref().map(|f| f(*b)),
            UiSignalKind::Changed(UiValue::Text(t)) => self.input.as_ref().map(|f| f(t.clone())),
            UiSignalKind::Submit(t) => self.submit.as_ref().map(|f| f(t.clone())),
            UiSignalKind::Focus(b) => self.focus.as_ref().map(|f| f(*b)),
            UiSignalKind::Hover(_) => None,
            UiSignalKind::Pointer { pos, buttons, wheel } => {
                self.pointer.as_ref().map(|f| f(PointerEvent { pos: *pos, buttons: *buttons, wheel: *wheel }))
            }
        }
    }

    fn map<N: Clone + Send + Sync + 'static>(self, f: &Map<Msg, N>) -> Handlers<N>
    where
        Msg: Send + Sync + 'static,
    {
        fn wrap<A: 'static, Msg: Send + Sync + 'static, N: Send + Sync + 'static>(
            h: Option<Map<A, Msg>>,
            f: &Map<Msg, N>,
        ) -> Option<Map<A, N>> {
            h.map(|h| {
                let f = f.clone();
                Arc::new(move |a: A| f(h(a))) as Map<A, N>
            })
        }
        Handlers {
            click: self.click.map(|m| f(m)),
            change: wrap(self.change, f),
            toggle: wrap(self.toggle, f),
            input: wrap(self.input, f),
            submit: wrap(self.submit, f),
            focus: wrap(self.focus, f),
            pointer: wrap(self.pointer, f),
        }
    }
}

/// A node of the view: kind data, classes, inline style, key, flags, children and message
/// handlers. Built by the constructor functions of [`view`](crate::view) (`text`, `button`,
/// `column`, ...) and chained setters; the reconciler turns it into retained-tree operations.
pub struct El<Msg> {
    pub(crate) kind: NodeKind,
    pub(crate) classes: Vec<String>,
    pub(crate) style: Style,
    pub(crate) key: Option<String>,
    pub(crate) visible: bool,
    pub(crate) enabled: bool,
    pub(crate) slot: bool,
    pub(crate) children: Vec<El<Msg>>,
    pub(crate) handlers: Handlers<Msg>,
}

impl<Msg> std::fmt::Debug for El<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("El")
            .field("kind", &self.kind)
            .field("key", &self.key)
            .field("classes", &self.classes)
            .field("children", &self.children)
            .finish()
    }
}

/// Anything that can be added as children: one `El`, or any iterator / collection of them
/// (`Vec`, `Option`, `map(..)` results), so `column![a, items.iter().map(row_view), b]` works.
pub trait IntoChildren<Msg> {
    fn append_to(self, out: &mut Vec<El<Msg>>);
}

impl<Msg> IntoChildren<Msg> for El<Msg> {
    fn append_to(self, out: &mut Vec<El<Msg>>) {
        out.push(self);
    }
}

impl<Msg, I: IntoIterator<Item = El<Msg>>> IntoChildren<Msg> for I {
    fn append_to(self, out: &mut Vec<El<Msg>>) {
        out.extend(self);
    }
}

impl<Msg> El<Msg> {
    pub(crate) fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            classes: Vec::new(),
            style: Style::default(),
            key: None,
            visible: true,
            enabled: true,
            slot: false,
            children: Vec::new(),
            handlers: Handlers::default(),
        }
    }

    /// The node kind this element describes.
    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    pub fn children_list(&self) -> &[El<Msg>] {
        &self.children
    }

    /// Adds a class (order matters for theme resolution, like `NodeRef::class`).
    pub fn class(mut self, name: &str) -> Self {
        if !self.classes.iter().any(|c| c == name) {
            self.classes.push(name.to_string());
        }
        self
    }

    pub fn classes(mut self, names: &[&str]) -> Self {
        self.classes = names.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Identity among the siblings (see the key scheme in [`view`](crate::view)). Without a key a
    /// child is matched by its position.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn visible(mut self, v: bool) -> Self {
        self.visible = v;
        self
    }

    pub fn enabled(mut self, v: bool) -> Self {
        self.enabled = v;
        self
    }

    /// Sets the inline style as a whole.
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// Adds children: an `El`, or an iterator of them.
    pub fn child(mut self, c: impl IntoChildren<Msg>) -> Self {
        c.append_to(&mut self.children);
        self
    }

    /// Same as [`child`](Self::child), named for iterators.
    pub fn children(self, c: impl IntoChildren<Msg>) -> Self {
        self.child(c)
    }

    pub fn placeholder(mut self, p: &str) -> Self {
        if let NodeKind::TextInput { placeholder, .. } = &mut self.kind {
            *placeholder = p.to_string();
        }
        self
    }

    /// Characters, 0 = unlimited. The value is cut to this length.
    pub fn max_len(mut self, n: u32) -> Self {
        if let NodeKind::TextInput { max_len, value, .. } = &mut self.kind {
            *max_len = n;
            if n > 0 && value.chars().count() > n as usize {
                *value = value.chars().take(n as usize).collect();
            }
        }
        self
    }

    pub fn password(mut self, on: bool) -> Self {
        if let NodeKind::TextInput { password, .. } = &mut self.kind {
            *password = on;
        }
        self
    }

    /// Slider step (0 = continuous).
    pub fn step(mut self, s: f32) -> Self {
        if let NodeKind::Slider { step, .. } = &mut self.kind {
            *step = s.max(0.0);
        }
        self
    }
}

macro_rules! def_el_style {
    ($($f:ident: $t:ty = $a:ty;)*) => {
        impl<Msg> El<Msg> {
            $(pub fn $f(mut self, v: $a) -> Self {
                self.style.$f = Some(v.into());
                self
            })*
        }
    };
}
style_fields!(def_el_style);

impl<Msg: Clone + Send + Sync + 'static> El<Msg> {
    /// Message for a click (button).
    pub fn on_click(mut self, msg: Msg) -> Self {
        self.handlers.click = Some(msg);
        self
    }

    /// Slider value changed (every edit). The closure may capture.
    pub fn on_change(mut self, f: impl Fn(f32) -> Msg + Send + Sync + 'static) -> Self {
        self.handlers.change = Some(Arc::new(f));
        self
    }

    /// Checkbox toggled.
    pub fn on_toggle(mut self, f: impl Fn(bool) -> Msg + Send + Sync + 'static) -> Self {
        self.handlers.toggle = Some(Arc::new(f));
        self
    }

    /// Text input edited (every edit, with the whole value).
    pub fn on_input(mut self, f: impl Fn(String) -> Msg + Send + Sync + 'static) -> Self {
        self.handlers.input = Some(Arc::new(f));
        self
    }

    /// Text input: Enter pressed.
    pub fn on_submit(mut self, f: impl Fn(String) -> Msg + Send + Sync + 'static) -> Self {
        self.handlers.submit = Some(Arc::new(f));
        self
    }

    /// Keyboard focus gained (`true`) or lost.
    pub fn on_focus(mut self, f: impl Fn(bool) -> Msg + Send + Sync + 'static) -> Self {
        self.handlers.focus = Some(Arc::new(f));
        self
    }

    /// GPU view: a pointer signal (move over the view, button, wheel) in view-relative logical
    /// px. Turns the view's `pointer` flag on.
    pub fn on_pointer(mut self, f: impl Fn(PointerEvent) -> Msg + Send + Sync + 'static) -> Self {
        if let NodeKind::GpuView { pointer, .. } = &mut self.kind {
            *pointer = true;
        }
        self.handlers.pointer = Some(Arc::new(f));
        self
    }

    /// Converts the message type of this element and all its children (Elm's `Html.map`), so a
    /// reusable component view with its own message enum can be embedded in a parent view:
    /// `component(&s.inner).map(Msg::Inner)`.
    pub fn map<N: Clone + Send + Sync + 'static>(self, f: impl Fn(Msg) -> N + Send + Sync + 'static) -> El<N> {
        let f: Map<Msg, N> = Arc::new(f);
        self.map_arc(&f)
    }

    fn map_arc<N: Clone + Send + Sync + 'static>(self, f: &Map<Msg, N>) -> El<N> {
        El {
            kind: self.kind,
            classes: self.classes,
            style: self.style,
            key: self.key,
            visible: self.visible,
            enabled: self.enabled,
            slot: self.slot,
            children: self.children.into_iter().map(|c| c.map_arc(f)).collect(),
            handlers: self.handlers.map(f),
        }
    }
}

// ----- constructors --------------------------------------------------------------------------

/// A text label.
pub fn text<Msg>(text: impl Into<String>) -> El<Msg> {
    El::new(NodeKind::Text { text: text.into() })
}

/// A button with a text label.
pub fn button<Msg>(label: impl Into<String>) -> El<Msg> {
    El::new(NodeKind::Button { label: label.into() })
}

/// A button whose content is its children (icon + text).
pub fn button_with<Msg>(children: impl IntoChildren<Msg>) -> El<Msg> {
    El::new(NodeKind::Button { label: String::new() }).child(children)
}

pub fn checkbox<Msg>(label: impl Into<String>, checked: bool) -> El<Msg> {
    El::new(NodeKind::Checkbox { label: label.into(), checked })
}

/// A slider; `value` is clamped into `range`.
pub fn slider<Msg>(range: RangeInclusive<f32>, value: f32) -> El<Msg> {
    El::new(NodeKind::slider(*range.start(), *range.end(), value))
}

pub fn text_input<Msg>(value: impl Into<String>) -> El<Msg> {
    El::new(NodeKind::text_input(value))
}

pub fn image<Msg>(texture: ImageId, fit: ImageFit) -> El<Msg> {
    El::new(NodeKind::Image { texture, fit })
}

/// A view the game draws with wgpu: the painter registered with `add_ui_painter(name, ..)`.
/// Give it a size (`.width(..)`, `.height(..)`); add `.on_pointer(..)` for pointer input.
pub fn gpu_view<Msg>(painter: &str) -> El<Msg> {
    El::new(NodeKind::gpu_view(painter))
}

/// A flex box without a fixed direction (set it with `.direction(..)`).
pub fn container<Msg>() -> El<Msg> {
    El::new(NodeKind::Container)
}

pub fn row<Msg>() -> El<Msg> {
    El::new(NodeKind::Container).direction(crate::style::Direction::Row)
}

pub fn column<Msg>() -> El<Msg> {
    El::new(NodeKind::Container).direction(crate::style::Direction::Column)
}

/// Clips its children and scrolls.
pub fn scroll<Msg>() -> El<Msg> {
    El::new(NodeKind::Scroll)
}

/// An empty container that the reconciler never looks inside: its children are managed with the
/// retained API (or by a script or mod). `key` is required, so the node can be found
/// (`ui.find("<view>/<key>")`).
pub fn slot<Msg>(key: impl Into<String>) -> El<Msg> {
    let mut e = El::new(NodeKind::Container);
    e.slot = true;
    e.key = Some(key.into());
    e
}

/// `column![a, b, iter]`: a column of the given elements; each argument is an `El` or anything
/// iterable of `El`.
#[macro_export]
macro_rules! column {
    ($($c:expr),* $(,)?) => { $crate::view::column() $(.child($c))* };
}

/// `row![a, b, iter]`, like [`column!`].
#[macro_export]
macro_rules! row {
    ($($c:expr),* $(,)?) => { $crate::view::row() $(.child($c))* };
}
