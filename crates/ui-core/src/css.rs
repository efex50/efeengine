//! Web CSS generation for the retained UI tree (API.md section 6).
//!
//! Provides utilities for generating CSS class names, inline styles, layer positioning,
//! and the state-layer stylesheet for interaction states (hover, pressed, focused, disabled).

use crate::node::{Anchor, KindTag, Layer};
use crate::style::{Align, Color, Direction, Justify, Size, Style, TextAlign};
use crate::theme::{StateStyles, Theme};

/// Prefix for kind-specific CSS classes, e.g. `efui-k-button`.
pub const KIND_PREFIX: &str = "efui-k-";

/// Prefix for theme-class-specific CSS classes, e.g. `efui-c-panel`.
pub const CLASS_PREFIX: &str = "efui-c-";

/// Attribute the backend sets on every effectively disabled node (itself or an ancestor disabled).
pub const DISABLED_ATTR: &str = "data-efui-disabled";

/// Formats a float without trailing zeros, mapping NaN and infinities to `"0"`.
pub fn fmt_float(v: f32) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let v = if v == 0.0 { 0.0 } else { v };
    let rounded = (v * 1000.0).round() / 1000.0;
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    let mut s = format!("{:.3}", rounded);
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

/// Formats a [`Color`] as `rgba(r, g, b, a)` where r, g, b are 0..=255 integers and alpha has up to 3 decimals.
fn fmt_rgba(color: Color) -> String {
    let [r, g, b, _] = color.to_rgba8();
    format!("rgba({}, {}, {}, {})", r, g, b, fmt_float(color.a))
}

/// Returns the CSS class name for a node kind, e.g. `"efui-k-button"`.
pub fn kind_class(tag: KindTag) -> String {
    format!("{}{}", KIND_PREFIX, tag.name())
}

/// Escapes a theme class name into a valid CSS class: `[A-Za-z0-9_-]` kept, every other char
/// (and a leading digit or '-' followed by digit) written as `_xHH_` / `_uHHHH_` hex. Injective.
pub fn class_class(name: &str) -> String {
    let mut out = String::from(CLASS_PREFIX);
    let mut chars = name.chars().peekable();
    let mut idx = 0;
    while let Some(c) = chars.next() {
        let is_leading_digit = idx == 0 && c.is_ascii_digit();
        let is_hyphen_before_digit = c == '-' && chars.peek().map_or(false, |next| next.is_ascii_digit());
        let keep = (c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && !is_leading_digit
            && !is_hyphen_before_digit;

        if keep {
            out.push(c);
        } else {
            let code = c as u32;
            if code <= 0xFF {
                use std::fmt::Write;
                let _ = write!(out, "_x{:02X}_", code);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "_u{:04X}_", code);
            }
        }
        idx += 1;
    }
    out
}

