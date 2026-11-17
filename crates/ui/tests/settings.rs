//! A settings menu driven through `UiTest`: the Phase 1 exit test for the retained API.

use efeengine_core::prelude::*;
use efeengine_ui::*;

#[derive(Default, Debug, PartialEq)]
struct Settings {
    volume: f32,
    fullscreen: bool,
    name: String,
    submitted: Option<String>,
    started: bool,
    changes: u32,
}

#[derive(Default)]
struct Score {
    value: u32,
}

fn setup_menu(engine: &mut Engine) {
    engine.insert_resource(Settings { volume: 0.8, ..Default::default() });
    engine.insert_resource(Score::default());
    {
        let ui = engine.resources_mut().get_mut::<UiTree>().unwrap();
        let hud = ui.layer("hud", Layer::top_left(0));
        ui.text(hud, Bind::res(|s: &Score| format!("Score: {}", s.value))).key("hud/score");
        let root = ui.layer("menu", Layer::centered(10));
        let col = ui.column(root).class("panel").gap(12.0).padding(24.0).id();
        ui.text(col, "Coin Dash").class("title");
        ui.button(col, "Play").key("menu/play");
        ui.slider(col, 0.0..=1.0, 0.8).step(0.05).key("menu/volume");
        ui.checkbox(col, "Fullscreen", false).key("menu/fullscreen");
        ui.text_input(col, "").placeholder("Player name").max_len(8).key("menu/name");
    }
    engine.add_system(Stage::Update, |cx| {
        for s in cx.ui_signals() {
            let st = cx.resources.get_mut::<Settings>().unwrap();
            match s.key_kind() {
                ("menu/play", UiSignalKind::Click) => {
                    st.started = true;
                    cx.resources.get_mut::<Score>().unwrap().value += 10;
                }
                ("menu/volume", UiSignalKind::Changed(UiValue::F32(v))) => st.volume = *v,
                ("menu/fullscreen", UiSignalKind::Changed(UiValue::Bool(b))) => st.fullscreen = *b,
                ("menu/name", UiSignalKind::Changed(UiValue::Text(n))) => st.name = n.clone(),
                ("menu/name", UiSignalKind::Submit(n)) => st.submitted = Some(n.clone()),
                _ => continue,
            }
            cx.resources.get_mut::<Settings>().unwrap().changes += 1;
        }
    });
}

#[test]
fn settings_menu() {
    let mut t = UiTest::new(setup_menu);
    t.step(1);
    assert!(!t.resource::<Settings>().started);
    assert_eq!(t.text("hud/score"), "Score: 0");
    assert_eq!(t.text("menu/play"), "Play");

    t.click("menu/play");
    t.set_value("menu/volume", 0.3);
    t.set_checked("menu/fullscreen", true);
    t.type_text("menu/name", "Efe");
    t.type_text("menu/name", "!!");
    t.submit("menu/name");
    t.step(1);

    {
        let s = t.resource::<Settings>();
        assert!(s.started);
        assert_eq!(s.volume, 0.3);
        assert!(s.fullscreen);
        assert_eq!(s.name, "Efe!!");
        assert_eq!(s.submitted.as_deref(), Some("Efe!!"));
        assert_eq!(s.changes, 6);
    }
    // The tree holds the values (write-back) and the bound score follows the resource
    // in the same frame (bindings run after the game systems).
    assert_eq!(t.value("menu/volume"), Some(UiValue::F32(0.3)));
    assert_eq!(t.value("menu/fullscreen"), Some(UiValue::Bool(true)));
    assert_eq!(t.text("menu/name"), "Efe!!");
    assert_eq!(t.text("hud/score"), "Score: 10");

    // Nothing else happens on idle frames.
    t.backend().clear();
    t.step(5);
    assert!(t.changes().is_empty());
    assert_eq!(t.resource::<Settings>().changes, 6);
}

#[test]
fn modal_pause_layer_hides_and_silences() {
    let mut t = UiTest::new(setup_menu);
    t.step(1);
    let menu = t.id("menu");
    t.ui_mut().set_visible(menu, false);
    t.click("menu/play");
    t.step(1);
    assert!(!t.resource::<Settings>().started);
    t.ui_mut().set_visible(menu, true);
    t.click("menu/play");
    t.step(1);
    assert!(t.resource::<Settings>().started);
}
