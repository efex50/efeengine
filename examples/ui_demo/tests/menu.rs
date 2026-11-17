//! The menu and HUD through `UiTest`: clicks, slider and text input, then the state and the HUD.

use efeengine_ui::UiTest;
use ui_demo::{Settings, setup};

#[test]
fn play_volume_and_name_update_settings_and_hud() {
    let mut t = UiTest::new(setup);
    t.step(1);
    assert_eq!(t.resource::<Settings>().plays, 0);
    assert_eq!(t.text("hud/text"), "Plays: 0  Volume: 80%");

    t.click("menu/play");
    t.set_value("menu/volume", 0.35);
    t.type_text("menu/name", "Ada");
    t.submit("menu/name");
    t.step(1);

    {
        let s = t.resource::<Settings>();
        assert_eq!(s.plays, 1);
        assert_eq!(s.volume, 0.35);
        assert_eq!(s.name, "Ada");
    }
    assert_eq!(t.text("hud/text"), "Plays: 1  Volume: 35%");
    assert_eq!(t.text("menu/plays"), "Plays: 1");
    assert_eq!(t.text("menu/volume_text"), "Volume: 35%");
}
