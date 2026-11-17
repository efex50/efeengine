//! Model -> iced `Element` tree, and the style mapping (API.md section 5, iced column).
//!
//! The mapping is a flexbox subset on top of iced's row / column / container:
//! - `direction` picks row or column, `gap` is `spacing`, `padding` and the size lengths go on a
//!   wrapping container that also paints `background` / `border` (with `opacity` folded into the
//!   colours).
//! - `Auto` is iced's `Length::Fit` (intrinsic size; `Fill` children inside a `Fit` parent are
//!   "fluid" and only stretch up to the parent's intrinsic size, which is CSS `align-items:
//!   stretch` in an auto-sized flex box).
//! - `align` is the cross axis (`Stretch`, the CSS default, makes children with an `Auto` cross
//!   size `Fill`).
//! - `justify` is emulated with `Fill` spacers around the children, only when the container's
//!   main size is definite (Fixed or Fill; there is no free space otherwise, same as CSS).
//! - `min_*` / `max_*` map to `Length::min` / `Length::max` (iced main's bounded lengths).

use iced_core::ContentFit;
use iced_core::alignment::{Horizontal, Vertical};
use iced_core::font::Weight;
use iced_core::Widget as _;
use iced_core::{Background, Border, Color as IColor, Font, Length, Padding, Theme};
use iced_widget::{button, checkbox, container, mouse_area, opaque, scrollable, slider, space, stack, text, text_input};
use iced_wgpu::Renderer;

use crate::id::WidgetId;
use crate::node::{Anchor, ImageFit, NodeKind};
use crate::style::{Align, Color, Direction, Edges, Justify, Size, Style, TextAlign};

use super::assets::{Assets, DEFAULT_FAMILY};
use super::gpuview::ViewProgram;
use super::keyed::Keyed;

use super::model::{DISABLED, FOCUSED, HOVERED, Model, NORMAL, PRESSED, VLayer, VNode};

/// What iced widgets send back.
#[derive(Clone, Debug)]
pub enum Msg {
    Click(WidgetId),
    Toggle(WidgetId, bool),
    Slide(WidgetId, f32),
    Input(WidgetId, String),
    Submit(WidgetId),
    Enter(WidgetId),
    Exit(WidgetId),
    Pointer(WidgetId, crate::signal::PointerEvent),
}

pub type Element<'a> = iced_core::Element<'a, Msg, Theme, Renderer>;

/// The iced widget id of a text input node (for focus queries).
pub fn iced_id(id: WidgetId) -> iced_core::widget::Id {
    iced_core::widget::Id::from(format!("efui-{}", id.to_bits()))
}

pub fn view<'a>(model: &'a Model, a: &'a Assets) -> Element<'a> {
    let keys = model.layers.iter().map(|l| l.root.id.to_bits()).collect();
    let layers: Vec<Element<'a>> = model.layers.iter().map(|l| layer_el(l, a)).collect();
    Keyed::new(keys, stack(layers).width(Length::Fill).height(Length::Fill)._boxed())._boxed()
}

fn layer_el<'a>(l: &'a VLayer, a: &'a Assets) -> Element<'a> {
    let fill = l.layer.anchor == Anchor::Fill;
    let root = node_el(&l.root, Parent::none(), fill, a);
    let (ax, ay) = alignment(l.layer.anchor);
    let (x, y) = l.layer.offset;
    let pad = |v: f32| (2.0 * v.max(0.0), 2.0 * (-v).max(0.0));
    let (pl, pr) = match l.layer.anchor {
        Anchor::Fill => (0.0, 0.0),
        Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => (x, 0.0),
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => (0.0, x),
        Anchor::Center | Anchor::Top | Anchor::Bottom => pad(x),
    };
    let (pt, pb) = match l.layer.anchor {
        Anchor::Fill => (0.0, 0.0),
        Anchor::TopLeft | Anchor::Top | Anchor::TopRight => (y, 0.0),
        Anchor::BottomLeft | Anchor::Bottom | Anchor::BottomRight => (0.0, y),
        Anchor::Center | Anchor::Left | Anchor::Right => pad(y),
    };
    let content = container(root)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(ax)
        .align_y(ay)
        .padding(Padding { top: pt, right: pr, bottom: pb, left: pl });
    if l.layer.modal {
        let backdrop = mouse_area(opaque(space().width(Length::Fill).height(Length::Fill)))
            .on_enter(Msg::Enter(l.root.id))
            .on_exit(Msg::Exit(l.root.id));
        stack([backdrop._boxed(), content._boxed()]).width(Length::Fill).height(Length::Fill)._boxed()
    } else {
        content._boxed()
    }
}

