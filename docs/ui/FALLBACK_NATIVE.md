# Fallback native UI backend: taffy + glyphon + own widgets

> **Status:** Design and roadmap, not implemented. Built only if the iced backend is blocked
> (wgpu lock, missing features, size) or as a lighter option for small games.\
> **Date:** 2026-11-17\
> **Implements:** the common API in [`API.md`](API.md). Game code does not change when switching.\
> **Feature:** `efeengine-ui/ui-fallback` (native only)

> **Revision planned (2026-11-18):** per [`../RENDERER_PLAN.md`](../RENDERER_PLAN.md) (section 3.5,
> phase 4C.6) this backend will draw through the shared `Draw2D` API instead of its own WGSL
> pipeline, so it also runs in the SDL rendering mode (no wgpu): rounded rectangles as triangle
> meshes or nine-slice textures, glyphs rasterised on the CPU (cosmic-text + swash) into an atlas
> texture instead of glyphon. Sections 1, 4 and 6 below describe the original wgpu-only design and
> will be rewritten at that point.

## 1. Pieces

| Part | Crate | Job |
|---|---|---|
| Layout | `taffy` 0.15 | Flexbox over a mirror of the `UiTree` |
| Text | `glyphon` 0.12 (cosmic-text + swash), wgpu 30 | Shaping, line breaking, glyph atlas, text drawing, text editing (`cosmic_text::Editor`) |
| Shapes | own WGSL pipeline | Rounded rectangles with border, images |
| Widgets | own code | Button, checkbox, slider, text input, scroll, container, text, image |
| Input | engine `InputEvent`s | Hit testing on layout rectangles, focus, capture |

Same wgpu major as the engine (30), so everything shares the engine's device, queue and surface
texture.

## 2. Data flow per frame

```text
UiTree + journal ──► sync: update taffy nodes, resolved styles, text buffers
                          │
input filter (raw events) ─► hit test on last layout ─► widget state machines ─► UiSignal
                          │
PostUpdate end ──► layout (taffy compute_layout with text measure) only if dirty
                          │
overlay pass ──► quads + images + text, per layer, in z order, with scissor clips
```

## 3. Layout (taffy)

- One `taffy::NodeId` per `WidgetId`, kept in a map. `Created`/`Removed`/`Moved` journal entries
  add, remove and re-parent taffy nodes. `Style`/`Classes`/`Theme` entries re-resolve the node's
  style and convert it to `taffy::Style` (direction, justify, align, gap, padding, size, min/max,
  `flex_grow` for `Fill`/`Portion`).
- Layers are taffy roots sized to the window (logical px) with the anchor turned into an
  alignment of the layer's single child.
- **Text measure:** text, button labels, checkbox labels and text inputs are leaf nodes with a
  measure function: given the available width, shape the cosmic-text `Buffer` with that width and
  return its size. Results are cached per (node, width) until the text or font changes.
- Layout runs once per frame, only when something marked it dirty (tree change, resize, scale
  change, font load). The result is a rectangle per node in logical px, converted to physical px for
  drawing.

## 4. Drawing

- **Quad pipeline:** one instanced draw per batch. Instance = rect, background color, border
  width, border color, radius, clip rect. The fragment shader draws the rounded rectangle and border
  with a signed distance function (anti-aliased edges). Checkbox marks and slider tracks/thumbs are
  quads too (the check mark can be a glyph from the bundled font).
- **Images:** the same pipeline with a texture bind group, from the backend's own texture table,
  filled from the engine's `TextureChanges`.
- **Text:** glyphon `TextRenderer` per layer, `TextArea`s with position, bounds (the clip rect) and
  color.
- **Order:** for each visible layer in z order: quads and images in tree order, then that layer's
  text. Text inside one layer is always above that layer's quads. That covers normal UI (labels on
  buttons and panels). Overlapping widgets that need text below a quad go in a higher layer.
