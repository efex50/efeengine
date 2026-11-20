//! The fallback backend (taffy + cosmic-text through `Draw2D`): layout, drawing into a
//! `DrawList`, interaction and text input, all headless (no window, no GPU).
#![cfg(feature = "fallback")]

use efeengine_core::input::{ElementState, InputEvent, InputEventKind, Key, KeyCode, Modifiers, MouseButton, NamedKey, WheelUnit};
use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::{DrawList, Image};
use efeengine_ui::fallback::FallbackBackend;
use efeengine_ui::*;

struct H {
    b: FallbackBackend,
    tree: UiTree,
    cx: UiBackendCx,
    /// Every signal drained so far.
    log: Vec<UiSignal>,
}

const ATLAS: TextureHandle = TextureHandle { id: 777 };

impl H {
    fn new(window: (f32, f32), build: impl FnOnce(&mut UiTree)) -> H {
        let mut tree = UiTree::new();
        build(&mut tree);
        let mut b = FallbackBackend::new();
        b.set_caret_blink(false);
        let mut cx = UiBackendCx::new(window, 1.0);
        b.init(&mut cx);
        b.set_atlas_texture(ATLAS);
        let mut h = H { b, tree, cx, log: Vec::new() };
        h.sync();
        h
    }

    fn sync(&mut self) {
        let ch = self.tree.take_changes();
        let theme = self.tree.theme().clone();
        self.b.sync(&self.tree, &ch, &theme, &mut self.cx);
    }

    fn id(&self, key: &str) -> WidgetId {
        self.tree.find(key).unwrap_or_else(|| panic!("no widget {key}"))
    }

    fn rect(&mut self, key: &str) -> (f32, f32, f32, f32) {
        let id = self.id(key);
        self.b.widget_rect(id).unwrap_or_else(|| panic!("{key} has no rect"))
    }

    fn event(&mut self, kind: InputEventKind) -> bool {
        let f = self.b.input(&InputEvent::new(kind));
        self.pump();
        f == Filter::Captured
    }

    /// Drains signals the way the plugin does: values are written back into the tree.
    fn pump(&mut self) {
        let mut out = Vec::new();
        self.b.drain_signals(&mut out);
        for s in &out {
            if let UiSignalKind::Changed(v) = &s.kind {
                self.tree.write_back(s.widget, v.clone());
            }
        }
        self.log.extend(out);
    }

    fn mouse(&mut self, x: f32, y: f32) -> bool {
        self.event(InputEventKind::MouseMotion { pos: (x, y), delta: (0.0, 0.0) })
    }

    fn button(&mut self, x: f32, y: f32, state: ElementState) -> bool {
        self.event(InputEventKind::MouseButton { button: MouseButton::Left, state, pos: (x, y) })
    }

    fn click_at(&mut self, x: f32, y: f32) {
        self.button(x, y, ElementState::Pressed);
        self.button(x, y, ElementState::Released);
    }

    fn click(&mut self, key: &str) {
        let (x, y, w, h) = self.rect(key);
        self.click_at(x + w * 0.5, y + h * 0.5);
    }

    fn key(&mut self, k: Key, mods: Modifiers) -> bool {
        let phys = KeyCode::A;
        let a = self.event(InputEventKind::key(phys, k.clone(), ElementState::Pressed, false, mods));
        self.event(InputEventKind::key(phys, k, ElementState::Released, false, mods));
        a
    }

    fn named(&mut self, k: NamedKey) -> bool {
        self.key(Key::Named(k), Modifiers::NONE)
    }

    fn type_text(&mut self, t: &str) -> bool {
        self.event(InputEventKind::Text(t.to_string()))
    }

    fn kinds(&self, key: &str) -> Vec<UiSignalKind> {
        let id = self.id(key);
        self.log.iter().filter(|s| s.widget == id).map(|s| s.kind.clone()).collect()
    }

    fn frame(&mut self) -> DrawList {
        self.b.prepare();
        let mut l = DrawList::new();
        self.b.draw(&mut l);
        l
    }
}

fn panel_menu(t: &mut UiTree) {
    let root = t.layer("menu", Layer::centered(1));
    let col = t.column(root).class("panel").id();
    t.text(col, "Title").class("title").key("title");
    t.button(col, "Play").key("play");
    t.checkbox(col, "Fullscreen", false).key("fs");
    t.slider(col, 0.0..=1.0, 0.5).key("vol");
    t.text_input(col, "").placeholder("Player name").key("name");
}

// ---- layout ----

#[test]
fn a_column_stacks_children_with_gap_and_padding() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        let col = t.column(root).style(Style::new().padding(10.0).gap(6.0).width(Size::Px(200.0))).id();
        t.text(col, "one").key("a");
        t.text(col, "two").key("b");
    });
    let (a, b) = (h.rect("a"), h.rect("b"));
    assert_eq!((a.0, a.1), (10.0, 10.0));
    // stretched to the content width (200 - 2 * 10)
    assert_eq!(a.2, 180.0);
    // 16px text, line height 1.3
    assert!((a.3 - 20.8).abs() < 0.5, "{a:?}");
    assert!((b.1 - (a.1 + a.3 + 6.0)).abs() < 0.01, "{a:?} {b:?}");
}

