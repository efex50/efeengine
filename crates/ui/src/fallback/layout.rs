//! Layout: a taffy tree per layer built from the model, with text, images and the value widgets
//! as measured leaves. Everything is in logical px; taffy rounding is off (drawing snaps to
//! physical pixels).
//!
//! The mapping follows the iced backend (API.md section 5): `Auto` is the intrinsic size, `Fill`
//! and `Portion` share the free space of a parent whose size is definite (in an intrinsically
//! sized parent they keep their own size), `justify` and `align` are the flexbox properties,
//! padding is inside the size and a border takes no layout space.

use std::collections::HashMap;

use efeengine_core::prelude::TextureHandle;

use crate::backend::ImageIdExt;
use taffy::prelude::*;
use taffy::{AlignItems, JustifyContent, Overflow};

use super::model::{LRect, Model, VNode};
use super::text::{LINE_HEIGHT, TextCell, TextSpec, TextSystem};
use crate::id::WidgetId;
use crate::node::{Anchor, NodeKind};
use crate::style::{Align, Direction, Edges, Justify, Size as USize, Style, TextAlign};

/// Everything a layout pass reads and updates.
pub struct LayoutCx<'a> {
    pub text: &'a mut TextSystem,
    pub cells: &'a mut HashMap<WidgetId, TextCell>,
    /// Image size per texture, from `UiBackendCx::images`.
    pub images: &'a HashMap<TextureHandle, (u32, u32)>,
}

/// What a measured leaf is.
#[derive(Clone)]
enum Leaf {
    /// Text node or button label.
    Text(WidgetId, TextSpec),
    Image(f32, f32),
    Checkbox(WidgetId, TextSpec, f32, f32),
    Slider,
    Input(f32),
    Nothing,
}

/// Side of a checkbox box for a font size.
pub fn checkbox_box(font_size: f32) -> f32 {
    font_size.round().max(8.0)
}

/// Default text size.
pub fn font_size(st: &Style) -> f32 {
    st.font_size.unwrap_or(16.0)
}

pub fn text_spec(st: &Style, text: &str, wrap: bool) -> TextSpec {
    TextSpec {
        text: text.to_string(),
        size: font_size(st),
        font: st.font,
        bold: st.bold == Some(true),
        align: st.text_align.unwrap_or(TextAlign::Start),
        wrap,
    }
}

#[derive(Clone, Copy)]
struct Parent {
    dir: Option<Direction>,
    /// The parent stretches `Auto` cross sizes (`align: Stretch`).
    stretch: bool,
    w_def: bool,
    h_def: bool,
    scroll: bool,
}

fn definite(s: USize, stretch: bool, parent_def: bool) -> bool {
    match s {
        USize::Px(_) => true,
        USize::Fill | USize::Portion(_) => parent_def,
        USize::Auto => stretch && parent_def,
    }
}

fn dim(v: Option<f32>) -> LengthPercentageAuto {
    v.map_or(auto(), length)
}

fn pad(e: Option<Edges>) -> taffy::Rect<LengthPercentage> {
    let e = e.unwrap_or(Edges::ZERO);
    taffy::Rect { left: length(e.left), right: length(e.right), top: length(e.top), bottom: length(e.bottom) }
}

/// The box properties every node shares: size, flex item behaviour, min / max.
fn item_style(st: &Style, parent: Parent) -> taffy::Style {
    let (w, h) = (st.width.unwrap_or(USize::Auto), st.height.unwrap_or(USize::Auto));
    let mut s = taffy::Style::default();
    let (mut grow, mut shrink, mut basis) = (0.0f32, 1.0f32, auto());
    let mut set = |size: USize, main: bool, def: bool| -> Dimension {
        match size {
            USize::Auto => auto(),
            USize::Px(v) => {
                if main {
                    shrink = 0.0;
                }
                length(v)
            }
            USize::Fill | USize::Portion(_) if !def => auto(),
            USize::Fill | USize::Portion(_) if main => {
                grow = if let USize::Portion(n) = size { n.max(1) as f32 } else { 1.0 };
                shrink = 1.0;
                basis = length(0.0);
                auto()
            }
            USize::Fill | USize::Portion(_) => percent(1.0),
        }
    };
    let (row, col) = (parent.dir == Some(Direction::Row), parent.dir == Some(Direction::Column));
    s.size.width = set(w, row, parent.w_def);
    s.size.height = set(h, col, parent.h_def);
    s.flex_grow = grow;
    s.flex_shrink = if parent.scroll { 0.0 } else { shrink };
    s.flex_basis = basis;
    s.min_size = taffy::Size { width: dim(st.min_width), height: dim(st.min_height) };
    s.max_size = taffy::Size { width: dim(st.max_width), height: dim(st.max_height) };
    s
}

