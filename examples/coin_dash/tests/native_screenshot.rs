//! Renders the coin_dash main menu (and the settings page) with the iced backend into an
//! offscreen texture on the real GPU and saves PNGs. Needs a wgpu adapter, so it is `#[ignore]`d:
//! `EFE_NATIVE_SCREENSHOT=/path/coin-menu.png cargo test -p coin_dash --test native_screenshot -- --ignored`.
//! The path is `EFE_NATIVE_SCREENSHOT` or `target/coin-menu.png`; the settings page goes next to
//! it with `-settings` added to the name.
#![cfg(feature = "iced")]

use coin_dash::{menu, setup};
use efeengine_ui::UiTest;
use efeengine_ui::native::render_offscreen;

fn out_path() -> std::path::PathBuf {
    std::env::var_os("EFE_NATIVE_SCREENSHOT").map_or_else(|| "target/coin-menu.png".into(), Into::into)
}

fn shoot(t: &UiTest, path: &std::path::Path) -> efeengine_ui::native::Screenshot {
    let shot = render_offscreen(&t.ui(), &[], (960, 600), 1.0, [0.02, 0.03, 0.05], None).expect("offscreen render");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, shot.png()).expect("write png");
    eprintln!("wrote {}", path.display());
    shot
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn main_menu_and_settings_screenshots() {
    let mut t = UiTest::new(|e| {
        setup(e);
        menu::setup(e, false);
    });
    t.step(2);
    let path = out_path();
    let shot = shoot(&t, &path);
    // the backdrop (60% black over the clear colour) and the panel were drawn: more than one colour
    let bg = shot.pixel(10, 10);
    assert!(shot.rgba.chunks(4).any(|p| p[..3] != bg[..3]), "nothing but the background");
    // the panel is lighter than the backdrop around the middle of the window
    let centre = shot.pixel(480, 300);
    assert!(centre[..3].iter().map(|&c| c as u32).sum::<u32>() > bg[..3].iter().map(|&c| c as u32).sum::<u32>(), "{centre:?} vs {bg:?}");
    t.click("main_menu/settings");
    t.step(2);
    let name = path.file_stem().unwrap().to_string_lossy().to_string();
    shoot(&t, &path.with_file_name(format!("{name}-settings.png")));
}