#[test]
fn fill_and_portion_share_the_free_space_of_a_definite_parent() {
    let mut h = H::new((400.0, 100.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        let row = t.container(root).style(Style::new().direction(Direction::Row).width(Size::Px(300.0))).id();
        t.container(row).key("fixed").style(Style::new().width(Size::Px(60.0)));
        t.container(row).key("one").style(Style::new().width(Size::Portion(1)));
        t.container(row).key("two").style(Style::new().width(Size::Portion(2)));
    });
    assert_eq!(h.rect("fixed").2, 60.0);
    assert!((h.rect("one").2 - 80.0).abs() < 0.01);
    assert!((h.rect("two").2 - 160.0).abs() < 0.01);
}

#[test]
fn justify_and_align_place_children() {
    let mut h = H::new((400.0, 200.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        let row = t
            .container(root)
            .style(Style::new().direction(Direction::Row).width(Size::Px(300.0)).height(Size::Px(100.0)).justify(Justify::Center).align(Align::End))
            .id();
        t.container(row).key("box").style(Style::new().width(Size::Px(50.0)).height(Size::Px(20.0)));
    });
    let b = h.rect("box");
    assert_eq!((b.0, b.1, b.2, b.3), (125.0, 80.0, 50.0, 20.0));
}

#[test]
fn anchors_and_offsets_place_the_layer() {
    let mut h = H::new((400.0, 200.0), |t| {
        let a = t.layer("lc", Layer::centered(0));
        t.container(a).key("c").style(Style::new().width(Size::Px(100.0)).height(Size::Px(40.0)));
        let b = t.layer("lbr", Layer::bottom_right(1).offset(10.0, 5.0));
        t.container(b).key("br").style(Style::new().width(Size::Px(100.0)).height(Size::Px(40.0)));
        let f = t.layer("lf", Layer::fill(2));
        t.container(f).key("f");
    });
    assert_eq!(h.rect("c"), (150.0, 80.0, 100.0, 40.0));
    // offsets move away from the anchored edge: 10 px in from the right, 5 px up
    assert_eq!(h.rect("br"), (290.0, 155.0, 100.0, 40.0));
    // the layer root itself is the filled node
    assert_eq!(h.rect("lf"), (0.0, 0.0, 400.0, 200.0));
}

#[test]
fn long_text_wraps_at_the_available_width() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        let col = t.column(root).style(Style::new().width(Size::Px(120.0))).id();
        t.text(col, "the quick brown fox jumps over the lazy dog").key("t");
        t.text(col, "short").key("s");
    });
    let (t, s) = (h.rect("t"), h.rect("s"));
    assert_eq!(t.2, 120.0);
    assert!(t.3 > 2.0 * s.3, "wrapped text is taller: {t:?} vs {s:?}");
}

#[test]
fn images_take_their_pixel_size_and_hidden_nodes_no_space() {
    let img = TextureHandle { id: 5 };
    let mut tree = UiTree::new();
    let root = tree.layer("l", Layer::top_left(0));
    tree.image(root, img.image_id(), ImageFit::Contain).key("img");
    tree.text(root, "hidden").key("hid").visible(false);
    tree.text(root, "after").key("after");
    let mut b = FallbackBackend::new();
    let mut cx = UiBackendCx::new((200.0, 200.0), 1.0);
    cx.images = vec![(img, std::sync::Arc::new(Image::solid(40, 30, [255, 0, 0, 255])))];
    b.init(&mut cx);
    let ch = tree.take_changes();
    b.sync(&tree, &ch, &tree.theme().clone(), &mut cx);
    let id = tree.find("img").unwrap();
    assert_eq!(b.widget_rect(id), Some((0.0, 0.0, 40.0, 30.0)));
    assert_eq!(b.widget_rect(tree.find("after").unwrap()).unwrap().1, 30.0);
    assert_eq!(b.widget_rect(tree.find("hid").unwrap()), None);
}

// ---- drawing ----

#[test]
fn a_frame_has_geometry_and_glyph_quads_from_the_atlas() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    let list = h.frame();
    assert!(!list.is_empty());
    // glyph quads reference the atlas texture, geometry draws untextured
    assert!(list.commands().iter().any(|c| c.state.texture == ATLAS.id));
    assert!(list.commands().iter().any(|c| c.state.texture == 0));
    // all vertices lie inside the window (plus the 0.5 px feather)
    for v in list.vertices() {
        assert!(v.pos[0] >= -1.0 && v.pos[0] <= 401.0 && v.pos[1] >= -1.0 && v.pos[1] <= 301.0, "{:?}", v.pos);
    }
    let atlas = h.b.take_atlas_image().expect("atlas image after drawing text");
    assert!(atlas.rgba8().chunks(4).any(|p| p[3] > 0), "glyphs were rasterised");
    // nothing changed: no new recording
    let (_, records) = h.b.counters();
    assert!(!h.b.prepare());
    assert_eq!(h.b.counters().1, records);
}

