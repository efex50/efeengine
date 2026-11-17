# efeengine UI Roadmap

> **Status:** Accepted direction, not started\
> **Date:** 2026-11-17 (replaces the 2026-11-15 egui + iced proposal)\
> **Scope:** Game-facing UI (menus, settings, HUD panels, dialogs, inventory) and developer tools\
> **Design docs:** [`docs/ui/API.md`](docs/ui/API.md) (common API, all backends),
> [`docs/ui/FALLBACK_NATIVE.md`](docs/ui/FALLBACK_NATIVE.md) (taffy + glyphon backend and its roadmap)

## 1. Direction

Games describe their UI once through an engine-owned API. Each platform has its own backend:

| Platform | Backend | Who does layout, text and input |
|---|---|---|
| Native (SDL3 + wgpu) | iced (`iced_wgpu` + `iced_runtime`), drawn on the engine's surface | iced |
| Web (wasm) | HTML elements + predefined CSS, in a layer over the canvas | the browser |

Both backends report user actions as the same `UiSignal` events, so game code, scripts and mods never
know which backend runs. This is the same pattern as the io/window backends (SDL, web, headless
behind one API).

Developer tools (inspector, console, profiler) are separate: egui on both platforms, drawn into the
canvas on the web too. They do not use the game UI API.

### Why

- **HTML on the web** gives text input, IME, clipboard, font shaping, accessibility, browser zoom and
  real CSS for free, and keeps the wasm small (no font rasterizer or shaper shipped).
- **iced on native** gives layout, text shaping, text editing and widget behaviour without writing
  them. The engine is on wgpu 30, the same as iced's main branch (`0.15.0-dev`, workspace
  `wgpu = "30"`; the last release, 0.14 from Dec 2025, is still on wgpu 27), so iced can share the
  engine's device and surface. Until 0.15 is released we depend on iced main by git, pinned to a commit. It does not need winit (only `iced_winit` does, and we skip it).
- **An engine-owned API** in between, because iced widgets are opaque (`Widget` trait objects) and
  cannot be turned into HTML. iced itself had a DOM backend (`iced_web`, 0.1 to 0.3) and dropped it in
  2022, so "write iced, get HTML" is not an option.

## 2. Architecture

```text
game code / scripts / mods
        │  build and update
        ▼
efeengine-ui: UiTree (nodes + Style subset + bindings)  ──►  Events<UiSignal>
        │
  ┌─────┴───────────────────────────┐
native backend                    web backend
  UiTree → iced Elements            UiTree → DOM elements + CSS classes
  SDL events → iced events          DOM events → UiSignal
  iced messages → UiSignal          patches only changed nodes
  own render pass after the game    #ui layer above the canvas
```

### Crates and features

```text
crates/
├── core          efeengine-core: no UI dependency
├── ui            efeengine-ui: UiTree, Style, UiSignal, bindings, plugin
│   ├── native    feature `iced` (default on native): iced_wgpu + iced_runtime
│   └── web       wasm only: web-sys DOM backend
└── editor        later: egui developer tools
```

- `efeengine-core` stays toolkit-agnostic: no iced or egui types in its API.
- Games opt in with a `UiPlugin`. A game without it pulls in neither iced nor the DOM backend.
- iced types never appear in the `efeengine-ui` public API. That keeps the backend replaceable (see 6).

### Shared API (sketch; full design in [`docs/ui/API.md`](docs/ui/API.md))

```rust
// Build: nodes are owned by the UiTree resource, addressed by WidgetId.
let menu = ui.column(root, Style::new().gap(8.0).padding(16.0).align(Align::Center));
let play = ui.button(menu, "Play");
let volume = ui.slider(menu, 0.0..=1.0, 0.8);
ui.text(menu, Bind::res(|s: &Score| format!("Best: {}", s.best)));

// React: the same events on both backends.
fn menu_system(mut signals: EventReader<UiSignal>, ...) {
    for s in signals.read() {
        match (s.widget, &s.kind) {
            (w, UiSignalKind::Click) if w == play => { /* start the game */ }
            (w, UiSignalKind::Changed(Value::F32(v))) if w == volume => { /* set volume */ }
            _ => {}
        }
    }
}
```

