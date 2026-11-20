//! The fallback native backend (roadmap 4C.6, `docs/ui/FALLBACK_NATIVE.md`): the `UiTree` laid
//! out with taffy, text shaped by cosmic-text and rasterised into a CPU glyph atlas, everything
//! drawn through the engine's [`Draw2D`](efeengine_core::render::Draw2D), so the same code runs
//! in the wgpu and in the SDL rendering mode.
//!
//! The backend is driven like the others (`sync` with the journal, `input` from the engine's
//! input filter, `drain_signals`) and produces a frame in two steps so a host can order its
//! texture uploads:
//!
//! 1. [`FallbackBackend::prepare`] lays out (when needed) and records the frame into a
//!    [`DrawList`]; glyphs missing from the atlas are rasterised.
//! 2. The host uploads [`FallbackBackend::take_atlas_image`] (when `Some`) to the texture it
//!    gave with [`FallbackBackend::set_atlas_texture`], then replays
//!    [`FallbackBackend::draw`] onto its `Draw2D`.
//!
//! Positions and sizes: the tree and the layout are logical pixels, the list is physical pixels
//! (`scale` from `resize`). `GpuView` nodes are not drawn (they need a wgpu pass); a faint
//! outline marks the spot.

mod edit;
mod input;
mod layout;
#[cfg(not(target_arch = "wasm32"))]
pub mod offscreen;
mod model;
mod paint;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use efeengine_core::input::{Filter, InputEvent};
use efeengine_core::prelude::TextureHandle;
use efeengine_core::render::{Draw2D, DrawList, Image};
use efeengine_core::window::CursorIcon;

use crate::backend::{TextInputArea, UiBackend, UiBackendCx};
use crate::capture::UiCapture;
use crate::id::WidgetId;
use crate::journal::UiChange;
use crate::signal::UiSignal;
use crate::theme::Theme;
use crate::tree::UiTree;

use edit::InputState;
use input::Interaction;
use layout::LayoutCx;
use model::Model;
use paint::{PaintState, Painter};
use shapes::Mesh;
use text::{TextCell, TextSystem};

pub use crate::text_kit::{shapes, text};
pub use text::DEFAULT_FAMILY;

/// What an `ellipsize` text shows for a value and available width (see `update_ellipsis`).
struct Ellipsis {
    value: String,
    avail: u32,
    shown: Option<String>,
}

fn collect_followers(n: &model::VNode, out: &mut std::collections::HashSet<WidgetId>) {
    if matches!(n.kind, crate::node::NodeKind::Scroll { follow_end: true }) {
        out.insert(n.id);
    }
    for c in &n.children {
        collect_followers(c, out);
    }
}

/// Caret blink half period.
const BLINK_MS: u128 = 530;

pub struct FallbackBackend {
    /// The window this instance draws (`UiBackendCx::window`).
    window: u32,
    text: TextSystem,
    cells: HashMap<WidgetId, TextCell>,
    inputs: HashMap<WidgetId, InputState>,
    /// Ellipsized one-line texts: what is shown (`None` fits) for a value and width, and what was
    /// last reported with `Truncated`.
    ellipsis: HashMap<WidgetId, Ellipsis>,
    model: Model,
    images: HashMap<TextureHandle, (u32, u32)>,
    logical: (f32, f32),
    scale: f32,
    atlas_tex: Option<TextureHandle>,
    atlas_pending: Option<Arc<Image>>,
    list: DrawList,
    mesh: Mesh,
    /// The model or the window changed: lay out again.
    layout_dirty: bool,
    /// Anything visible changed: record again.
    dirty: bool,
    ui: Interaction,
    signals: Vec<UiSignal>,
    text_request: Option<Option<TextInputArea>>,
    text_sent: Option<TextInputArea>,
    icon: CursorIcon,
    icon_sent: CursorIcon,
    icon_request: Option<CursorIcon>,
    blink: bool,
    blink_epoch: Instant,
    last_caret_on: bool,
    layouts: u32,
    records: u32,
}

