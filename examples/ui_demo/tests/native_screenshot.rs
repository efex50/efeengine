//! Renders the demo menu and HUD with the iced backend into an offscreen texture on the real
//! GPU and saves a PNG. Needs a wgpu adapter, so it is `#[ignore]`d:
//! `cargo test -p ui_demo --test native_screenshot -- --ignored`.
//! The PNG path is `EFE_NATIVE_SCREENSHOT` or `target/native-menu.png`.
#![cfg(feature = "iced")]

use efeengine_ui::UiTest;
use efeengine_ui::native::{render_offscreen, render_offscreen_represented};
use ui_demo::setup;

fn out_path() -> std::path::PathBuf {
    std::env::var_os("EFE_NATIVE_SCREENSHOT").map_or_else(|| "target/native-menu.png".into(), Into::into)
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn native_menu_screenshot() {
    let mut t = UiTest::new(setup);
    t.step(2);
    // The logo texture the menu's Image node shows.
    let logo = t.resource::<ui_demo::Settings>().logo.expect("logo texture");
    let img = t.engine.textures().get(logo).expect("logo image").clone();
    let shot = render_offscreen(&t.ui(), &[(logo, img)], (960, 600), 1.0, [0.08, 0.09, 0.11]).expect("offscreen render");
    let path = out_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, shot.png()).expect("write png");
    eprintln!("wrote {}", path.display());
    // Not everything is background: the panel and the HUD were drawn.
    let bg = shot.pixel(900, 560);
    assert!(shot.rgba.chunks(4).any(|p| p[..3] != bg[..3]));
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn presenting_the_drawn_frame_again_gives_the_same_pixels() {
    let mut t = UiTest::new(setup);
    t.step(2);
    let once = render_offscreen(&t.ui(), &[], (640, 400), 1.0, [0.08, 0.09, 0.11]).expect("render");
    let again = render_offscreen_represented(&t.ui(), &[], (640, 400), 1.0, [0.08, 0.09, 0.11], 3).expect("render");
    assert!(once.rgba == again.rgba, "re-presenting changed the image");
}
