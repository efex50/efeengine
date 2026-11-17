//! Extra drawing surfaces on the engine's shared device (UI GPU views on the web, windows
//! later). See [`ExtraSurface`].
//!
//! Two backings:
//! - **Direct**: a real `wgpu::Surface` (WebGPU canvas now, a native window later), configured
//!   on the shared device, acquired and presented normally.
//! - **Copy** (web, WebGL2 only): the device lives in the main canvas's GL context, which no
//!   other canvas can share. The frame is an internal `Rgba8UnormSrgb` texture; `present` copies
//!   it to a mapped buffer, and once the map completes the pixels are un-premultiplied and
//!   put on the canvas's 2D context with `putImageData`. One frame of latency, one CPU pass.

use wgpu::{CompositeAlphaMode, TextureFormat};

use super::Gpu;

/// A target to draw one frame into; hand it back to [`ExtraSurface::present`].
pub struct SurfaceFrame {
    /// Draw here. Origin top-left, exactly `size` big.
    pub view: wgpu::TextureView,
    pub format: TextureFormat,
    /// Physical px.
    pub size: (u32, u32),
    surface_texture: Option<wgpu::SurfaceTexture>,
}

/// A drawing surface on the engine's device, owned by the caller (main thread). Contents are
/// premultiplied alpha: transparent pixels show what is behind the canvas / window.
pub struct ExtraSurface {
    format: TextureFormat,
    size: (u32, u32),
    backing: Backing,
    #[cfg(target_arch = "wasm32")]
    canvas: Option<web_sys::HtmlCanvasElement>,
}

enum Backing {
    Direct { surface: wgpu::Surface<'static>, config: wgpu::SurfaceConfiguration },
    #[cfg(target_arch = "wasm32")]
    Copy(CopyPath),
}

/// Choose the surface format (the *storage* format): the main surface's format when the surface
/// supports it, else the first non-sRGB (canvases take bgra8unorm / rgba8unorm), else the first.
/// Frames are always handed out through the sRGB variant of it, see [`srgb_variant`].
pub(crate) fn pick_format(supported: &[TextureFormat], preferred: TextureFormat) -> Option<TextureFormat> {
    if supported.contains(&preferred) {
        return Some(preferred);
    }
    supported.iter().copied().find(|f| !f.is_srgb()).or_else(|| supported.first().copied())
}

/// The sRGB variant of a format (itself when it is sRGB already or has none).
pub(crate) fn srgb_variant(f: TextureFormat) -> TextureFormat {
    f.add_srgb_suffix()
}

/// Premultiplied if supported, else Inherit (page compositing decides), else the first mode.
/// Without premultiplied support transparent pixels may composite wrongly (often opaque).
pub(crate) fn pick_alpha(modes: &[CompositeAlphaMode]) -> CompositeAlphaMode {
    [CompositeAlphaMode::PreMultiplied, CompositeAlphaMode::Inherit]
        .into_iter()
        .find(|m| modes.contains(m))
        .or_else(|| modes.first().copied())
        .unwrap_or(CompositeAlphaMode::Auto)
}

impl ExtraSurface {
    /// Wrap a surface created from `gpu.instance`, configured for the shared device. Not wasm
    /// specific: a native `for_window` will use this.
    #[allow(dead_code)]
    pub(crate) fn from_surface(gpu: &Gpu, surface: wgpu::Surface<'static>) -> Result<Self, String> {
        let caps = surface.get_capabilities(gpu.adapter);
        let format = pick_format(&caps.formats, gpu.surface_format)
            .ok_or_else(|| "the surface supports no texture format".to_string())?;
        let srgb = srgb_variant(format);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 1,
            height: 1,
            present_mode: caps.present_modes.first().copied().unwrap_or(wgpu::PresentMode::Fifo),
            alpha_mode: pick_alpha(&caps.alpha_modes),
            view_formats: if srgb != format { vec![srgb] } else { vec![] },
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        Ok(Self {
            format: srgb,
            size: (0, 0),
            backing: Backing::Direct { surface, config },
            #[cfg(target_arch = "wasm32")]
            canvas: None,
        })
    }

