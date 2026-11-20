//! Signals: what the user did (API.md section 7). The types are `efeui_core::signal`; this
//! adds the engine's `cx.ui_signals()`.

use efeengine_core::prelude::{Ctx, ParCtx};

pub use efeui_core::signal::*;

/// `cx.ui_signals()` for systems: the [`UiSignal`]s of this frame (the same as
/// `cx.events::<UiSignal>().iter()`). The iterator borrows `cx`; `collect()` it first if the
/// loop body needs `cx` mutably.
pub trait UiCtxExt {
    fn ui_signals(&self) -> std::slice::Iter<'_, UiSignal>;
}

impl UiCtxExt for Ctx<'_> {
    fn ui_signals(&self) -> std::slice::Iter<'_, UiSignal> {
        self.events::<UiSignal>().iter()
    }
}

impl UiCtxExt for ParCtx<'_> {
    fn ui_signals(&self) -> std::slice::Iter<'_, UiSignal> {
        self.events::<UiSignal>().iter()
    }
}