impl Default for FallbackBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FallbackBackend {
    pub fn new() -> Self {
        Self::with_system_fonts(false)
    }

    /// `system_fonts`: also load the installed fonts (covers scripts the bundled Inter lacks;
    /// slower start).
    pub fn with_system_fonts(system_fonts: bool) -> Self {
        Self {
            window: 0,
            text: TextSystem::new(system_fonts),
            cells: HashMap::new(),
            inputs: HashMap::new(),
            ellipsis: HashMap::new(),
            model: Model::default(),
            images: HashMap::new(),
            logical: (0.0, 0.0),
            scale: 1.0,
            atlas_tex: None,
            atlas_pending: None,
            list: DrawList::new(),
            mesh: Mesh::new(),
            layout_dirty: true,
            dirty: true,
            ui: Interaction::default(),
            signals: Vec::new(),
            text_request: None,
            text_sent: None,
            icon: CursorIcon::Default,
            icon_sent: CursorIcon::Default,
            icon_request: None,
            blink: true,
            blink_epoch: Instant::now(),
            last_caret_on: true,
            layouts: 0,
            records: 0,
        }
    }

    /// The texture the glyph atlas lives in (the host adds a texture and passes its handle;
    /// updates arrive through [`take_atlas_image`](Self::take_atlas_image)).
    pub fn set_atlas_texture(&mut self, handle: TextureHandle) {
        self.atlas_tex = Some(handle);
        self.dirty = true;
        // The host's texture needs the current pixels.
        self.atlas_pending = self.text.atlas.take_image().or_else(|| self.atlas_pending.take());
    }

    pub fn atlas_texture(&self) -> Option<TextureHandle> {
        self.atlas_tex
    }

    /// The atlas pixels when they changed since the last call (replace the whole texture).
    pub fn take_atlas_image(&mut self) -> Option<Arc<Image>> {
        if let Some(img) = self.text.atlas.take_image() {
            self.atlas_pending = Some(img);
        }
        self.atlas_pending.take()
    }

    /// Switch the caret blink off (always visible): for tests and screenshots.
    pub fn set_caret_blink(&mut self, on: bool) {
        self.blink = on;
        self.dirty = true;
    }

    /// Anti-aliased rounded shapes (default on). Switch off for a renderer that rasterises on
    /// integer vertices, like SDL's software renderer: shapes then have hard edges.
    pub fn set_antialias(&mut self, on: bool) {
        self.mesh.feather = on;
        self.dirty = true;
    }

    /// Number of layout passes and of recorded frames so far (cost counters for tests).
    pub fn counters(&self) -> (u32, u32) {
        (self.layouts, self.records)
    }

    /// Glyph bitmaps currently in the atlas.
    pub fn cached_glyphs(&self) -> usize {
        self.text.cached_glyphs()
    }

    /// Lays out when needed and records the frame. Returns `true` when the list changed (the
    /// host must draw it again; a host that redraws every frame anyway may ignore this).
    pub fn prepare(&mut self) -> bool {
        self.ensure_layout();
        let caret_on = !self.blink || self.ui.focus.is_none() || (self.blink_epoch.elapsed().as_millis() / BLINK_MS) % 2 == 0;
        if caret_on != self.last_caret_on {
            self.last_caret_on = caret_on;
            self.dirty = true;
        }
        if self.text.begin_frame() {
            self.dirty = true;
        }
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        self.records += 1;
        self.list.clear();
        let model = std::mem::take(&mut self.model);
        {
            let mut p = Painter {
                out: &mut self.list,
                text: &mut self.text,
                cells: &mut self.cells,
                inputs: &mut self.inputs,
                ellipsis: &self.ellipsis,
                images: &self.images,
                mesh: &mut self.mesh,
                st: PaintState {
                    hovered: self.ui.hovered,
                    pressed: self.ui.pressed,
                    focus: self.ui.focus,
                    focus_visible: self.ui.focus_visible,
                    scroll: &self.ui.scroll,
                    caret_on,
                },
                scale: self.scale,
                atlas: self.atlas_tex,
            };
            for l in &model.layers {
                p.node(&l.root, (0.0, 0.0), None);
            }
            p.finish();
        }
        self.model = model;
        if self.text.atlas_full() {
            // A glyph did not fit: record again next frame with a fresh atlas.
            self.dirty = true;
        }
        if let Some(img) = self.text.atlas.take_image() {
            self.atlas_pending = Some(img);
        }
        self.update_ime_area();
        true
    }