    /// A surface drawing into `canvas` on the engine's device. With WebGPU the canvas gets a
    /// real surface; with WebGL2 it gets a 2D context and frames are copied (see the module
    /// doc). The canvas must not have another context yet.
    #[cfg(target_arch = "wasm32")]
    pub fn for_canvas(gpu: &Gpu, canvas: web_sys::HtmlCanvasElement) -> Result<Self, String> {
        use std::sync::atomic::{AtomicBool, Ordering};
        static LOGGED: AtomicBool = AtomicBool::new(false);
        let copy = gpu.backend == wgpu::Backend::Gl;
        if !LOGGED.swap(true, Ordering::Relaxed) {
            crate::global_info(&if copy {
                "extra surfaces: WebGL2 copy path (offscreen texture -> readback -> putImageData, 1 frame latency)".to_string()
            } else {
                "extra surfaces: direct canvas surfaces on the shared device".to_string()
            });
        }
        if copy {
            let copy = CopyPath::new(&canvas)?;
            return Ok(Self {
                format: CopyPath::FORMAT,
                size: (0, 0),
                backing: Backing::Copy(copy),
                canvas: Some(canvas),
            });
        }
        let surface = gpu.instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|e| format!("could not create canvas surface: {e}"))?;
        let mut s = Self::from_surface(gpu, surface)?;
        s.canvas = Some(canvas);
        Ok(s)
    }

    /// Fixed for the run.
    pub fn format(&self) -> TextureFormat {
        self.format
    }

    /// Physical px; also sets the canvas backing size. 0 in any axis = nothing is drawn.
    pub fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) {
        self.size = size;
        #[cfg(target_arch = "wasm32")]
        if let Some(c) = &self.canvas {
            if c.width() != size.0 {
                c.set_width(size.0);
            }
            if c.height() != size.1 {
                c.set_height(size.1);
            }
        }
        let zero = size.0 == 0 || size.1 == 0;
        match &mut self.backing {
            Backing::Direct { surface, config } => {
                config.width = size.0;
                config.height = size.1;
                if !zero {
                    surface.configure(&gpu.device, config);
                }
            }
            #[cfg(target_arch = "wasm32")]
            Backing::Copy(c) => c.resize(gpu, size),
        }
    }

    /// A target to draw this frame's contents into, or `None` (zero size, surface lost:
    /// reconfigured, try next frame).
    pub fn acquire(&mut self, gpu: &Gpu) -> Option<SurfaceFrame> {
        if self.size.0 == 0 || self.size.1 == 0 {
            return None;
        }
        let size = self.size;
        let format = self.format;
        match &mut self.backing {
            Backing::Direct { surface, config } => {
                use wgpu::CurrentSurfaceTexture as Cst;
                let (t, suboptimal) = match surface.get_current_texture() {
                    Cst::Success(t) => (t, false),
                    Cst::Suboptimal(t) => (t, true),
                    Cst::Outdated | Cst::Lost => {
                        surface.configure(&gpu.device, config);
                        return None;
                    }
                    Cst::Timeout | Cst::Occluded | Cst::Validation => return None,
                };
                if suboptimal {
                    surface.configure(&gpu.device, config);
                }
                let view = t.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(format), ..Default::default() });
                Some(SurfaceFrame { view, format, size, surface_texture: Some(t) })
            }
            #[cfg(target_arch = "wasm32")]
            Backing::Copy(c) => c.acquire(gpu).map(|view| SurfaceFrame { view, format, size, surface_texture: None }),
        }
    }

    /// Show the frame. Call after submitting the commands that drew into `frame.view`.
    pub fn present(&mut self, gpu: &Gpu, frame: SurfaceFrame) {
        match &mut self.backing {
            Backing::Direct { .. } => {
                if let Some(t) = frame.surface_texture {
                    gpu.queue.present(t);
                }
            }
            #[cfg(target_arch = "wasm32")]
            Backing::Copy(c) => c.present(gpu),
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = gpu;
    }
}

/// Bytes per row padded to wgpu's copy alignment.
pub(crate) fn padded_row(width: u32) -> u32 {
    let a = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    (width * 4).div_ceil(a) * a
}

fn srgb_to_linear(b: u8) -> f32 {
    let c = b as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0 + 0.5) as u8
}

