//! Renders coin_dash's first frames with the built-in `SpriteRenderer` into an offscreen texture
//! on the real GPU and saves a PNG, so frames of the wgpu and (later, 4C.4) SDL 2D backends can
//! be compared. Needs a wgpu adapter, so it is `#[ignore]`d:
//! `EFE_SCREENSHOT=/path/coin-sprites.png cargo test -p coin_dash --test sprite_screenshot -- --ignored`
//! (default path `target/coin-sprites.png`).

use coin_dash::setup;
use efeengine_core::prelude::*;
use efeengine_core::render::{write_png, DebugDraw2D, OffscreenGpu, TextureChanges};

const DT: f64 = 1.0 / 60.0;
const SIZE: (u32, u32) = (960, 600);

fn out_path() -> std::path::PathBuf {
    std::env::var_os("EFE_SCREENSHOT").map_or_else(|| "target/coin-sprites.png".into(), Into::into)
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn coin_dash_frame_through_sprite_renderer() {
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
    shoot(&mut e, false);
    for _ in 0..30 {
        shoot(&mut e, false);
    }
    let frame = shoot(&mut e, true);
    assert!(frame.d2.camera.is_some() && !frame.d2.draw.is_empty());

    let mut off = OffscreenGpu::new(SIZE).expect("a GPU adapter");
    let mut r = efeengine_core::render::SpriteRenderer::new();
    off.render_2d(&mut r, &frame.d2, &changes);
    let a = off.read_rgba8();
    off.render_2d(&mut r, &frame.d2, &TextureChanges::default());
    assert_eq!(a, off.read_rgba8(), "rendering the same frame twice gives the same pixels");
    let bg = &a[..4];
    assert!(a.chunks(4).filter(|p| *p != bg).count() > 1000, "mostly blank");
    // the yellow debug box is in the frame
    assert!(a.chunks(4).any(|p| p[0] > 240 && p[1] > 200 && p[2] < 80), "debug box missing");

    let path = out_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    write_png(&path, SIZE.0, SIZE.1, &a).expect("write png");
    eprintln!("wrote {}", path.display());
}