/// CSS declarations for a style, in a fixed order, skipping None fields.
///
/// `parent` is the parent container's main-axis direction (`None` for a layer root or no flex parent):
/// - `Size::Fill` on main axis -> `flex: 1 1 0`, on cross axis -> `align-self: stretch`, when no parent -> `width/height: 100%`.
/// - `Size::Portion(n)` on main axis -> `flex: n 1 0`, on cross axis treat as Fill.
/// - `Size::Px(v)` -> `width/height: {v}px` plus `flex-shrink: 0` on main axis.
/// - `Size::Auto` -> nothing.
pub fn declarations(style: &Style, parent: Option<Direction>) -> Vec<(&'static str, String)> {
    let mut decls = Vec::new();

    // 1. direction
    if let Some(dir) = style.direction {
        let val = match dir {
            Direction::Row => "row",
            Direction::Column => "column",
        };
        decls.push(("flex-direction", val.to_string()));
    }

    // 2. justify
    if let Some(justify) = style.justify {
        let val = match justify {
            Justify::Start => "flex-start",
            Justify::Center => "center",
            Justify::End => "flex-end",
            Justify::SpaceBetween => "space-between",
        };
        decls.push(("justify-content", val.to_string()));
    }

    // 3. align
    if let Some(align) = style.align {
        let val = match align {
            Align::Start => "flex-start",
            Align::Center => "center",
            Align::End => "flex-end",
            Align::Stretch => "stretch",
        };
        decls.push(("align-items", val.to_string()));
    }

    // 4. gap
    if let Some(gap) = style.gap {
        decls.push(("gap", format!("{}px", fmt_float(gap))));
    }

    // 5. padding
    if let Some(pad) = style.padding {
        decls.push((
            "padding",
            format!(
                "{}px {}px {}px {}px",
                fmt_float(pad.top),
                fmt_float(pad.right),
                fmt_float(pad.bottom),
                fmt_float(pad.left)
            ),
        ));
    }

    // 6. width
    if let Some(w) = style.width {
        match parent {
            Some(Direction::Row) => match w {
                Size::Auto => {}
                Size::Px(v) => {
                    decls.push(("width", format!("{}px", fmt_float(v))));
                    decls.push(("flex-shrink", "0".to_string()));
                }
                Size::Fill => {
                    decls.push(("flex", "1 1 0".to_string()));
                }
                Size::Portion(n) => {
                    decls.push(("flex", format!("{} 1 0", n)));
                }
            },
            Some(Direction::Column) => match w {
                Size::Auto => {}
                Size::Px(v) => {
                    decls.push(("width", format!("{}px", fmt_float(v))));
                }
                Size::Fill | Size::Portion(_) => {
                    decls.push(("align-self", "stretch".to_string()));
                }
            },
            None => match w {
                Size::Auto => {}
                Size::Px(v) => {
                    decls.push(("width", format!("{}px", fmt_float(v))));
                }
                Size::Fill | Size::Portion(_) => {
                    decls.push(("width", "100%".to_string()));
                }
            },
        }
    }

    // 7. height
    if let Some(h) = style.height {
        match parent {
            Some(Direction::Row) => match h {
                Size::Auto => {}
                Size::Px(v) => {
                    decls.push(("height", format!("{}px", fmt_float(v))));
                }
                Size::Fill | Size::Portion(_) => {
                    decls.push(("align-self", "stretch".to_string()));
                }
            },
            Some(Direction::Column) => match h {
                Size::Auto => {}
                Size::Px(v) => {
                    decls.push(("height", format!("{}px", fmt_float(v))));
                    decls.push(("flex-shrink", "0".to_string()));
                }
                Size::Fill => {
                    decls.push(("flex", "1 1 0".to_string()));
                }
                Size::Portion(n) => {
                    decls.push(("flex", format!("{} 1 0", n)));
                }
            },
            None => match h {
                Size::Auto => {}
                Size::Px(v) => {
                    decls.push(("height", format!("{}px", fmt_float(v))));
                }
                Size::Fill | Size::Portion(_) => {
                    decls.push(("height", "100%".to_string()));
                }
            },
        }
    }

    // 8. min_width
    if let Some(v) = style.min_width {
        decls.push(("min-width", format!("{}px", fmt_float(v))));
    }

    // 9. max_width
    if let Some(v) = style.max_width {
        decls.push(("max-width", format!("{}px", fmt_float(v))));
    }

    // 10. min_height
    if let Some(v) = style.min_height {
        decls.push(("min-height", format!("{}px", fmt_float(v))));
    }

    // 11. max_height
    if let Some(v) = style.max_height {
        decls.push(("max-height", format!("{}px", fmt_float(v))));
    }

    // 12. background
    if let Some(bg) = style.background {
        decls.push(("background-color", fmt_rgba(bg)));
    }

    // 13. border
    if let Some(b) = style.border {
        decls.push(("border", format!("{}px solid {}", fmt_float(b.width), fmt_rgba(b.color))));
        decls.push(("border-radius", format!("{}px", fmt_float(b.radius))));
    }

    // 14. text_color
    if let Some(tc) = style.text_color {
        decls.push(("color", fmt_rgba(tc)));
    }

    // 15. font_size
    if let Some(fs) = style.font_size {
        decls.push(("font-size", format!("{}px", fmt_float(fs))));
    }

    // 16. font
    if let Some(font) = style.font {
        decls.push(("font-family", format!("var(--efui-font-{}, sans-serif)", font.0)));
    }

    // 17. bold
    if let Some(bold) = style.bold {
        decls.push(("font-weight", if bold { "700".to_string() } else { "400".to_string() }));
    }

    // 18. text_align
    if let Some(ta) = style.text_align {
        let val = match ta {
            TextAlign::Start => "start",
            TextAlign::Center => "center",
            TextAlign::End => "end",
        };
        decls.push(("text-align", val.to_string()));
    }

    // 19. opacity
    if let Some(op) = style.opacity {
        decls.push(("opacity", fmt_float(op)));
    }

    decls
}

