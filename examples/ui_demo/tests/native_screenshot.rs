//! Renders the demo menu and HUD with the iced backend into an offscreen texture on the real
//! GPU and saves a PNG. Needs a wgpu adapter, so it is `#[ignore]`d:
//! `cargo test -p ui_demo --test native_screenshot -- --ignored`.
//! The PNG path is `EFE_NATIVE_SCREENSHOT` or `target/native-menu.png`.
#![cfg(feature = "iced")]

use efeengine_ui::{EngineUiExt, UiTest};
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
    let painters = t.engine.ui_painters();
    let shot = render_offscreen(&t.ui(), &[(logo, img)], (960, 600), 1.0, [0.08, 0.09, 0.11], Some(&painters)).expect("offscreen render");
    let path = out_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, shot.png()).expect("write png");
    eprintln!("wrote {}", path.display());
    // Not everything is background: the panel and the HUD were drawn.
    let bg = shot.pixel(900, 560);
    assert!(shot.rgba.chunks(4).any(|p| p[..3] != bg[..3]));

    // The GPU view's triangle (palette[0], orange): its bounding box lies inside the 160x120
    // view, its center pixel has the shape color, and the view's corners are transparent (the
    // panel shows through, so they are not the shape color).
    let orange = |p: [u8; 4]| p[0] >= 252 && (125..=131).contains(&p[1]) && p[2] <= 3;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..shot.height {
        for x in 0..shot.width {
            if orange(shot.pixel(x, y)) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    assert!(x0 != u32::MAX, "no shape pixels: the GPU view was not painted");
    assert!(x1 - x0 < 160 && y1 - y0 < 120, "shape bigger than its view: {x0},{y0}..{x1},{y1}");
    assert!(x1 - x0 > 40 && y1 - y0 > 40, "shape too small");
    // The view's center: the triangle's centroid is the center of the view. Find it as the mean
    // of the shape pixels.
    let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if orange(shot.pixel(x, y)) {
                (sx, sy, n) = (sx + x as u64, sy + y as u64, n + 1);
            }
        }
    }
    let (mx, my) = ((sx / n) as u32, (sy / n) as u32);
    let c = shot.pixel(mx, my);
    eprintln!("shape center pixel: {c:?}");
    assert!(orange(c), "the view's center is not the palette orange (255,128,0): {c:?}");
    // View corners (from the centroid: +-79, +-59) show the panel, not the shape.
    for (dx, dy) in [(-76i32, -56i32), (76, -56), (-76, 56), (76, 56)] {
        let p = shot.pixel((mx as i32 + dx) as u32, (my as i32 + dy) as u32);
        assert!(!orange(p), "corner {dx},{dy} is the shape color: {p:?}");
    }
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn presenting_the_drawn_frame_again_gives_the_same_pixels() {
    let mut t = UiTest::new(setup);
    t.step(2);
    let once = render_offscreen(&t.ui(), &[], (640, 400), 1.0, [0.08, 0.09, 0.11], None).expect("render");
    let again = render_offscreen_represented(&t.ui(), &[], (640, 400), 1.0, [0.08, 0.09, 0.11], 3, None).expect("render");
    assert!(once.rgba == again.rgba, "re-presenting changed the image");
}
