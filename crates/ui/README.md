# efeengine-ui

The engine-owned UI API: a retained tree (`UiTree`) with a change journal, styles, themes, signals, bindings and a backend trait. Backends turn the tree into real widgets (web DOM, iced, a native fallback); a headless backend and `UiTest` drive it in tests. Design: `docs/ui/API.md`. Layout and details: `structure.md`.

```rust
engine.add_plugin(UiPlugin::new())?;            // web DOM backend on wasm32, iced on native (feature `iced`, default) when there is a window, else headless (UiPlugin::headless() forces headless)

let ui = engine.resources_mut().get_mut::<UiTree>().unwrap();
let root = ui.layer("menu", Layer::centered(10));
let col = ui.column(root).class("panel").gap(12.0).padding(24.0).id();
ui.button(col, "Play").key("menu/play");
ui.slider(col, 0.0..=1.0, 0.8).key("menu/volume");

engine.add_system(Stage::Update, |cx| {
    for s in cx.ui_signals() {                  // trait UiCtxExt: Events<UiSignal>, same frame
        if let ("menu/play", UiSignalKind::Click) = s.key_kind() { /* start */ }
    }
});

// or the view layer: a function from state to elements, typed messages
use efeengine_ui::{column, view::*};
fn menu(s: &State) -> El<Msg> {
    column![button("Play").key("play").on_click(Msg::Play), slider(0.0..=1.0, s.volume).key("vol").on_change(Msg::Volume)]
}
engine.add_plugin(UiView::new("menu", menu).layer(Layer::centered(10)))?;   // t.click("menu/play")

// GPU views: a node the game draws with wgpu (API.md section 17)
use efeengine_ui::EngineUiExt;
engine.add_ui_painter("minimap", Minimap::new());          // impl UiPainter: resize(..), paint(gpu, encoder, &ViewTarget)
// view layer: gpu_view("minimap").width(Size::Px(200.0)).height(Size::Px(200.0)).on_pointer(Msg::Map)
// retained:   ui.gpu_view(panel, "minimap").pointer_signals();   -> UiSignalKind::Pointer { pos, buttons, wheel }

// tests
let mut t = UiTest::new(|e| { /* setup */ });
t.click("menu/play");
t.step(1);
```

Run the tests with `cargo test -p efeengine-ui`.
