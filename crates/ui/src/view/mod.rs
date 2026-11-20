//! The Elm-style view layer (API.md section 16), built on the retained tree.
//!
//! The element descriptions and the reconciler are `efeui_core::view`; the runtime that runs
//! the views every frame (`UiView`) is here.
//!
//! ```ignore
//! use efeengine_ui::view::*;
//! use efeengine_ui::{column, Layer};
//!
//! fn menu_view(s: &MenuState) -> El<MenuMsg> {
//!     column![
//!         text("Coin Dash").class("title"),
//!         button("Play").key("play").on_click(MenuMsg::Play),
//!         slider(0.0..=1.0, s.volume).key("volume").on_change(MenuMsg::Volume),
//!     ]
//! }
//! engine.add_plugin(UiPlugin::new())?;
//! engine.add_plugin(UiView::new("menu", menu_view).layer(Layer::centered(10)))?;
//! ```
//!
//! # Key scheme
//! A keyed element becomes a tree key `<view>/<k1>/<k2>/...`: the keys of its keyed ancestors
//! inside the view (outermost first) and its own, joined with `/`, under the view name. Unkeyed
//! elements add nothing, so `column![button("Play").key("play")]` in the view `menu` is found
//! with `ui.find("menu/play")`, however deep it sits. Two keyed elements must not produce the
//! same full key (the view panics, before anything is applied); in practice: keys are unique
//! among the keyed elements that share the same nearest keyed ancestor. A list row is
//! `row.key("item-7")` and its button `.key("buy")` gives `menu/item-7/buy`.
//!
//! # Per-frame order
//! - `PreUpdate`, `order::INPUT + 10`: the UI plugin makes `Events<UiSignal>` readable.
//! - `PreUpdate`, `order::INPUT + 11`: every view turns the signals of its widgets into
//!   `Msg` events (`Ctx::send_batch_now`), readable in the same frame by `Update` systems.
//!   A delivered message also marks the view to run this frame even under `.when(..)`, so an
//!   edit that `update` rejected is written back.
//! - `PostUpdate`, `order::LATE + 10`: the plugin's bindings. Bindings are for retained nodes;
//!   a view owns its layer, so nothing is bound inside it.
//! - `PostUpdate`, `order::LATE + 11`: the views (after bindings, deterministic), then the
//!   plugin's sync at `LATE + 20` hands the journal to the backend.

mod plugin;

pub use efeui_core::view::{
    El, IntoChildren, button, button_with, checkbox, column, container, el, gpu_view, image, reconcile, row, scroll, selectable_text, slider, slot, text,
    text_input,
};
pub use plugin::{RunCondition, UiView, UiViews, ViewCx, changed};
