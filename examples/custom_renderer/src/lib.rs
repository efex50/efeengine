//! custom_renderer: a template for writing your own renderer.
//!
//! This game spawns a camera, three generated textures and a few sprites (one parented to
//! another, one moving with `Velocity`), installs [`renderer::MyRenderer`] with
//! `Engine::set_renderer`, and logs a summary of the `RenderFrame` once a second so you can see
//! the data the renderer receives. `MyRenderer` clears the screen only: drawing is the TODO.
//!
//! Run: `cargo xtask run custom_renderer native` (or `./build.py run custom_renderer native`).

// `#[main]` leaves `main` out of the native lib target, so its callees look unused there.
#![allow(dead_code)]

mod renderer;

use efeengine_core::prelude::*;
use efeengine_core::render::{Image, RenderFrame};

/// A checkerboard of `cells` x `cells` squares, each `cell_px` pixels wide.
fn checkerboard(cells: u32, cell_px: u32, a: [u8; 4], b: [u8; 4]) -> Image {
    let size = cells * cell_px;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / cell_px) + (y / cell_px)) % 2 == 0;
            rgba.extend_from_slice(if on { &a } else { &b });
        }
    }
    Image::new(size, size, rgba).expect("checkerboard size")
}

/// A horizontal red -> blue gradient.
fn gradient(w: u32, h: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _y in 0..h {
        for x in 0..w {
            let t = x as f32 / (w - 1) as f32;
            rgba.extend_from_slice(&[(255.0 * (1.0 - t)) as u8, 64, (255.0 * t) as u8, 255]);
        }
    }
    Image::new(w, h, rgba).expect("gradient size")
}

fn spawn_scene(engine: &mut Engine) {
    let checker = engine.textures_mut().add(checkerboard(8, 8, [240, 240, 240, 255], [40, 40, 60, 255]));
    let grad = engine.textures_mut().add(gradient(64, 32));
    let white = engine.textures_mut().add(Image::solid(4, 4, [255, 200, 60, 255]));

    let world = engine.world_mut();
    world.spawn((Camera { clear_color: [0.08, 0.1, 0.14, 1.0], ..Camera::orthographic_2d() }, Transform::default()));

    // A spinning checkerboard (size 0 = the texture's own 64x64 pixels) ...
    let parent = world.spawn((
        Transform::from_position(Vec2::new(-150.0, 0.0)),
        Velocity { linear: Vec2::ZERO, angular: 1.0 },
        Sprite { layer: 0, ..Sprite::new(checker) },
    ));
    // ... with a small gradient sprite attached to it, orbiting as it spins.
    world.spawn((
        Transform::from_position(Vec2::new(80.0, 0.0)),
        Parent::new(parent),
        Sprite { size: Vec2::new(48.0, 24.0), layer: 1, ..Sprite::new(grad) },
    ));
    // A tinted sprite drifting to the right, drawn above the others.
    world.spawn((
        Transform::from_position(Vec2::new(-60.0, -80.0)),
        Velocity { linear: Vec2::new(40.0, 0.0), angular: 0.0 },
        Sprite { size: Vec2::new(40.0, 40.0), color: [1.0, 0.8, 0.8, 1.0], layer: 2, ..Sprite::new(white) },
    ));
}

/// Once a second, log what the renderer was handed.
fn log_frame_summary(engine: &mut Engine) {
    let mut next = 1.0;
    // Runs after the engine's own render extract (systems of a stage run in insertion order).
    engine.add_system(Stage::RenderExtract, move |ctx| {
        let Some(frame) = ctx.resources.get::<RenderFrame>() else { return };
        // texture changes arrive only once, so log them when they come
        if !frame.textures.is_empty() {
            global_info(&format!(
                "[custom_renderer] frame {}: textures +{} ~{} -{}",
                frame.frame,
                frame.textures.added.len(),
                frame.textures.updated.len(),
                frame.textures.removed.len()
            ));
        }
        if frame.time < next {
            return;
        }
        next += 1.0;
        let cam = match &frame.camera {
            Some(c) => format!("camera at ({:.1}, {:.1}) zoom {} viewport {}x{}", c.position.x, c.position.y, c.zoom, c.viewport.0, c.viewport.1),
            None => "no camera".to_string(),
        };
        let first = frame.sprites.first().map(|s| format!(", first sprite layer {} size {:?}", s.layer, s.size)).unwrap_or_default();
        global_info(&format!(
            "[custom_renderer] t={:.0}s frame {}: {cam}, {} sprites, {} meshes{first}",
            frame.time,
            frame.frame,
            frame.sprites.len(),
            frame.meshes.len()
        ));
    });
}

#[main]
fn main() {
    match Engine::new("custom renderer") {
        Ok(mut engine) => {
            spawn_scene(&mut engine);
            engine.set_renderer(renderer::MyRenderer::new());
            log_frame_summary(&mut engine);
            engine.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