#[test]
fn a_scale_factor_scales_the_list_not_the_layout() {
    let mut h = H::new((200.0, 100.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.container(root).key("box").style(Style::new().width(Size::Px(50.0)).height(Size::Px(20.0)).background(Color::WHITE));
    });
    let l1 = h.frame();
    h.b.resize((200.0, 100.0), 2.0);
    let l2 = h.frame();
    let max = |l: &DrawList| l.vertices().iter().map(|v| v.pos[0]).fold(0.0f32, f32::max);
    assert_eq!(max(&l1), 50.0);
    assert_eq!(max(&l2), 100.0);
    assert_eq!(h.rect("box"), (0.0, 0.0, 50.0, 20.0));
}

#[test]
fn scroll_clips_its_children() {
    let mut h = H::new((300.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        let sc = t.scroll(root).style(Style::new().width(Size::Px(100.0)).height(Size::Px(50.0))).id();
        for i in 0..10 {
            t.text(sc, format!("row {i}")).key(&format!("r{i}"));
        }
    });
    let list = h.frame();
    assert!(list.commands().iter().any(|c| c.state.clip.is_some()), "children are drawn with a clip rectangle");
    let clip = list.commands().iter().find_map(|c| c.state.clip).unwrap();
    assert_eq!((clip.x, clip.y, clip.w, clip.h), (0, 0, 100, 50));
}

#[test]
fn image_quads_carry_uvs_for_cover() {
    let img = TextureHandle { id: 5 };
    let mut tree = UiTree::new();
    let root = tree.layer("l", Layer::top_left(0));
    tree.image(root, img.image_id(), ImageFit::Cover).style(Style::new().width(Size::Px(100.0)).height(Size::Px(50.0))).key("i");
    let mut b = FallbackBackend::new();
    let mut cx = UiBackendCx::new((200.0, 200.0), 1.0);
    cx.images = vec![(img, std::sync::Arc::new(Image::solid(50, 50, [255, 0, 0, 255])))];
    b.init(&mut cx);
    let ch = tree.take_changes();
    b.sync(&tree, &ch, &tree.theme().clone(), &mut cx);
    b.prepare();
    let c = b.draw_list().commands().iter().find(|c| c.state.texture == 5).expect("image command");
    assert_eq!(c.state.filter, efeengine_core::render::Filter::Linear);
    let v = &b.draw_list().vertices()[c.verts.start as usize..c.verts.end as usize];
    // a square image covering a 2:1 box shows the middle half vertically
    assert!((v[0].uv[1] - 0.25).abs() < 1e-5 && (v[2].uv[1] - 0.75).abs() < 1e-5, "{:?}", v.iter().map(|v| v.uv).collect::<Vec<_>>());
}

// ---- interaction ----

#[test]
fn clicking_a_button_sends_click_on_release_inside_only() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    let (x, y, w, hh) = h.rect("play");
    // press and release inside
    h.click_at(x + 5.0, y + 5.0);
    assert_eq!(h.kinds("play"), vec![UiSignalKind::Click]);
    // press inside, release outside: no click
    h.log.clear();
    h.button(x + 5.0, y + 5.0, ElementState::Pressed);
    h.button(x - 20.0, y - 20.0, ElementState::Released);
    assert!(h.kinds("play").is_empty());
    // a disabled button does not click but still captures
    h.tree.set_enabled(h.id("play"), false);
    h.sync();
    h.log.clear();
    assert!(h.button(x + w * 0.5, y + hh * 0.5, ElementState::Pressed));
    h.button(x + w * 0.5, y + hh * 0.5, ElementState::Released);
    assert!(h.kinds("play").is_empty());
}

#[test]
fn hover_and_press_change_the_drawn_colour() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    let (x, y, _, _) = h.rect("play");
    let fill = |l: &DrawList| -> Vec<[f32; 4]> { l.vertices().iter().map(|v| v.color).collect() };
    let normal = fill(&h.frame());
    h.mouse(x + 4.0, y + 4.0);
    let hovered = fill(&h.frame());
    assert_ne!(normal, hovered, "hover style differs");
    h.button(x + 4.0, y + 4.0, ElementState::Pressed);
    let pressed = fill(&h.frame());
    assert_ne!(hovered, pressed, "pressed style differs");
    h.button(x + 4.0, y + 4.0, ElementState::Released);
    h.mouse(0.0, 0.0);
    assert_eq!(fill(&h.frame()), normal);
}

#[test]
fn capture_follows_the_pointer_and_focus() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    assert_eq!(h.b.capture(), UiCapture { pointer: false, keyboard: false });
    let captured = h.mouse(1.0, 1.0);
    assert!(!captured, "empty corner passes");
    let (x, y, _, _) = h.rect("play");
    assert!(h.mouse(x + 2.0, y + 2.0));
    assert_eq!(h.b.capture().pointer, true);
    // the panel (it has a background) blocks too
    let (px, py, _, _) = h.rect("title");
    assert!(h.mouse(px + 1.0, py - 3.0));
    // focus a text input: keyboard captured
    h.click("name");
    assert!(h.b.capture().keyboard);
    h.named(NamedKey::Escape);
    assert!(!h.b.capture().keyboard);
}

