# efeui-core

The engine-agnostic UI model: a retained tree (`UiTree`) with a change journal, styles and themes, signals, bindings, the Elm-style `El` view descriptions with their reconciler, CSS generation for web targets and a headless consistency checker. Design: `docs/ui/API.md`. Layout: `structure.md`.

**The rule: no engine.** This crate must not depend on `efeengine-core`, hecs, wgpu, iced, web-sys or any other engine, renderer or windowing crate. Only small pure crates (`smallvec`, `thiserror`) are allowed. Check with `cargo tree -p efeui-core`; it builds for `wasm32-unknown-unknown` too. Where the model needs something an engine provides, it owns a small type and the adapter converts: images are an opaque `ImageId`, input capture is `UiCapture`, binding inputs are a type-keyed `BindCx` the adapter fills.

It is the base for adapters:

- `efeengine-ui` (`crates/ui`): the efeengine plugin, backends (web DOM, iced, fallback), scripting, painters, `UiTest`. It re-exports every module of this crate under the same path, so `efeengine_ui::tree::UiTree` and `efeui_core::tree::UiTree` are the same type.
- a future Bevy adapter would do the same for Bevy: a plugin that owns a `UiTree`, hands `take_changes()` to a backend, and feeds `UiSignal`s back through `filter_signals`.

```rust
use efeui_core::*;

let mut ui = UiTree::new();
let root = ui.layer("menu", Layer::centered(10));
ui.button(root, "Play").key("menu/play");
let changes = ui.take_changes();            // what a backend has to apply

// the view layer, applied by hand (an adapter does this every frame)
let el: El<()> = column![view::text("hi"), view::button("ok").key("ok")];
```

Run the tests with `cargo test -p efeui-core`.
