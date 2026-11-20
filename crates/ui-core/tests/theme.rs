//! Theme resolution.

use efeui_core::*;

fn red() -> Color {
    Color::hex(0xff0000)
}
fn green() -> Color {
    Color::hex(0x00ff00)
}
fn blue() -> Color {
    Color::hex(0x0000ff)
}

fn setup() -> (UiTree, WidgetId) {
    let mut ui = UiTree::new();
    let root = ui.layer("root", Layer::fill(0));
    let mut t = Theme::empty();
    t.base = Style::new().font_size(10.0).text_color(Color::WHITE).gap(1.0);
    t.set_kind(
        KindTag::Button,
        StateStyles {
            normal: Style::new().font_size(12.0).background(red()).padding(4.0),
            hovered: Style::new().background(green()),
            pressed: Style::new().background(blue()),
            focused: Style::new().opacity(0.9),
            disabled: Style::new().opacity(0.4),
        },
    );
    t.set_class("a", StateStyles { normal: Style::new().font_size(14.0), hovered: Style::new().gap(9.0), ..Default::default() });
    t.set_class("b", StateStyles::normal(Style::new().font_size(16.0).bold(true)));
    ui.set_theme(t);
    (ui, root)
}

#[test]
fn order_base_kind_classes_inline() {
    let (mut ui, root) = setup();
    let plain = ui.container(root).id();
    let s = ui.resolve(plain, WidgetState::default()).unwrap();
    assert_eq!(s.font_size, Some(10.0)); // base only
    assert_eq!(s.background, None);

    let b = ui.button(root, "x").id();
    let s = ui.resolve(b, WidgetState::default()).unwrap();
    assert_eq!(s.font_size, Some(12.0)); // kind over base
    assert_eq!(s.gap, Some(1.0)); // base survives
    assert_eq!(s.background, Some(red()));

    // classes in order: later wins
    ui.set_classes(b, &["a", "b"]);
    let s = ui.resolve(b, WidgetState::default()).unwrap();
    assert_eq!(s.font_size, Some(16.0));
    assert_eq!(s.bold, Some(true));
    ui.set_classes(b, &["b", "a"]);
    assert_eq!(ui.resolve(b, WidgetState::default()).unwrap().font_size, Some(14.0));

    // inline over classes
    ui.edit_style(b, |st| st.font_size = Some(20.0));
    assert_eq!(ui.resolve(b, WidgetState::default()).unwrap().font_size, Some(20.0));
    // unknown classes are harmless
    ui.set_class(b, "does-not-exist");
    assert_eq!(ui.resolve(b, WidgetState::default()).unwrap().font_size, Some(20.0));
}

#[test]
fn state_styles_go_on_top() {
    let (mut ui, root) = setup();
    let b = ui.button(root, "x").class("a").id();
    ui.edit_style(b, |st| st.background = Some(Color::BLACK)); // inline

    let hov = WidgetState { hovered: true, ..Default::default() };
    let s = ui.resolve(b, hov).unwrap();
    // state style of the kind beats the inline style
    assert_eq!(s.background, Some(green()));
    // class state style applies, kind state style too
    assert_eq!(s.gap, Some(9.0));

    let both = WidgetState { hovered: true, pressed: true, ..Default::default() };
    assert_eq!(ui.resolve(b, both).unwrap().background, Some(blue())); // pressed after hovered

    let foc = WidgetState { focused: true, ..Default::default() };
    let s = ui.resolve(b, foc).unwrap();
    assert_eq!(s.opacity, Some(0.9));
    assert_eq!(s.background, Some(Color::BLACK)); // no focused background: inline stays
}

#[test]
fn disabled_comes_from_the_node() {
    let (mut ui, root) = setup();
    let col = ui.container(root).id();
    let b = ui.button(col, "x").id();
    assert_eq!(ui.resolve(b, WidgetState::default()).unwrap().opacity, None);
    ui.set_enabled(b, false);
    assert_eq!(ui.resolve(b, WidgetState::default()).unwrap().opacity, Some(0.4));
    ui.set_enabled(b, true);
    ui.set_enabled(col, false); // ancestor disables too
    assert_eq!(ui.resolve(b, WidgetState::default()).unwrap().opacity, Some(0.4));
}

#[test]
fn merge_overlay_semantics() {
    let a = Style::new().gap(1.0).bold(true);
    let b = Style::new().gap(2.0).font_size(9.0);
    let m = a.overlaid(&b);
    assert_eq!((m.gap, m.bold, m.font_size), (Some(2.0), Some(true), Some(9.0)));
    assert!(Style::new().is_empty());
    assert!(!m.is_empty());
}

#[test]
fn set_theme_is_one_entry_and_default_theme_exists() {
    let mut ui = UiTree::new();
    let root = ui.layer("r", Layer::fill(0));
    let b = ui.button(root, "ok").id();
    ui.take_changes();
    // built-in dark theme
    let s = ui.resolve(b, WidgetState::default()).unwrap();
    assert!(s.background.is_some() && s.text_color.is_some() && s.font_size.is_some());
    assert!(ui.theme().class("panel").is_some());
    ui.set_theme(Theme::empty());
    assert_eq!(ui.take_changes(), vec![UiChange::Theme]);
    assert!(ui.resolve(b, WidgetState::default()).unwrap().is_empty());
}
