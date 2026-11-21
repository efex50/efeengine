//! Themes and class resolution (API.md section 6).

use std::collections::HashMap;
use std::sync::Arc;

use crate::node::KindTag;
use crate::style::{Border, Color, Edges, Size, Style, TextAlign};

/// Interaction state of a node while resolving its style.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct WidgetState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub disabled: bool,
}

#[derive(Clone, PartialEq, Default, Debug)]
pub struct StateStyles {
    pub normal: Style,
    pub hovered: Style,
    pub pressed: Style,
    pub focused: Style,
    pub disabled: Style,
}

impl StateStyles {
    pub fn normal(style: Style) -> Self {
        Self { normal: style, ..Default::default() }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    /// Defaults for every node.
    pub base: Style,
    /// Per node kind.
    pub kinds: HashMap<KindTag, StateStyles>,
    /// Named classes ("panel", "title", ...).
    pub classes: HashMap<Arc<str>, StateStyles>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::dark()
    }
}

impl Theme {
    /// No styles at all.
    pub fn empty() -> Self {
        Self { base: Style::default(), kinds: HashMap::new(), classes: HashMap::new() }
    }

    pub fn class(&self, name: &str) -> Option<&StateStyles> {
        self.classes.get(name)
    }

    pub fn set_class(&mut self, name: &str, styles: StateStyles) {
        self.classes.insert(name.into(), styles);
    }

    pub fn set_kind(&mut self, kind: KindTag, styles: StateStyles) {
        self.kinds.insert(kind, styles);
    }