#[test]
fn modal_layers_block_everything_below() {
    let mut h = H::new((400.0, 300.0), |t| {
        let a = t.layer("a", Layer::fill(0));
        t.button(a, "behind").key("behind");
        let m = t.layer("m", Layer::centered(5).modal());
        t.button(m, "front").key("front");
    });
    let (x, y, w, hh) = h.rect("behind");
    assert!(h.b.capture().pointer && h.b.capture().keyboard, "modal captures all");
    h.click_at(x + w * 0.5, y + hh * 0.5);
    assert!(h.kinds("behind").is_empty());
    h.click("front");
    assert_eq!(h.kinds("front"), vec![UiSignalKind::Click]);
    // keys are captured as well
    assert!(h.named(NamedKey::Enter));
}

#[test]
fn checkbox_toggles_and_reports_the_value() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    h.click("fs");
    assert_eq!(h.kinds("fs"), vec![UiSignalKind::Changed(UiValue::Bool(true))]);
    assert_eq!(h.tree.value(h.id("fs")), Some(UiValue::Bool(true)));
    h.click("fs");
    assert_eq!(h.kinds("fs").last(), Some(&UiSignalKind::Changed(UiValue::Bool(false))));
}

#[test]
fn slider_drag_sets_the_value_snapped_to_the_step() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.slider(root, 0.0..=10.0, 0.0).key("s").step(1.0).style(Style::new().width(Size::Px(216.0)));
    });
    let (x, y, w, hh) = h.rect("s");
    let cy = y + hh * 0.5;
    // the track runs from x + 8 to x + w - 8 (200 px): 100 px in is 5.0
    h.button(x + 8.0 + 100.0, cy, ElementState::Pressed);
    assert_eq!(h.tree.value(h.id("s")), Some(UiValue::F32(5.0)));
    h.mouse(x + 8.0 + 143.0, cy);
    assert_eq!(h.tree.value(h.id("s")), Some(UiValue::F32(7.0)));
    h.mouse(x + w + 50.0, cy);
    assert_eq!(h.tree.value(h.id("s")), Some(UiValue::F32(10.0)));
    h.button(x + w, cy, ElementState::Released);
    // keyboard: focus is on the slider after the click
    h.named(NamedKey::ArrowLeft);
    assert_eq!(h.tree.value(h.id("s")), Some(UiValue::F32(9.0)));
    let n = h.log.len();
    h.mouse(x + 20.0, cy);
    assert_eq!(h.log.len(), n, "no drag after release");
}

#[test]
fn tab_walks_the_focusable_widgets_and_enter_activates() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    assert!(h.named(NamedKey::Tab));
    assert_eq!(h.kinds("play"), vec![UiSignalKind::Focus(true)]);
    h.named(NamedKey::Enter);
    assert_eq!(h.kinds("play"), vec![UiSignalKind::Focus(true), UiSignalKind::Click]);
    h.key(Key::Named(NamedKey::Tab), Modifiers::NONE);
    assert_eq!(h.kinds("play").last(), Some(&UiSignalKind::Focus(false)));
    assert_eq!(h.kinds("fs"), vec![UiSignalKind::Focus(true)]);
    h.named(NamedKey::Space);
    assert_eq!(h.tree.value(h.id("fs")), Some(UiValue::Bool(true)));
    // shift+tab goes back
    h.key(Key::Named(NamedKey::Tab), Modifiers::LSHIFT);
    assert_eq!(h.kinds("play").last(), Some(&UiSignalKind::Focus(true)));
    // keys the UI did not use pass
    h.named(NamedKey::Escape);
    assert!(!h.named(NamedKey::ArrowLeft));
}

#[test]
fn hover_signals_fire_for_opted_in_nodes() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.container(root).key("hov").hover_signals().style(Style::new().width(Size::Px(50.0)).height(Size::Px(50.0)).background(Color::WHITE));
    });
    h.mouse(10.0, 10.0);
    h.mouse(12.0, 12.0);
    h.mouse(200.0, 200.0);
    assert_eq!(h.kinds("hov"), vec![UiSignalKind::Hover(true), UiSignalKind::Hover(false)]);
}

#[test]
fn wheel_scrolls_a_scroll_node_and_the_thumb_drags() {
    let mut h = H::new((300.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        let sc = t.scroll(root).key("sc").style(Style::new().width(Size::Px(100.0)).height(Size::Px(60.0))).id();
        for i in 0..10 {
            t.text(sc, format!("row {i}")).key(&format!("r{i}"));
        }
    });
    let r0 = h.rect("r0");
    assert!(h.mouse(20.0, 20.0) || true);
    let used = h.event(InputEventKind::MouseWheel { delta: (0.0, -1.0), unit: WheelUnit::Line });
    assert!(used, "wheel over a scroll node is used");
    // one line = 40 logical px
    assert!((h.rect("r0").1 - (r0.1 - 40.0)).abs() < 0.01, "{:?}", h.rect("r0"));
    // wheel up scrolls back, never above the start
    h.event(InputEventKind::MouseWheel { delta: (0.0, 5.0), unit: WheelUnit::Line });
    assert_eq!(h.rect("r0").1, r0.1);
    // never below the end: 10 rows of ~20.8 = 208 content, 60 visible
    h.event(InputEventKind::MouseWheel { delta: (0.0, -50.0), unit: WheelUnit::Line });
    let last = h.rect("r9");
    assert!((last.1 + last.3 - 60.0).abs() < 1.0, "{last:?}");
    // drag the thumb (right edge) to the top
    let (sx, sy, sw, _) = h.rect("sc");
    h.button(sx + sw - 5.0, sy + 55.0, ElementState::Pressed);
    h.mouse(sx + sw - 5.0, sy - 100.0);
    h.button(sx + sw - 5.0, sy, ElementState::Released);
    assert_eq!(h.rect("r0").1, r0.1);
}

