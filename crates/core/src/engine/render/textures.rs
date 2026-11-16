//! The texture store: CPU-side images with handles, and the queue of changes the render
//! extract hands to the renderer (each change exactly once).
//!
//! The store holds pixels only; creating GPU textures is the renderer's job (it learns what
//! to create, replace or drop from `RenderFrame::textures`).

use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::components::TextureHandle;

/// Why an [`Image`] or a store operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextureError {
    #[error("an image must be at least 1x1 (got {0}x{1})")]
    Empty(u32, u32),
    #[error("a {width}x{height} RGBA8 image needs {expected} bytes, got {got}")]
    WrongLength { width: u32, height: u32, expected: usize, got: usize },
    #[error("no texture with id {0}")]
    NoSuchTexture(u32),
}

/// A decoded image: `width * height` pixels, 4 bytes each (R, G, B, A, straight alpha, sRGB
/// encoded), row by row from the top-left. Immutable once built (the fields are private so
/// the length invariant holds); the store shares it as `Arc<Image>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

impl Image {
    /// Checks `rgba8.len() == width * height * 4` and a non-zero size.
    pub fn new(width: u32, height: u32, rgba8: Vec<u8>) -> Result<Self, TextureError> {
        if width == 0 || height == 0 {
            return Err(TextureError::Empty(width, height));
        }
        let expected = width as usize * height as usize * 4;
        if rgba8.len() != expected {
            return Err(TextureError::WrongLength { width, height, expected, got: rgba8.len() });
        }
        Ok(Self { width, height, rgba8 })
    }

    /// An image filled with one colour. Panics if `width` or `height` is 0.
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let rgba8 = rgba.iter().copied().cycle().take(width as usize * height as usize * 4).collect();
        Self::new(width, height, rgba8).expect("solid image")
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The pixels, 4 bytes each, rows from the top.
    pub fn rgba8(&self) -> &[u8] {
        &self.rgba8
    }
}

/// What changed in the store since the last frame. Handed over once, in `RenderFrame`.
#[derive(Clone, Debug, Default)]
pub struct TextureChanges {
    /// New textures: create the GPU texture and upload the image.
    pub added: Vec<(TextureHandle, Arc<Image>)>,
    /// Existing textures whose image was replaced (the size may differ).
    pub updated: Vec<(TextureHandle, Arc<Image>)>,
    /// Textures that are gone: drop their GPU resources.
    pub removed: Vec<TextureHandle>,
}

impl TextureChanges {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Added(u32),
    Updated(u32),
    Removed(u32),
}

/// The texture store (a resource). Handles are never reused; id 0 is reserved for "none".
#[derive(Debug)]
pub struct Textures {
    images: HashMap<u32, Arc<Image>>,
    next_id: u32,
    pending: Vec<Op>,
}

impl Default for Textures {
    fn default() -> Self {
        Self { images: HashMap::new(), next_id: 1, pending: Vec::new() }
    }
}

impl Textures {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `image` and queue it as added.
    pub fn add(&mut self, image: Image) -> TextureHandle {
        let id = self.next_id;
        self.next_id += 1;
        self.images.insert(id, Arc::new(image));
        self.pending.push(Op::Added(id));
        TextureHandle { id }
    }

    /// Replace the image of `handle` (any size) and queue it as updated.
    pub fn replace(&mut self, handle: TextureHandle, image: Image) -> Result<(), TextureError> {
        match self.images.get_mut(&handle.id) {
            Some(slot) => {
                *slot = Arc::new(image);
                self.pending.push(Op::Updated(handle.id));
                Ok(())
            }
            None => Err(TextureError::NoSuchTexture(handle.id)),
        }
    }

    /// Remove `handle`; `false` if there was no such texture.
    pub fn remove(&mut self, handle: TextureHandle) -> bool {
        if self.images.remove(&handle.id).is_some() {
            self.pending.push(Op::Removed(handle.id));
            true
        } else {
            false
        }
    }

    /// Decode PNG `bytes` (all targets; use `include_bytes!` for assets that ship with a web
    /// build) and [`add`](Self::add) the image.
    pub fn add_png(&mut self, bytes: &[u8]) -> Result<TextureHandle, super::png::PngError> {
        Ok(self.add(Image::from_png(bytes)?))
    }

    /// Read a PNG file and [`add`](Self::add) it. Native only (the web has no file system).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_png(&mut self, path: impl AsRef<std::path::Path>) -> Result<TextureHandle, super::png::PngError> {
        let bytes = std::fs::read(path.as_ref()).map_err(|e| super::png::PngError::Io(format!("{}: {e}", path.as_ref().display())))?;
        self.add_png(&bytes)
    }

    pub fn get(&self, handle: TextureHandle) -> Option<&Arc<Image>> {
        self.images.get(&handle.id)
    }

    /// `(width, height)` of the texture, if it exists.
    pub fn size(&self, handle: TextureHandle) -> Option<(u32, u32)> {
        self.get(handle).map(|i| i.size())
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Whether changes are waiting for the next frame.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Take the net changes since the last drain: an add followed by a remove vanishes, an add
    /// followed by a replace is one add with the latest image, and so on. Called by the render
    /// extract; each change is returned exactly once.
    pub fn drain_changes(&mut self) -> TextureChanges {
        let ops = std::mem::take(&mut self.pending);
        // first op and last op per id, in first-appearance order
        let mut order: Vec<u32> = Vec::new();
        let mut first: HashMap<u32, Op> = HashMap::new();
        let mut last: HashMap<u32, Op> = HashMap::new();
        for op in ops {
            let (Op::Added(id) | Op::Updated(id) | Op::Removed(id)) = op;
            if let std::collections::hash_map::Entry::Vacant(v) = first.entry(id) {
                v.insert(op);
                order.push(id);
            }
            last.insert(id, op);
        }
        let mut out = TextureChanges::default();
        for id in order {
            let h = TextureHandle { id };
            match (first[&id], last[&id]) {
                (Op::Added(_), Op::Removed(_)) => {}
                (Op::Added(_), _) => out.added.push((h, self.images[&id].clone())),
                (_, Op::Removed(_)) => out.removed.push(h),
                (_, _) => out.updated.push((h, self.images[&id].clone())),
            }
        }
        out
    }

    /// Put undelivered `changes` back at the front of the queue (the engine does this when a
    /// frame could not be rendered, so the renderer still sees them once).
    pub(crate) fn requeue(&mut self, changes: TextureChanges) {
        let mut ops: Vec<Op> = Vec::new();
        ops.extend(changes.added.iter().map(|(h, _)| Op::Added(h.id)));
        ops.extend(changes.updated.iter().map(|(h, _)| Op::Updated(h.id)));
        ops.extend(changes.removed.iter().map(|h| Op::Removed(h.id)));
        self.pending.splice(0..0, ops);
    }
}