fn alignment(a: Anchor) -> (Horizontal, Vertical) {
    let h = match a {
        Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft | Anchor::Fill => Horizontal::Left,
        Anchor::Top | Anchor::Center | Anchor::Bottom => Horizontal::Center,
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => Horizontal::Right,
    };
    let v = match a {
        Anchor::TopLeft | Anchor::Top | Anchor::TopRight | Anchor::Fill => Vertical::Top,
        Anchor::Left | Anchor::Center | Anchor::Right => Vertical::Center,
        Anchor::BottomLeft | Anchor::Bottom | Anchor::BottomRight => Vertical::Bottom,
    };
    (h, v)
}

/// The parent's direction and whether it stretches children on the cross axis.
#[derive(Clone, Copy)]
struct Parent {
    dir: Option<Direction>,
    stretch: bool,
    /// The parent's own width / height is definite (Fixed or Fill). In an intrinsically sized
    /// parent a `Fill` (or stretched `Auto`) child must not claim the free space: iced would make
    /// the parent `Fill` too ("Fill wins over Fit"), and `Fluid(Max)` makes leaf widgets such as
    /// text take all the space. It stays `Fit`, i.e. the child keeps its own size, like a flex
    /// item in an auto-sized CSS box when nothing is left to share (only a stretch to the widest
    /// sibling is lost).
    w_def: bool,
    h_def: bool,
}

impl Parent {
    fn none() -> Self {
        Parent { dir: None, stretch: false, w_def: true, h_def: true }
    }
}

/// `definite`: the parent's size on this axis is definite.
fn length(s: Size, stretch: bool, definite: bool) -> Length {
    let fill = |l: Length| if definite { l } else { Length::Fit };
    match s {
        Size::Auto if stretch => fill(Length::Fill),
        Size::Auto => Length::Fit,
        Size::Px(v) => Length::Fixed(v),
        Size::Fill => fill(Length::Fill),
        Size::Portion(n) => fill(Length::FillPortion(n.max(1))),
    }
}

fn bounded(l: Length, min: Option<f32>, max: Option<f32>) -> Length {
    let l = match min {
        Some(m) => l.min(m),
        None => l,
    };
    match max {
        Some(m) => l.max(m),
        None => l,
    }
}

/// Width and height lengths of a node inside `parent`.
fn dims(st: &Style, parent: Parent) -> (Length, Length) {
    let (w, h) = (st.width.unwrap_or(Size::Auto), st.height.unwrap_or(Size::Auto));
    let (w, h) = match parent.dir {
        Some(Direction::Column) => (length(w, parent.stretch, parent.w_def), length(h, false, parent.h_def)),
        Some(Direction::Row) => (length(w, false, parent.w_def), length(h, parent.stretch, parent.h_def)),
        None => (length(w, false, true), length(h, false, true)),
    };
    (bounded(w, st.min_width, st.max_width), bounded(h, st.min_height, st.max_height))
}

fn definite(l: Length) -> bool {
    !matches!(l, Length::Fluid(_)) && (l.is_fill() || matches!(l, Length::Fixed(_)))
}

fn color(c: Color, opacity: Option<f32>) -> IColor {
    IColor::from_rgba(c.r, c.g, c.b, c.a * opacity.unwrap_or(1.0).clamp(0.0, 1.0))
}

fn text_color(st: &Style) -> IColor {
    color(st.text_color.unwrap_or(Color::WHITE), st.opacity)
}

fn border(st: &Style) -> Border {
    match st.border {
        Some(b) => Border { color: color(b.color, st.opacity), width: b.width, radius: b.radius.into() },
        None => Border::default(),
    }
}