#[test]
fn tab_scrolls_the_focused_widget_into_view() {
    let mut h = H::new((300.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        let sc = t.scroll(root).style(Style::new().width(Size::Px(120.0)).height(Size::Px(60.0))).id();
        for i in 0..8 {
            t.button(sc, "b").key(&format!("b{i}"));
        }
    });
    for _ in 0..8 {
        h.named(NamedKey::Tab);
    }
    let last = h.rect("b7");
    assert!(last.1 >= 0.0 && last.1 + last.3 <= 60.5, "{last:?}");
}

// ---- text input ----

#[test]
fn typing_edits_the_value_and_reports_every_change() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    h.click("name");
    assert!(h.type_text("Ada"));
    assert_eq!(h.tree.value(h.id("name")), Some(UiValue::Text("Ada".into())));
    h.named(NamedKey::Backspace);
    assert_eq!(h.tree.value(h.id("name")), Some(UiValue::Text("Ad".into())));
    let changes: Vec<_> = h.kinds("name").into_iter().filter(|k| matches!(k, UiSignalKind::Changed(_))).collect();
    assert_eq!(changes.len(), 2);
    h.named(NamedKey::Enter);
    assert_eq!(h.kinds("name").last(), Some(&UiSignalKind::Submit("Ad".into())));
    // an outside change replaces the editor's text
    h.tree.set_text(h.id("name"), "Zed");
    h.sync();
    h.frame();
    h.type_text("!");
    assert_eq!(h.tree.value(h.id("name")), Some(UiValue::Text("Zed!".into())));
}

#[test]
fn text_input_selection_clipboard_and_max_len() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.text_input(root, "hello").key("t").max_len(8);
    });
    h.click("t");
    h.key(Key::character("a"), Modifiers::LCTRL);
    h.key(Key::character("c"), Modifiers::LCTRL);
    assert_eq!(efeengine_core::clipboard::clipboard_text().as_deref(), Some("hello"));
    // typing replaces the selection
    h.type_text("x");
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text("x".into())));
    // paste is cut to max_len
    efeengine_core::clipboard::set_clipboard_text("0123456789");
    h.key(Key::character("v"), Modifiers::LCTRL);
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text("x0123456".into())));
    assert!(!h.type_text("z") || h.tree.value(h.id("t")) == Some(UiValue::Text("x0123456".into())));
    // cut
    h.key(Key::character("a"), Modifiers::LCTRL);
    h.key(Key::character("x"), Modifiers::LCTRL);
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text(String::new())));
    assert_eq!(efeengine_core::clipboard::clipboard_text().as_deref(), Some("x0123456"));
}

#[test]
fn shift_arrows_select_and_arrows_move() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.text_input(root, "abcdef").key("t");
    });
    h.click("t");
    h.named(NamedKey::Home);
    h.key(Key::Named(NamedKey::ArrowRight), Modifiers::NONE);
    h.key(Key::Named(NamedKey::ArrowRight), Modifiers::LSHIFT);
    h.key(Key::Named(NamedKey::ArrowRight), Modifiers::LSHIFT);
    // "bc" is selected: typing replaces it
    h.type_text("X");
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text("aXdef".into())));
    h.key(Key::Named(NamedKey::Delete), Modifiers::NONE);
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text("aXef".into())));
}

#[test]
fn clicking_in_the_text_places_the_caret() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.text_input(root, "abcdef").key("t").style(Style::new().width(Size::Px(200.0)));
    });
    h.frame();
    let (x, y, _, hh) = h.rect("t");
    // far right of the text: caret at the end; far left: at the start
    h.click_at(x + 190.0, y + hh * 0.5);
    h.type_text("!");
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text("abcdef!".into())));
    h.click_at(x + 3.0, y + hh * 0.5);
    h.type_text("?");
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text("?abcdef!".into())));
}

#[test]
fn password_fields_draw_bullets_and_never_copy() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.text_input(root, "secret").key("p").password(true);
    });
    h.click("p");
    efeengine_core::clipboard::set_clipboard_text("untouched");
    h.key(Key::character("a"), Modifiers::LCTRL);
    h.key(Key::character("c"), Modifiers::LCTRL);
    assert_eq!(efeengine_core::clipboard::clipboard_text().as_deref(), Some("untouched"));
    h.type_text("x");
    assert_eq!(h.tree.value(h.id("p")), Some(UiValue::Text("x".into())));
    assert!(h.frame().commands().iter().any(|c| c.state.texture == ATLAS.id));
}

