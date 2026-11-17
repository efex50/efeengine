//! What iced needs besides the model: decoded images (`image::Handle`) and font families.
//!
//! Fonts go straight into iced's global font system (`iced_graphics::text::font_system`), the
//! same store `iced::font::load` writes to, because the backend has no iced runtime to run the
//! `font::load` task. The two bundled Inter files are loaded once, the first time any
//! [`Assets`] is created.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::Image;
use iced_core::image::Handle;
use iced_wgpu::graphics::text::font_system;

use crate::style::FontId;

/// The family name of the bundled default font (Inter, SIL OFL 1.1, see `THIRD_PARTY_NOTICES.md`).
pub const DEFAULT_FAMILY: &str = "Inter";

static INTER_REGULAR: &[u8] = include_bytes!("../../assets/Inter-Regular.ttf");
static INTER_BOLD: &[u8] = include_bytes!("../../assets/Inter-Bold.ttf");

#[derive(Default)]
pub struct Assets {
    pub images: HashMap<TextureHandle, Handle>,
    /// `FontId` -> family name read from the font file (leaked once per font; fonts live as
    /// long as the process anyway).
    pub fonts: HashMap<FontId, &'static str>,
}

impl Assets {
    pub fn new() -> Self {
        static DEFAULTS: OnceLock<()> = OnceLock::new();
        DEFAULTS.get_or_init(|| {
            let mut fs = font_system().write().expect("font system");
            fs.load_font(Cow::Borrowed(INTER_REGULAR));
            fs.load_font(Cow::Borrowed(INTER_BOLD));
        });
        Self::default()
    }

    /// Registers the font `bytes` under `id`. Returns the family name, `None` when the bytes
    /// are not a font (nothing new was found in the font database).
    pub fn load_font(&mut self, id: FontId, bytes: &Arc<[u8]>) -> Option<&'static str> {
        let mut fs = font_system().write().expect("font system");
        let before: Vec<_> = fs.raw().db().faces().map(|f| f.id).collect();
        fs.load_font(Cow::Owned(bytes.to_vec()));
        let family = fs
            .raw()
            .db()
            .faces()
            .find(|f| !before.contains(&f.id))
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()))?;
        let family: &'static str = Box::leak(family.into_boxed_str());
        let _ = self.fonts.insert(id, family);
        Some(family)
    }

    /// A new handle for `texture` (a replaced texture gets a new handle id, so iced uploads it
    /// again).
    pub fn set_image(&mut self, texture: TextureHandle, img: &Image) {
        let (w, h) = img.size();
        let _ = self.images.insert(texture, Handle::from_rgba(w, h, img.rgba8().to_vec()));
    }
}