fn padding(e: Option<Edges>) -> Padding {
    let e = e.unwrap_or(Edges::ZERO);
    Padding { top: e.top, right: e.right, bottom: e.bottom, left: e.left }
}

fn font(st: &Style, a: &Assets) -> Font {
    let family = st.font.and_then(|f| a.fonts.get(&f).copied()).unwrap_or(DEFAULT_FAMILY);
    Font {
        family: iced_core::font::Family::Name(family),
        weight: if st.bold == Some(true) { Weight::Bold } else { Weight::Normal },
        ..Font::DEFAULT
    }
}

fn font_size(st: &Style) -> f32 {
    st.font_size.unwrap_or(16.0)
}

fn text_align(st: &Style) -> Horizontal {
    match st.text_align.unwrap_or(TextAlign::Start) {
        TextAlign::Start => Horizontal::Left,
        TextAlign::Center => Horizontal::Center,
        TextAlign::End => Horizontal::Right,
    }
}

fn container_style(st: Style) -> container::Style {
    container::Style {
        text_color: Some(text_color(&st)),
        background: st.background.map(|c| Background::Color(color(c, st.opacity))),
        border: border(&st),
        ..Default::default()
    }
}

/// Wraps a node's element so the backend learns when the pointer is over it.
fn wrap<'a>(n: &'a VNode, el: Element<'a>) -> Element<'a> {
    if n.blocking || n.hover_signals {
        mouse_area(el).on_enter(Msg::Enter(n.id)).on_exit(Msg::Exit(n.id))._boxed()
    } else {
        el
    }
}

