# Fallback native UI backend: taffy + cosmic-text through Draw2D

> **Status:** built, tested and wired in (roadmap 4C.6 done). `crates/ui/src/fallback`, feature
> `fallback`; it draws through `Draw2D` from the engine's Draw2D overlay hook, so it works in the wgpu
> and in the SDL rendering mode (coin_dash menus checked in both). Open items are in F4.\
> **Date:** 2026-11-18\
> **Implements:** the common API in [`API.md`](API.md). Game code does not change when switching.\
> **Feature:** `efeengine-ui/fallback` (native only; wasm uses the DOM backend)

## 1. Pieces

| Part | Crate | Job |
|---|---|---|
| Layout | `taffy` 0.15 (flexbox only) | One tree per layer, built from the model on every `sync` |
| Text | `cosmic-text` (the fork and rev `iced_wgpu` uses, so both backends compile it once; features `std`, `swash`, no fontconfig) | Shaping, wrapping, hit testing, `Editor` for text inputs; swash rasterises glyphs on the CPU |
| Glyph atlas | own (`text.rs`) | 1024x1024 RGBA8 image, shelf packed, one bitmap per glyph and sub-pixel bin; the host owns the texture |
| Shapes | own (`shapes.rs`) | Rounded rectangles, borders, rails and circles as triangle meshes with a feathered edge |
| Widgets | own (`input.rs`, `edit.rs`) | Button, checkbox, slider, text input, scroll, container, text, image |
| Output | `efeengine_core::render::Draw2D` | Everything is a `triangles` call plus `set_clip` / `set_filter`, recorded into a `DrawList` |

Nothing talks to wgpu or SDL: the backend produces a `DrawList` (plain data, physical pixels) and
the host replays it on whatever `Draw2D` it has (`WgpuDraw2D` after a `DrawList`, `SdlDraw2D`
directly). `GpuView` nodes need a wgpu pass of their own and are not drawn; a faint outline marks
their place.

## 2. Data flow per frame

```text
UiTree + journal ──► sync: Model (styles resolved per state, text props inherited), layout dirty
                          │
input filter (raw events) ─► hit test on the last layout ─► widget state machines ─► UiSignal
                          │                                   (values edited locally, plugin writes back)
host, once per frame:
  prepare()            ─► layout if dirty, record the frame into a DrawList (only when something changed)
  take_atlas_image()   ─► Some(image) when glyphs were added: replace the atlas texture
  draw(&mut dyn Draw2D)─► replay the list
```

The two-step frame exists so the host can upload the atlas **before** the list is drawn (SDL draws
immediately). `prepare` returns `false` when nothing changed (the list is kept; hosts that clear
the target every frame still replay it). Counters: `counters()` returns (layout passes, recorded
frames).

## 3. Layout (taffy)

- The model (`model.rs`) is an owned, theme-resolved copy of the visible tree, rebuilt on every
  `sync` (same idea as the iced backend). Text, hover, focus and scroll state live in maps keyed by
  `WidgetId`, so they survive a rebuild; entries of removed nodes are dropped.
- Layout uses the **normal** style only (a hover style never changes the layout). Everything is
  laid out in logical px with taffy rounding off; drawing multiplies by the scale factor and snaps
  to physical pixels.
- Mapping (same rules as the iced column of API.md section 5): `Auto` is the intrinsic size;
  `Fill` / `Portion(n)` get `flex_grow` (1 / n) with basis 0 when the parent's size on that axis is
  definite, and stay intrinsic in an intrinsically sized parent; `Px` never shrinks; `justify`,
  `align`, `gap` are the flexbox properties; padding is inside the size; a border takes no layout
  space; `Scroll` has `overflow: scroll` (its children never shrink).
