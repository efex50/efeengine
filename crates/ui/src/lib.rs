//! efeengine-ui: the common UI API (see `docs/ui/API.md`).
//!
//! A retained [`UiTree`] with a change journal, styles and themes, signals, bindings, the
//! [`UiBackend`] trait and a headless backend with the [`UiTest`] helper. Backends (web, iced,
//! fallback) and the Elm-style view layer build on this API.

pub mod backend;
pub mod bindings;
pub mod capture;
pub mod headless;
pub mod id;
pub mod journal;
pub mod node;
pub mod plugin;
pub mod signal;
pub mod style;
pub mod testing;
pub mod theme;
pub mod tree;
pub mod view;

pub use backend::{Filter, UiBackend, UiBackendCx};
pub use bindings::{Bind, BindCx, BindTarget, BindUpdate, BoundValue, TextSrc, UiBindings};
pub use capture::UiCapture;
pub use headless::{HeadlessBackend, HeadlessHandle};
pub use id::{Owner, WidgetId};
pub use journal::UiChange;
pub use node::{Anchor, ClassId, ImageFit, KindTag, Layer, Node, NodeKind};
pub use plugin::UiPlugin;
pub use signal::{UiCtxExt, UiSignal, UiSignalKind, UiValue, filter_signals};
pub use style::{Align, Border, Color, Direction, Edges, FontId, Justify, Size, Style, TextAlign};
pub use testing::UiTest;
pub use theme::{StateStyles, Theme, WidgetState};
pub use view::{El, RunCondition, UiView, UiViews, ViewCx};
pub use tree::{FontData, NodeRef, StyleMut, TreeError, UiTree};