- **Clipping:** `Scroll` nodes push a clip rect (intersection with the parent's). Quads carry the
  clip in the instance (discard in the shader). Text uses `TextArea::bounds`. No stencil needed.
- **DPI:** everything is laid out in logical px and multiplied by the scale factor when drawing.
  glyphon gets the physical font size, so text is sharp at 2x.

## 5. Widgets

Each widget is a small state machine fed by input events and the node's data:

| Widget | State | Signals |
|---|---|---|
| Button | normal, hovered, pressed, focused, disabled | `Click` on release inside after a press inside; Enter/Space when focused |
| Checkbox | as button + checked | `Changed(Bool)` |
| Slider | normal, hovered, dragging, focused | `Changed(F32)` while dragging (value snapped to `step`); arrows when focused |
| TextInput | cosmic-text `Editor`: cursor, selection, scroll offset, IME preedit | `Changed(Text)` per edit, `Submit` on Enter, `Focus` |
| Scroll | offset, dragging thumb | none (wheel and thumb drag scroll it) |
| Container, Text, Image | none | `Hover` if opted in |

- **Hit testing:** walk visible layers from the top, nodes in reverse tree order, skipping
  invisible and clipped-out parts. The first enabled interactive node under the pointer gets the
  event. A visible modal layer stops the walk.
- **Focus:** click to focus, Tab / Shift+Tab in tree order, Escape blurs a text input. Keyboard
  capture while a text input is focused.
- **Text input details:**
  - editing through `cosmic_text::Editor` (insert, delete, cursor moves, word moves with Ctrl,
    selection with Shift, select all);
  - clipboard copy/cut/paste through SDL3's clipboard (a small window-trait addition);
  - IME: needs the `TextEditing` input event and `set_text_input_area` from the API doc
    (section 11); the preedit text is drawn underlined at the cursor;
  - `password` draws bullets, `max_len` counts characters;
  - text input mode is enabled on the window only while a text input is focused.
- **Scroll:** wheel scrolls the main axis, a thin thumb is drawn when content overflows and can be
  dragged; focus moving into a hidden child scrolls it into view.

## 6. Size and cost

- New dependencies: taffy, glyphon (cosmic-text, swash, fontdb, etagere, lru). Measure binary size
  and compile time in phase F1 and record them next to the iced spike numbers.
- Expected frame cost: layout only on change; one quad batch and one text batch per layer; glyph
  atlas uploads only for new glyphs.
- Code estimate: about 3-5k lines for layout sync, renderer, widgets and text input. The text input
  (editing, clipboard, IME) is the largest single piece.

## 7. Roadmap

### F0: decide whether it is needed

- [ ] After the iced spike (UI roadmap phase 0), compare: iced works with the engine's wgpu? binary
  size? frame cost? If iced is fine, F1+ waits. F1 can still run as a small proof that the API is
  backend-neutral.

### F1: static UI

- [ ] `taffy` mirror of the `UiTree`, journal-driven sync, style conversion.
- [ ] Quad pipeline (rounded rect + border, SDF), images, glyphon text, per-layer order, DPI.
- [ ] Container, Text, Image; layers and anchors.
- [ ] Overlay pass through the engine's `add_render_overlay` (API doc section 12).

**Exit:** a static panel with text and an image draws correctly at 1x and 2x and on resize.

### F2: interaction

- [ ] Input filter, hit testing, capture flags, modal layers.
- [ ] Button, Checkbox, Slider with all states and theme state styles.
- [ ] Focus and Tab order, keyboard activation.

**Exit:** the API doc's settings menu (minus the text input) works and passes the shared
conformance tests.

### F3: text input and scroll

- [ ] TextInput with `cosmic_text::Editor`, selection, clipboard, password, max length.
- [ ] IME: `TextEditing` event and `set_text_input_area` in core, preedit drawing.
- [ ] Scroll with clipping, wheel, thumb drag, scroll-into-view.

**Exit:** the full settings menu works, including typing a name with an IME.

### F4: polish

- [ ] Font loading at runtime, bold, text alignment, opacity.
- [ ] Screenshot comparison against the web and iced backends for the example menus.
- [ ] Performance check with a large list (1000 rows in a scroll).

## 8. Risks

| Risk | Plan |
|---|---|
| Text input and IME take longer than expected | It is the last phase; F1-F2 are useful without it |
| glyphon API changes between versions | Pin the version; glyphon is small and easy to follow |
| Draw order limits (text above quads per layer) | Documented; a widget that needs more uses a higher layer |
| Look differs from web and iced | Same theme resolution code; screenshot comparison in F4 |