fn container_style(n: &VNode, parent: Parent, own: (bool, bool)) -> (taffy::Style, Parent) {
    let st = n.style();
    let mut s = item_style(st, parent);
    let default_dir = if matches!(n.kind, NodeKind::Button { .. }) { Direction::Row } else { Direction::Column };
    let dir = st.direction.unwrap_or(default_dir);
    s.display = Display::Flex;
    s.flex_direction = if dir == Direction::Row { FlexDirection::Row } else { FlexDirection::Column };
    s.justify_content = match st.justify.unwrap_or(Justify::Start) {
        Justify::Start => JustifyContent::FLEX_START,
        Justify::Center => JustifyContent::CENTER,
        Justify::End => JustifyContent::FLEX_END,
        Justify::SpaceBetween => JustifyContent::SPACE_BETWEEN,
    };
    s.align_items = match st.align.unwrap_or(Align::Stretch) {
        Align::Start => AlignItems::FLEX_START,
        Align::Center => AlignItems::CENTER,
        Align::End => AlignItems::FLEX_END,
        Align::Stretch => AlignItems::STRETCH,
    };
    let g = st.gap.unwrap_or(0.0);
    s.gap = taffy::Size { width: length(g), height: length(g) };
    s.padding = pad(st.padding);
    let scroll = matches!(n.kind, NodeKind::Scroll { .. });
    if scroll {
        s.overflow = taffy::Point { x: Overflow::Scroll, y: Overflow::Scroll };
    }
    let stretch = st.align.unwrap_or(Align::Stretch) == Align::Stretch;
    (s, Parent { dir: Some(dir), stretch, w_def: own.0, h_def: own.1, scroll })
}

fn build(n: &mut VNode, parent: Parent, tree: &mut TaffyTree<Leaf>, cx: &LayoutCx, force_fill: bool) -> NodeId {
    let st = *n.style();
    let (w, h) = (st.width.unwrap_or(USize::Auto), st.height.unwrap_or(USize::Auto));
    // Cross axis stretch of the parent makes an `Auto` size definite too.
    let (row, col) = (parent.dir == Some(Direction::Row), parent.dir == Some(Direction::Column));
    let own = (
        definite(w, col && parent.stretch, parent.w_def) || force_fill,
        definite(h, row && parent.stretch, parent.h_def) || force_fill,
    );
    let mut node = match &n.kind {
        NodeKind::Container | NodeKind::Scroll { .. } => {
            let (s, cp) = container_style(n, parent, own);
            let kids: Vec<NodeId> = n.children.iter_mut().map(|c| build(c, cp, tree, cx, false)).collect();
            let id = tree.new_with_children(s, &kids).expect("taffy node");
            n.tn = Some(id);
            return finish(n, id, tree, force_fill);
        }
        NodeKind::Button { .. } if !n.children.is_empty() => {
            let (s, cp) = container_style(n, parent, own);
            let kids: Vec<NodeId> = n.children.iter_mut().map(|c| build(c, cp, tree, cx, false)).collect();
            let id = tree.new_with_children(s, &kids).expect("taffy node");
            n.tn = Some(id);
            return finish(n, id, tree, force_fill);
        }
        NodeKind::Text { text } => (item_style(&st, parent), Leaf::Text(n.id, text_spec(&st, text, true))),
        NodeKind::Button { label } => (item_style(&st, parent), Leaf::Text(n.id, text_spec(&st, label, true))),
        NodeKind::Image { texture, .. } => {
            let (iw, ih) = cx.images.get(&texture.texture()).copied().unwrap_or((0, 0));
            (item_style(&st, parent), Leaf::Image(iw as f32, ih as f32))
        }
        NodeKind::Checkbox { label, .. } => {
            let fs = font_size(&st);
            (item_style(&st, parent), Leaf::Checkbox(n.id, text_spec(&st, label, true), checkbox_box(fs), st.gap.unwrap_or(8.0)))
        }
        NodeKind::Slider { .. } => (item_style(&st, parent), Leaf::Slider),
        // Wrapped, read-only text: measured like a text, drawn by `paint::text_multiline`.
        NodeKind::TextInput { value, multiline: true, read_only: true, .. } => (item_style(&st, parent), Leaf::Text(n.id, text_spec(&st, value, true))),
        NodeKind::TextInput { .. } => (item_style(&st, parent), Leaf::Input(font_size(&st) * LINE_HEIGHT)),
        NodeKind::GpuView { .. } => (item_style(&st, parent), Leaf::Nothing),
    };
    node.0.padding = pad(st.padding);
    if matches!(n.kind, NodeKind::Slider { .. } | NodeKind::Image { .. } | NodeKind::GpuView { .. }) {
        node.0.padding = pad(None);
    }
    let id = tree.new_leaf_with_context(node.0, node.1).expect("taffy leaf");
    n.tn = Some(id);
    finish(n, id, tree, force_fill)
}

/// Layer roots with the `Fill` anchor fill the window.
fn finish(_n: &mut VNode, id: NodeId, tree: &mut TaffyTree<Leaf>, force_fill: bool) -> NodeId {
    if force_fill {
        let mut s = tree.style(id).expect("style").clone();
        // The wrapper is a row: grow along it, stretch across it.
        s.size = taffy::Size { width: auto(), height: auto() };
        s.flex_grow = 1.0;
        s.flex_basis = length(0.0);
        s.align_self = Some(AlignItems::STRETCH);
        tree.set_style(id, s).expect("set style");
    }
    id
}

