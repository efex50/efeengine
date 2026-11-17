//! GPU views (API.md section 17) on the headless backend: node, builder, signals, view layer,
//! painter registry. Nothing is painted headless.

use std::cell::RefCell;
use std::rc::Rc;

use efeengine_core::prelude::*;
use efeengine_core::render::Gpu;
use efeengine_core::wgpu;
use efeengine_ui::view::{self, *};
use efeengine_ui::*;

struct Noop;
impl UiPainter for Noop {
    fn paint(&mut self, _: &Gpu, _: &mut wgpu::CommandEncoder, _: &ViewTarget) {
        panic!("headless never paints");
    }
}

#[test]
fn builder_creates_a_gpu_view_with_pointer_flag() {
    let mut t = UiTest::new(|_| {});
    let r = t.ui_mut().layer("root", Layer::fill(0));
    let a = t.ui_mut().gpu_view(r, "map").key("a").id();
    let b = t.ui_mut().gpu_view(r, "map").pointer_signals().key("b").id();
    t.step(1);
    assert_eq!(t.ui().kind(a), Some(&NodeKind::GpuView { painter: "map".into(), pointer: false }));
    assert_eq!(t.ui().kind(b), Some(&NodeKind::GpuView { painter: "map".into(), pointer: true }));
    assert_eq!(t.ui().kind(a).unwrap().tag(), KindTag::GpuView);
    assert!(t.backend().errors().is_empty());
}

#[test]
fn pointer_signals_need_the_flag() {
    let mut t = UiTest::new(|_| {});
    let r = t.ui_mut().layer("root", Layer::fill(0));
    t.ui_mut().gpu_view(r, "map").key("off");
    t.ui_mut().gpu_view(r, "map").pointer_signals().key("on");
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    t.engine.add_system(Stage::Update, move |cx| l.borrow_mut().extend(cx.ui_signals().cloned()));
    t.pointer("off", (1.0, 2.0), 1);
    t.pointer("on", (3.0, 4.0), PointerEvent::LEFT);
    t.step(1);
    let log = log.borrow();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].key.as_deref(), Some("on"));
    assert_eq!(log[0].kind, UiSignalKind::Pointer { pos: (3.0, 4.0), buttons: 1, wheel: (0.0, 0.0) });
}

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Map(PointerEvent),
}

fn map_view(_: &()) -> El<Msg> {
    view::column().child(gpu_view("map").key("map").width(Size::Px(100.0)).height(Size::Px(80.0)).on_pointer(Msg::Map))
}

#[test]
fn view_layer_gpu_view_reconciles_and_dispatches() {
    let mut t = UiTest::new(|e| {
        e.insert_resource(());
        e.add_plugin(UiView::new("v", map_view).layer(Layer::fill(0))).unwrap();
    });
    t.step(2);
    let id = t.id("v/map");
    assert_eq!(t.ui().kind(id), Some(&NodeKind::GpuView { painter: "map".into(), pointer: true }));
    let got = Rc::new(RefCell::new(Vec::new()));
    let g = got.clone();
    t.engine.add_system(Stage::Update, move |cx| g.borrow_mut().extend(cx.events::<Msg>().iter().cloned()));
    t.pointer("v/map", (5.0, 6.0), 1);
    t.step(1);
    assert_eq!(got.borrow().as_slice(), &[Msg::Map(PointerEvent { pos: (5.0, 6.0), buttons: 1, wheel: (0.0, 0.0) })]);
    // Stable across frames: the same node, not recreated.
    t.step(2);
    assert_eq!(t.id("v/map"), id);
}

#[test]
fn painters_register_before_and_after_the_plugin() {
    let mut t = UiTest::new(|e| {
        e.add_ui_painter("early", Noop);
    });
    t.engine.add_ui_painter("late", Noop);
    let p = t.engine.ui_painters();
    assert!(p.contains("early") && p.contains("late") && !p.contains("nope"));
}