    /// The resolved style of a node of `kind` with `classes` (in order), inline `inline` and
    /// interaction `state`.
    ///
    /// Order, later wins: `base`, the kind's `normal`, each class's `normal`, the inline style;
    /// then the active state styles on top of everything including the inline style, state by
    /// state in the order hovered, pressed, focused, disabled (for each state the kind's style,
    /// then each class's). So a later state beats an earlier one across kind and classes:
    /// `disabled` always wins.
    pub fn resolve<'a>(&self, kind: KindTag, classes: impl IntoIterator<Item = &'a str> + Clone, inline: &Style, state: WidgetState) -> Style {
        let kind_styles = self.kinds.get(&kind);
        let mut out = self.base;
        if let Some(k) = kind_styles {
            out.merge(&k.normal);
        }
        for c in classes.clone() {
            if let Some(c) = self.classes.get(c) {
                out.merge(&c.normal);
            }
        }
        out.merge(inline);
        // State layers on top of everything, one state at a time (hovered, pressed, focused,
        // disabled), the kind's before the classes': a later state always beats an earlier one,
        // whoever defines it, so `disabled` wins.
        let layers: [(bool, fn(&StateStyles) -> &Style); 4] = [
            (state.hovered, |s| &s.hovered),
            (state.pressed, |s| &s.pressed),
            (state.focused, |s| &s.focused),
            (state.disabled, |s| &s.disabled),
        ];
        for (on, pick) in layers {
            if !on {
                continue;
            }
            if let Some(k) = kind_styles {
                out.merge(pick(k));
            }
            for c in classes.clone() {
                if let Some(c) = self.classes.get(c) {
                    out.merge(pick(c));
                }
            }
        }
        out
    }

    /// The built-in dark theme (readable at 1x and 2x).
    pub fn dark() -> Self {
        let text = Color::hex(0xe6e8ee);
        let surface = Color::hex(0x1e2128);
        let control = Color::hex(0x3a3f4b);
        let control_hover = Color::hex(0x4a5060);
        let control_press = Color::hex(0x2c303a);
        let accent = Color::hex(0x5b9dff);
        let mut t = Theme::empty();
        t.base = Style::new().text_color(text).font_size(16.0).text_align(TextAlign::Start).opacity(1.0);

        t.set_kind(
            KindTag::Button,
            StateStyles {
                normal: Style::new()
                    .background(control)
                    .padding(Edges::symmetric(16.0, 8.0))
                    .border(Border::new(1.0, Color::hex(0x4a5060), 6.0))
                    .text_align(TextAlign::Center),
                hovered: Style::new().background(control_hover),
                pressed: Style::new().background(control_press),
                focused: Style::new().border(Border::new(2.0, accent, 6.0)),
                disabled: Style::new().opacity(0.5),
            },
        );
        t.set_kind(
            KindTag::Checkbox,
            StateStyles {
                normal: Style::new().gap(8.0),
                hovered: Style::new().background(Color::hex(0x2a2e38)),
                pressed: Style::default(),
                focused: Style::new().border(Border::new(2.0, accent, 4.0)),
                disabled: Style::new().opacity(0.5),
            },
        );
        t.set_kind(
            KindTag::Slider,
            StateStyles {
                normal: Style::new().width(Size::Px(200.0)).background(control).border(Border::new(0.0, Color::TRANSPARENT, 4.0)).text_color(accent),
                hovered: Style::new().text_color(Color::hex(0x7fb2ff)),
                pressed: Style::default(),
                focused: Style::new().border(Border::new(2.0, accent, 4.0)),
                disabled: Style::new().opacity(0.5),
            },
        );
        t.set_kind(
            KindTag::TextInput,
            StateStyles {
                normal: Style::new()
                    .background(Color::hex(0x14161b))
                    .padding(Edges::symmetric(10.0, 6.0))
                    .border(Border::new(1.0, Color::hex(0x4a5060), 4.0))
                    .width(Size::Px(220.0)),
                hovered: Style::new().border(Border::new(1.0, Color::hex(0x6a7388), 4.0)),
                pressed: Style::default(),
                focused: Style::new().border(Border::new(2.0, accent, 4.0)),
                disabled: Style::new().opacity(0.5),
            },
        );
        t.set_kind(KindTag::Scroll, StateStyles::normal(Style::new().gap(4.0)));

        t.set_class(
            "panel",
            StateStyles::normal(Style::new().background(surface).padding(16.0).border(Border::new(1.0, Color::hex(0x2d323d), 8.0))),
        );
        t.set_class("title", StateStyles::normal(Style::new().font_size(28.0).bold(true)));
        t.set_class("muted", StateStyles::normal(Style::new().text_color(Color::hex(0x8b90a0))));
        t.set_class("danger", StateStyles::normal(Style::new().text_color(Color::hex(0xff6b6b))));

        // Wireframe classes (tool windows such as the debug menu): square, high-contrast lines.
        let line = Color::hex(0xb4b9c8);
        t.set_class(
            "wire-panel",
            StateStyles::normal(Style::new().background(Color::hex(0x15171c)).border(Border::new(1.0, line, 0.0)).gap(0.0).padding(0.0)),
        );
        // A one-pixel rule across a column (header, tab bar and row separators).
        t.set_class("separator", StateStyles::normal(Style::new().height(Size::Px(1.0)).width(Size::Fill).background(line)));
        // The same, vertical (between tabs).
        t.set_class("separator-v", StateStyles::normal(Style::new().width(Size::Px(1.0)).background(line)));
        t.set_class("wire-header", StateStyles::normal(Style::new().padding(Edges::symmetric(10.0, 8.0)).text_align(TextAlign::Center)));
        t.set_class("wire-row", StateStyles::normal(Style::new().padding(Edges::symmetric(8.0, 6.0)).gap(8.0)));
        // Tab buttons: square, flat, the active one filled and bold. A tab bar has no tab widget:
        // it is a row of buttons with `tab` (and `tab-active` on the current one).
        let flat = Border::new(0.0, Color::TRANSPARENT, 0.0);
        let active = Color::hex(0x2c4a7a);
        t.set_class(
            "tab",
            StateStyles {
                normal: Style::new().background(Color::hex(0x15171c)).border(flat).padding(Edges::symmetric(22.0, 8.0)),
                hovered: Style::new().background(Color::hex(0x262a33)),
                pressed: Style::new().background(Color::hex(0x20242c)),
                focused: Style::new().border(flat),
                disabled: Style::default(),
            },
        );
        t.set_class(
            "tab-active",
            StateStyles {
                normal: Style::new().background(active).bold(true),
                hovered: Style::new().background(active),
                pressed: Style::new().background(active),
                focused: Style::default(),
                disabled: Style::default(),
            },
        );
        // Selectable text in a row: no box, no focus ring, so it reads as text.
        t.set_class(
            "wire-text",
            StateStyles {
                normal: Style::new().background(Color::TRANSPARENT).border(flat).padding(Edges::symmetric(2.0, 2.0)),
                hovered: Style::new().border(flat),
                pressed: Style::default(),
                focused: Style::new().border(flat).background(Color::hex(0x1c2027)),
                disabled: Style::default(),
            },
        );
        // A small bordered field next to a setting label.
        t.set_class(
            "wire-field",
            StateStyles {
                normal: Style::new().border(Border::new(1.0, line, 0.0)).background(Color::hex(0x0f1115)).padding(Edges::symmetric(6.0, 3.0)).width(Size::Px(72.0)),
                hovered: Style::new().border(Border::new(1.0, Color::WHITE, 0.0)),
                pressed: Style::default(),
                focused: Style::new().border(Border::new(2.0, accent, 0.0)),
                disabled: Style::default(),
            },
        );
        // A field whose text was rejected (add it after `wire-field`): red border and text.
        let red = Color::hex(0xff6b6b);
        t.set_class(
            "wire-error",
            StateStyles {
                normal: Style::new().border(Border::new(1.0, red, 0.0)).text_color(red),
                hovered: Style::new().border(Border::new(1.0, red, 0.0)),
                pressed: Style::default(),
                focused: Style::new().border(Border::new(2.0, red, 0.0)),
                disabled: Style::default(),
            },
        );
        t
    }
}