- Leaves with a measure function: text, button labels, checkboxes (box + gap + label), text
  inputs (one line high), sliders (16 px high), images (pixel size, aspect kept when one side is
  given). Text is shaped at the logical size with line height 1.3 (iced's default) and wraps at
  the available width; min-content is the longest word.
- A layer is a window-sized wrapper whose alignment is the anchor; `Fill` makes the root grow and
  stretch. The layer `offset` shifts the root away from the anchored edge (inwards), like iced.
- Layout runs only when the model or the window size/scale changed.

## 4. Drawing

- **Order:** layers by z, nodes in tree order; each node draws its background, border, then its
  content, then its children. The glyph quads of a text come right after the box they belong to,
  so text sits where the tree puts it (an overlapping later widget covers it), unlike the
  one-text-pass-per-layer limit of the original design.
- **Shapes (`shapes.rs`):** a rounded rectangle is a convex polygon (corner arcs of 3 to 16
  segments). A fill is a fan plus a one pixel ring whose outer vertices have alpha 0, so edge
  pixels get their coverage under `Blend::Alpha`; a border is a four-row strip (fade in, solid,
  solid, fade out) of the same ring pair. Radii under 1 px and plain rectangles are pixel snapped
  and not feathered. `set_antialias(false)` (used for SDL's **software** renderer, which
  rasterises on integer vertices) gives hard edges at whole-pixel coordinates instead.
- **Text:** glyph quads (`Nearest`, integer positions, sub-pixel bins) from the atlas texture,
  tinted by the vertex colour; colour glyphs (emoji) are drawn untinted. One `triangles` call per
  text. Glyphs that do not fit are skipped and the atlas is cleared and re-recorded on the next
  frame (also when it is 90% used).
- **Images:** one textured quad (`Filter::Linear`) with uv cropped for `Cover` / `None`, clipped
  to the node.
- **Clipping:** `Scroll` and text inputs use `Draw2D::set_clip` (the clip is flushed between
  geometry and text so draw order is kept); the scrollbar thumb is drawn over the content.
- **Colours** are the style's sRGB values, straight alpha, `opacity` folded into the alpha.
  wgpu blends the feathered edges in linear space and SDL in encoded space, so borders look a
  little brighter on wgpu (measured: 0.7% of the pixels of the demo menu differ by more than 64
  between the two modes, mean 0.9/255).
- **DPI:** the list is physical pixels; glyphs are rasterised at `font_size * scale`, so text is
  sharp at 2x. `resize(logical, scale)` re-layouts nothing but the list (and the layout, which
  uses logical px, is unchanged apart from the window size).
- **Fonts:** the bundled Inter (regular, bold) and the fonts the game loads
  (`UiTree::load_font`, journal `Font`). System fonts are opt-in (`with_system_fonts(true)`,
  slower start); without them a script Inter lacks shows missing-glyph boxes.

## 5. Widgets

| Widget | State | Signals |
|---|---|---|
| Button | normal, hovered, pressed, focused (keyboard), disabled | `Click` on release inside after a press inside; Enter / Space when focused |
| Checkbox | as button + checked | `Changed(Bool)` on release inside, Space / Enter |
| Slider | normal, hovered, dragging, focused | `Changed(F32)` while dragging (snapped to `step`), Left / Right / Up / Down / Home / End |
| TextInput | `cosmic_text::Editor`: cursor, selection, scroll offset, IME preedit | `Changed(Text)` per edit, `Submit` on Enter, `Focus` |
| Scroll | offset per node; thumb drag | none |
| Container, Text, Image | none | `Hover` if opted in |
| GpuView | not drawn | `Pointer` signals when `pointer` is set |

- **Hit testing:** layers from the top (a modal layer ends the walk), children in reverse tree
  order, `Scroll` nodes clip and shift. A node blocks the pointer when it is not a plain
  Container / Text (or has a background or border). The topmost interactive node under the pointer
  is the target; disabled ones capture but do nothing.
- **Capture** (API.md section 10): pointer while over a blocking node, a modal layer is visible or
  a drag is active; keyboard while a text input has focus or a modal layer is visible. Keys the UI
  used (Tab, Escape with a focus, Enter / Space on a focused widget) are captured too, and so is
  their release.
- **Focus:** clicking a text input or slider focuses it; Tab / Shift+Tab walk enabled buttons,
  checkboxes, sliders and text inputs in tree order (inside the topmost modal layer only);
  Escape blurs. The focus ring (the theme's `focused` style) shows on buttons and checkboxes only
  for keyboard focus. Tab scrolls the widget into view in its `Scroll` ancestors.
- **Text input:** `cosmic_text::Editor` does insertion, deletion, grapheme / word motions and
  selection; Shift extends, a plain arrow collapses; Ctrl+A / C / X / V use
  `efeengine_core::clipboard`; double click selects a word; `max_len` counts characters and cuts
  inserted text; newlines become spaces; `password` draws bullets, caret and selection are mapped
  between the real and the drawn text, copy and cut are disabled. The IME preedit (`TextEditing`)
  is drawn at the caret with an underline; `take_text_input` reports the field's area
  (logical px) while one has focus. The caret blinks (530 ms) unless switched off.
- **Scroll:** the wheel scrolls the main axis (one line = 40 logical px, pixel wheels are divided
  by the scale), the thumb is a 6 px bar at the right edge shown when the content overflows and can
  be dragged.
- Values edited by the user are written into the backend's model at once and reported as
  `Changed`; the plugin writes them back into the tree (API.md section 7).

## 6. Size and cost

- **New dependencies:** `taffy` (+ `slotmap`, `serde` as taffy's own dependency) and `cosmic-text`
  (already in the tree through iced; with iced off it brings `fontdb`, `swash`, `skrifa`,
  `harfrust`, `unicode-*`).
- **Binary size** (release, stripped, measured 2026-11-18): a probe binary that builds a tree,
  runs the backend for one frame and replays the `DrawList` is 4.45 MB against 0.77 MB for the
  same probe with the headless backend, so the fallback adds about **3.7 MB** (of which the two
  bundled Inter faces are 0.83 MB; the rest is cosmic-text / swash / harfrust / skrifa,
  taffy and the backend). The spike measured the iced backend at +3.9 MB on `ui_demo` (iced,
  wgpu glue and its own cosmic-text), so the sizes are comparable; with both features on the
  shared cosmic-text is compiled once. `ui_demo`'s own binary does not grow until the plugin
  wires the backend in (Stage 2), the linker drops it.
- **Frame cost:** layout only when the model or window changed; the frame is recorded only when
  something visible changed (hover, press, focus, edit, scroll, caret phase, new model); one
  `triangles` call per box mesh run and one per text; atlas uploads only when glyphs were added
  (whole texture replace, 4 MB; a sub-rect update would cut that).
- **Code:** about 2.8k lines in `crates/ui/src/fallback` and 750 lines of tests.

## 7. API of the backend (what a host calls)

```rust
let mut b = FallbackBackend::new();            // or with_system_fonts(true)
b.init(&mut cx);                               // UiBackend: init / input / sync / drain_signals / capture / resize / take_*
b.set_atlas_texture(handle);                   // a texture the host created for the glyph atlas
// every frame:
b.prepare();                                   // layout + record; false = nothing changed
if let Some(img) = b.take_atlas_image() { /* replace the atlas texture with img */ }
b.draw(&mut draw2d);                           // replay onto a Draw2D (or read b.draw_list())
```

Also: `set_caret_blink(false)`, `set_antialias(false)` (software renderer), `widget_rect(id)`
(absolute logical rectangle, for tests and focus rings), `counters()`, and
`fallback::offscreen::{record, render_sdl, render_wgpu}` which run the whole path into an
offscreen target and return RGBA8 pixels (used by the tests and the screenshots).

## 8. Tests and screenshots

- `crates/ui/tests/fallback.rs` (29, headless): layout (column gap/padding, Fill/Portion,
  justify/align, anchors and offsets, wrapping, image size, hidden nodes), recording (atlas and
  geometry commands, clip of a scroll node, image uv for Cover, scale factor, no re-record when
  idle), interaction (click on release inside, disabled, hover / press colours, capture, modal,
  checkbox, slider drag and keys, Tab / Enter / Space, hover signals, wheel, thumb drag,
  scroll-into-view) and text input (typing, Backspace / Delete, selection, clipboard, `max_len`,
  caret placement by click, password, IME request, cursor icon, removed widgets, glyph cache).
- `crates/ui/tests/fallback_pixels.rs`: pixel checks through SDL's software renderer (runs
  anywhere, 1x and 2x) and, `#[ignore]`d, through wgpu on a GPU plus the wgpu/SDL comparison.
- `examples/ui_demo/tests/fallback_screenshot.rs`: PNGs of the demo menu
  (`EFE_FALLBACK_SDL=...`, `EFE_FALLBACK_WGPU=...`; the wgpu one is `#[ignore]`d).
- Unit tests in `shapes.rs`, `paint.rs`, `edit.rs`.

Compared with the iced screenshot the layout, colours, rounded panel, checkbox, slider and text
input match closely; differences are a 1 to 3 px vertical drift of some rows (text metrics), the
slider rail drawn the full width (as iced), and the GPU view shown as an outline.

## 9. Engine wiring (as built)

1. **Core hook:** `Draw2DOverlay` / `Overlay2D { window, size, scale_factor, integer_vertices, draw,
   textures }` / `OverlayTextures` in `efeengine_core::render`; `Engine::add_draw2d_overlay`. wgpu
   mode records one `DrawList` per window, uploads what the overlays asked for, and draws it with
   a `WgpuDraw2D` over the window's surface view after the `RenderOverlay`s (the overlay runner keeps
   its own `GpuTextures`, fed from the frame's texture changes). SDL mode records the same way
   and replays onto the `SdlDraw2D` after the sprites, before `present`. `integer_vertices` is true
   for SDL's software renderer.
2. **Overlay textures:** the overlay picks the handle itself, ids from `OVERLAY_TEXTURE_BASE`
   (0xFFFF_0000) up (the fallback uses base + window), and `OverlayTextures::upload(handle, &Image)`
   replaces the texture at once (whole texture; a sub-rect update is still the optimisation).
3. **Backend trait:** `UiBackend::draws_with_draw2d()` and `UiBackend::draw2d(&mut Overlay2D)`. The
   plugin registers a Draw2D overlay (instead of the wgpu `RenderOverlay`) when the main backend
   draws with Draw2D, and routes it to the window's backend instance. The fallback reads
   `cx.window` in `init` and shows `tree.layers_in(window)`.
4. **`UiPlugin`:** `UiPlugin::fallback()` forces the fallback. `UiPlugin::new()` on native picks iced
   when it is compiled in and the mode is not SDL, else the fallback when compiled in (with a log
   line when iced is also compiled in). Limitation: `auto` mode finding no wgpu adapter only switches
   to SDL at graphics init, after the choice, so use `UiPlugin::fallback()` or `EFE_GPU_MODE=sdl`
   for UI there. SDL mode draws the main window only.
5. **Examples:** `ui_demo`, `coin_dash`, `coin_dash_mods`, `multi_window` have a `fallback` feature
   (`cargo build -p coin_dash --no-default-features --features fallback`).
6. **Verified:** coin_dash menu and HUD in `EFE_GPU_MODE=sdl` and `wgpu` (window screenshots),
   ui_demo in SDL mode, workspace check, wasm checks of core and ui.

## 10. Measurements

| What | Value |
|---|---|
| Probe binary, headless backend | 0.77 MB |
| Probe binary, fallback backend | 4.45 MB (+3.7 MB) |
| wgpu vs SDL (software) pixels, demo scene | 0.66% of the pixels differ by more than 64/255, mean 0.93/255 |

## 11. Roadmap

### F1: static UI (done)

- [x] taffy mirror of the tree, journal-driven rebuild, style conversion.
- [x] Rounded rect + border meshes, images, cosmic-text glyph atlas, DPI.
- [x] Container, Text, Image; layers and anchors.
- [x] Overlay pass through the engine (`Draw2DOverlay`).

### F2: interaction (done)

- [x] Hit testing, capture flags, modal layers.
- [x] Button, Checkbox, Slider with state styles.
- [x] Focus and Tab order, keyboard activation.

### F3: text input and scroll (done)

- [x] TextInput with `cosmic_text::Editor`, selection, clipboard, password, max length.
- [x] IME preedit drawing and area request (needs the `TextEditing` event and
  `set_text_input_area` that the plugin already forwards).
- [x] Scroll with clipping, wheel, thumb drag, scroll-into-view.

### F4: polish (open)

- [ ] Opacity on images in a scroll, per-widget font loading at runtime is done (journal `Font`).
- [ ] Shaped-text cache across rebuilds for very large lists (cells are kept per node already).
- [ ] 1000-row scroll performance check (layout is O(n) per change; painting culls by clip).
- [ ] Sub-rect atlas update; premultiplied atlas to blend correctly in `Premultiplied` mode.
- [ ] RTL caret placement (caret x uses the first glyph of the cluster).
- [ ] `GpuView` through a render-to-texture path (needs wgpu; not available in SDL mode).

## 12. Risks

| Risk | Plan |
|---|---|
| SDL's software renderer rasterises on integer vertices | `set_antialias(false)`: hard-edged shapes, text and images unaffected |
| Look differs from web and iced | Same theme resolution; screenshot comparison in section 8 |
| Atlas re-upload cost | Whole-texture replace only when glyphs were added; sub-rect update is the planned fix |
| cosmic-text fork pinned to iced's rev | Same rev, compiled once with iced on; move both together |
