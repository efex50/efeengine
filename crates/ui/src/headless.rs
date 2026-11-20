//! The headless backend (API.md section 14): no widgets; records the journal, checks it is
//! consistent and injects signals. The model lives in `efeui_core::headless`; this wraps it
//! in a [`UiBackend`].

pub use efeui_core::headless::*;

use crate::backend::{UiBackend, UiBackendCx};
use crate::capture::UiCapture;
use crate::journal::UiChange;
use crate::signal::UiSignal;
use crate::theme::Theme;
use crate::tree::UiTree;

/// The backend. See [`HeadlessModel`] for the checks it makes per `sync`.
pub struct HeadlessBackend {
    model: HeadlessModel,
}

impl HeadlessBackend {
    /// A backend and the handle to reach it.
    pub fn new() -> (Self, HeadlessHandle) {
        let (model, handle) = HeadlessModel::new();
        (Self { model }, handle)
    }
}

impl UiBackend for HeadlessBackend {
    fn handles_all_windows(&self) -> bool {
        true
    }

    fn init(&mut self, cx: &mut UiBackendCx) {
        self.model.init(cx.logical_size, cx.scale_factor);
    }

    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], _theme: &Theme, _cx: &mut UiBackendCx) {
        self.model.sync(tree, changes);
    }

    fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
        self.model.drain_signals(out);
    }

    fn capture(&self) -> UiCapture {
        self.model.capture()
    }

    fn resize(&mut self, logical: (f32, f32), scale: f32) {
        self.model.resize(logical, scale);
    }
}
