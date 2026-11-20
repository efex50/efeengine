//! Display-only world-space UI: text, rectangles, bars, sprites and containers drawn over the
//! 2D scene.
//!
//! There is no input handling. Each element is anchored to an entity (following its transform)
//! or to a fixed world point, and sized in logical pixels, so it keeps its size when the camera
//! zooms. [`Container2D`] stacks its children; children must not carry [`Anchor2D`], the
//! container's own anchor places the whole stack.
//!
//! Drawing uses the fallback UI backend's text shaping (cosmic-text, bundled Inter faces) and
//! `Draw2D`, so it runs on native and web alike.

pub mod components;
mod draw;
mod extract;

use std::cell::RefCell;
use std::rc::Rc;

use efeengine_core::prelude::*;

pub use components::{Anchor, Anchor2D, Bar2D, Color, Container2D, Floating2D, Rect2D, Sprite2D, Text2D};

/// Adds the world-space UI systems and overlay.
pub struct WorldUiPlugin;

impl Plugin for WorldUiPlugin {
    fn name(&self) -> &str {
        "efeengine-world-ui"
    }

    fn build(&self, engine: &mut Engine) {
        let shared = Rc::new(RefCell::new(extract::Snapshot::default()));
        let s = shared.clone();
        engine.add_system(Stage::PostUpdate, move |cx| {
            let mut snap = s.borrow_mut();
            extract::run(cx, &mut snap);
        });
        engine.add_world_overlay(draw::WorldUiOverlay::new(shared));
    }
}