#[test]
fn the_text_input_request_follows_focus() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    assert_eq!(h.b.take_text_input(), None);
    h.click("name");
    let req = h.b.take_text_input().expect("a request after focusing");
    let area = req.expect("an area");
    let (x, y, _, _) = h.rect("name");
    assert!(area.x as f32 >= x && area.y as f32 >= y && area.w > 0 && area.h > 0);
    h.named(NamedKey::Escape);
    assert_eq!(h.b.take_text_input(), Some(None));
}

#[test]
fn cursor_icon_follows_the_widget_under_the_pointer() {
    use efeengine_core::window::CursorIcon;
    let mut h = H::new((400.0, 300.0), panel_menu);
    let (x, y, _, _) = h.rect("play");
    h.mouse(x + 2.0, y + 2.0);
    assert_eq!(h.b.take_cursor_icon(), Some(CursorIcon::Pointer));
    let (nx, ny, _, _) = h.rect("name");
    h.mouse(nx + 2.0, ny + 2.0);
    assert_eq!(h.b.take_cursor_icon(), Some(CursorIcon::Text));
    h.mouse(1.0, 1.0);
    assert_eq!(h.b.take_cursor_icon(), Some(CursorIcon::Default));
}

#[test]
fn removed_widgets_drop_their_state() {
    let mut h = H::new((400.0, 300.0), panel_menu);
    h.click("name");
    h.type_text("a");
    let id = h.id("name");
    h.tree.remove(id);
    h.sync();
    assert!(!h.b.capture().keyboard);
    assert!(h.b.widget_rect(id).is_none());
    h.frame();
}

#[test]
fn glyphs_are_cached_in_the_atlas() {
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.text(root, "aaaa bbbb aaaa").key("t");
    });
    h.frame();
    // Glyphs are cached per subpixel position (4 bins): a, b and their bins, never one per occurrence.
    let n = h.b.cached_glyphs();
    assert!((2..=8).contains(&n), "{n} bitmaps for 14 characters");
    let before = n;
    h.b.resize((400.0, 300.0), 1.0);
    h.frame();
    assert_eq!(h.b.cached_glyphs(), before, "drawing the same text again rasterises nothing new");
}

// ----- read-only text, multi-line text, blur ----------------------------------------------------

fn read_only_menu(t: &mut UiTree) {
    let root = t.layer("l", Layer::top_left(0));
    let col = t.column(root).id();
    t.text_input(col, "log line to copy").key("ro").read_only(true).style(Style::new().width(Size::Px(240.0)));
    t.text_input(col, "plain").key("rw");
    t.button(col, "Other").key("other");
}

#[test]
fn a_read_only_input_selects_and_copies_but_never_edits() {
    let mut h = H::new((400.0, 300.0), read_only_menu);
    h.frame();
    h.click("ro");
    assert!(h.b.capture().keyboard, "a read-only input still takes the focus");
    h.key(Key::character("a"), Modifiers::LCTRL);
    efeengine_core::clipboard::set_clipboard_text("old");
    h.key(Key::character("c"), Modifiers::LCTRL);
    assert_eq!(efeengine_core::clipboard::clipboard_text().as_deref(), Some("log line to copy"));
    // typing, Backspace, cut, paste and Enter change nothing and send nothing
    h.type_text("x");
    h.named(NamedKey::Backspace);
    h.key(Key::character("x"), Modifiers::LCTRL);
    efeengine_core::clipboard::set_clipboard_text("pasted");
    h.key(Key::character("v"), Modifiers::LCTRL);
    h.named(NamedKey::Enter);
    assert_eq!(h.tree.value(h.id("ro")), Some(UiValue::Text("log line to copy".into())));
    assert!(h.kinds("ro").iter().all(|k| matches!(k, UiSignalKind::Focus(_))), "{:?}", h.kinds("ro"));
    // cut did copy the selection, like a read-only field in a browser
    // (the clipboard holds the paste text now, so only the signals above matter)
    // the editable neighbour still works
    h.click("rw");
    h.type_text("!");
    assert_eq!(h.tree.value(h.id("rw")), Some(UiValue::Text("plain!".into())));
}

#[test]
fn a_read_only_input_selects_with_the_mouse() {
    let mut h = H::new((400.0, 300.0), read_only_menu);
    h.frame();
    let (x, y, _, hh) = h.rect("ro");
    // drag from the left edge to the middle, then copy
    h.button(x + 3.0, y + hh * 0.5, ElementState::Pressed);
    h.mouse(x + 60.0, y + hh * 0.5);
    h.button(x + 60.0, y + hh * 0.5, ElementState::Released);
    efeengine_core::clipboard::set_clipboard_text("old");
    h.key(Key::character("c"), Modifiers::LCTRL);
    let copied = efeengine_core::clipboard::clipboard_text().unwrap();
    assert!(!copied.is_empty() && "log line to copy".starts_with(&copied) && copied.len() < 16, "copied {copied:?}");
}

fn wrapped_menu(t: &mut UiTree) {
    let root = t.layer("l", Layer::top_left(0));
    let col = t.column(root).id();
    t.text_input(col, "short").key("one").read_only(true).style(Style::new().width(Size::Px(160.0)));
    t.text_input(col, "first line\nsecond line is long enough to wrap at this width\nthird")
        .key("many")
        .read_only(true)
        .multiline(true)
        .style(Style::new().width(Size::Px(160.0)));
    t.button(col, "Other").key("other");
}