    /// The absolute rectangle `(x, y, w, h)` of a widget in logical px (after layout and scrolling),
    /// `None` when the widget is not shown.
    pub fn widget_rect(&mut self, id: WidgetId) -> Option<(f32, f32, f32, f32)> {
        self.ensure_layout();
        self.abs_rect(id).map(|r| (r.x, r.y, r.w, r.h))
    }

    /// The recorded frame (physical pixels).
    pub fn draw_list(&self) -> &DrawList {
        &self.list
    }

    /// Replays the recorded frame onto `d`.
    pub fn draw(&self, d: &mut dyn Draw2D) {
        self.list.replay(d);
    }

    /// `follow_end` flags of this model: a node that starts following sticks to its end at once.
    fn follow_flags(&mut self) {
        let mut now = std::collections::HashSet::new();
        for l in &self.model.layers {
            collect_followers(&l.root, &mut now);
        }
        for id in &now {
            if !self.ui.following.contains(id) {
                self.ui.pinned.insert(*id);
            }
        }
        self.ui.pinned.retain(|id| now.contains(id));
        self.ui.following = now;
    }

    /// Cuts the one-line `ellipsize` texts that do not fit their width (`...` at the cut) and
    /// reports `Truncated` when that changes. Runs after every layout, so a resize is covered.
    fn update_ellipsis(&mut self) {
        use crate::node::NodeKind;
        use crate::signal::UiSignalKind;
        let mut jobs = Vec::new();
        fn walk(n: &model::VNode, out: &mut Vec<(WidgetId, String, f32, crate::style::Style)>) {
            if let NodeKind::TextInput { value, read_only: true, multiline: false, ellipsize: true, .. } = &n.kind {
                let pad = paint::padding(n.style());
                out.push((n.id, value.clone(), (n.rect.w - pad.left - pad.right - 2.0).max(1.0), *n.style()));
            }
            for c in &n.children {
                walk(c, out);
            }
        }
        for l in &self.model.layers {
            walk(&l.root, &mut jobs);
        }
        let alive: std::collections::HashSet<WidgetId> = jobs.iter().map(|j| j.0).collect();
        self.ellipsis.retain(|id, _| alive.contains(id));
        for (id, value, avail, style) in jobs {
            if self.ellipsis.get(&id).is_some_and(|e| e.value == value && e.avail == avail.to_bits()) {
                continue;
            }
            let width = |text: &mut TextSystem, cells: &mut HashMap<WidgetId, TextCell>, s: &str| {
                let cell = cells.entry(id).or_default();
                text.layout(cell, &layout::text_spec(&style, s, false), None).0
            };
            let shown = if width(&mut self.text, &mut self.cells, &value) <= avail {
                None
            } else {
                // The longest prefix (of the value without a trailing `...`) that fits with `...`.
                let base: Vec<char> = value.strip_suffix("...").unwrap_or(&value).chars().collect();
                let (mut lo, mut hi) = (0usize, base.len());
                while lo < hi {
                    let mid = (lo + hi).div_ceil(2);
                    let cand: String = base[..mid].iter().collect::<String>() + "...";
                    if width(&mut self.text, &mut self.cells, &cand) <= avail {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                    }
                }
                Some(base[..lo].iter().collect::<String>() + "...")
            };
            let cut = shown.is_some();
            let was = self.ellipsis.get(&id).is_some_and(|e| e.shown.is_some());
            if cut != was {
                self.signals.push(crate::signal::UiSignal { widget: id, key: None, owner: crate::id::Owner::Game, kind: UiSignalKind::Truncated(cut) });
            }
            self.ellipsis.insert(id, Ellipsis { value, avail: avail.to_bits(), shown });
            self.dirty = true;
        }
    }

