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

/// Whether a backend used a raw input event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    /// The game sees the event.
    Pass,
    /// The UI used it.
    Captured,
}

impl UiBackendCx {
    pub fn new(logical_size: (f32, f32), scale_factor: f32) -> Self {
        Self { logical_size, scale_factor, images: Vec::new() }
    }
}

/// Resources a backend gets while it runs (GPU handles join it with the overlay pass).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct UiBackendCx {
    /// Window size in logical pixels.
    pub logical_size: (f32, f32),
    pub scale_factor: f32,
    /// The engine images (RGBA8) of the textures that `Image` nodes touched by this `sync`
    /// (created, or `Kind` changed) use. Filled by the plugin right before `sync`, empty
    /// otherwise. A texture that is replaced in `Textures` later does not refresh the widget.
    pub images: Vec<(TextureHandle, Arc<Image>)>,
}

/// Turns the tree into real widgets and reports what the user did. Backends are main-thread
/// only and need not be `Send` (web-sys elements, iced renderer); the UI plugin keeps the boxed
/// backend inside main-thread engine systems.
///
/// `render(gpu, target)` (native backends) comes with the core overlay render pass; until then
/// there is no GPU type in this trait.
pub trait UiBackend: 'static {
    /// Once, when the plugin is built.
    fn init(&mut self, cx: &mut UiBackendCx);

    /// A raw input event, before the game sees it (native backends; web: no-op).
    fn input(&mut self, _ev: &InputEvent, _tree: &UiTree) -> Filter {
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
}
