//! Renders a `UiTree` with the iced backend into an offscreen texture on a real GPU and reads
//! the pixels back. For screenshot comparison with the web backend and for tests that need to
//! see the layout (they are `#[ignore]`d where there is no GPU).

use std::sync::Arc;

use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::Image;
use iced_core::time::Instant;
use iced_core::{Color, Size, Theme, mouse, renderer, shell, window};
use iced_runtime::user_interface::{self, UserInterface};
use iced_wgpu::graphics::{Shell, Viewport};
use iced_wgpu::{Engine, Renderer, wgpu};

use efeengine_core::render::Gpu;

use crate::painter::UiPainters;
use crate::tree::UiTree;

use super::gpuview::ViewPainter;

use super::assets::Assets;
use super::model::Model;
use super::view::view;

/// Pixels read back from an offscreen render (RGBA8, sRGB encoded, rows from the top).
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Screenshot {
    pub fn png(&self) -> Vec<u8> {
        encode_png(self.width, self.height, &self.rgba)
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// Draws the visible layers of `tree` over `background` (a dark game-like colour) at the physical
/// size `size` with the window `scale`. `images` are the textures `Image` nodes use. Needs a
/// wgpu adapter; returns an error string without one. `painters` serve the `GpuView` nodes
/// (the target format is `Rgba8UnormSrgb`; without painters the views stay transparent).
pub fn render_offscreen(
    tree: &UiTree,
    images: &[(TextureHandle, Arc<Image>)],
    size: (u32, u32),
    scale: f32,
    background: [f32; 3],
    painters: Option<&UiPainters>,
) -> Result<Screenshot, String> {
    render(tree, images, size, scale, background, 1, painters)
}

/// Like [`render_offscreen`], but presents the drawn frame `passes` times (clearing the target
/// each time, without rebuilding the interface) and returns the last one: what the backend's
/// idle path does. `passes == 2` must give the same pixels as one.
pub fn render_offscreen_represented(
    tree: &UiTree,
    images: &[(TextureHandle, Arc<Image>)],
    size: (u32, u32),
    scale: f32,
    background: [f32; 3],
    passes: usize,
    painters: Option<&UiPainters>,
) -> Result<Screenshot, String> {
    render(tree, images, size, scale, background, passes.max(1), painters)
}

fn render(
    tree: &UiTree,
    images: &[(TextureHandle, Arc<Image>)],
    size: (u32, u32),
    scale: f32,
    background: [f32; 3],
    passes: usize,
    painters: Option<&UiPainters>,
) -> Result<Screenshot, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or_default(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .map_err(|e| format!("no wgpu adapter: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .map_err(|e| format!("no wgpu device: {e}"))?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;

    let engine = Engine::new(&adapter, device.clone(), queue.clone(), format, None, Shell::headless());
    let mut renderer = Renderer::new(engine, renderer::Settings::default());
    let viewport = Viewport::with_physical_size(Size::new(size.0, size.1), renderer::Scale { window: scale, application: 1.0 });

    let mut assets = Assets::new();
    for f in tree.fonts().iter().enumerate() {
        let _ = assets.load_font(crate::style::FontId(f.0 as u32), &f.1.bytes);
    }
    for (h, img) in images {
        assets.set_image(*h, img);
    }
    let model = Model::build(tree);

    let mut ui = UserInterface::build(view(&model, &assets), viewport.logical_size(), user_interface::Cache::new(), &mut renderer);
    let mut bus = shell::Bus::new();
    let events = [iced_core::Event::Window(window::Event::RedrawRequested(Instant::now()))];
    let _ = ui.update(&window::Headless, &shell::Waker::noop(), &events, mouse::Cursor::Unavailable, &mut renderer, &mut bus);
    let views = assets.views.clone();
    views.begin_draw();
    ui.draw(&mut renderer, &Theme::Dark, &renderer::Style { text_color: Color::WHITE }, mouse::Cursor::Unavailable);
    let gpu = Gpu {
        adapter: &adapter,
        instance: &instance,
        backend: adapter.get_info().backend,
        device: &device,
        queue: &queue,
        surface_format: format,
        size,
        scale_factor: scale,
    };
    ViewPainter::default().paint(&views, painters, &gpu);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("efeengine-ui offscreen"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let bg = Color::from_rgb(background[0], background[1], background[2]);
    for _ in 0..passes {
        let _ = renderer.present(Some(bg), format, &view, &viewport);
    }

    let unpadded = size.0 as usize * 4;
    let padded = unpadded.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("efeengine-ui readback"),
        size: (padded * size.1 as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded as u32), rows_per_image: Some(size.1) },
        },
        wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
    );
    let _ = queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("poll: {e:?}"))?;
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("map: {e:?}"))?;
    let data = slice.get_mapped_range().map_err(|e| format!("map range: {e:?}"))?;
    let mut rgba = Vec::with_capacity(unpadded * size.1 as usize);
    for row in data.chunks(padded).take(size.1 as usize) {
        rgba.extend_from_slice(&row[..unpadded]);
    }
    drop(data);
    buffer.unmap();
    Ok(Screenshot { width: size.0, height: size.1, rgba })
}

/// A PNG encoder without compression (stored deflate blocks); enough for test output.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}