fn node_el<'a>(n: &'a VNode, parent: Parent, fill: bool, a: &'a Assets) -> Element<'a> {
    let st = n.style();
    let el = match &n.kind {
        NodeKind::Container => flex_el(n, parent, fill, false, a),
        NodeKind::Scroll => flex_el(n, parent, fill, true, a),
        NodeKind::Text { text: t } => {
            let (w, h) = dims(st, parent);
            text(t.as_str())
                .size(font_size(st))
                .color(text_color(st))
                .font(font(st, a))
                .width(w)
                .height(h)
                .align_x(text_align(st))
                ._boxed()
        }
        NodeKind::Image { texture, fit } => {
            let (w, h) = dims(st, parent);
            match a.images.get(texture) {
                Some(handle) => iced_widget::image(handle.clone())
                    .content_fit(match fit {
                        ImageFit::Contain => ContentFit::Contain,
                        ImageFit::Cover => ContentFit::Cover,
                        ImageFit::Fill => ContentFit::Fill,
                        ImageFit::None => ContentFit::None,
                    })
                    .opacity(st.opacity.unwrap_or(1.0).clamp(0.0, 1.0))
                    .width(w)
                    .height(h)
                    ._boxed(),
                None => space().width(w).height(h)._boxed(),
            }
        }
        NodeKind::GpuView { painter, pointer } => {
            let (w, h) = dims(st, parent);
            let program = ViewProgram { id: n.id, painter: painter.clone(), pointer: *pointer, views: a.views.clone() };
            iced_widget::shader(program).width(w).height(h)._boxed()
        }
        NodeKind::Button { label } => button_el(n, parent, label, a),
        NodeKind::Checkbox { label, checked } => {
            let (w, _) = dims(st, parent);
            let id = n.id;
            let styles = n.styles;
            checkbox(*checked)
                .label(label.as_str())
                .on_toggle_maybe(n.enabled.then_some(move |v| Msg::Toggle(id, v)))
                .spacing(st.gap.unwrap_or(8.0))
                .text_size(font_size(st))
                .font(font(st, a))
                .width(w)
                .style(move |_t, status| {
                    use checkbox::Status::*;
                    let s = &styles[match status {
                        Active { .. } => NORMAL,
                        Hovered { .. } => HOVERED,
                        Disabled { .. } => DISABLED,
                    }];
                    let tc = text_color(s);
                    let mut b = border(s);
                    if s.border.is_none_or(|b| b.width <= 0.0) {
                        b = Border { color: IColor { a: tc.a * 0.6, ..tc }, width: 1.0, radius: 3.0.into() };
                    }
                    checkbox::Style {
                        background: Background::Color(s.background.map_or(IColor::TRANSPARENT, |c| color(c, s.opacity))),
                        icon_color: tc,
                        border: b,
                        text_color: Some(tc),
                    }
                })
                ._boxed()
        }
        NodeKind::Slider { min, max, step, value } => {
            let (w, h) = dims(st, parent);
            let id = n.id;
            let styles = n.styles;
            let (lo, hi) = if max > min { (*min, *max) } else { (*min, *min + 1e-6) };
            let step = if *step > 0.0 { *step } else { (hi - lo) / 1000.0 };
            let mut s = slider(lo..=hi, value.clamp(lo, hi), move |v| Msg::Slide(id, v)).step(step).width(w);
            if let Size::Px(px) = st.height.unwrap_or(Size::Auto) {
                s = s.height(px);
            }
            let _ = h;
            let enabled = n.enabled;
            s.style(move |_t, status| {
                let s = &styles[if !enabled {
                    DISABLED
                } else {
                    match status {
                        slider::Status::Active => NORMAL,
                        slider::Status::Hovered => HOVERED,
                        slider::Status::Dragged => PRESSED,
                    }
                }];
                let accent = text_color(s);
                let rail = s.background.map_or(IColor::from_rgb(0.23, 0.25, 0.29), |c| color(c, s.opacity));
                slider::Style {
                    rail: slider::Rail {
                        backgrounds: (Background::Color(accent), Background::Color(rail)),
                        width: 4.0,
                        border: Border { radius: s.border.map_or(2.0, |b| b.radius).into(), ..Border::default() },
                    },
                    handle: slider::Handle {
                        shape: slider::HandleShape::Circle { radius: 8.0 },
                        background: Background::Color(accent),
                        border_width: 0.0,
                        border_color: IColor::TRANSPARENT,
                    },
                }
            })
            ._boxed()
        }
        NodeKind::TextInput { value, placeholder, password, .. } => {
            let (w, _) = dims(st, parent);
            let id = n.id;
            let styles = n.styles;
            let mut t = text_input(placeholder.as_str(), value.as_str())
                .id(iced_id(id))
                .secure(*password)
                .size(font_size(st))
                .padding(padding(st.padding))
                .width(w)
                .font(font(st, a));
            if n.enabled {
                t = t.on_input(move |s| Msg::Input(id, s)).on_submit(Msg::Submit(id));
            }
            t.style(move |_t, status| {
                use text_input::Status::*;
                let s = &styles[match status {
                    Active => NORMAL,
                    Hovered => HOVERED,
                    Focused { .. } => FOCUSED,
                    Disabled => DISABLED,
                }];
                let tc = text_color(s);
                text_input::Style {
                    background: Background::Color(s.background.map_or(IColor::TRANSPARENT, |c| color(c, s.opacity))),
                    border: border(s),
                    placeholder: IColor { a: tc.a * 0.5, ..tc },
                    value: tc,
                    selection: IColor::from_rgba(0.36, 0.62, 1.0, 0.4),
                }
            })
            ._boxed()
        }
    };
    wrap(n, el)
}

fn button_el<'a>(n: &'a VNode, parent: Parent, label: &'a str, a: &'a Assets) -> Element<'a> {
    let st = n.style();
    let (w, h) = dims(st, parent);
    let id = n.id;
    let styles = n.styles;
    let content: Element<'a> = if n.children.is_empty() {
        let mut t = text(label).size(font_size(st)).color(text_color(st)).font(font(st, a)).align_x(text_align(st));
        if w != Length::Fit {
            t = t.width(Length::Fill);
        }
        t._boxed()
    } else {
        let row = Style { direction: Some(st.direction.unwrap_or(Direction::Row)), ..*st };
        flex_children(n, &row, (Length::Fit, Length::Fit), (Length::Fit, Length::Fit), a)
    };
    button(content)
        .padding(padding(st.padding))
        .width(w)
        .height(h)
        .on_press_maybe(n.enabled.then_some(Msg::Click(id)))
        .style(move |_t, status| {
            let s = &styles[match status {
                button::Status::Active => NORMAL,
                button::Status::Hovered => HOVERED,
                button::Status::Pressed => PRESSED,
                button::Status::Disabled => DISABLED,
            }];
            button::Style {
                background: s.background.map(|c| Background::Color(color(c, s.opacity))),
                text_color: text_color(s),
                border: border(s),
                ..Default::default()
            }
        })
        ._boxed()
}

