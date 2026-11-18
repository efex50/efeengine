//! Offscreen rendering (native only): a GPU context without a window and a render target
//! texture, for tests and screenshots.
//!
//! [`OffscreenGpu::new`] creates its own wgpu device (no surface), [`OffscreenGpu::gpu`] hands out
//! the [`Gpu`] a renderer expects (surface format `Rgba8UnormSrgb`, like a typical window
//! surface), [`OffscreenGpu::render_2d`] runs any [`Renderer2D`] on a [`Frame2D`] and
//! [`OffscreenGpu::read_rgba8`] / [`write_png`] get the pixels out. `SpriteRenderer` frames
//! rendered this way are what the SDL backend (4C.4) is compared against.

use super::textures::TextureChanges;
use super::{Frame2D, Gpu, Renderer2D, Target};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A window-less GPU with one `size` render target.
pub struct OffscreenGpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    inited: bool,
}

impl OffscreenGpu {
    /// Create the device and an `Rgba8UnormSrgb` target of `size` pixels. Errors when no adapter
    /// is available (a machine without a GPU).
    pub fn new(size: (u32, u32)) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no graphics adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("offscreen device"),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("could not create device: {e}"))?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self { instance, adapter, device, queue, texture, view, size, inited: false })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The borrowed handles a renderer gets (scale factor 1).
    pub fn gpu(&self) -> Gpu<'_> {
        Gpu {
            adapter: &self.adapter,
            instance: &self.instance,
            backend: self.adapter.get_info().backend,
            device: &self.device,
            queue: &self.queue,
            surface_format: FORMAT,
            size: self.size,
            scale_factor: 1.0,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Run `renderer` on `frame` into the target as the first (clearing) pass; calls `init` on
    /// the first use.
    pub fn render_2d(&mut self, renderer: &mut dyn Renderer2D, frame: &Frame2D, textures: &TextureChanges) {
        if !self.inited {
            renderer.init(&self.gpu());
            self.inited = true;
        }
        renderer.render(&self.gpu(), &Target { view: &self.view, clear: true }, frame, textures);
    }

    /// The target's pixels as RGBA8 (sRGB-encoded bytes), rows from the top, no padding.
    pub fn read_rgba8(&self) -> Vec<u8> {
        let (w, h) = self.size;
        let row = (w * 4) as usize;
        let padded = row.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen readback"),
            size: (padded * h as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded as u32), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(std::iter::once(encoder.finish()));
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            out.extend_from_slice(&data[y * padded..y * padded + row]);
        }
        out
    }
}

