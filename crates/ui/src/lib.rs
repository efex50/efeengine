//! efeengine-ui: the common UI API (see `docs/ui/API.md`).
//!
//! A retained [`UiTree`] with a change journal, styles and themes, signals, bindings, the
//! [`UiBackend`] trait and a headless backend with the [`UiTest`] helper. Backends (web, iced,
//! fallback) and the Elm-style view layer build on this API.

pub mod backend;
pub mod bindings;
pub mod capture;
pub use efeui_core::css;
pub use efeui_core::dom_plan;
#[cfg(all(feature = "fallback", not(target_arch = "wasm32")))]
pub mod fallback;
pub mod headless;
pub use efeui_core::id;
pub use efeui_core::journal;
#[cfg(all(feature = "iced", not(target_arch = "wasm32")))]
pub mod native;
pub use efeui_core::node;
pub mod painter;
pub mod plugin;
pub mod scripting;
pub mod signal;
pub use efeui_core::style;
pub mod testing;
pub use efeui_core::theme;
pub use efeui_core::tree;
pub mod view;
#[cfg(target_arch = "wasm32")]
pub mod web;

pub use backend::{Filter, ImageIdExt, TextInputArea, TextureExt, UiBackend, UiBackendCx};
pub use bindings::{Bind, BindCx, BindCxExt, BindExt, BindTarget, BindUpdate, BoundValue, TextSrc, UiBindings, engine_bind_cx};
pub use capture::UiCapture;
pub use headless::{HeadlessBackend, HeadlessHandle};
pub use id::{Owner, WidgetId};
pub use journal::UiChange;
pub use node::{Anchor, ClassId, ImageFit, ImageId, KindTag, Layer, Node, NodeKind};
pub use painter::{EngineUiExt, UiPainter, UiPainters, ViewTarget};
pub use plugin::UiPlugin;
pub use scripting::{UiScripts, Violation};
pub use signal::{PointerEvent, UiCtxExt, UiSignal, UiSignalKind, UiValue, filter_signals};
pub use style::{Align, Border, Color, Direction, Edges, FontId, Justify, Size, Style, TextAlign};
pub use testing::UiTest;
pub use theme::{StateStyles, Theme, WidgetState};
#[cfg(target_arch = "wasm32")]
pub use web::WebBackend;
pub use efeui_core::{column, row};
pub use view::{El, RunCondition, UiView, UiViews, ViewCx};
pub use tree::{FontData, NodeRef, StyleMut, TreeError, UiTree};