    pub(crate) fn ensure_layout(&mut self) {
        if !self.layout_dirty {
            return;
        }
        self.layout_dirty = false;
        self.layouts += 1;
        let mut cx = LayoutCx { text: &mut self.text, cells: &mut self.cells, images: &self.images };
        layout::layout(&mut self.model, self.logical, &mut cx);
        self.update_ellipsis();
        self.ui.clamp_scroll(&self.model);
        self.dirty = true;
        self.refresh_hover();
    }
}

impl UiBackend for FallbackBackend {
    fn init(&mut self, cx: &mut UiBackendCx) {
        self.window = cx.window;
        self.logical = cx.logical_size;
        self.scale = cx.scale_factor.max(0.01);
    }

    fn input(&mut self, ev: &InputEvent) -> Filter {
        self.handle_input(ev)
    }

    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], _theme: &Theme, cx: &mut UiBackendCx) {
        for (h, img) in &cx.images {
            self.images.insert(*h, img.size());
        }
        for ch in changes {
            if let UiChange::Font(id) = ch
                && let Some(f) = tree.font(*id)
            {
                if self.text.load_font(*id, &f.bytes).is_none() {
                    efeengine_core::prelude::global_warn(&format!("[efeengine-ui] font \"{}\" has no usable face", f.name));
                }
                // Shaped text must pick the new face up.
                self.cells.clear();
            }
        }
        self.model = Model::build(tree, self.window);
        self.follow_flags();
        self.layout_dirty = true;
        self.dirty = true;
        // Forget state of nodes that are gone.
        let m = &self.model;
        self.cells.retain(|id, _| m.find(*id).is_some());
        self.inputs.retain(|id, _| m.find(*id).is_some());
        self.ui.retain(&self.model);
        // `UiTree::blur`: drop the focus of a text input (reports `Focus(false)`).
        if changes.iter().any(|c| matches!(c, UiChange::Blur(w) if w.is_none_or(|w| w == self.window)))
            && self.focused_input().is_some()
        {
            self.set_focus(None, false);
        }
        self.after_focus_change();
    }

    fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
        out.append(&mut self.signals);
    }

    fn capture(&self) -> UiCapture {
        UiCapture {
            pointer: self.ui.over || self.model.modal() || self.ui.drag.is_some(),
            keyboard: self.ui.focus.is_some_and(|f| self.model.find(f).is_some_and(|n| matches!(n.kind, crate::node::NodeKind::TextInput { .. }))) || self.model.modal(),
        }
    }

    fn resize(&mut self, logical: (f32, f32), scale: f32) {
        self.logical = logical;
        self.scale = scale.max(0.01);
        self.layout_dirty = true;
        self.dirty = true;
    }

    fn draws_with_draw2d(&self) -> bool {
        true
    }

    fn draw2d(&mut self, cx: &mut efeengine_core::render::Overlay2D) {
        if self.atlas_tex.is_none() {
            self.set_atlas_texture(TextureHandle { id: efeengine_core::render::OVERLAY_TEXTURE_BASE + cx.window });
        }
        if cx.integer_vertices == self.mesh.feather {
            self.set_antialias(!cx.integer_vertices);
        }
        self.prepare();
        if let (Some(h), Some(img)) = (self.atlas_tex, self.take_atlas_image()) {
            cx.textures.upload(h, &img);
        }
        self.draw(cx.draw);
    }

    fn take_text_input(&mut self) -> Option<Option<TextInputArea>> {
        self.text_request.take()
    }

    fn take_cursor_icon(&mut self) -> Option<CursorIcon> {
        self.icon_request.take()
    }
}
