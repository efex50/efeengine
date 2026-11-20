//! The engine-free part of the Elm-style view layer (API.md section 16): [`El`] element
//! descriptions and the reconciler that applies them to a [`UiTree`](crate::UiTree). The
//! runtime that calls views every frame (`UiView`) lives in the adapter crate.

pub mod el;
pub mod reconcile;

pub use el::{
    El, IntoChildren, button, button_with, checkbox, column, container, gpu_view, image, row, scroll, selectable_text, slider, slot, text, text_input,
};
