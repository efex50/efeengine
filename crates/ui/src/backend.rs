//! The backend trait (API.md section 13).

use std::sync::Arc;

use efeengine_core::input::InputEvent;
use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::Image;

use crate::capture::UiCapture;
use crate::journal::UiChange;
use crate::signal::UiSignal;
use crate::theme::Theme;
use crate::tree::UiTree;

pub use efeengine_core::input::Filter;
use efeengine_core::render::Gpu;

/// Where the OS text-input (IME) area should be, in logical pixels; `cursor` is the caret offset
/// in characters inside the area. Sent by the plugin as `set_text_input_area`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextInputArea {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub cursor: i32,
}

impl UiBackendCx {
    /// For the main window.
    pub fn new(logical_size: (f32, f32), scale_factor: f32) -> Self {
        Self { window: 0, logical_size, scale_factor, images: Vec::new() }
    }

    /// For window `window` (0 = main).
    pub fn for_window(window: u32, logical_size: (f32, f32), scale_factor: f32) -> Self {
        Self { window, logical_size, scale_factor, images: Vec::new() }
    }
}

/// Resources a backend gets while it runs (GPU handles join it with the overlay pass).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct UiBackendCx {
    /// The window this backend instance draws (0 = main). The plugin makes one instance per
    /// window that has layers; an instance shows the layers whose `Layer::window` is this
    /// ([`UiTree::layers_in`]) and receives only that window's input events. It is told the
    /// whole journal (`sync`) and skips what is not its own.
    pub window: u32,
    /// Window size in logical pixels.
    pub logical_size: (f32, f32),
    pub scale_factor: f32,
    /// The engine images (RGBA8) of the textures that `Image` nodes touched by this `sync`
    /// (created, or `Kind` changed) use. Filled by the plugin right before `sync`, empty
    /// otherwise. When a texture an `Image` node shows is replaced in `Textures`, the plugin
    /// adds the new image here and a synthesized `UiChange::Kind(node)` for every node that
    /// shows it to the `changes` of that `sync` (the journal of the tree is untouched), so a
    /// backend refreshes replaced textures the same way it handles a new image.
    pub images: Vec<(TextureHandle, Arc<Image>)>,
}

/// Turns the tree into real widgets and reports what the user did. Backends are main-thread
/// only and need not be `Send` (web-sys elements, iced renderer); the UI plugin keeps the boxed
/// backend inside main-thread engine systems.
///
pub trait UiBackend: 'static {
    /// Once, when the plugin is built (the main window's instance) or when the window's first
    /// layer appears (the others). `cx.window` is the window.
    fn init(&mut self, cx: &mut UiBackendCx);

    /// `true` for a backend that handles every window in one instance (headless: it records the
    /// whole tree). The plugin then makes no further instances for other windows.
    fn handles_all_windows(&self) -> bool {
        false
    }

    /// A raw input event of this backend's window (`ev.window`), before the game sees it (native
    /// backends; web: no-op). Called from the engine's input filter, so it also sees window events;
    /// it must not call back into the engine. The backend keeps its own copy of what it needs from the tree.
    fn input(&mut self, _ev: &InputEvent) -> Filter {
        Filter::Pass
    }

    /// Applies the journal of this frame (never called with an empty one). The backend reads
    /// the new state from `tree`.
    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], theme: &Theme, cx: &mut UiBackendCx);

    /// Moves the signals produced since the last call into `out`. Key and owner are filled in
    /// by the plugin; `widget` and `kind` are what counts.
    fn drain_signals(&mut self, out: &mut Vec<UiSignal>);

    /// Whether the UI used this frame's pointer and keyboard input.
    fn capture(&self) -> UiCapture;

    /// Window size in logical pixels and scale factor changed.
    fn resize(&mut self, _logical: (f32, f32), _scale: f32) {}

    /// Once, right after `init`: the registry of GPU-view painters (API.md section 17).
    /// Backends that draw `GpuView` nodes keep the handle (it is a cheap clone; painters added
    /// to the engine later show up in it) and look painters up by `NodeKind::GpuView::painter`
    /// at render time. Backends that never paint them (headless) ignore it.
    fn set_painters(&mut self, _painters: &crate::painter::UiPainters) {}

    /// Native: the GPU is ready for this backend's window (`gpu.window`; the engine's render
    /// overlay `init`, or the first `render` when the instance is created after the window).
    fn gpu_init(&mut self, _gpu: &Gpu) {}

    /// `true` for a backend that draws through [`Draw2D`](efeengine_core::render::Draw2D) (the
    /// fallback): the plugin then registers a Draw2D overlay that calls [`draw2d`](Self::draw2d)
    /// (works in the wgpu and in the SDL mode) instead of relying on `render`.
    fn draws_with_draw2d(&self) -> bool {
        false
    }

    /// Native, Draw2D backends: draw this window's UI for this frame.
    fn draw2d(&mut self, _cx: &mut efeengine_core::render::Overlay2D) {}

    /// Native: surface size or scale factor changed (overlay `resize`).
    fn gpu_resize(&mut self, _gpu: &Gpu) {}

    /// Native: draw the UI on top of the game into `target` (`LoadOp::Load`).
    fn render(&mut self, _gpu: &Gpu, _target: &efeengine_core::wgpu::TextureView) {}

    /// Native: the text-input request of the UI since the last call. `Some(Some(area))` means
    /// "a text field has focus here", `Some(None)` "no text field has focus", `None` no change.
    /// The plugin turns it into `SetTextInput` / `SetTextInputArea` commands.
    fn take_text_input(&mut self) -> Option<Option<TextInputArea>> {
        None
    }

    /// Native: the mouse cursor icon the UI wants since the last call (`None` = no change).
    /// The plugin turns it into `set_cursor_icon`; it only changes when the UI's interaction
    /// changes (over a button, a text input, ...).
    fn take_cursor_icon(&mut self) -> Option<efeengine_core::window::CursorIcon> {
        None
    }
}