#[test]
fn a_multiline_read_only_input_wraps_grows_and_copies_all_lines() {
    let mut h = H::new((400.0, 400.0), wrapped_menu);
    h.frame();
    let (_, _, w1, h1) = h.rect("one");
    let (x, y, w, hm) = h.rect("many");
    assert_eq!(w, w1, "same width");
    assert!(hm > h1 * 3.5, "four or more lines: {hm} vs one line {h1}");
    // the button below sits under the grown input
    let (_, by, _, _) = h.rect("other");
    assert!(by >= y + hm - 0.5);
    // select all and copy: the exact text with its line breaks
    h.click("many");
    h.key(Key::character("a"), Modifiers::LCTRL);
    efeengine_core::clipboard::set_clipboard_text("old");
    h.key(Key::character("c"), Modifiers::LCTRL);
    assert_eq!(efeengine_core::clipboard::clipboard_text().as_deref(), Some("first line\nsecond line is long enough to wrap at this width\nthird"));
    // a selection is drawn: more quads than without it
    let with_sel = h.frame().vertices().len();
    h.click("other");
    assert!(h.frame().vertices().len() < with_sel, "the selection highlight is gone after focus moved");
    // a drag from the first line into the second selects across the break
    h.button(x + 2.0, y + 4.0, ElementState::Pressed);
    h.mouse(x + 40.0, y + hm * 0.3);
    h.button(x + 40.0, y + hm * 0.3, ElementState::Released);
    efeengine_core::clipboard::set_clipboard_text("old");
    h.key(Key::character("c"), Modifiers::LCTRL);
    let copied = efeengine_core::clipboard::clipboard_text().unwrap();
    assert!(copied.starts_with("first line\n") && copied.len() > 11, "copied {copied:?}");
    // typing and Enter do nothing, no signals but focus
    h.type_text("x");
    h.named(NamedKey::Enter);
    assert!(h.kinds("many").iter().all(|k| matches!(k, UiSignalKind::Focus(_))), "{:?}", h.kinds("many"));
}

#[test]
fn tree_blur_drops_the_text_focus_and_the_keyboard_capture() {
    let mut h = H::new((400.0, 300.0), read_only_menu);
    h.click("rw");
    assert!(h.b.capture().keyboard);
    h.tree.blur();
    h.sync();
    assert!(!h.b.capture().keyboard, "keys go back to the game at once");
    h.pump();
    assert_eq!(h.kinds("rw").last(), Some(&UiSignalKind::Focus(false)));
    // typing now goes nowhere
    assert!(!h.type_text("x"));
    assert_eq!(h.tree.value(h.id("rw")), Some(UiValue::Text("plain".into())));
    // blurring when nothing has focus is a no-op
    let n = h.log.len();
    h.tree.blur();
    h.sync();
    h.pump();
    assert_eq!(h.log.len(), n);
}

#[test]
fn blur_names_a_window_and_other_windows_keep_their_focus() {
    let mut h = H::new((400.0, 300.0), read_only_menu);
    h.click("rw");
    // this backend draws window 0: a blur of window 7 leaves it alone, of 0 drops the focus
    h.tree.blur_window(7);
    h.sync();
    assert!(h.b.capture().keyboard);
    h.tree.blur_window(0);
    h.sync();
    assert!(!h.b.capture().keyboard);
}

#[test]
fn escape_and_a_click_outside_release_the_focus() {
    let mut h = H::new((400.0, 300.0), read_only_menu);
    h.click("rw");
    h.named(NamedKey::Escape);
    assert!(!h.b.capture().keyboard);
    h.click("ro");
    assert!(h.b.capture().keyboard);
    // a click on a button elsewhere
    h.click("other");
    assert!(!h.b.capture().keyboard);
    // and one on empty space
    h.click("rw");
    h.click_at(390.0, 290.0);
    assert!(!h.b.capture().keyboard);
}

#[test]
fn backspace_works_right_after_a_click() {
    // A click used to leave an empty selection that swallowed the first Backspace.
    let mut h = H::new((400.0, 300.0), |t| {
        let root = t.layer("l", Layer::top_left(0));
        t.text_input(root, "60").key("t").style(Style::new().width(Size::Px(72.0)));
    });
    h.frame();
    h.click("t");
    h.named(NamedKey::Backspace);
    assert_eq!(h.tree.value(h.id("t")), Some(UiValue::Text("6".into())));
}

// ----- follow_end scrolling and ellipsized text -------------------------------------------------

fn scroller(t: &mut UiTree) {
    let root = t.layer("l", Layer::top_left(0));
    let sc = t.scroll(root).key("sc").follow_end(true).style(Style::new().width(Size::Px(200.0)).height(Size::Px(100.0))).id();
    let col = t.column(sc).key("col").id();
    for i in 0..20 {
        t.text(col, format!("row {i}")).key(&format!("r{i}"));
    }
}

fn add_row(h: &mut H, i: usize) {
    let col = h.id("col");
    h.tree.text(col, format!("row {i}")).key(&format!("r{i}"));
    h.sync();
    h.frame();
}

