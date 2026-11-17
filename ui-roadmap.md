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

### Phase 0: native spike (about a day)

- [ ] Add iced main as a git dependency pinned to a commit (`iced_wgpu`, `iced_runtime`, `iced_widget`).
- [ ] Draw one iced button and one text input over coin_dash with `iced_wgpu` on the engine's device,
  queue and surface texture, in a pass after the sprite renderer.
- [ ] Convert SDL events to iced events (mouse, keys, text, scale factor) and drive
  `iced_runtime::UserInterface` from the engine loop.
- [ ] Report whether iced consumed the input; the game ignores clicks on the button.
- [ ] Measure binary size, compile time and frame time cost.

**Exit:** the button and text field work on native and the costs are written down. If iced cannot
share the surface or the costs are unacceptable, switch the native plan to section 6 now.

### Phase 1: shared API (`efeengine-ui`)

- [ ] `UiTree`, `WidgetId`, node types, `Style` subset, themes.
- [ ] `UiSignal` events, bindings, input-ownership flag in `InputState`.
- [ ] A headless backend for tests: signals injected by tests, the tree inspected directly.

**Exit:** a settings menu can be built and driven in headless tests.

### Phase 2: web backend

- [ ] `UiTree` → DOM under a `#ui` layer above the canvas (`templates/web` gets the layer and the
  predefined CSS).
- [ ] DOM events → `UiSignal`; patch only changed nodes.
- [ ] Focus rules: a focused text input keeps keyboard input from the game; the focus-loss input
  reset in `web_backend.rs` must not fire when focus moves from the canvas into the UI.
- [ ] Theme classes in a stylesheet the game can override.

**Exit:** the Phase 1 settings menu works in the browser.

### Phase 3: native backend (iced)

- [ ] `UiTree` → iced `Element`s, iced messages → `UiSignal`.
- [ ] Style subset and themes mapped to iced styles; compare screenshots with the web backend.
- [ ] Text input, IME and clipboard through SDL.
- [ ] DPI and resize through the engine's scale factor.

**Exit:** the same settings menu, unchanged, works on native.

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
