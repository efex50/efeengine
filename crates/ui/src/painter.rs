//! GPU-view painters (API.md section 17): the game's wgpu drawing code for `GpuView` nodes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use efeengine_core::prelude::Engine;
use efeengine_core::render::Gpu;
use efeengine_core::wgpu;

use crate::id::WidgetId;

/// The game's drawing code for GPU views. Painters are main-thread objects (no `Send`) and are
/// registered by name with [`EngineUiExt::add_ui_painter`]; a `GpuView` node names its painter.
/// One painter may serve several nodes (see [`ViewTarget::widget`]).
pub trait UiPainter: 'static {
    /// The view's size changed (physical px), or the target format is new: recreate size- and
    /// format-dependent resources (depth buffers, pipelines). Also called before the first
    /// `paint` of a node.
    fn resize(&mut self, _gpu: &Gpu, _target: &ViewTarget) {}

    /// Draw the view. `target` is the view's own texture: origin (0,0) is its top-left corner
    /// and it is exactly `target.size` big. The painter owns these pixels (clear or load), and
    /// writes **premultiplied alpha**. Record into `encoder`; the backend submits it.
    fn paint(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, target: &ViewTarget);
}

/// Where a painter draws.
pub struct ViewTarget<'a> {
    pub view: &'a wgpu::TextureView,
    /// Fixed for the run (the surface format on native).
    pub format: wgpu::TextureFormat,
    /// Physical px.
    pub size: (u32, u32),
    /// Window scale factor.
    pub scale: f32,
    /// Which node is drawn (one painter may serve several nodes).
    pub widget: WidgetId,
}

type Map = HashMap<Arc<str>, Box<dyn UiPainter>>;

/// The painters of one engine, by name. A cheap handle (`Rc`): clones share the registry.
/// Backends get it from [`UiBackend::set_painters`](crate::UiBackend::set_painters). Main
/// thread only.
#[derive(Clone, Default)]
pub struct UiPainters {
    inner: Rc<RefCell<Map>>,
}

impl UiPainters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers (or replaces) the painter `name`.
    pub fn add(&self, name: &str, painter: impl UiPainter) {
        let _ = self.inner.borrow_mut().insert(name.into(), Box::new(painter));
    }

    pub fn contains(&self, name: &str) -> bool {
        self.inner.borrow().contains_key(name)
    }

    /// Runs `f` with the painter `name`; `None` if there is none. Painters must not call back
    /// into the registry (it is borrowed while `f` runs).
    pub fn with<R>(&self, name: &str, f: impl FnOnce(&mut dyn UiPainter) -> R) -> Option<R> {
        let mut m = self.inner.borrow_mut();
        let p = m.get_mut(name)?;
        Some(f(p.as_mut()))
    }
}

// The registry is not `Send`, but engine resources must be: the resource is only a key into a
// main-thread table.
thread_local! {
    static REGISTRIES: RefCell<HashMap<u64, UiPainters>> = RefCell::new(HashMap::new());
}

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// Engine resource: the key of this engine's painter registry in the main-thread table.
struct PaintersKey(u64);

impl Drop for PaintersKey {
    fn drop(&mut self) {
        let _ = REGISTRIES.try_with(|r| r.borrow_mut().remove(&self.0));
    }
}

/// Painter registration on the engine (the engine is a core type, so this is an extension
/// trait: `use efeengine_ui::EngineUiExt`).
pub trait EngineUiExt {
    /// Registers the GPU-view painter `name` (replacing one with the same name). Works before or
    /// after `UiPlugin` is added, and later at any time on the main thread.
    fn add_ui_painter(&mut self, name: &str, painter: impl UiPainter) -> &mut Self;

    /// The painter registry of this engine (created on first use).
    fn ui_painters(&mut self) -> UiPainters;
}

impl EngineUiExt for Engine {
    fn add_ui_painter(&mut self, name: &str, painter: impl UiPainter) -> &mut Self {
        self.ui_painters().add(name, painter);
        self
    }

    fn ui_painters(&mut self) -> UiPainters {
        if let Some(k) = self.resources().get::<PaintersKey>()
            && let Some(p) = REGISTRIES.with(|r| r.borrow().get(&k.0).cloned())
        {
            return p;
        }
        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
        let p = UiPainters::new();
        REGISTRIES.with(|r| r.borrow_mut().insert(key, p.clone()));
        self.insert_resource(PaintersKey(key));
        p
    }
}
