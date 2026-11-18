//! The fallback backend's frames rendered with SDL's software renderer (runs anywhere) and,
//! `#[ignore]`d, with wgpu on a real GPU; pixels are checked and the two modes compared.
#![cfg(feature = "fallback")]

use std::sync::Arc;

use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::Image;
use efeengine_ui::fallback::offscreen::{Shot, render_sdl, render_wgpu};
use efeengine_ui::*;

const BG: [f32; 3] = [0.0, 0.0, 0.0];

fn scene() -> (UiTree, Vec<(TextureHandle, Arc<Image>)>) {
    let img = TextureHandle { id: 3 };
    let mut t = UiTree::new();
    let root = t.layer("l", Layer::top_left(0));
    let row = t.container(root).style(Style::new().direction(Direction::Row).gap(10.0).padding(10.0)).id();
    t.container(row)
        .style(Style::new().width(Size::Px(40.0)).height(Size::Px(40.0)).background(Color::rgb(1.0, 0.0, 0.0)).border(Border::new(2.0, Color::rgb(0.0, 1.0, 0.0), 10.0)));
    t.image(row, img, ImageFit::Fill).style(Style::new().width(Size::Px(40.0)).height(Size::Px(40.0)));
    t.text(row, "Hello").style(Style::new().text_color(Color::WHITE).font_size(20.0));
    (t, vec![(img, Arc::new(Image::solid(8, 8, [0, 0, 255, 255])))])
}

fn px(s: &Shot, x: u32, y: u32) -> [u8; 3] {
    let p = s.pixel(x, y);
    [p[0], p[1], p[2]]
}

fn close(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    a.iter().zip(&b).all(|(x, y)| (*x as i32 - *y as i32).abs() <= tol)
}

fn check(s: &Shot, tol: i32) {
    // box at (10,10) 40x40: green border ring 2px, red inside, rounded corner shows the background
    assert!(close(px(s, 30, 30), [255, 0, 0], tol), "fill {:?}", px(s, 30, 30));
    assert!(close(px(s, 30, 11), [0, 255, 0], tol), "border {:?}", px(s, 30, 11));
    assert!(close(px(s, 10, 10), [0, 0, 0], tol), "rounded corner is background: {:?}", px(s, 10, 10));
    // the image at (60, 10): blue
    assert!(close(px(s, 80, 30), [0, 0, 255], tol), "image {:?}", px(s, 80, 30));
    // the text starts at x = 110: some near-white pixels in its row
    let white = (110..190).any(|x| (10..40).any(|y| px(s, x, y).iter().all(|c| *c > 230)));
    assert!(white, "no text pixels");
    // nothing right of the text
    assert_eq!(px(s, 300, 30), [0, 0, 0]);
}

#[test]
fn sdl_software_pixels() {
    let (t, images) = scene();
    let s = render_sdl(&t, &images, (320, 80), 1.0, BG).expect("render");
    check(&s, 2);
}

#[test]
fn sdl_at_2x_is_sharp_and_twice_as_big() {
    let (t, images) = scene();
    let s = render_sdl(&t, &images, (640, 160), 2.0, BG).expect("render");
    // the red box now spans 20..100
    assert!(close(px(&s, 60, 60), [255, 0, 0], 2));
    assert!(close(px(&s, 60, 22), [0, 255, 0], 2), "border {:?}", px(&s, 60, 22));
    assert!(close(px(&s, 160, 60), [0, 0, 255], 2));
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn wgpu_pixels_and_parity_with_sdl() {
    let (t, images) = scene();
    let a = render_wgpu(&t, &images, (320, 80), 1.0, BG).expect("wgpu render");
    check(&a, 3);
    let b = render_sdl(&t, &images, (320, 80), 1.0, BG).expect("sdl render");
    // Edges (feathered meshes, glyphs) blend in linear space under wgpu and in encoded space
    // under SDL, so compare the share of pixels that differ a lot, and the mean.
    let n = a.rgba.len() / 4;
    let mut big = 0usize;
    let mut sum = 0u64;
    for i in 0..n {
        let d = (0..3).map(|c| (a.rgba[i * 4 + c] as i32 - b.rgba[i * 4 + c] as i32).unsigned_abs()).max().unwrap();
        sum += d as u64;
        big += (d > 64) as usize;
    }
    eprintln!("wgpu vs sdl: {big} of {n} pixels differ by more than 64, mean {:.3}", sum as f64 / n as f64);
    assert!(big * 100 < n, "more than 1% of the pixels differ strongly");
    assert!((sum as f64 / n as f64) < 3.0);
}