/// The children of `n` as one row or column (direction, gap, align, justify), no box. `own` is
/// the node's own width and height (they decide `justify` and the stretch of the children),
/// `inner` the lengths of the row / column itself.
fn flex_children<'a>(n: &'a VNode, st: &Style, own: (Length, Length), inner: (Length, Length), a: &'a Assets) -> Element<'a> {
    let dir = st.direction.unwrap_or(Direction::Column);
    let is_row = dir == Direction::Row;
    let main = if is_row { own.0 } else { own.1 };
    let stretch = st.align.unwrap_or(Align::Stretch) == Align::Stretch;
    let child_parent = Parent { dir: Some(dir), stretch, w_def: definite(own.0), h_def: definite(own.1) };
    let mut kids: Vec<Element<'a>> = Vec::with_capacity(n.children.len() + 2);
    let mut keys: Vec<u64> = Vec::with_capacity(n.children.len() + 2);
    let spacer = || -> Element<'a> {
        if is_row { space().width(Length::Fill)._boxed() } else { space().height(Length::Fill)._boxed() }
    };
    // Spacers are keyed from the top of the key space; node ids are small.
    let mut spacer_key = u64::MAX;
    let justify = if definite(main) { st.justify.unwrap_or(Justify::Start) } else { Justify::Start };
    if matches!(justify, Justify::Center | Justify::End) {
        kids.push(spacer());
        keys.push(spacer_key);
        spacer_key -= 1;
    }
    let count = n.children.len();
    for (i, c) in n.children.iter().enumerate() {
        kids.push(node_el(c, child_parent, false, a));
        keys.push(c.id.to_bits());
        if justify == Justify::SpaceBetween && i + 1 < count {
            kids.push(spacer());
            keys.push(spacer_key);
            spacer_key -= 1;
        }
    }
    if justify == Justify::Center {
        kids.push(spacer());
        keys.push(spacer_key);
    }
    let gap = st.gap.unwrap_or(0.0);
    let (iw, ih) = inner;
    if is_row {
        let v = match st.align.unwrap_or(Align::Stretch) {
            Align::Center => Vertical::Center,
            Align::End => Vertical::Bottom,
            _ => Vertical::Top,
        };
        Keyed::new(keys, iced_widget::row(kids).spacing(gap).width(iw).height(ih).align_y(v)._boxed())._boxed()
    } else {
        let hz = match st.align.unwrap_or(Align::Stretch) {
            Align::Center => Horizontal::Center,
            Align::End => Horizontal::Right,
            _ => Horizontal::Left,
        };
        Keyed::new(keys, iced_widget::column(kids).spacing(gap).width(iw).height(ih).align_x(hz)._boxed())._boxed()
    }
}

fn flex_el<'a>(n: &'a VNode, parent: Parent, fill: bool, scroll: bool, a: &'a Assets) -> Element<'a> {
    let st = n.style();
    let (mut w, mut h) = dims(st, parent);
    if fill {
        (w, h) = (Length::Fill, Length::Fill);
    }
    // The flex box inside the padded, painted container fills it unless the container is
    // intrinsically sized.
    let in_len = |l: Length| if matches!(l, Length::Fit | Length::Shrink) { Length::Fit } else { Length::Fill };
    let inner = flex_children(n, st, (w, h), (in_len(w), in_len(h)), a);
    let inner: Element<'a> =
        if scroll { scrollable(inner).width(Length::Fill).height(Length::Fill)._boxed() } else { inner };
    let st_copy = *st;
    container(inner)
        .width(w)
        .height(h)
        .padding(padding(st.padding))
        .style(move |_t| container_style(st_copy))
        ._boxed()
}