/// Encode RGBA8 pixels as a PNG (uncompressed deflate blocks: big, but dependency-free).
pub fn encode_png(width: u32, height: u32, rgba8: &[u8]) -> Vec<u8> {
    assert_eq!(rgba8.len(), (width * height * 4) as usize, "pixel buffer size");
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for row in rgba8.chunks(width as usize * 4) {
        raw.push(0); // filter: none
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

/// Write RGBA8 pixels to a PNG file.
pub fn write_png(path: impl AsRef<std::path::Path>, width: u32, height: u32, rgba8: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, encode_png(width, height, rgba8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::render::{Draw2D, DrawList, Rect, Textures, Image, SpriteRenderer, SpriteInstance, CameraView, Blend, RectI};
    use crate::engine::components::TextureHandle;
    use hecs::Entity;

    #[test]
    fn png_encoder_round_trips() {
        let px: Vec<u8> = (0..4 * 5 * 3).map(|i| (i * 7) as u8).collect();
        let png = encode_png(5, 3, &px);
        let img = Image::from_png(&png).expect("decodes");
        assert_eq!((img.size(), img.rgba8()), ((5, 3), &px[..]));
    }

    fn px(p: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [p[i], p[i + 1], p[i + 2], p[i + 3]]
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
        a.iter().zip(&b).all(|(x, y)| (*x as i32 - *y as i32).abs() <= tol)
    }

    fn camera(size: (u32, u32)) -> CameraView {
        // orthographic, 1 pixel per world unit, centred, y up
        let (hw, hh) = (size.0 as f32 / 2.0, size.1 as f32 / 2.0);
        let m = [[1.0 / hw, 0.0, 0.0, 0.0], [0.0, 1.0 / hh, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        CameraView {
            entity: Entity::DANGLING, position: glam::Vec2::ZERO, rotation: 0.0, zoom: 1.0, kind: 0, target: 0,
            near: -1.0, far: 1.0, fov_y: 1.0, clear_color: [0.0, 0.0, 0.0, 1.0], viewport: size,
            view: m, projection: m, view_projection: m,
        }
    }

    /// Needs a GPU: `cargo test -p efeengine-core -- --ignored draw2d_pixels`.
    #[test]
    #[ignore = "needs a GPU"]
    fn draw2d_pixels() {
        let size = (64, 64);
        let mut off = OffscreenGpu::new(size).expect("a GPU adapter");
        // a 2x2 texture: red, green / blue, white (nearest-sampled, so quadrants stay crisp)
        let mut store = Textures::new();
        let tex = store.add(Image::new(2, 2, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]).unwrap());
        let changes = store.drain_changes();

        let mut frame = Frame2D { camera: Some(camera(size)), ..Default::default() };
        let d: &mut DrawList = &mut frame.draw;
        d.fill_rect(Rect::new(2.0, 2.0, 10.0, 10.0), [1.0, 0.0, 0.0, 1.0]); // opaque red
        d.fill_rect(Rect::new(14.0, 2.0, 10.0, 10.0), [0.0, 0.0, 1.0, 0.5]); // 50% blue over black
        d.quad(Some(tex), Rect::new(26.0, 2.0, 10.0, 10.0), [0.0, 0.0, 1.0, 1.0], [1.0; 4]); // texture quadrants
        d.set_clip(Some(RectI::new(0, 20, 64, 5))); // clip: only rows 20..25 survive
        d.fill_rect(Rect::new(2.0, 14.0, 20.0, 20.0), [1.0, 1.0, 0.0, 1.0]);
        d.set_clip(None);
        d.set_blend(Blend::Add);
        d.fill_rect(Rect::new(30.0, 30.0, 10.0, 10.0), [0.2, 0.2, 0.2, 1.0]);
        d.fill_rect(Rect::new(30.0, 30.0, 10.0, 10.0), [0.2, 0.2, 0.2, 1.0]);
        d.set_blend(Blend::Alpha);
        d.line([0.0, 50.0], [64.0, 50.0], 2.0, [0.0, 1.0, 0.0, 1.0]);
        // a triangle with per-vertex colours: the centroid is the average
        let v = |x, y, c| crate::engine::render::Vertex2D::new([x, y], [0.0, 0.0], c);
        d.triangles(None, &[v(44.0, 44.0, [1.0, 0.0, 0.0, 1.0]), v(62.0, 44.0, [0.0, 1.0, 0.0, 1.0]), v(53.0, 62.0, [0.0, 0.0, 1.0, 1.0])], &[0, 1, 2]);
        // a sprite under it all (drawn before the list): a white 8x8 square at the world origin
        frame.sprites.push(SpriteInstance {
            entity: Entity::DANGLING,
            world: glam::Mat4::from_translation(glam::vec3(-24.0, -24.0, 0.0)).to_cols_array_2d(),
            size: [8.0, 8.0], uv_min: [0.0; 2], uv_max: [1.0; 2], color: [1.0; 4],
            texture: TextureHandle::NONE, layer: 0, flip: 0,
        });

        let mut r = SpriteRenderer::new();
        off.render_2d(&mut r, &frame, &changes);
        let p = off.read_rgba8();
        let w = size.0;
        assert!(near(px(&p, w, 7, 7), [255, 0, 0, 255], 1), "red rect {:?}", px(&p, w, 7, 7));
        // 50% sRGB blue over black: blending happens in linear space on the sRGB target
        let b = px(&p, w, 19, 7);
        assert!(b[0] == 0 && b[1] == 0 && b[2] > 100 && b[2] < 200, "half blue {b:?}");
        assert!(near(px(&p, w, 28, 4), [255, 0, 0, 255], 1), "texture top-left");
        assert!(near(px(&p, w, 34, 4), [0, 255, 0, 255], 1), "texture top-right");
        assert!(near(px(&p, w, 28, 10), [0, 0, 255, 255], 1), "texture bottom-left");
        assert!(near(px(&p, w, 34, 10), [255, 255, 255, 255], 1), "texture bottom-right");
        assert!(near(px(&p, w, 10, 22), [255, 255, 0, 255], 1), "inside the clip");
        assert!(near(px(&p, w, 10, 16), [0, 0, 0, 255], 0), "above the clip stays clear");
        assert!(near(px(&p, w, 10, 30), [0, 0, 0, 255], 0), "below the clip stays clear");
        assert!(px(&p, w, 35, 35)[0] > 0 && px(&p, w, 35, 35)[0] < 255, "additive adds twice: {:?}", px(&p, w, 35, 35));
        assert!(near(px(&p, w, 20, 49), [0, 255, 0, 255], 1) && near(px(&p, w, 20, 50), [0, 255, 0, 255], 1), "line");
        assert!(near(px(&p, w, 20, 51), [0, 0, 0, 255], 0) && near(px(&p, w, 20, 48), [0, 0, 0, 255], 0), "line is 2 px thick");
        let t = px(&p, w, 53, 50);
        assert!(t[0] > 40 && t[1] > 40 && t[2] > 40 && t[3] == 255, "gradient triangle centre {t:?}");
        // the sprite is at the world origin shifted (-24,-24): pixels around (32-24, 32+24)=(8, 56)
        assert!(near(px(&p, w, 8, 56), [255, 255, 255, 255], 1), "sprite pass still draws");

        if let Ok(dir) = std::env::var("EFE_SCREENSHOT_DIR") {
            write_png(format!("{dir}/draw2d.png"), w, size.1, &p).expect("write png");
        }
    }
}
