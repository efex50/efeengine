//! The iced backend driven directly on a real GPU (an offscreen texture instead of a window):
//! focus navigation, widget identity across tree changes, idle frames. All `#[ignore]`d: they
//! need a wgpu adapter (`cargo test -p efeengine-ui --test native_gpu -- --ignored`).
#![cfg(all(feature = "iced", not(target_arch = "wasm32")))]

use efeengine_core::input::{ElementState, InputEvent, InputEventKind, Key, KeyCode, Modifiers, NamedKey};
use efeengine_core::render::Gpu;
use efeengine_core::wgpu;
use efeengine_ui::native::IcedBackend;
use efeengine_ui::{Layer, UiBackend, UiBackendCx, UiSignal, UiSignalKind, UiTree, UiValue, WidgetId};

struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    target: wgpu::TextureView,
    be: IcedBackend,
    tree: UiTree,
}

const SIZE: (u32, u32) = (640, 480);

impl Rig {
    fn new(tree: UiTree) -> Rig {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or_default(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut be = IcedBackend::new();
        let mut cx = UiBackendCx::new((SIZE.0 as f32, SIZE.1 as f32), 1.0);
        be.init(&mut cx);
        let gpu = Gpu {
            device: &device,
            queue: &queue,
            adapter: &adapter,
            surface_format: wgpu::TextureFormat::Rgba8UnormSrgb,
            size: SIZE,
            scale_factor: 1.0,
        };
        be.gpu_init(&gpu);
        let mut rig = Rig { device, queue, adapter, target, be, tree };
        rig.sync();
        rig
    }

    fn sync(&mut self) {
        let changes = self.tree.take_changes();
        let mut cx = UiBackendCx::new((SIZE.0 as f32, SIZE.1 as f32), 1.0);
        let theme = self.tree.theme().clone();
        self.be.sync(&self.tree, &changes, &theme, &mut cx);
    }

    fn frame(&mut self) {
        let gpu = Gpu {
            device: &self.device,
            queue: &self.queue,
            adapter: &self.adapter,
            surface_format: wgpu::TextureFormat::Rgba8UnormSrgb,
            size: SIZE,
            scale_factor: 1.0,
        };
        self.be.render(&gpu, &self.target);
    }

    fn send(&mut self, kind: InputEventKind) {
        let _ = self.be.input(&InputEvent::new(kind));
    }

    fn key(&mut self, code: KeyCode, named: NamedKey, mods: Modifiers) {
        self.send(InputEventKind::key(code, Key::Named(named), ElementState::Pressed, false, mods));
    }

    fn type_text(&mut self, t: &str) {
        self.send(InputEventKind::Text(t.into()));
    }

    /// Drains the signals and writes value changes back to the tree like the plugin does.
    fn signals(&mut self) -> Vec<UiSignal> {
        let mut out = Vec::new();
        self.be.drain_signals(&mut out);
        for s in &out {
            if let UiSignalKind::Changed(v) = &s.kind {
                let _ = self.tree.write_back(s.widget, v.clone());
            }
        }
        out
    }
}

fn two_inputs() -> (UiTree, WidgetId, WidgetId) {
    let mut t = UiTree::new();
    let top = t.layer("top", Layer::centered(10));
    let col = t.column(top).id();
    let a = t.text_input(col, "").key("a").id();
    let b = t.text_input(col, "").key("b").id();
    (t, a, b)
}

fn changed(sigs: &[UiSignal], id: WidgetId) -> Option<String> {
    sigs.iter().rev().find_map(|s| match (&s.kind, s.widget == id) {
        (UiSignalKind::Changed(UiValue::Text(t)), true) => Some(t.clone()),
        _ => None,
    })
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn tab_moves_focus_between_inputs_and_escape_blurs() {
    let (t, a, b) = two_inputs();
    let mut rig = Rig::new(t);
    rig.frame();
    assert!(!rig.be.capture().keyboard);

    rig.key(KeyCode::Tab, NamedKey::Tab, Modifiers::NONE);
    rig.frame();
    let s = rig.signals();
    assert!(s.iter().any(|s| s.widget == a && s.kind == UiSignalKind::Focus(true)), "{s:?}");
    assert!(rig.be.capture().keyboard);
    rig.type_text("x");
    assert_eq!(changed(&rig.signals(), a).as_deref(), Some("x"));

    rig.key(KeyCode::Tab, NamedKey::Tab, Modifiers::NONE);
    let s = rig.signals();
    assert!(s.iter().any(|s| s.widget == a && s.kind == UiSignalKind::Focus(false)));
    assert!(s.iter().any(|s| s.widget == b && s.kind == UiSignalKind::Focus(true)));

    // Shift+Tab goes back.
    rig.key(KeyCode::Tab, NamedKey::Tab, Modifiers::LSHIFT);
    let s = rig.signals();
    assert!(s.iter().any(|s| s.widget == a && s.kind == UiSignalKind::Focus(true)));

    rig.key(KeyCode::Escape, NamedKey::Escape, Modifiers::NONE);
    let s = rig.signals();
    assert!(s.iter().any(|s| s.widget == a && s.kind == UiSignalKind::Focus(false)));
    assert!(!rig.be.capture().keyboard);
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn focus_and_text_survive_a_layer_inserted_below_and_a_reorder() {
    let (t, a, b) = two_inputs();
    let mut rig = Rig::new(t);
    rig.frame();
    rig.key(KeyCode::Tab, NamedKey::Tab, Modifiers::NONE);
    rig.type_text("a");
    let _ = rig.signals();

    // A new layer with a lower z lands before the focused one in the stack.
    let below = rig.tree.layer("below", Layer::fill(1));
    let _ = rig.tree.button(below, "under");
    rig.sync();
    rig.frame();
    rig.type_text("b");
    let s = rig.signals();
    assert_eq!(changed(&s, a).as_deref(), Some("ab"), "focus lost after inserting a layer: {s:?}");

    // Reorder the two inputs: the focus follows the node, not the position.
    let col = rig.tree.parent(a).unwrap();
    rig.tree.move_to(a, col, 1).unwrap();
    rig.sync();
    rig.frame();
    rig.type_text("c");
    let s = rig.signals();
    assert_eq!(changed(&s, a).as_deref(), Some("abc"), "focus moved with the position: {s:?}");
    assert!(changed(&s, b).is_none());
}

#[test]
#[ignore = "needs a GPU (wgpu adapter)"]
fn idle_frames_do_not_rebuild() {
    let (t, ..) = two_inputs();
    let mut rig = Rig::new(t);
    // Warm up (first draws shape text), then time the frames: an unchanged interface takes the
    // present-only path.
    for _ in 0..5 {
        rig.frame();
    }
    let t0 = std::time::Instant::now();
    for _ in 0..200 {
        rig.frame();
    }
    let idle = t0.elapsed() / 200;
    let t0 = std::time::Instant::now();
    for _ in 0..200 {
        rig.sync_dirty();
        rig.frame();
    }
    let active = t0.elapsed() / 200;
    eprintln!("idle {idle:?} per frame, active {active:?} per frame");
    assert!(idle < active, "idle {idle:?} should be cheaper than active {active:?}");
}

impl Rig {
    /// Marks the interface changed (a resize does it) without touching the tree.
    fn sync_dirty(&mut self) {
        self.be.resize((SIZE.0 as f32, SIZE.1 as f32), 1.0);
    }
}
