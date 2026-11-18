//! Renders coin_dash's first frames with the built-in 2D renderers into offscreen targets and
//! saves PNGs:
//! * wgpu (`SpriteRenderer`, needs a GPU adapter, so `#[ignore]`d):
//!   `EFE_SCREENSHOT=/path/coin-sprites.png cargo test -p coin_dash --test sprite_screenshot -- --ignored`
//!   (default path `target/coin-sprites.png`).
//! * SDL (`SdlSpriteRenderer` on a software renderer, runs anywhere): `EFE_SCREENSHOT_SDL=path`.
//! * Parity (`#[ignore]`d, needs a GPU): both of the above on the same frame, compared pixel by
//!   pixel; `EFE_SCREENSHOT_DIR=dir` writes `coin-wgpu.png`, `coin-sdl.png` and `coin-diff.png`.

use coin_dash::setup;
use efeengine_core::prelude::*;
use efeengine_core::render::{write_png, DebugDraw2D, Frame2D, OffscreenGpu, SdlDraw2D, SdlSpriteRenderer, TextureChanges};

const DT: f64 = 1.0 / 60.0;
const SIZE: (u32, u32) = (960, 600);

fn out_path() -> std::path::PathBuf {
    std::env::var_os("EFE_SCREENSHOT").map_or_else(|| "target/coin-sprites.png".into(), Into::into)
}

/// Run coin_dash headless for 32 frames; the frame 32 (with a debug box) and every texture change so far.
fn capture() -> (Frame2D, TextureChanges) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: DT, size: SIZE, ..EngineConfig::headless() }).expect("headless engine");
    setup(&mut e);
    // the texture changes are delivered once, with the frame that follows their creation: collect all
    let mut changes = TextureChanges::default();
    let mut shoot = |e: &mut Engine, bbox: bool| {
        if bbox {
            // 4A.17: a debug bounding box over the first sprite of the last layer
            let s = e.render_frame().d2.sprites.last().cloned().expect("a sprite");
            let (c, h) = (Vec2::new(s.world[3][0], s.world[3][1]), Vec2::new(s.size[0], s.size[1]) * 0.5);
            e.resources_mut().get_mut::<DebugDraw2D>().unwrap().set_width(2.0).rect(c - h, c + h, DebugDraw2D::YELLOW);
        }
        e.run_frames(1, DT);
        let f = e.render_frame().clone();
        changes.added.extend(f.textures.added.iter().cloned());
        changes.updated.extend(f.textures.updated.iter().cloned());
        f
    };
    for _ in 0..31 {
        shoot(&mut e, false);
    }
    let frame = shoot(&mut e, true);
    assert!(frame.d2.camera.is_some() && !frame.d2.draw.is_empty());
    (frame.d2, changes)
}

fn wgpu_pixels(frame: &Frame2D, changes: &TextureChanges) -> Vec<u8> {
    let mut off = OffscreenGpu::new(SIZE).expect("a GPU adapter");
    let mut r = efeengine_core::render::SpriteRenderer::new();
    off.render_2d(&mut r, frame, changes);
    let a = off.read_rgba8();
    off.render_2d(&mut r, frame, &TextureChanges::default());
    assert_eq!(a, off.read_rgba8(), "rendering the same frame twice gives the same pixels");
    a
}

fn sdl_pixels(frame: &Frame2D, changes: &TextureChanges) -> Vec<u8> {
    let mut d = SdlDraw2D::offscreen(SIZE).expect("SDL software renderer");
    d.sync(changes);
    let r = SdlSpriteRenderer::new();
    r.render(&mut d, frame);
    let a = d.read_rgba8().expect("pixels");
    r.render(&mut d, frame);
    assert_eq!(a, d.read_rgba8().unwrap(), "rendering the same frame twice gives the same pixels");
    a
}

fn save(path: &std::path::Path, px: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    write_png(path, SIZE.0, SIZE.1, px).expect("write png");
    eprintln!("wrote {}", path.display());
}

fn assert_content(a: &[u8]) {
    let bg = &a[..4];
    assert!(a.chunks(4).filter(|p| *p != bg).count() > 1000, "mostly blank");
    // the yellow debug box is in the frame
    assert!(a.chunks(4).any(|p| p[0] > 240 && p[1] > 200 && p[2] < 80), "debug box missing");
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn coin_dash_frame_through_sprite_renderer() {
    let (frame, changes) = capture();
    let a = wgpu_pixels(&frame, &changes);
    assert_content(&a);
    save(&out_path(), &a);
}

#[test]
fn coin_dash_frame_through_sdl_renderer() {
    let (frame, changes) = capture();
    let a = sdl_pixels(&frame, &changes);
    assert_content(&a);
    if let Some(p) = std::env::var_os("EFE_SCREENSHOT_SDL") {
        save(std::path::Path::new(&p), &a);
    }
}

/// The same frame through both backends. Opaque interiors must match closely; edges (filtering,
/// blending in linear vs encoded space) may differ, so the test allows a small share of differing
/// pixels and a small mean error (measured: 0.107% of pixels differ, all around the animated
/// sprites at fractional positions where nearest sampling rounds differently; mean 0.12/255).
#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn coin_dash_wgpu_and_sdl_match() {
    let (frame, changes) = capture();
    let a = wgpu_pixels(&frame, &changes);
    let b = sdl_pixels(&frame, &changes);
    let n = a.len() / 4;
    let (mut off_by_more_than_4, mut off_by_more_than_16, mut sum) = (0usize, 0usize, 0u64);
    let mut diff = vec![255u8; a.len()];
    for i in 0..n {
        let d = (0..3).map(|c| (a[i * 4 + c] as i32 - b[i * 4 + c] as i32).unsigned_abs()).max().unwrap();
        sum += d as u64;
        off_by_more_than_4 += (d > 4) as usize;
        off_by_more_than_16 += (d > 16) as usize;
        let v = (d * 8).min(255) as u8;
        diff[i * 4..i * 4 + 3].copy_from_slice(&[v, v, v]);
    }
    let non_bg = a.chunks(4).filter(|p| *p != &a[..4]).count();
    eprintln!(
        "parity: {n} px, {non_bg} non-background; max-channel diff > 4: {off_by_more_than_4} ({:.3}%), > 16: {off_by_more_than_16} ({:.3}%), mean {:.4}",
        off_by_more_than_4 as f64 * 100.0 / n as f64,
        off_by_more_than_16 as f64 * 100.0 / n as f64,
        sum as f64 / n as f64
    );
    if let Some(dir) = std::env::var_os("EFE_SCREENSHOT_DIR") {
        let dir = std::path::PathBuf::from(dir);
        save(&dir.join("coin-wgpu.png"), &a);
        save(&dir.join("coin-sdl.png"), &b);
        save(&dir.join("coin-diff.png"), &diff);
    }
    assert!(off_by_more_than_16 * 1000 < n * 3, "more than 0.3% of the pixels differ by more than 16/255");
    assert!((sum as f64 / n as f64) < 0.5, "mean error too large");
}