/// Formats CSS declarations as `"prop: value; prop: value"` (no trailing semicolon),
/// optionally each with `" !important"`.
pub fn declarations_text(decls: &[(&'static str, String)], important: bool) -> String {
    let suffix = if important { " !important" } else { "" };
    decls
        .iter()
        .map(|(k, v)| format!("{}: {}{}", k, v, suffix))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Inline style declarations for a layer root inside the `#ui` layer element.
///
/// Produces `position: absolute`, anchor placement with logical offsets,
/// `z-index: {z}`, `display: flex`, and `pointer-events: auto`.
pub fn layer_declarations(layer: &Layer) -> Vec<(&'static str, String)> {
    let mut decls = Vec::new();
    decls.push(("position", "absolute".to_string()));

    let (x, y) = layer.offset;
    match layer.anchor {
        Anchor::Fill => {
            decls.push(("inset", "0".to_string()));
        }
        Anchor::Center => {
            decls.push(("left", "50%".to_string()));
            decls.push(("top", "50%".to_string()));
            decls.push((
                "transform",
                format!(
                    "translate(calc(-50% + {}px), calc(-50% + {}px))",
                    fmt_float(x),
                    fmt_float(y)
                ),
            ));
        }
        Anchor::TopLeft => {
            decls.push(("left", format!("{}px", fmt_float(x))));
            decls.push(("top", format!("{}px", fmt_float(y))));
        }
        Anchor::Top => {
            decls.push(("left", "50%".to_string()));
            decls.push(("top", format!("{}px", fmt_float(y))));
            decls.push(("transform", format!("translateX(calc(-50% + {}px))", fmt_float(x))));
        }
        Anchor::TopRight => {
            decls.push(("right", format!("{}px", fmt_float(x))));
            decls.push(("top", format!("{}px", fmt_float(y))));
        }
        Anchor::Left => {
            decls.push(("left", format!("{}px", fmt_float(x))));
            decls.push(("top", "50%".to_string()));
            decls.push(("transform", format!("translateY(calc(-50% + {}px))", fmt_float(y))));
        }
        Anchor::Right => {
            decls.push(("right", format!("{}px", fmt_float(x))));
            decls.push(("top", "50%".to_string()));
            decls.push(("transform", format!("translateY(calc(-50% + {}px))", fmt_float(y))));
        }
        Anchor::BottomLeft => {
            decls.push(("left", format!("{}px", fmt_float(x))));
            decls.push(("bottom", format!("{}px", fmt_float(y))));
        }
        Anchor::Bottom => {
            decls.push(("left", "50%".to_string()));
            decls.push(("bottom", format!("{}px", fmt_float(y))));
            decls.push(("transform", format!("translateX(calc(-50% + {}px))", fmt_float(x))));
        }
        Anchor::BottomRight => {
            decls.push(("right", format!("{}px", fmt_float(x))));
            decls.push(("bottom", format!("{}px", fmt_float(y))));
        }
    }

    decls.push(("z-index", layer.z.to_string()));
    decls.push(("display", "flex".to_string()));
    decls.push(("pointer-events", "auto".to_string()));

    decls
}

/// Generates the interaction-state stylesheet for the given [`Theme`].
///
/// Emits rules in state order: `hovered`, `pressed`, `focused`, `disabled`.
/// For each state, rules are generated first for kinds in [`KindTag::ALL`] order,
/// then for theme classes sorted alphabetically by class name.
/// Each rule uses `!important` declarations with `parent = None`.
pub fn theme_state_stylesheet(theme: &Theme) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Hovered,
        Pressed,
        Focused,
        Disabled,
    }

    let states = [State::Hovered, State::Pressed, State::Focused, State::Disabled];

    let mut class_names: Vec<&str> = theme.classes.keys().map(|k| &**k).collect();
    class_names.sort_unstable();

    let mut rules = Vec::new();

    for state in states {
        fn pick(state: State, ss: &StateStyles) -> &Style {
            match state {
                State::Hovered => &ss.hovered,
                State::Pressed => &ss.pressed,
                State::Focused => &ss.focused,
                State::Disabled => &ss.disabled,
            }
        }

        let make_selector = |sel: &str| -> String {
            match state {
                State::Hovered => format!("{}:hover:not([{}])", sel, DISABLED_ATTR),
                State::Pressed => format!("{}:active:not([{}])", sel, DISABLED_ATTR),
                State::Focused => format!(
                    "{}:focus-visible:not([{}]), {}:focus-within:not([{}])",
                    sel, DISABLED_ATTR, sel, DISABLED_ATTR
                ),
                State::Disabled => format!("{}[{}]", sel, DISABLED_ATTR),
            }
        };

        // 1. Kinds in KindTag::ALL order
        for tag in KindTag::ALL {
            if let Some(ss) = theme.kinds.get(&tag) {
                let style = pick(state, ss);
                if !style.is_empty() {
                    let decls = declarations(style, None);
                    if !decls.is_empty() {
                        let sel = format!(".{}", kind_class(tag));
                        let selector = make_selector(&sel);
                        let body = declarations_text(&decls, true);
                        rules.push(format!("{} {{ {} }}", selector, body));
                    }
                }
            }
        }

        // 2. Theme classes sorted by class name
        for class_name in &class_names {
            if let Some(ss) = theme.classes.get(*class_name) {
                let style = pick(state, ss);
                if !style.is_empty() {
                    let decls = declarations(style, None);
                    if !decls.is_empty() {
                        let sel = format!(".{}", class_class(class_name));
                        let selector = make_selector(&sel);
                        let body = declarations_text(&decls, true);
                        rules.push(format!("{} {{ {} }}", selector, body));
                    }
                }
            }
        }
    }

    if rules.is_empty() {
        String::new()
    } else {
        let mut out = rules.join("\n");
        out.push('\n');
        out
    }
}

/// Static base CSS for efui elements and the UI layer.
pub fn base_stylesheet() -> &'static str {
    "\
/* Reset: all efui elements use border-box, zero margins, min sizes 0, inherit typography */
.efui {
  box-sizing: border-box;
  margin: 0;
  min-width: 0;
  min-height: 0;
  font: inherit;
  color: inherit;
}

/* Hidden elements are completely removed from layout */
.efui[hidden] {
  display: none !important;
}

/* Flex display for layers, containers, scroll areas, and buttons */
.efui-layer,
.efui-k-container,
.efui-k-scroll,
.efui-k-button {
  display: flex;
}

/* Containers default to column flex-direction */
.efui-k-container {
  flex-direction: column;
}

/* Scroll areas clip and allow scrolling on the main axis */
.efui-k-scroll {
  overflow: auto;
}

/* Image elements default to contain fit */
.efui-k-image {
  object-fit: contain;
}

/* Button appearance reset; native range/checkbox appearance is preserved */
.efui-k-button {
  appearance: none;
  -webkit-appearance: none;
  border: none;
  background: transparent;
  user-select: none;
  cursor: pointer;
}

/* Text input appearance reset */
.efui-k-text-input {
  appearance: none;
  -webkit-appearance: none;
}

/* Disabled elements: cursor default; hover suppression handled by :not([data-efui-disabled]) */
[data-efui-disabled] {
  cursor: default;
}
"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Border, Edges, FontId};

    #[test]
    fn test_fmt_float() {
        assert_eq!(fmt_float(12.0), "12");
        assert_eq!(fmt_float(0.5), "0.5");
        assert_eq!(fmt_float(0.0), "0");
        assert_eq!(fmt_float(-0.0), "0");
        assert_eq!(fmt_float(f32::NAN), "0");
        assert_eq!(fmt_float(f32::INFINITY), "0");
        assert_eq!(fmt_float(f32::NEG_INFINITY), "0");
        assert_eq!(fmt_float(12.5), "12.5");
        assert_eq!(fmt_float(100.0), "100");
        assert_eq!(fmt_float(0.123), "0.123");
        assert_eq!(fmt_float(0.12), "0.12");
        assert_eq!(fmt_float(1.0), "1");
        assert_eq!(fmt_float(-12.5), "-12.5");
    }

    #[test]
    fn test_kind_class() {
        assert_eq!(kind_class(KindTag::Container), "efui-k-container");
        assert_eq!(kind_class(KindTag::Button), "efui-k-button");
        assert_eq!(kind_class(KindTag::TextInput), "efui-k-text-input");
        assert_eq!(kind_class(KindTag::Scroll), "efui-k-scroll");
    }

    #[test]
    fn test_class_class_escaping() {
        // Basic alphanumeric kept
        assert_eq!(class_class("panel"), "efui-c-panel");
        assert_eq!(class_class("danger_btn"), "efui-c-danger_btn");

        // Leading digit escaped
        assert_eq!(class_class("123"), "efui-c-_x31_23");
        assert_eq!(class_class("1panel"), "efui-c-_x31_panel");

        // Hyphen followed by digit escaped
        assert_eq!(class_class("-1"), "efui-c-_x2D_1");
        assert_eq!(class_class("col-6"), "efui-c-col_x2D_6");

        // Other chars escaped
        assert_eq!(class_class("a b"), "efui-c-a_x20_b");
        assert_eq!(class_class("foo.bar"), "efui-c-foo_x2E_bar");
        assert_eq!(class_class("menu_★"), "efui-c-menu__u2605_");

        // Injective: distinct inputs map to distinct class names
        let samples = [
            "panel",
            "1panel",
            "panel1",
            "a b",
            "a_b",
            "a-b",
            "foo.bar",
            "foo:bar",
            "danger",
            "-1",
            "1",
        ];
        let mut classes = std::collections::HashSet::new();
        for &s in &samples {
            assert!(classes.insert(class_class(s)), "Collision for {}", s);
        }
    }

    #[test]
    fn test_declarations_full_style() {
        let style = Style::new()
            .direction(Direction::Row)
            .justify(Justify::SpaceBetween)
            .align(Align::Center)
            .gap(12.0)
            .padding(Edges::new(4.0, 8.0, 12.0, 16.0))
            .min_width(50.0)
            .max_width(500.0)
            .min_height(20.0)
            .max_height(200.0)
            .background(Color::rgba(0.2, 0.4, 0.6, 0.8))
            .border(Border::new(2.0, Color::BLACK, 6.0))
            .text_color(Color::WHITE)
            .font_size(18.0)
            .font(FontId(5))
            .bold(true)
            .text_align(TextAlign::End)
            .opacity(0.9);

        let decls = declarations(&style, None);
        let map: std::collections::HashMap<_, _> = decls.into_iter().collect();

        assert_eq!(map.get("flex-direction").unwrap(), "row");
        assert_eq!(map.get("justify-content").unwrap(), "space-between");
        assert_eq!(map.get("align-items").unwrap(), "center");
        assert_eq!(map.get("gap").unwrap(), "12px");
        assert_eq!(map.get("padding").unwrap(), "4px 8px 12px 16px");
        assert_eq!(map.get("min-width").unwrap(), "50px");
        assert_eq!(map.get("max-width").unwrap(), "500px");
        assert_eq!(map.get("min-height").unwrap(), "20px");
        assert_eq!(map.get("max-height").unwrap(), "200px");
        assert_eq!(map.get("background-color").unwrap(), "rgba(51, 102, 153, 0.8)");
        assert_eq!(map.get("border").unwrap(), "2px solid rgba(0, 0, 0, 1)");
        assert_eq!(map.get("border-radius").unwrap(), "6px");
        assert_eq!(map.get("color").unwrap(), "rgba(255, 255, 255, 1)");
        assert_eq!(map.get("font-size").unwrap(), "18px");
        assert_eq!(map.get("font-family").unwrap(), "var(--efui-font-5, sans-serif)");
        assert_eq!(map.get("font-weight").unwrap(), "700");
        assert_eq!(map.get("text-align").unwrap(), "end");
        assert_eq!(map.get("opacity").unwrap(), "0.9");
    }

    #[test]
    fn test_declarations_sizes() {
        // Main vs cross axis for Fill
        let fill_w = Style::new().width(Size::Fill);
        assert_eq!(declarations(&fill_w, Some(Direction::Row)), vec![("flex", "1 1 0".to_string())]);
        assert_eq!(declarations(&fill_w, Some(Direction::Column)), vec![("align-self", "stretch".to_string())]);
        assert_eq!(declarations(&fill_w, None), vec![("width", "100%".to_string())]);

        let fill_h = Style::new().height(Size::Fill);
        assert_eq!(declarations(&fill_h, Some(Direction::Row)), vec![("align-self", "stretch".to_string())]);
        assert_eq!(declarations(&fill_h, Some(Direction::Column)), vec![("flex", "1 1 0".to_string())]);
        assert_eq!(declarations(&fill_h, None), vec![("height", "100%".to_string())]);

        // Portion
        let portion_w = Style::new().width(Size::Portion(3));
        assert_eq!(declarations(&portion_w, Some(Direction::Row)), vec![("flex", "3 1 0".to_string())]);
        assert_eq!(declarations(&portion_w, Some(Direction::Column)), vec![("align-self", "stretch".to_string())]);

        // Px
        let px_w = Style::new().width(Size::Px(150.0));
        assert_eq!(
            declarations(&px_w, Some(Direction::Row)),
            vec![("width", "150px".to_string()), ("flex-shrink", "0".to_string())]
        );
        assert_eq!(declarations(&px_w, Some(Direction::Column)), vec![("width", "150px".to_string())]);

        // Auto produces nothing
        let auto_style = Style::new().width(Size::Auto).height(Size::Auto);
        assert!(declarations(&auto_style, Some(Direction::Row)).is_empty());
    }

    #[test]
    fn test_declarations_text() {
        let decls = vec![("color", "red".to_string()), ("font-size", "16px".to_string())];
        assert_eq!(declarations_text(&decls, false), "color: red; font-size: 16px");
        assert_eq!(
            declarations_text(&decls, true),
            "color: red !important; font-size: 16px !important"
        );
        assert_eq!(declarations_text(&[], false), "");
    }

    #[test]
    fn test_layer_declarations() {
        // Fill
        let fill = Layer::fill(5);
        assert_eq!(
            layer_declarations(&fill),
            vec![
                ("position", "absolute".to_string()),
                ("inset", "0".to_string()),
                ("z-index", "5".to_string()),
                ("display", "flex".to_string()),
                ("pointer-events", "auto".to_string()),
            ]
        );

        // Center
        let center = Layer::centered(10).offset(5.0, -10.0);
        assert_eq!(
            layer_declarations(&center),
            vec![
                ("position", "absolute".to_string()),
                ("left", "50%".to_string()),
                ("top", "50%".to_string()),
                (
                    "transform",
                    "translate(calc(-50% + 5px), calc(-50% + -10px))".to_string()
                ),
                ("z-index", "10".to_string()),
                ("display", "flex".to_string()),
                ("pointer-events", "auto".to_string()),
            ]
        );

        // TopLeft
        let top_left = Layer::top_left(0).offset(8.0, 16.0);
        assert_eq!(
            layer_declarations(&top_left),
            vec![
                ("position", "absolute".to_string()),
                ("left", "8px".to_string()),
                ("top", "16px".to_string()),
                ("z-index", "0".to_string()),
                ("display", "flex".to_string()),
                ("pointer-events", "auto".to_string()),
            ]
        );

        // BottomRight
        let bottom_right = Layer::bottom_right(2).offset(20.0, 30.0);
        assert_eq!(
            layer_declarations(&bottom_right),
            vec![
                ("position", "absolute".to_string()),
                ("right", "20px".to_string()),
                ("bottom", "30px".to_string()),
                ("z-index", "2".to_string()),
                ("display", "flex".to_string()),
                ("pointer-events", "auto".to_string()),
            ]
        );
    }

    #[test]
    fn test_theme_state_stylesheet_empty() {
        assert_eq!(theme_state_stylesheet(&Theme::empty()), "");

        let mut theme = Theme::empty();
        theme.set_kind(KindTag::Button, StateStyles::normal(Style::new().gap(8.0)));
        theme.set_class("panel", StateStyles::normal(Style::new().padding(16.0)));
        assert_eq!(theme_state_stylesheet(&theme), "");
    }

    #[test]
    fn test_theme_state_stylesheet_order_selectors_and_important() {
        let mut theme = Theme::empty();
        theme.set_kind(
            KindTag::Button,
            StateStyles {
                normal: Style::default(),
                hovered: Style::new().background(Color::hex(0x111111)),
                pressed: Style::new().background(Color::hex(0x222222)),
                focused: Style::new().border(Border::new(2.0, Color::hex(0x333333), 4.0)),
                disabled: Style::new().opacity(0.5),
            },
        );
        theme.set_class(
            "panel",
            StateStyles {
                normal: Style::default(),
                hovered: Style::new().background(Color::hex(0x444444)),
                pressed: Style::default(),
                focused: Style::default(),
                disabled: Style::new().opacity(0.3),
            },
        );

        let sheet = theme_state_stylesheet(&theme);
        let lines: Vec<&str> = sheet.lines().collect();

        // 6 rules expected:
        // 1. hovered Button
        // 2. hovered panel
        // 3. pressed Button
        // 4. focused Button
        // 5. disabled Button
        // 6. disabled panel
        assert_eq!(lines.len(), 6);

        // Check selectors and ordering
        assert!(lines[0].starts_with(".efui-k-button:hover:not([data-efui-disabled]) {"));
        assert!(lines[1].starts_with(".efui-c-panel:hover:not([data-efui-disabled]) {"));
        assert!(lines[2].starts_with(".efui-k-button:active:not([data-efui-disabled]) {"));
        assert!(lines[3].starts_with(".efui-k-button:focus-visible:not([data-efui-disabled]), .efui-k-button:focus-within:not([data-efui-disabled]) {"));
        assert!(lines[4].starts_with(".efui-k-button[data-efui-disabled] {"));
        assert!(lines[5].starts_with(".efui-c-panel[data-efui-disabled] {"));

        // All rules must have !important
        for line in lines {
            assert!(line.contains("!important"));
        }
    }

    #[test]
    fn test_base_stylesheet() {
        let base = base_stylesheet();
        assert!(base.contains(".efui {"));
        assert!(base.contains("box-sizing: border-box;"));
        assert!(base.contains(".efui[hidden] {"));
        assert!(base.contains("display: none !important;"));
        assert!(base.contains(".efui-layer"));
        assert!(base.contains(".efui-k-container"));
        assert!(base.contains(".efui-k-scroll"));
        assert!(base.contains(".efui-k-button"));
        assert!(base.contains("[data-efui-disabled] {"));
        assert!(base.contains("cursor: default;"));
    }
}
