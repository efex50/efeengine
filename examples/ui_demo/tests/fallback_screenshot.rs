//! Renders the demo menu and HUD with the fallback backend (taffy + cosmic-text through
//! `Draw2D`) into offscreen targets and saves PNGs:
//! * SDL software renderer (runs anywhere): `EFE_FALLBACK_SDL=/path/fallback-sdl.png`.
//! * wgpu (needs a GPU adapter, so `#[ignore]`d): `EFE_FALLBACK_WGPU=/path/fallback-wgpu.png`.
//! Default paths are `target/fallback-sdl.png` and `target/fallback-wgpu.png`.
#![cfg(feature = "fallback")]

use efeengine_ui::fallback::offscreen::{Shot, render_sdl, render_wgpu};
use efeengine_ui::UiTest;
use ui_demo::setup;

const SIZE: (u32, u32) = (960, 600);
const BG: [f32; 3] = [0.08, 0.09, 0.11];

fn out(var: &str, default: &str) -> std::path::PathBuf {
    std::env::var_os(var).map_or_else(|| default.into(), Into::into)
}

fn save(path: &std::path::Path, shot: &Shot) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, shot.png()).expect("write png");
    eprintln!("wrote {}", path.display());
}

fn menu() -> (UiTest, Vec<(efeengine_core::prelude::TextureHandle, std::sync::Arc<efeengine_core::render::Image>)>) {
    let mut t = UiTest::new(setup);
    t.step(2);
    let logo = t.resource::<ui_demo::Settings>().logo.expect("logo texture");
    let img = t.engine.textures().get(logo).expect("logo image").clone();
    (t, vec![(logo, img)])
}

/// Not blank: the panel, the title and the HUD put many pixels on screen.
fn assert_content(shot: &Shot) {
    let bg = shot.pixel(shot.width - 5, shot.height - 5);
    let non_bg = shot.rgba.chunks(4).filter(|p| p[..3] != bg[..3]).count();
    assert!(non_bg > 5000, "mostly blank: {non_bg} non-background pixels");
}

#[test]
fn menu_screenshot_sdl() {
    let (t, images) = menu();
    let shot = render_sdl(&t.ui(), &images, SIZE, 1.0, BG).expect("SDL software render");
    assert_content(&shot);
    save(&out("EFE_FALLBACK_SDL", "target/fallback-sdl.png"), &shot);
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn menu_screenshot_wgpu() {
    let (t, images) = menu();
    let shot = render_wgpu(&t.ui(), &images, SIZE, 1.0, BG).expect("offscreen wgpu render");
    assert_content(&shot);
    save(&out("EFE_FALLBACK_WGPU", "target/fallback-wgpu.png"), &shot);
}