fn read(n: &mut VNode, tree: &TaffyTree<Leaf>) {
    let Some(tn) = n.tn.take() else { return };
    let l = tree.layout(tn).expect("layout");
    n.rect = LRect { x: l.location.x, y: l.location.y, w: l.size.width, h: l.size.height };
    for c in &mut n.children {
        read(c, tree);
    }
    let pad = n.style().padding.unwrap_or(Edges::ZERO);
    let (mut cw, mut ch) = (n.rect.w, n.rect.h);
    for c in &n.children {
        cw = cw.max(c.rect.x + c.rect.w + pad.right);
        ch = ch.max(c.rect.y + c.rect.h + pad.bottom);
    }
    n.content = (cw, ch);
}

fn measure(leaf: &Leaf, known: taffy::Size<Option<f32>>, avail: taffy::Size<AvailableSpace>, cx: &mut LayoutCx) -> taffy::Size<f32> {
    let limit = known.width.or(match avail.width {
        AvailableSpace::Definite(w) => Some(w),
        AvailableSpace::MinContent => Some(0.0),
        AvailableSpace::MaxContent => None,
    });
    let (w, h) = match leaf {
        Leaf::Text(id, spec) => {
            let cell = cx.cells.entry(*id).or_default();
            cx.text.layout(cell, spec, limit)
        }
        Leaf::Image(iw, ih) => match (known.width, known.height) {
            (Some(w), None) if *iw > 0.0 => (w, w * ih / iw),
            (None, Some(h)) if *ih > 0.0 => (h * iw / ih, h),
            _ => (*iw, *ih),
        },
        Leaf::Checkbox(id, spec, b, gap) => {
            let cell = cx.cells.entry(*id).or_default();
            let (tw, th) = cx.text.layout(cell, spec, limit.map(|l| (l - b - gap).max(0.0)));
            (b + gap + tw, th.max(*b))
        }
        Leaf::Slider => (
            match avail.width {
                AvailableSpace::Definite(w) => w.min(160.0),
                _ => 120.0,
            },
            16.0,
        ),
        Leaf::Input(lh) => (
            match avail.width {
                AvailableSpace::Definite(w) => w.min(180.0),
                _ => 160.0,
            },
            *lh,
        ),
        Leaf::Nothing => (0.0, 0.0),
    };
    taffy::Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) }
}

/// Lays out every layer of the model for a window of `window` logical px.
pub fn layout(model: &mut Model, window: (f32, f32), cx: &mut LayoutCx) {
    for l in &mut model.layers {
        let mut tree: TaffyTree<Leaf> = TaffyTree::new();
        tree.disable_rounding();
        let fill = l.layer.anchor == Anchor::Fill;
        let root = build(&mut l.root, Parent { dir: Some(Direction::Row), stretch: false, w_def: true, h_def: true, scroll: false }, &mut tree, cx, fill);
        let (jc, ai) = anchor_alignment(l.layer.anchor);
        let wrapper = tree
            .new_with_children(
                taffy::Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    size: taffy::Size { width: length(window.0), height: length(window.1) },
                    justify_content: jc,
                    align_items: ai,
                    ..Default::default()
                },
                &[root],
            )
            .expect("wrapper");
        let space = taffy::Size { width: AvailableSpace::Definite(window.0), height: AvailableSpace::Definite(window.1) };
        tree.compute_layout_with_measure(wrapper, space, |inputs, _id, ctx, style| {
            taffy::compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known, avail| match ctx {
                    Some(leaf) => measure(leaf, known, avail, cx),
                    None => taffy::Size::ZERO,
                },
            )
        })
        .expect("layout");
        read(&mut l.root, &tree);
        let (ox, oy) = layer_offset(&l.layer);
        l.root.rect.x += ox;
        l.root.rect.y += oy;
    }
}

fn anchor_alignment(a: Anchor) -> (JustifyContent, AlignItems) {
    let x = match a {
        Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft | Anchor::Fill => JustifyContent::FLEX_START,
        Anchor::Top | Anchor::Center | Anchor::Bottom => JustifyContent::CENTER,
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => JustifyContent::FLEX_END,
    };
    let y = match a {
        Anchor::TopLeft | Anchor::Top | Anchor::TopRight | Anchor::Fill => AlignItems::FLEX_START,
        Anchor::Left | Anchor::Center | Anchor::Right => AlignItems::CENTER,
        Anchor::BottomLeft | Anchor::Bottom | Anchor::BottomRight => AlignItems::FLEX_END,
    };
    (x, y)
}

/// The layer offset as a shift of the root: positive `x` / `y` move away from the anchored edge
/// (towards the inside of the window) like the iced backend.
fn layer_offset(l: &crate::node::Layer) -> (f32, f32) {
    let (x, y) = l.offset;
    let dx = match l.anchor {
        Anchor::Fill => 0.0,
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => -x,
        _ => x,
    };
    let dy = match l.anchor {
        Anchor::Fill => 0.0,
        Anchor::BottomLeft | Anchor::Bottom | Anchor::BottomRight => -y,
        _ => y,
    };
    (dx, dy)
}