- **Nodes (first set):** panel/row/column, text, image, button, checkbox, slider, text input,
  scroll area. Small on purpose: every widget is implemented twice.
- **Style subset** (one Rust struct, rendered as CSS on the web and as iced layout/style on native):
  direction, gap, padding, alignment, width/height (fill, shrink, fixed), colors, border, radius,
  font size, visibility. Anything outside the subset is not offered rather than looking different
  per platform.
- **Themes:** named style classes. On the web they become predefined CSS classes, on native an iced
  theme/style table.
- **`UiSignal { widget: WidgetId, kind: Click | Changed(Value) | Submit | Focus(bool) }`**, delivered
  as normal engine events in the frame after the input.
- **Engine → UI:** bindings from resources/components to node properties. Backends only patch
  changed nodes (DOM writes on the web, rebuilding iced `Element`s on native when the tree changed).
- **Input ownership:** each frame the backend reports whether the UI consumed pointer/keyboard
  input (iced event status on native, DOM focus/target on the web). `InputState` exposes it so game
  systems skip clicks that hit the UI.

## 3. Phases

### Phase 0: native spike (about a day) — DONE, verdict: GO

- [x] Add iced main as a git dependency pinned to a commit (`iced_wgpu`, `iced_runtime`, `iced_widget`, `iced_core`): `e5c350f271a8046f875c6662d452fde9e462d486` (iced main, 2026-11-16, `0.15.0-dev`). Its workspace uses `wgpu = 30`; `cargo tree -d` shows exactly one `wgpu` (30.0.2, shared with the engine) and no duplicate naga / wgpu-* crates. The dependencies live in `crates/ui` under `cfg(not(target_arch = "wasm32"))` behind the default feature `iced`; the wasm32 build does not see them (`cargo tree --target wasm32-unknown-unknown` has no iced). Only `iced_wgpu`, `iced_runtime`, `iced_widget`, `iced_core` are used (no `iced_winit`, no winit; cosmic-text and cryoglyph come in as iced's git dependencies).
- [x] Draw iced widgets over the game with `iced_wgpu` on the engine's device, queue and surface texture, in a pass after the game renderer: the engine's `RenderOverlay` (`LoadOp::Load`) calls `Renderer::present` with the engine's target view. Written as the real backend (`crates/ui/src/native/`), not throwaway code: layers (z stack, anchors, modal backdrop), container / row / column, text, button, text input, checkbox, slider, scroll.
- [x] Engine events to iced events (mouse, wheel, touch, keys with modifiers, text -> `InputMethod::Commit`, IME preedit, focus, scale factor) and `iced_runtime::UserInterface` driven from the engine loop (one build + `update` per input event from the engine's `InputFilter`, one per frame for drawing).
- [x] Report whether iced consumed the input: the filter answers `Captured` from iced's event status, "pointer over a blocking node" and "text input focused"; `InputState` flags the events and `cx.game_input()` ignores them.
- [x] Measure binary size, compile time and frame time cost (numbers below).

**Measurements** (this machine: Intel UHD 620, Vulkan, Fedora; `examples/ui_demo`, release profile of the workspace, i.e. no strip / LTO):

| | without `iced` | with `iced` | delta |
|---|---|---|---|
| release binary | 13.25 MB | 18.13 MB | +4.88 MB (+37%) |
| same, `strip`ped | 10.09 MB | 13.95 MB | +3.86 MB (+38%) |
| clean release build of `efeengine-ui` (core already built) | 3.2 s | 145.6 s | about +142 s (iced, cosmic-text, cryoglyph, lyon, ... plus the native module) |

Frame time of the iced pass (`EFE_UI_STATS=1`, menu + HUD shown, release): steady state about 1.1 to 1.2 ms per frame (about 0.2 ms build + layout + update + draw on the CPU, about 1 ms in `present`: encoding and `queue.submit`); the first 300 frames, which include font loading and first text shaping, were 2.8 to 7 ms. The pass does nothing while no layer is shown (coin_dash is unaffected: it does not use the UI, and its tests pass). (Phase 3 skips rebuild and layout on idle frames, see below.)

**Findings**
- The commit needs no `[patch]`: iced main, the engine and cryoglyph agree on wgpu 30.0.2.
- iced main changed its layout model: `Length::Fit` (intrinsic size, `Fill` children inside a `Fit` parent are "fluid") and `Length::min` / `max` (bounded lengths). With them `Auto`, `Stretch` and `min_*` / `max_*` map cleanly, so the style table has no gap there (API.md section 5 updated).
- Gaps in iced for our style subset: no `justify` (emulated with `Fill` spacers, only when the main size is definite), buttons / checkboxes / sliders are not focusable (the focused style only shows on text inputs), a checkbox style paints the box, not the whole row, `opacity` is per colour, not per subtree.
- `iced_wgpu::Engine::new` needs a `wgpu::Adapter` and the core `Gpu` had none: the spike created a throwaway adapter; Phase 3 uses `Gpu.adapter`.
- Widget state lives in iced's `user_interface::Cache` keyed by tree position; the model is rebuilt on every journal batch, so state survives as long as the structure does not shift (a layer inserted below another shifts positions; fix in Phase 3 with ids or keyed children).
- iced widgets produce `Msg`s that the backend applies to its own copy first (typing and dragging never wait a frame) and reports as `UiSignal`s; `max_len` is enforced on our side.

**Exit:** the button and text field work on native and the costs are written down: +4.9 MB, +142 s clean build, about 1 ms per frame. Not unacceptable for a native-only, default-on-but-optional feature. **Go: native stays on iced** (section 6 fallback is not needed now). Re-check at every wgpu upgrade (section 5).

### Phase 1: shared API (`efeengine-ui`)

- [x] `UiTree`, `WidgetId`, node types, `Style` subset, themes.
- [x] View layer (`docs/ui/API.md` section 16): `El<Msg>`, reconciler with keys, `UiView`, typed
  message events, `when(changed::<T>())` (needs change ticks on resources).
- [x] `UiSignal` events, bindings, input-ownership flag in `InputState`.
- [x] A headless backend for tests: signals injected by tests, the tree inspected directly.

**Exit:** a settings menu written as a view (and one with the retained API) can be built and driven
in headless tests.

### Phase 2: web backend

- [x] `UiTree` → DOM under a `#ui` layer above the canvas (`templates/web` gets the layer and the
  predefined CSS).
- [x] DOM events → `UiSignal`; patch only changed nodes.
- [x] Focus rules: a focused text input keeps keyboard input from the game; the focus-loss input
  reset in `web_backend.rs` must not fire when focus moves from the canvas into the UI.
- [x] Theme classes in a stylesheet the game can override.

**Exit:** the Phase 1 settings menu works in the browser.

### Phase 3: native backend (iced)

Started by the Phase 0 spike (`crates/ui/src/native/`); what is ticked works, the rest is listed in `crates/ui/structure.md` ("Not done yet").

- [x] `UiTree` → iced `Element`s, iced messages → `UiSignal` (layers, container / row / column, text, button, text input, checkbox, slider, scroll, images).
- [x] Images: `Image` nodes through iced's image widget (`Handle::from_rgba`); a texture replaced in `Textures` refreshes both backends (plugin-level: new image in `UiBackendCx.images` + a synthesized `UiChange::Kind`; no web change needed).
- [x] Fonts: Inter Regular and Bold (OFL, `crates/ui/assets/`, `THIRD_PARTY_NOTICES.md`) are the native default; `UiChange::Font` loads into iced's font system, `Style.font` is the family, `bold` the weight.
- [x] Idle cost: frames without journal entries, input, window change or redraw request skip rebuild, layout and draw (they present the last drawn primitives again); the caret blink is honoured through `RedrawRequest`.
- [x] Stable widget identity: `Keyed` wrapper around every row / column and the layer stack; focus and caret survive layer insertions and reorders (GPU test).
- [x] Tab / Shift+Tab move between text inputs, Escape blurs; physical key codes are mapped to iced's `Physical`.
- [x] Clipboard (copy, cut, paste) and mouse cursor icons through core's `clipboard` and `CursorIcon`.
- [x] Style subset mapped to iced styles and checked property by property against the web backend; remaining differences are in `docs/ui/API.md` ("Per-backend notes"). Layer placement and `Auto` sizes fixed (see below).
- [x] Screenshot comparison: `render_offscreen` renders a tree on the real GPU into a PNG (`cargo test -p ui_demo --test native_screenshot -- --ignored`); the demo menu is a centred panel, the HUD sits top-left, the Play button has its natural height, as on the web.
- [ ] IME preedit drawn inline (the preedit reaches iced's editor, the OS popup works), focus style / tab stops for buttons, checkboxes and sliders, subtree opacity.
- [x] DPI and resize through the engine's scale factor (`Viewport` from `Gpu.size` / `scale_factor`, events divided by the scale).

**Frame cost with the Phase 3 work** (release, `EFE_UI_STATS=1`, Intel UHD 620 Vulkan, ui_demo menu + HUD):
- Real window, nothing happening: about 1.0 to 1.1 ms per frame, all of it `present` (encode + `queue.submit` + compositing into the window's frame); the first active frames (font loading, first shaping) take 70 to 80 ms.
- Offscreen (no window, vsync or compositor; `crates/ui/tests/native_gpu.rs`, release, 2 text inputs): idle (present only) 0.21 ms, active (rebuild + layout + update + draw + present) 0.28 ms. In debug builds 0.92 ms vs 1.13 ms. The saving is the CPU part, about 0.07 ms for a tiny interface and growing with the tree; `present` still costs the same because the engine re-draws its target every frame and the overlay must be composited again.
- Bundled font: +832 KB in the binary (Inter Regular 412 KB + Bold 420 KB, no subsetting; the OFL allows it). Enabling the `image` features of iced adds the `image` crate and guillotiere to the native build.

**Phase 3 findings**
- Bug found by the screenshot: in this iced revision `Length::merge` makes "Fill win over Fit", so the old mapping (stretched `Auto` children are `Fill`, "fluid inside a `Fit` parent") made every auto-sized panel as big as its layer: layers looked glued together and the Play button grew to the bottom. Fixed: `Fill`, `Portion` and stretch are only `Fill` when the parent's size on that axis is definite, else the child keeps its own size (`Fluid(Max)` was tried and makes text widgets take all the space). Lost: stretching children to the widest sibling in an auto-sized parent.
- `UserInterface::operate` runs one pass, `focus_next` / `focus_previous` are chained operations (`Outcome::Chain`): the backend loops until the chain ends.
- iced's layer lists keep the primitives of the last `draw` until the next `ui.draw`, so re-presenting is valid (tested pixel for pixel).
- iced's `Tree::diff` matches children by position only (`keyed::Column` only searches for a single insertion); `Keyed` permutes child trees by our own keys instead.

**Exit:** the same settings menu, unchanged, works on native.

### Phase 3b: GPU views (`docs/ui/API.md` section 17)

- [ ] Core `SurfaceSet` (main surface moved onto it, no behaviour change); extra surfaces on the web incl. WebGL2.
- [x] `GpuView` node, `UiPainter`, `add_ui_painter` (`EngineUiExt`), view-layer `gpu_view`, pointer signals, headless sizing.
- [x] iced backend: `shader` widget mapping (offscreen texture, premultiplied).
- [x] Web backend: canvas per view on WebGPU (compile-checked only); WebGL2 path (tested in headless Firefox: pixels, click, no errors).
- [ ] ui_demo: a spinning-shape GPU view; native and web screenshots compared. (Native done: rotating triangle, click recolors, native screenshot test; web: pixel and click test in headless Firefox passes, side-by-side comparison with native still open.)

**Exit:** the same painter draws the same picture in the same place on native, WebGPU and WebGL2.

### Phase 4: scripts, mods and examples

- [ ] UI signals as script events; scripts build nodes through host functions, under the existing
  grant rules (a mod only sees and changes its own widgets).
- [ ] Example: main menu + settings + pause menu in coin_dash.
- [ ] Docs: building UI, themes, input ownership, replacing the backend.

### Later

- World-space UI (labels and bars over entities): HTML cannot draw under the canvas, so either
  position HTML elements from the camera each frame or draw these in the game renderer.
- egui developer tools (`crates/editor`): inspector, console, profiler.
- Accessibility on native (iced's support is limited; the web gets it from the DOM).

## 4. Rules

1. Game code only uses `efeengine-ui` types. No iced or DOM types leak into the API.
2. Every node type and style property exists on both backends, or it is not offered.
3. UI changes are applied on the main thread during the frame (the DOM is only reachable there;
   iced runs with the renderer). Systems on pool workers change the `UiTree`, not the backend.
4. Signals are events, not callbacks: no closures stored in the tree.
5. The UI stays optional: no `UiPlugin`, no UI dependencies.

## 5. Risks

| Risk | Plan |
|---|---|
| wgpu lock: the engine cannot move past the wgpu version iced uses (30 on iced main), and iced releases slowly (0.14 had no successor for about a year) | Before every wgpu major upgrade, check iced main (not only the last release). If main lags too, keep `iced_wgpu` + `cryoglyph` current with a `[patch]` branch (all of iced's wgpu code is in those two crates, ~8.5k lines in `iced_wgpu`), or switch the native backend (section 6) |
| iced main is a dev version (API can change before 0.15) | Pin a commit; move to the crates.io release when 0.15 ships |
| Native and web look different | Small style subset, screenshot comparison in Phase 3 |
| HTML is always above the canvas | World-space UI handled separately (see Later) |
| Two implementations of every widget | Keep the widget set small; shared tests through the headless backend |
| Input ownership and focus edge cases | Defined in Phase 1, tested on both backends |
| iced needs a change we cannot make from outside | `[patch.crates-io]` with a small branch and an upstream PR. A full fork only if iced's design blocks us (iced is far larger than hecs, every upgrade would be a manual merge) |

## 6. Fallback for native

Full design and phases F0-F4: [`docs/ui/FALLBACK_NATIVE.md`](docs/ui/FALLBACK_NATIVE.md).


Because the API is engine-owned, the native backend can be replaced without changing game code:
**taffy** (flexbox layout, no wgpu dependency) + **glyphon** (text on wgpu) + our own widgets. The
newest glyphon (0.12) uses wgpu 30, the engine's version. The hard part
to write ourselves would be text editing (cursor, selection, IME).

## 7. Decision summary

| Area | Choice |
|---|---|
| Engine core | UI-toolkit agnostic |
| Game UI API | Engine-owned `efeengine-ui` (UiTree, Style subset, UiSignal, bindings) |
| Native game UI backend | iced main / 0.15 (`iced_wgpu` + `iced_runtime`), git dependency pinned to a commit until 0.15 is released, not forked |
| wgpu | 30 (engine, iced main, cryoglyph and glyphon all match) |
| Web game UI backend | HTML + predefined CSS, layer above the canvas |
| Native fallback | taffy + glyphon + own widgets |
| Developer tools | egui, both platforms, separate from game UI |
