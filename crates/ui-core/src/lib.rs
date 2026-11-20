//! efeui-core: the engine-agnostic UI model (see `docs/ui/API.md`).
//!
//! A retained [`UiTree`] with a change journal, styles and themes, signals, bindings, the
//! Elm-style view layer ([`El`], [`view::reconcile`]) and a headless consistency checker. It has no
//! dependency on any engine, renderer or windowing crate; adapters such as `efeengine-ui`
//! turn the journal into real widgets and feed signals back.

pub mod bindings;
pub mod capture;
pub mod css;
pub mod dom_plan;
pub mod headless;
pub mod id;
pub mod journal;
pub mod node;
pub mod signal;
pub mod style;
pub mod theme;
pub mod tree;
pub mod view;

pub use bindings::{Bind, BindCx, BindTarget, BindUpdate, BoundValue, TextSrc, UiBindings};
pub use capture::UiCapture;
pub use headless::{HeadlessHandle, HeadlessModel};
pub use id::{Owner, WidgetId};
pub use journal::UiChange;
pub use node::{Anchor, ClassId, ImageFit, ImageId, KindTag, Layer, Node, NodeKind};
pub use signal::{PointerEvent, UiSignal, UiSignalKind, UiValue, filter_signals};
pub use style::{Align, Border, Color, Direction, Edges, FontId, Justify, Size, Style, TextAlign};
pub use theme::{StateStyles, Theme, WidgetState};
pub use tree::{FontData, NodeRef, StyleMut, TreeError, UiTree};
pub use view::El;