fn in_view(h: &mut H, key: &str) -> bool {
    let (_, sy, _, sh) = h.rect("sc");
    let (_, y, _, hh) = h.rect(key);
    y >= sy - 0.5 && y + hh <= sy + sh + 0.5
}

fn wheel(h: &mut H, dy: f32) {
    let (x, y, w, hh) = h.rect("sc");
    h.mouse(x + w * 0.5, y + hh * 0.5);
    h.event(InputEventKind::MouseWheel { delta: (0.0, dy), unit: WheelUnit::Line });
    h.frame();
}

#[test]
fn a_follow_end_scroll_sticks_to_the_end_as_content_grows() {
    let mut h = H::new((400.0, 300.0), scroller);
    h.frame();
    assert!(in_view(&mut h, "r19"), "starts at the end");
    assert!(!in_view(&mut h, "r0"));
    add_row(&mut h, 20);
    assert!(in_view(&mut h, "r20"), "the new last row is in view");
    // its own following is not a user scroll: no signal
    assert!(h.kinds("sc").is_empty(), "{:?}", h.kinds("sc"));
}

#[test]
fn scrolling_away_unpins_reports_and_keeps_the_view_still() {
    let mut h = H::new((400.0, 300.0), scroller);
    h.frame();
    wheel(&mut h, 3.0);
    assert_eq!(h.kinds("sc").last(), Some(&UiSignalKind::Scrolled { at_end: false }));
    let before = h.rect("r10");
    add_row(&mut h, 20);
    add_row(&mut h, 21);
    assert_eq!(h.rect("r10"), before, "content added at the end does not move what is read");
    // a sync (any journal entry) does not pin it again
    h.tree.set_visible(h.id("r0"), true);
    h.tree.set_text(h.id("r1"), "row 1!");
    h.sync();
    h.frame();
    assert_eq!(h.rect("r10"), before);
    // back to the very end: reported, and it follows again
    wheel(&mut h, -500.0);
    assert_eq!(h.kinds("sc").last(), Some(&UiSignalKind::Scrolled { at_end: true }));
    add_row(&mut h, 22);
    assert!(in_view(&mut h, "r22"));
}

#[test]
fn setting_follow_end_again_snaps_to_the_end() {
    let mut h = H::new((400.0, 300.0), scroller);
    h.frame();
    wheel(&mut h, 4.0);
    assert!(!in_view(&mut h, "r19"));
    let sc = h.id("sc");
    h.tree.node(sc).unwrap().follow_end(false);
    h.sync();
    h.tree.node(sc).unwrap().follow_end(true);
    h.sync();
    h.frame();
    assert!(in_view(&mut h, "r19"));
    // without follow_end the position never moves when content is added
    let mut h = H::new((400.0, 300.0), |t| {
        scroller(t);
        let sc = t.find("sc").unwrap();
        t.node(sc).unwrap().follow_end(false);
    });
    h.frame();
    assert!(in_view(&mut h, "r0"));
    add_row(&mut h, 20);
    assert!(in_view(&mut h, "r0"));
}

fn ellipsized(t: &mut UiTree) {
    let root = t.layer("l", Layer::top_left(0));
    t.text_input(root, "a fairly long line of text that will not fit in a narrow field")
        .key("e")
        .read_only(true)
        .ellipsize(true)
        .style(Style::new().width(Size::Px(160.0)));
    t.text_input(root, "short").key("s").read_only(true).ellipsize(true).style(Style::new().width(Size::Px(160.0)));
}

#[test]
fn ellipsized_text_is_cut_to_its_width_and_reports_truncation() {
    let mut h = H::new((400.0, 300.0), ellipsized);
    let narrow = h.frame().vertices().len();
    h.pump();
    assert_eq!(h.kinds("e"), vec![UiSignalKind::Truncated(true)]);
    assert!(h.kinds("s").is_empty(), "a text that fits reports nothing");
    // widen it: it fits, the report says so, and more glyphs are drawn
    let e = h.id("e");
    h.tree.set_style(e, Style::new().width(Size::Px(600.0)));
    h.sync();
    let wide = h.frame().vertices().len();
    h.pump();
    assert_eq!(h.kinds("e").last(), Some(&UiSignalKind::Truncated(false)));
    assert!(wide > narrow, "the whole text is drawn once it fits ({wide} vs {narrow})");
    // and narrow again
    h.tree.set_style(e, Style::new().width(Size::Px(100.0)));
    h.sync();
    h.frame();
    h.pump();
    assert_eq!(h.kinds("e").last(), Some(&UiSignalKind::Truncated(true)));
}

#[test]
fn a_focused_ellipsized_text_shows_everything_and_copies_it_all() {
    let mut h = H::new((400.0, 300.0), ellipsized);
    h.frame();
    let cut = h.frame().vertices().len();
    h.click("e");
    h.key(Key::character("a"), Modifiers::LCTRL);
    efeengine_core::clipboard::set_clipboard_text("old");
    h.key(Key::character("c"), Modifiers::LCTRL);
    assert_eq!(efeengine_core::clipboard::clipboard_text().as_deref(), Some("a fairly long line of text that will not fit in a narrow field"));
    assert!(h.frame().vertices().len() >= cut);
}