/// Copy `rows` of `width` px out of a padded buffer into `out` (tightly packed), converting
/// RGBA with colour premultiplied in linear space and stored sRGB-encoded (an `Rgba8UnormSrgb`
/// target) to straight sRGB RGBA (what `ImageData` expects): decode, divide by alpha, encode.
pub(crate) fn unpremultiply_rows(src: &[u8], width: u32, height: u32, out: &mut [u8]) {
    let stride = padded_row(width) as usize;
    let row = width as usize * 4;
    for y in 0..height as usize {
        let s = &src[y * stride..y * stride + row];
        let d = &mut out[y * row..(y + 1) * row];
        for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
            let a = sp[3] as u32;
            if a == 255 {
                dp.copy_from_slice(sp);
            } else if a == 0 {
                dp.copy_from_slice(&[0, 0, 0, 0]);
            } else {
                let af = a as f32 / 255.0;
                for i in 0..3 {
                    dp[i] = linear_to_srgb(srgb_to_linear(sp[i]) / af);
                }
                dp[3] = a as u8;
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
use copy_path::CopyPath;

#[cfg(target_arch = "wasm32")]
mod copy_path {
    use std::cell::Cell;
    use std::rc::Rc;

    use wasm_bindgen::JsCast;

    use super::{padded_row, unpremultiply_rows, Gpu};

    /// WebGL2 backing: render into `texture`, read it back, `putImageData` onto the canvas.
    pub(super) struct CopyPath {
        ctx: web_sys::CanvasRenderingContext2d,
        size: (u32, u32),
        texture: Option<wgpu::Texture>,
        view: Option<wgpu::TextureView>,
        buffer: Option<wgpu::Buffer>,
        /// Set by the map callback of the in-flight readback.
        mapped: Rc<Cell<bool>>,
        in_flight: bool,
        /// A frame was presented while a readback was in flight: read the (latest) texture
        /// again at the next `acquire`.
        stale: bool,
        scratch: Vec<u8>,
    }

    impl CopyPath {
        pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

        pub(super) fn new(canvas: &web_sys::HtmlCanvasElement) -> Result<Self, String> {
            let ctx = canvas.get_context("2d")
                .map_err(|e| format!("canvas 2d context failed: {e:?}"))?
                .ok_or_else(|| "canvas already has a different context type".to_string())?
                .dyn_into::<web_sys::CanvasRenderingContext2d>()
                .map_err(|_| "canvas context is not 2d".to_string())?;
            Ok(Self {
                ctx,
                size: (0, 0),
                texture: None,
                view: None,
                buffer: None,
                mapped: Rc::new(Cell::new(false)),
                in_flight: false,
                stale: false,
                scratch: Vec::new(),
            })
        }

        pub(super) fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) {
            if size == self.size && self.texture.is_some() {
                return;
            }
            self.size = size;
            // An in-flight map belongs to the old buffer; drop it (its callback only sets a
            // flag on the old Rc, which is replaced).
            self.mapped = Rc::new(Cell::new(false));
            self.in_flight = false;
            self.stale = false;
            self.texture = None;
            self.view = None;
            self.buffer = None;
            if size.0 == 0 || size.1 == 0 {
                return;
            }
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("extra surface (copy path)"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: Self::FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
            self.texture = Some(texture);
            self.buffer = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("extra surface readback"),
                size: padded_row(size.0) as u64 * size.1 as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }

        pub(super) fn acquire(&mut self, gpu: &Gpu) -> Option<wgpu::TextureView> {
            self.finish(gpu);
            if self.stale && !self.in_flight {
                self.stale = false;
                self.start_readback(gpu);
            }
            self.view.clone()
        }

        pub(super) fn present(&mut self, gpu: &Gpu) {
            self.finish(gpu);
            if self.in_flight {
                // The previous readback has not completed: skip this one, redo it at the next
                // acquire (the texture keeps the latest contents).
                self.stale = true;
            } else {
                self.start_readback(gpu);
            }
        }

        /// Write a completed readback to the canvas.
        fn finish(&mut self, gpu: &Gpu) {
            if !self.in_flight {
                return;
            }
            let _ = gpu.device.poll(wgpu::PollType::Poll);
            if !self.mapped.get() {
                return;
            }
            self.in_flight = false;
            self.mapped.set(false);
            let Some(buffer) = &self.buffer else { return };
            let (w, h) = self.size;
            let n = w as usize * h as usize * 4;
            self.scratch.resize(n, 0);
            {
                match buffer.slice(..).get_mapped_range() {
                    Ok(view) => unpremultiply_rows(&view, w, h, &mut self.scratch),
                    Err(e) => log_once(&format!("extra surface map range failed: {e:?}")),
                }
            }
            buffer.unmap();
            // Copy into a JS-owned array: wasm memory may be a SharedArrayBuffer, which
            // ImageData rejects.
            let arr = js_sys::Uint8ClampedArray::new_with_length(n as u32);
            arr.copy_from(&self.scratch);
            match web_sys::ImageData::new_with_js_u8_clamped_array_and_sh(&arr, w, h) {
                Ok(img) => {
                    if let Err(e) = self.ctx.put_image_data(&img, 0.0, 0.0) {
                        log_once(&format!("extra surface putImageData failed: {e:?}"));
                    }
                }
                Err(e) => log_once(&format!("extra surface ImageData failed: {e:?}")),
            }
        }

        fn start_readback(&mut self, gpu: &Gpu) {
            let (Some(texture), Some(buffer)) = (&self.texture, &self.buffer) else { return };
            let (w, h) = self.size;
            let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("extra surface copy") });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row(w)),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            gpu.queue.submit(std::iter::once(enc.finish()));
            let flag = self.mapped.clone();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                if r.is_ok() {
                    flag.set(true);
                }
            });
            self.in_flight = true;
        }
    }

    fn log_once(msg: &str) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::Relaxed) {
            crate::global_info(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_padding_is_256_aligned() {
        assert_eq!(padded_row(1), 256);
        assert_eq!(padded_row(64), 256);
        assert_eq!(padded_row(65), 512);
    }

    #[test]
    fn unpremultiply_strips_padding_and_divides_alpha() {
        let w = 2u32;
        let mut src = vec![0u8; 256 * 2];
        // row 0: half-alpha red (premultiplied 128,0,0,128), opaque green
        src[..8].copy_from_slice(&[128, 0, 0, 128, 0, 255, 0, 255]);
        // row 1: transparent, quarter alpha
        src[256..264].copy_from_slice(&[0, 0, 0, 0, 16, 32, 64, 64]);
        let mut out = vec![0u8; 16];
        unpremultiply_rows(&src, w, 2, &mut out);
        // Opaque and transparent pixels are unchanged / zeroed.
        assert_eq!(&out[4..8], &[0, 255, 0, 255]);
        assert_eq!(&out[8..12], &[0, 0, 0, 0]);
        // Partial alpha: decode, divide by alpha, encode (in sRGB space, not a plain divide).
        let expect = |c: u8, a: u8| linear_to_srgb(srgb_to_linear(c) / (a as f32 / 255.0));
        assert_eq!(out[..4], [expect(128, 128), 0, 0, 128]);
        assert_eq!(out[12..16], [expect(16, 64), expect(32, 64), expect(64, 64), 64]);
        assert!(out[0] > 128 && out[0] < 255);
    }

    #[test]
    fn srgb_roundtrip_is_exact() {
        for b in 0..=255u8 {
            assert_eq!(linear_to_srgb(srgb_to_linear(b)), b);
        }
    }

    #[test]
    fn format_and_alpha_choice() {
        use TextureFormat::*;
        assert_eq!(pick_format(&[Bgra8Unorm, Rgba8Unorm], Rgba8Unorm), Some(Rgba8Unorm));
        assert_eq!(pick_format(&[Bgra8UnormSrgb, Bgra8Unorm], Rgba8UnormSrgb), Some(Bgra8Unorm));
        assert_eq!(pick_format(&[], Rgba8Unorm), None);
        use CompositeAlphaMode::*;
        assert_eq!(pick_alpha(&[Opaque, PreMultiplied]), PreMultiplied);
        assert_eq!(pick_alpha(&[Opaque, Inherit]), Inherit);
        assert_eq!(pick_alpha(&[Opaque]), Opaque);
        assert_eq!(srgb_variant(Bgra8Unorm), Bgra8UnormSrgb);
        assert_eq!(srgb_variant(Rgba8Unorm), Rgba8UnormSrgb);
    }
}
