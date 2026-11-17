# efeengine-ui: the common UI API

> **Status:** Implemented in `crates/ui`: sections 1 to 8, 10, 12 (capture, same-frame events), 13 to 16; not yet: scripts and mods (9), native backends (11 fonts bundled, 13 render); the web backend is done except fonts (Phase 2)\
> **Date:** 2026-11-17\
> **Roadmap:** [`ui-roadmap.md`](../../ui-roadmap.md)\
> **Backends:** web (HTML + CSS), native iced, native fallback ([`FALLBACK_NATIVE.md`](FALLBACK_NATIVE.md)), headless (tests)

Game code, scripts and mods build UI through this API only. A backend turns it into real widgets
and reports what the user did. No backend type (iced, web-sys, taffy, glyphon) appears in it.

## 1. Model in one paragraph

The UI is a **retained tree** stored in the `UiTree` resource. Systems create nodes, change their
properties and remove them. Every change is also written to a **change journal**. Once per frame
the active backend reads the journal and patches its own widgets (DOM elements, iced elements, or
fallback layout nodes). User actions come back as **`UiSignal` events**. Values that follow game
state are kept up to date by **bindings**. The backend also reports whether the UI used this frame's
pointer and keyboard input (**`UiCapture`**) so game systems can ignore it.

```text
 systems / scripts / mods ── mutate ──► UiTree ── journal ──► backend.sync()
          ▲                                ▲                       │
          │ Events<UiSignal>          bindings (read game state)   │ user input
          └──────────── backend.drain_signals() ◄──────────────────┘
```

**Two layers.** Rust game code normally uses the **view layer** (section 16): an Elm-style
`view(&State) -> El<Msg>` plus typed messages, diffed in Rust into this tree. The retained tree API
(sections 2 to 13) is the base layer every backend implements, and what scripts, mods and future
`.efui` files use directly.

```text
Rust game:  view(&State) -> El<Msg>   update = a system reading Events<Msg>   (section 16)
                    │ reconcile (diff with the last view, in Rust)
                    ▼
            UiTree + change journal   ◄── scripts, mods, .efui files, low-level Rust
                    ▼
   web (DOM patches) · iced · fallback · headless
```

Why a dedicated resource and not ECS entities: children need a stable order, the journal makes
backend patching cheap, and scripts/mods address widgets by id or key without touching the game
world. Widgets anchored to entities (world-space labels) are a later extension (section 11).

## 2. Ids, keys and ownership

```rust
pub struct WidgetId { index: u32, generation: u32 }   // generational, Copy, stale ids are rejected
```

- `UiTree::key(id, "menu/play")` gives a node a string key, unique in the tree. `UiTree::find("menu/play")`
  returns its id. Keys let scripts, mods and future data files (`.efui`) refer to widgets.
- Every node has an **owner**: `Owner::Game`, `Owner::Plugin(name)` or `Owner::Mod(name)`, set from
  the current plugin/mod context (the same owner tracking the schedule uses). A mod can only see and
  change nodes it owns, and only under its own root (section 9).
- Removing a node removes its subtree. Removing an owner (mod unload) removes all its nodes.

## 3. Layers

The tree has several roots called **layers**, drawn in z order:

```rust
let hud   = ui.layer("hud",   Layer::fill(0));
let pause = ui.layer("pause", Layer::centered(100).modal());   // Layer { z, modal, anchor, offset }
ui.set_visible(pause, false);
```

- `anchor` places the layer's root: `Fill`, `Center`, `TopLeft`, `Top`, `TopRight`, `Left`, `Right`,
  `BottomLeft`, `Bottom`, `BottomRight`, each with a logical-pixel `offset` (`Layer::top_right(z).offset(-8.0, 8.0)`). A layer root is a
  container node whose key is the layer name; `ui.layer(name, ..)` for an existing name updates it.
- Signals from widgets on layers below a visible modal layer (lower `z`) are dropped by the plugin
  (a `Focus(false)` still passes). A visible `modal` layer captures all pointer and keyboard input below it, even outside its
  widgets (pause menus, dialogs).
- Layers are the only absolute positioning in v1. Inside a layer everything is flex layout.

## 4. Nodes

```rust
pub enum NodeKind {
    Container,                                   // flex box: row, column (Style::direction)
    Text    { text: String },
    Image   { texture: TextureHandle, fit: ImageFit },          // Fit: Contain, Cover, Fill, None
    Button  { label: String },                   // or children instead of a label (icon + text)
    Checkbox{ label: String, checked: bool },
    Slider  { min: f32, max: f32, step: f32, value: f32 },
    TextInput { value: String, placeholder: String, max_len: u32, password: bool },
    Scroll,                                      // clips children, scrolls on the main axis
}

pub struct Node {
    kind: NodeKind,
    parent: Option<WidgetId>,
    children: Vec<WidgetId>,
    classes: SmallVec<[ClassId; 2]>,            // theme classes, in order (section 6)
    style: Style,                                // inline overrides
    visible: bool,                               // false = not laid out (display: none)
    enabled: bool,                               // false = greyed, no signals
    key: Option<Arc<str>>,
    owner: Owner,
    layer: Option<Layer>,                        // Some for a layer root
    hover_signals: bool,                         // opt in to Hover signals
}
```

The set is small on purpose: every kind is implemented by every backend. Adding one later means
adding it to all of them (web, iced, fallback, headless) in the same change.

### Builder API

```rust
fn setup_menu(engine: &mut Engine) {
    let ui = engine.resource_mut::<UiTree>();
    let root = ui.layer("menu", Layer::centered(10));
    let col  = ui.column(root).class("panel").gap(12.0).padding(24.0).id();
    ui.text(col, "Coin Dash").class("title");
    ui.button(col, "Play").key("menu/play");
    ui.slider(col, 0.0..=1.0, 0.8).key("menu/volume");
    ui.checkbox(col, "Fullscreen", false).key("menu/fullscreen");
    ui.text_input(col, "").placeholder("Player name").max_len(16).key("menu/name");
}
```

Builders return a small handle with chained setters (`.class`, `.key`, `.gap`, `.width`, ...) and
`.id()`. Reads: `ui.get(id)`, `children`, `parent`, `text_of`, `value`, `find(key)`. Setters on an existing node: `ui.set_text(id, ..)`, `ui.set_value(id, ..)`,
`ui.set_visible(id, ..)`, `ui.set_enabled(id, ..)`, `ui.set_class(id, ..)`, `ui.style_mut(id)`,
`ui.move_to(id, parent, index)`, `ui.remove(id)`. Each writes one journal entry.

## 5. Style

One struct, three renderings: CSS on the web, iced layout and style on native, taffy and the
fallback renderer. **A property exists only if all backends implement it** (iced is the narrowest;
the spike confirms each row).

```rust
pub struct Style {
    // layout (flexbox subset)
    pub direction: Option<Direction>,        // Row | Column
    pub justify:   Option<Justify>,          // Start | Center | End | SpaceBetween
    pub align:     Option<Align>,            // Start | Center | End | Stretch (cross axis)
    pub gap:       Option<f32>,
    pub padding:   Option<Edges>,
    pub width:     Option<Size>,             // Auto | Px(f32) | Fill | Portion(u16)
    pub height:    Option<Size>,
    pub min_width: Option<f32>, pub max_width: Option<f32>,
    pub min_height: Option<f32>, pub max_height: Option<f32>,
    // look
    pub background: Option<Color>,
    pub border:     Option<Border>,          // width, color, radius (one radius for all corners)
    pub text_color: Option<Color>,
    pub font_size:  Option<f32>,
    pub font:       Option<FontId>,
    pub bold:       Option<bool>,
    pub text_align: Option<TextAlign>,       // Start | Center | End
    pub opacity:    Option<f32>,
}
```

- Units are **logical pixels**. Backends multiply by the window scale factor (the engine's
  `ScaleFactorChanged`). On the web CSS pixels are already logical.
- `Fill` means "take the remaining space", `Portion(n)` shares it n:m between siblings (CSS
  `flex-grow`, iced `FillPortion`, taffy `flex_grow`).
- Every field is `Option`: `None` means "inherit from the classes, then the theme default".
- **Web:** every element's inline `style` holds the node's fully RESOLVED normal style
  (`tree.resolve(id, WidgetState::default())`: base, kind, classes in order, inline; one CSS
  property per set field, mapped by the table below; `Fill` / `Portion` depend on the parent's
  resolved direction, so a direction change restyles the children). A style or class change writes
  only the changed properties of that element (the backend keeps the last declarations per node);
  a theme change rewrites every node. Hover, pressed, focus and disabled come from the generated
  state stylesheet (section 6).
- Left out on purpose in v1: margins (use gap and padding), wrapping, absolute positioning inside
  layers, per-corner radius, shadows, transforms, animations. Each can be added when all three
  backends support it.

| Property | CSS | iced | taffy / fallback |
|---|---|---|---|
| direction | `flex-direction` | `row` / `column` | `flex_direction` |
| justify, align | `justify-content`, `align-items` | align: `align_x` / `align_y` on the row / column (`Stretch` = children `Fill` when the parent's size on that axis is definite, else they keep their own size); justify: `Fill` spacers around the children, only when the container's main size is definite (iced has no justify) | `justify_content`, `align_items` |
| gap, padding | `gap`, `padding` | `spacing`, `padding` | `gap`, `padding` |
| width/height Auto, Fill, Portion | `flex-grow` | `Length::Fit` (Auto), `Length::Fill`, `FillPortion` (a `Fill` inside an auto-sized parent stays `Fit`) | `flex_grow` |
| min/max | `min-width` ... | `Length::min(v)` / `Length::max(v)` on the width / height (iced main's bounded lengths; spike: works for both axes) | `min_size`, `max_size` |
| background, border, radius | `background`, `border`, `border-radius` | `container::Style` | quad pipeline |
| text color, size, bold, align | `color`, `font-size`, `font-weight`, `text-align` | `text` style | glyphon attrs |
| opacity | `opacity` | folded into every colour's alpha of the node (background, border, text); a child does not inherit it (the theme puts it on the node that needs it, e.g. disabled) | multiply alpha |

### Per-backend notes (web vs iced)

Checked property by property with an offscreen render of `examples/ui_demo` (`cargo test -p ui_demo
--test native_screenshot -- --ignored`) against the web page. Differences that remain:

| Property | Web (CSS) | Native (iced) | Note |
|---|---|---|---|
| layers | `position: absolute`, `inset: 0` for `Fill`, shrink-to-content otherwise, offsets as left / right / top / bottom (centred anchors: `translate`) | every layer is a full-size `container` with `align_x` / `align_y` from the anchor and padding for the offset; a `Fill` root fills it | same placement |
| `Auto` size | `auto` | `Length::Fit` (never `Fill`) | same |
| `Fill` / `Portion` / `align: Stretch` | `flex-grow`; stretch to the widest sibling in an auto-sized parent | `Fill` / `FillPortion` when the parent's size on that axis is definite (`Px` or `Fill`); in an auto-sized parent the child keeps its own size | native does not stretch children to the widest sibling of an auto-sized parent (iced would turn the parent into `Fill`) |
| `justify` | `justify-content` | `Fill` spacers, only when the container's main size is definite | same result, no free space to share otherwise in CSS either |
| `min_*` / `max_*` | `min-width` ... | `Length::min` / `max` | same |
| `background`, `border`, `radius` | CSS | `container::Style`; button, text input, checkbox, slider have their own style closures | a checkbox paints its box, not the whole row; on web the row element has the background |
| `text_color`, `font_size`, `bold`, `font`, `text_align` | CSS (inherited by children) | per node; the backend copies unset values of `bold`, `font`, `text_color`, `font_size`, `text_align` from the parent's resolved style (per state) | built-in theme sets colour, size and alignment on every node, so in practice only `bold` and `font` inherit |
| `font` | `font-family: var(--efui-font-N)` | family name read from the font file | web font loading: separate change |
| `opacity` | whole subtree (CSS `opacity`) | folded into the node's own colours (background, border, text); children do not inherit it | known difference; put `opacity` on leaf nodes |
| state styles | `:hover`, `:active`, `:focus`, `[disabled]` | button, text input, checkbox, slider through iced status closures | focus style only on text inputs (iced buttons, checkboxes and sliders are not focusable) |
| focus navigation | browser Tab order | Tab / Shift+Tab move between text inputs, Escape blurs the focused one (the key is captured) | buttons are not in the native tab order |
| mouse cursor | CSS `cursor` | iced's `mouse::Interaction` mapped to `CursorIcon`, sent when it changes | the UI also resets the cursor to `Default` when the pointer leaves an interactive widget |
| clipboard | browser (DOM inputs) | copy, cut, paste through `efeengine_core::clipboard` (SDL) | main thread only |
| IME | browser | `TextEditing` preedit reaches iced's editor; the OS popup follows `SetTextInputArea` | preedit is not drawn inline |
| images | `<canvas>` + `object-fit` | `image` widget with `ContentFit` | `Cover` crops to the box on both |

## 6. Themes and classes

```rust
pub struct Theme {
    pub base: Style,                                   // defaults for every node
    pub kinds: HashMap<KindTag, StateStyles>,          // per node kind: Button, Slider, ...
    pub classes: HashMap<Arc<str>, StateStyles>,       // named classes: "panel", "title", ...
}
pub struct StateStyles { pub normal: Style, pub hovered: Style, pub pressed: Style,
                         pub focused: Style, pub disabled: Style }
```

- Resolution order for a node: `base` → its kind → its classes in order → inline `style`, then the
  active state styles on top, state by state in the order hovered → pressed → focused → disabled
  (within one state: the kind's, then each class's). A later state beats an earlier one whoever
  defines it, so `disabled` always wins over a class's hovered or pressed style.
- The engine ships a default theme (dark, readable at 1x and 2x). `UiTree::set_theme` replaces it;
  the change is one journal entry and backends restyle everything.
- **Web:** two stylesheets in `<head>`: `<style id="efui-base">` (static reset; every element has
  class `efui`, plus its kind class `efui-k-<kind>` and one `efui-c-<name>` per theme class) and
  `<style id="efui-theme">` (generated from the theme: the hover, active, focus-visible and
  disabled (`[data-efui-disabled]`) rules of kinds and classes, all `!important`). It is replaced on
  `UiChange::Theme`. Normal styles are NOT in the stylesheet: they are inline (section 5), so the
  class order of a node is respected exactly as in `resolve`. Consequences:
  - A game's own CSS that changes a normal style must use `!important` to beat the inline style;
    one that changes a state style must out-rank an `!important` state rule (a later `!important`
    rule of equal or higher specificity). The engine's `<style>` elements are appended to
    `<head>` at start-up, after the page's own `<link>`s.
  - State rules follow the theme's kind-then-class order of the generated sheet (classes sorted by
    name), not the node's class order, when two classes define the same state property.
  - Disabled is also in the inline style (`resolve` adds it for an effectively disabled node), so
    re-enabling rewrites the node and its descendants.
  - Element mapping: Container, Scroll `div`; Text `span`; Button `<button type=button>` (the label
    text, or the node's children); Checkbox `<label><input type=checkbox><span>`; Slider
    `<input type=range>` (`step` 0 is `any`); TextInput `<input type=text|password>`; Image
    `<canvas>` drawn from the texture's RGBA (`object-fit` from `ImageFit`). Invisible nodes get
    `hidden`; effectively disabled ones `data-efui-disabled` and the native `disabled` attribute.
  - Layers: a layer root is a `div.efui.efui-layer` inside the `#ui` element, positioned by
    `css::layer_declarations` (z-index = `Layer::z`), with `pointer-events: none`. A modal layer
    also gets a transparent full-window `div.efui-backdrop` under its root (same z-index) that takes
    all pointer events, so the content may sit anywhere by its anchor. Inside layers a node takes
    pointer events (`auto`), except a Container or Text without background and border, which lets
    the pointer through to the game (a transparent layout box does not block clicks).
- **iced:** the theme is resolved per node when the tree changes (`WidgetState` variants: normal, hovered, pressed, focused, disabled) and the widgets' status closures pick one (`button::Style` for Active / Hovered / Pressed / Disabled, `text_input` Active / Hovered / Focused / Disabled, checkbox and slider Active / Hovered(Dragged) / Disabled). iced buttons, checkboxes and sliders are not focusable, so only text inputs show `focused`. A checkbox's background / border style paints its box (not the whole row as on the web).
- **Fallback:** resolved per node at sync time and cached until the node, its classes or the theme
  change.

## 7. Signals

```rust
pub struct UiSignal {
    pub widget: WidgetId,
    pub key: Option<Arc<str>>,          // copied from the node, so handlers can match on keys
    pub owner: Owner,
    pub kind: UiSignalKind,
}
pub enum UiSignalKind {
    Click,                              // Button: press and release inside
    Changed(UiValue),                   // Checkbox(Bool), Slider(F32), TextInput(Text) on every edit
    Submit(String),                     // TextInput: Enter
    Focus(bool),                        // gained / lost keyboard focus
    Hover(bool),                        // pointer entered / left (only if the node opts in: `.hover_signals()`)
}
pub enum UiValue { Bool(bool), F32(f32), Text(String) }
```

- Delivered in the **same frame** as the input: the UI plugin's input system runs at
  `order::INPUT + 10` in `PreUpdate` and puts the signals into the readable buffer of
  `Events<UiSignal>` with `Ctx::send_batch_now` (core; plain `Ctx::send` is only readable next
  frame). Later systems of the frame read them with `cx.events::<UiSignal>()`, or the shorthand
  `cx.ui_signals()` (trait `UiCtxExt`, an iterator over the same events). The iterator borrows
  `cx`, so `.cloned().collect()` it first when the loop body needs `cx` mutably. They are dropped at
  the next frame's event update like every readable event.
- Disabled and invisible widgets send nothing.
- Signals are data. The tree stores no callbacks (rule from the roadmap); helpers make matching short:

```rust
fn menu(cx: &mut Ctx) {
    let signals: Vec<UiSignal> = cx.ui_signals().cloned().collect();
    for s in &signals {
        match s.key_kind() {
            ("menu/play", UiSignalKind::Click) => cx.send(StartGame),
            ("menu/volume", UiSignalKind::Changed(UiValue::F32(v))) => cx.resources.get_mut::<Audio>().unwrap().volume = *v,
            _ => {}
        }
    }
}
```

### Value widgets (checkbox, slider, text input)

The tree is the source of truth, but the backend edits locally so typing and dragging never wait
a frame. On every edit the backend sends `Changed`, and the UI plugin writes the new value into the
tree **before** game systems run (no journal entry, the backend already shows it). If a game system
then sets a different value (clamping, validation), that is a normal change and the backend
replaces what it shows (text inputs keep the cursor, clamped to the new length).

## 8. Bindings

```rust
ui.text(col, Bind::res(|s: &Score| format!("Score: {}", s.value)));
ui.bind_visible(pause, Bind::res(|g: &GameState| g.paused));
ui.bind_value(hp_bar, Bind::query::<Health>(player, |h| h.hp / h.max));
```

- A binding is a read-only function from game state to one node property (text, value, visible,
  enabled, class, image). It is not a callback: it cannot change the game.
- The UI plugin evaluates bindings once per frame at `order::LATE + 10` in `PostUpdate`, compares
  with the last value and writes a journal entry only on change. Text bindings compare the produced
  string, so unchanged text costs one format, no DOM or layout work.
- Bindings are stored in a `UiBindings` resource next to the tree, keyed by widget id, removed with
  the widget. They need `Send + Sync` (systems may run on the pool).

## 9. Scripts and mods

Script host functions mirror the builder (registered by the UI plugin before the registration lock
closes, so `.efapi` exports include them):

```text
ui.layer(name, z, modal) -> id      ui.node(parent, kind) -> id     ui.remove(id)
ui.set_text(id, text)               ui.set_value(id, f32)           ui.set_checked(id, bool)
ui.set_class(id, name)              ui.set_visible(id, bool)        ui.key(id, name)
ui.find(name) -> id
event ui.signal { widget: u64, kind: u32, value: f32, text: str }   ; delivered as a script event
```

- A mod gets its own root per layer it creates (`mod:<mod>/<layer>`). It can only find, change or
  remove nodes it owns. A grant limits the number of nodes per mod (default 256).
- Mod unload removes the mod's nodes and bindings; reload rebuilds them through the mod's `init`.

## 10. Input capture

```rust
pub type UiCapture = efeengine_core::input::InputCapture;   // { pointer: bool, keyboard: bool }
```

- Written every frame by the UI plugin into core's `InputState::set_capture` (at
  `order::INPUT + 10`; there is no separate UI resource, so there is one source of truth). It is
  the backend's `capture()` OR a visible modal layer OR "the backend answered `Filter::Captured`
  for a pointer / key event this frame". Pointer events (mouse, wheel, touch) count into
  `pointer`, key and text events into `keyboard`. The backend's own `capture()` means: `pointer` when the pointer is over a widget or a modal layer
  is visible, `keyboard` when a text input has focus (or a modal layer is visible).
- `InputState` gains filtered helpers (`cx.game_input()`) that return "not pressed" for captured
  devices. Raw input stays available for systems that want it.
- **Native** backends see raw `InputEvent`s before the game systems (a core hook, section 12) and
  answer captured or not per event.
- **Web:** DOM events on UI elements do not reach the canvas (the `#ui` element is a sibling).
  One delegated listener per event type sits on the `#ui` element and finds the widget through the
  `data-efui-id` attribute (`WidgetId::to_bits`): `click` on a button gives `Click`, checkbox
  `change` and range / text `input` give `Changed`, Enter in a text input gives `Submit`,
  `focusin` / `focusout` give `Focus`, `pointerover` / `pointerout` give `Hover` for nodes with
  `hover_signals`. `capture().pointer` is "the pointer is over something inside `#ui`" (tracked
  from `pointerover` / `pointerout`, confirmed with `:hover`, cleared when a touch ends);
  `capture().keyboard` is "`document.activeElement` is an input or button inside `#ui`".
  A mouse click on a button or checkbox blurs it again so the game keeps its keys (a keyboard
  activation keeps the focus for keyboard navigation).
  `web_backend.rs` listens for keys on `window`: its key, text and composition handling now ignores
  events whose target is an editable element or sits inside an element with `data-efui-layer` (the
  `#ui` element), so UI typing and Space / Enter on a focused UI button never reach the game. The
  window `blur` / `focus` reset only reacts to the window (element focus changes do not fire it),
  so moving focus from the canvas into the UI needs no special case.

## 11. Text, fonts and images

- **Fonts:** `FontId` registry in the UI plugin. `ui.load_font(name, bytes)`. Web: `FontFace` from
  the bytes. iced: the bytes go into iced's global font system (what `font::load` writes to; the
  backend has no iced runtime to run the task) and `Style.font` becomes the iced font family named
  inside the file, `bold` the bold weight. Fallback: cosmic-text font database. `efeengine-ui`
  bundles Inter Regular and Bold (SIL OFL, `crates/ui/assets/`, listed in `THIRD_PARTY_NOTICES.md`) as
  the native default; the web default is the browser's sans-serif unless the game loads a font.
- **Text input on native** needs IME composition events. The engine's `InputEventKind` has
  `Text(String)` and `TextEditing { text, cursor, len }` (IME preedit, from SDL3), and
  `cx.commands.set_text_input_area(x, y, w, h, cursor)` places the IME popup next to the field.
- **Images** reference engine `TextureHandle`s. Web: the backend draws the decoded RGBA into a
  `<canvas>` (`putImageData`) when the node is created or its kind changes. A texture replaced later
  in `Textures` refreshes every backend the same way: the plugin remembers the image (`Arc`) each
  texture had when it was last handed over and, when it differs, adds the new image to
  `UiBackendCx.images` and a synthesized `UiChange::Kind(node)` for each `Image` node showing it to
  that `sync`'s changes (the tree's journal is untouched). iced: `image::Handle::from_rgba`
  (a new handle per replaced image). Fallback:
  its own GPU texture table fed by the same `TextureChanges` the game renderer gets.
- **World-space UI** (labels and bars over entities) is out of v1: on the web HTML always draws above
  the canvas, so these would be positioned from the camera each frame or drawn by the game renderer.

## 12. Engine integration (changes in efeengine-core)

1. **Overlay render pass (done):** `engine.add_render_overlay(impl RenderOverlay)`; trait
   `RenderOverlay { init(&Gpu), resize(&Gpu), render(&Gpu, &wgpu::TextureView) }` renders into the same
   surface texture after `Renderer::render`, before present, in add order (`LoadOp::Load`). `resize` is
   also called when the scale factor changes; `Gpu` has `scale_factor: f32`. Not called headless.
2. **Input pre-filter (done):** `engine.add_input_filter(impl InputFilter)` (`filter(&mut self, &InputEvent)
   -> Filter::{Pass, Captured}`), every filter is called for each raw `InputEvent` in add order before
   `InputState::apply`; any `Captured` sets `InputEvent::captured`. Captured events still reach
   `InputState`; `game_input()` ignores them. A key / button belongs to whoever got its press (repeats do
   not change that) and the release follows the owner, so a captured press never becomes a game press
   and a game press always gets its release.
3. **Main-thread backend slot:** the backend holds non-`Send` objects (web-sys elements, iced
   renderer). It lives in the engine next to the renderer, not in resources, and is driven from the
   frame: `sync` after `PostUpdate`, `render` in the overlay pass, input in the pre-filter.
4. **IME and text input area (done):** `InputEventKind::TextEditing { text, cursor, len }` (SDL3, web
   composition events) and `cx.commands.set_text_input_area(x, y, w, h, cursor)` (logical px) →
   `WindowManager::set_text_input_area` (SDL `SDL_SetTextInputArea`, no-op on the web).
5. **Web (done):** a `uiLayerId` in the startup config (default `"ui"`; the backend creates the
   element in the canvas's parent when missing), and the template's `#ui` element above the canvas
   inside `#stage` (so fullscreen includes it); key handling in `web_backend.rs` as in section 10.

## 13. Backend trait

```rust
pub trait UiBackend: 'static {
    fn init(&mut self, cx: &mut UiBackendCx);                          // GPU/DOM resources
    fn input(&mut self, ev: &InputEvent) -> Filter;                    // native only; web: no-op (no tree: the backend keeps its own copy)
    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], theme: &Theme, cx: &mut UiBackendCx);
    fn drain_signals(&mut self, out: &mut Vec<UiSignal>);
    fn capture(&self) -> UiCapture;
    fn gpu_init(&mut self, gpu: &Gpu) {}                               // native only (overlay init)
    fn gpu_resize(&mut self, gpu: &Gpu) {}                             // native only (surface size / scale)
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView) {}   // native only, LoadOp::Load
    fn take_text_input(&mut self) -> Option<Option<TextInputArea>> { None }  // Some(Some(area)): a text field has focus
    fn take_cursor_icon(&mut self) -> Option<CursorIcon> { None }    // native: mouse cursor the UI wants (only when it changes)
    fn resize(&mut self, logical: (f32, f32), scale: f32) {}
}

pub enum UiChange {
    Created(WidgetId), Removed(WidgetId), Moved { id: WidgetId, parent: WidgetId, index: u32 },
    Kind(WidgetId), Style(WidgetId), Classes(WidgetId), Visible(WidgetId), Enabled(WidgetId),
    HoverSignals(WidgetId), Layer(WidgetId), Theme, Font(FontId),
}
```

- Placement rule: a backend applies the journal against the FINAL tree of the frame. `Moved` is
  coalesced when the frame's journal is taken: one entry per node, with its final parent and final
  index (`tree.children(parent)[index] == id`); it is not a script of intermediate moves. `Created`
  nodes sit where the tree has them. A backend may simply order the children of every parent named
  by a `Created` or `Moved` entry like `tree.children(parent)`. The headless backend checks the index.
- The journal names what changed, the backend reads the new state from the tree. A frame with no
  changes costs nothing in `sync`.
- `UiBackendCx.images` carries the engine RGBA images of the textures that `Image` nodes created
  or changed in this `sync` use, or that were replaced in `Textures` since the last hand-over
  (filled by the plugin from `Textures`; for replaced textures the `changes` slice also holds one
  synthesized `UiChange::Kind` per node that shows the texture).
- The plugin installs one engine `InputFilter` (calls `backend.input`, answers `Filter::Captured` / `Pass`) and one `RenderOverlay` (`gpu_init`, `gpu_resize`, `render`). The backend never calls back into the engine: signals and the text-input request are queued and the plugin's `ui_signals` system hands them over (`cx.commands.set_text_input(bool)`, `set_text_input_area`). The filter uses `try_borrow_mut`, never a blocking borrow of the shared backend.
- `UiPlugin::new()` picks the backend: web on wasm32 (`UiPlugin::headless()` forces headless; `UiTest` uses it), iced on native when the default `iced` cargo feature is on and the engine is not headless (`EngineConfig::headless`), the fallback with the
  `ui-fallback` feature, headless when there is no window (tests, servers).

## 14. Testing (headless backend)

```rust
let mut t = UiTest::new(|engine| setup_menu(engine));
t.click("menu/play");
t.set_value("menu/volume", 0.3);
t.step(1);
assert!(t.engine.resource::<GameState>().started);
assert_eq!(t.text("hud/score"), "Score: 0");
```

The headless backend keeps no widgets: it records the journal, checks it is consistent (no change
for a removed id, parents exist), and injects signals. Every backend shares a conformance test list
(tree operations, theme resolution, value widgets, capture rules); web and iced run the parts that
need a browser or GPU in their own smoke tests.

## 15. Threading

- `UiTree`, `UiBindings` and `Events<UiSignal>` are resources: systems declare them in `Access` like
  any other resource and may run on the pool.
- Backends run on the main thread only (DOM access, the overlay pass). Systems never call a backend.

## 16. View layer (Elm-style)

The recommended API for Rust game code. It is built entirely on the retained tree: backends do not
know it exists.

### 16.1 Shape

```rust
use efeengine_ui::view::*;
use efeengine_ui::{column, row, Layer};   // import the macros explicitly: `column!` also exists in std

#[derive(Clone, Debug)]
enum MenuMsg { Play, Volume(f32), Name(String), Fullscreen(bool) }

fn menu_view(s: &MenuState) -> El<MenuMsg> {
    column![
        text("Coin Dash").class("title"),
        button("Play").key("play").on_click(MenuMsg::Play),
        slider(0.0..=1.0, s.volume).step(0.05).key("volume").on_change(MenuMsg::Volume),
        checkbox("Fullscreen", s.fullscreen).key("fullscreen").on_toggle(MenuMsg::Fullscreen),
        text_input(&s.name).placeholder("Player name").max_len(16).key("name").on_input(MenuMsg::Name),
        text(format!("Best: {}", s.best)),
    ]
    .class("panel")
    .gap(12.0)
}

// One view = one layer, built from one state resource (UiPlugin must be added first).
engine.add_plugin(UiView::new("menu", menu_view).layer(Layer::centered(10)))?;

// update = a normal system reading the typed messages.
engine.add_system(Stage::Update, |cx: &mut Ctx| {
    let msgs: Vec<MenuMsg> = cx.events::<MenuMsg>().iter().cloned().collect();
    for m in msgs {
        let s = cx.resources.get_mut::<MenuState>().unwrap();
        match m {
            MenuMsg::Play => s.start = true,
            MenuMsg::Volume(v) => s.volume = v,
            MenuMsg::Name(n) => s.name = n,
            MenuMsg::Fullscreen(f) => s.fullscreen = f,
        }
    }
});
```

This example is a test (`crates/ui/tests/view.rs`, module `api_example`).

- `El<Msg>` is a lightweight description: node kind data, classes, inline style (every `Style`
  setter chained: `.gap(12.0)`, `.width(..)`, ...), key, `visible` / `enabled`, children, and the
  message handlers `on_click(Msg)`, `on_change(f)` (slider), `on_toggle(f)` (checkbox),
  `on_input(f)` and `on_submit(f)` (text input), `on_focus(f)`. The `f` handlers are closures
  `impl Fn(X) -> Msg + Send + Sync + 'static`, so they may capture (`move |v| Msg::Pick(id, v)`).
  Kind data: `text_input(..).placeholder/.max_len/.password`, `slider(..).step`.
- Constructors (module `efeengine_ui::view`): `text`, `button(label)`, `button_with(children)`,
  `checkbox(label, checked)`, `slider(range, value)`, `text_input(value)`, `image(texture, fit)`,
  `container()`, `row()`, `column()`, `scroll()`, `slot(key)`. Children: `.child(x)` /
  `.children(x)` where `x` is an `El` or anything iterable of `El` (`Vec`, `Option`, `map(..)`);
  the macros `column![..]` / `row![..]` take a mix of both: `column![title, items.iter().map(row_view), footer]`.
- `Msg: Clone + Send + Sync + 'static`. Messages are delivered as `Events<Msg>`, registered by
  `UiView` (no `add_event` needed). A typed message that has no handler is simply never sent.
  `El::map(f)` converts the message type (Elm's `Html.map`) for embedding a component view.
- The state is any resource (`UiView::new` takes `Fn(&S) -> El<Msg>` for resource `S`; while `S`
  is missing the view does nothing). For views over several resources or queries,
  `UiView::with(|cx: &ViewCx| -> El<Msg>)` gets `cx.resources`, `cx.world`, `cx.time` and `cx.res::<T>()`.
- Builder: `.layer(Layer)` (default `Layer::fill(0)`), `.when(RunCondition)`, `.enabled(bool)`.
  The plugin is named `efeengine-ui/view/<name>` (so a view name can be used once), depends on
  `efeengine-ui`, the layer key is `<name>`, and every node of the view is owned by
  `Owner::Plugin("efeengine-ui/view/<name>")`.
- No bindings are needed: the view reads the state directly. Bindings (section 8) remain for the
  retained API.
- Type inference: a view without any handler has no `Msg` to infer; write `UiView::<()>::new(..)`
  or annotate the function's return type `El<()>`.

### 16.2 Reconciler

Each time the view runs (see 16.3), the reconciler compares the new `El` tree with the
retained tree itself (not with the previous `El`, so edits made behind the view's back are
overwritten and a value the plugin wrote back from a signal counts as current), node by node, and
turns the differences into retained-tree operations:

| Difference | Tree operation |
|---|---|
| Same position (or same key), same kind | update only changed properties (kind data such as text, value, placeholder; classes, style, enabled, visible) |
| Different kind | remove the old node (with its subtree), create the new one |
| New child / missing child | create / remove |
| Keyed child moved inside its parent | `move_to` (the node keeps its id, so focus, cursor, scroll offset survive) |
| Keyed child now under another parent | remove + create (a new id) |

- **Identity:** children are matched by `.key(..)` if they have one, otherwise by position (an
  unkeyed element only matches an unkeyed node at the same index). Lists whose items can be
  inserted, removed or reordered must use keys (inventory rows, chat lines). Reordering needs at
  most one `move_to` per out-of-place child.
- **Key scheme:** a keyed element becomes the tree key `<view>/<k1>/<k2>/...`: the keys of its keyed
  ancestors inside the view (outermost first) then its own, joined with `/`; unkeyed elements
  contribute nothing. `column![button("Play").key("play")]` in the view `menu` is `menu/play`,
  at any depth; a row `.key("item-7")` holding a button `.key("buy")` gives `menu/item-7/buy`. The
  layer root has the key `<view>`. Two keyed elements that would get the same full key make the
  view panic before anything is applied (keys must be unique among the keyed elements with the same
  nearest keyed ancestor).
- **Cost:** proportional to the size of the view (plus the tree children it visits), tree
  operations only for what changed. A menu of a few hundred nodes is cheap to rebuild every frame.
  A run with no differences writes no journal entry (tested), so backends do no work.
- **Messages:** the plugin keeps a table widget id -> handlers, replaced on every run. At
  `order::INPUT + 11` in `PreUpdate` (right after the `UiSignal`s become readable at `INPUT + 10`)
  every `UiSignal` for a widget in the table becomes a `Msg` (`Click` -> the `on_click` message,
  `Changed(F32(v))` -> `on_change(v)`, `Changed(Bool)` -> `on_toggle`, `Changed(Text)` -> `on_input`,
  `Submit` -> `on_submit`, `Focus` -> `on_focus`; `Hover` has none) sent with `Ctx::send_batch_now`,
  so `Update` systems read them in the same frame. Widgets without a handler send nothing. A frame
  that delivered a message of the view runs the view even under `.when(..)`.
- **Value widgets:** as in section 7, the backend shows the edit immediately, the plugin writes it
  into the tree and the message is sent. The view returns the value from the state on its next run
  (the same frame, in `PostUpdate`): if `update` stored it, it equals the tree value and nothing is
  written; if `update` clamped or rejected it, the reconciler writes the corrected value. This is a
  controlled input: a text input or slider without a handler snaps back to the view's value.
  A text value longer than `max_len` is cut when the `El` is built.
- **Slots:** `slot(key)` is an empty container (key required, no children allowed) whose content
  the reconciler never touches; its own properties (classes, style, ...) still follow the `El`.
  Children are managed through the retained API (for example a mod panel inside the game's pause
  menu) and survive reruns; removing the slot removes them with it.

### 16.3 When the view runs

- Default: every frame, in `PostUpdate` at `order::LATE + 11` (after game systems changed the
  state, after the plugin's bindings at `LATE + 10` so the order does not depend on registration
  order, before the plugin's sync at `LATE + 20`).
- `UiView::new(..).when(changed::<MenuState>())` runs it only when the resource changed since the
  view last ran (core's resource change ticks: `ResMut` derefs, `Resources::get_mut` / `insert`).
  `.when` may be repeated, or conditions combined with `changed::<A>().or(changed::<B>())`
  (`RunCondition::custom(|cx| ..)` for anything else); the view runs when any holds. It also runs
  on its first frame, after being enabled, and after delivering a message.
- `UiView::new(..).enabled(false)` / `UiViews::set_enabled(name, bool)` (the `UiViews` resource,
  inserted by the first view: `cx.resources.get_mut::<UiViews>().unwrap().set_enabled("pause", true)`)
  hides the layer (`Visible` change) and stops running the view and delivering its messages. A
  view disabled from the start creates nothing. Enabling shows the layer and rebuilds at once.
- A view that panics is a bug in game code like any system panic; the description is built
  completely before the first tree operation, so nothing from a half-built tree is applied.

### 16.4 Mixing layers

- A view owns its layer and every node in it: retained-API changes to those nodes are overwritten on
  the next run (also stray children added under view nodes). To combine both, put the retained or
  script-owned UI in another layer, or use a `slot(key)` element.
- Tests use the same headless backend: `t.click("menu/play")` finds the node by its reconciled key
  and the test then reads `Events<MenuMsg>` or the state.

### 16.5 Why not an Elm runtime on the web

The diff runs once in Rust for every backend. The web backend only applies the journal to DOM
elements it already created, so no virtual DOM or Elm/JS framework is needed in the page, and the
web and native backends stay identical in behaviour. iced on native rebuilds its own `Element` tree
from the retained tree when the journal is not empty, which matches iced's own model.

### 16.6 Scripts and mods

Scripts and mods keep the retained API (section 9). Calling a VM `view` function every frame and
moving a whole tree across the host boundary would cost more than direct node operations, and grant
checks per node are simpler on the retained API. A script-side view layer can be added later on top
of the same host functions if it proves useful.

## 17. GPU views

A **GPU view** is a UI node whose contents the game draws with wgpu: minimaps, 3D model previews,
live portraits, custom-shader panels. It is laid out, clipped, scrolled and covered by other UI like
any node. It is not a window: windows (several top-level surfaces with their own input and cameras)
are a separate, later engine API that reuses the same core surface layer (section 17.4).

### 17.1 API (the same on every backend)

```rust
pub trait UiPainter: 'static {
    /// The view's size changed (physical px), or the target format is new: recreate
    /// size- and format-dependent resources (depth buffers, pipelines).
    fn resize(&mut self, _gpu: &Gpu, _target: &ViewTarget) {}
    /// Draw the view. `target` is the view's own texture/surface: origin (0,0) is its top-left
    /// corner and it is exactly `target.size` big. The painter owns these pixels (clear or load).
    fn paint(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, target: &ViewTarget);
}

pub struct ViewTarget<'a> {
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),            // physical px
    pub scale: f32,                  // window scale factor
    pub widget: WidgetId,            // which node (one painter may serve several nodes)
}

engine.add_ui_painter("minimap", Minimap::new());           // name -> painter, main thread
ui.gpu_view(panel, "minimap").width(Size::Px(200.0)).height(Size::Px(200.0));   // retained
gpu_view("minimap").on_pointer(Msg::Map)                     // view layer
```

- `NodeKind::GpuView { painter: Arc<str>, pointer: bool }`, sized and styled like `Image`.
- Pointer input: with `.pointer_signals()` (view layer: `on_pointer`), the node sends
  `UiSignalKind::Pointer { pos: (f32, f32), buttons: u8, wheel: (f32, f32) }` in view-relative logical
  px, plus normal `Hover`. A captured pointer over the view is UI-owned input like any widget.
- Registration: the engine is a core type, so `add_ui_painter` is the extension trait method
  `efeengine_ui::EngineUiExt::add_ui_painter(&mut self, name, impl UiPainter) -> &mut Self`
  (before or after `UiPlugin`). Backends reach painters through the `UiPainters` handle that
  `UiBackend::set_painters` hands them after `init` (an `Rc` registry; clones share it).
- Pointer: `UiSignalKind::Pointer { pos, buttons, wheel }` as above; the view layer's
  `.on_pointer(fn(PointerEvent) -> Msg)` (`PointerEvent { pos, buttons, wheel }`, `LEFT`/`RIGHT`/`MIDDLE`
  masks) also turns the `pointer` flag on. Events: move inside the view, button press / release
  (a release after leaving the view still arrives), wheel.
- Size: like `Image`, an `Auto` size is empty and nothing is painted; give the view a size or `Fill`. (Web: an Auto axis is pinned to `0px` so the canvas cannot grow from its own backing size.)
- A painter name with no registered painter: the view stays transparent and a warning is logged once.

### 17.2 Contract every backend meets

| Point | Rule |
|---|---|
| Target | Always the view's own target, sized to the view, origin top-left. No viewport/scissor math in painters. |
| Format | `ViewTarget.format` is fixed for the run and is **always an sRGB format** (`Rgba8UnormSrgb` / `Bgra8UnormSrgb`) on every backend. Painters build pipelines for it (in `resize`) and **output linear colour**: the hardware encodes on write. Convert sRGB palette values to linear (in the shader or on the CPU); an sRGB byte value `v` is shown exactly as `v`. |
| Alpha | Contents are **premultiplied alpha**. Transparent pixels show the UI behind the view. |
| Timing | `paint` runs at most once per frame per shown node, after the game renderer, in that frame. Never for hidden nodes; it may run for a node scrolled out of sight. No order between views is promised. |
| Size | Physical px; `resize` is called before the first `paint` and on every size or scale change. Zero-size views are not painted. |
| Limits | The device's limits apply (WebGL2: no compute, no storage buffers); `gpu.limits()` tells the painter. |
| No wgpu | Headless: never painted, layout only. SDL render mode (no wgpu): never painted, warning once. |

Native (iced) as built: `ViewTarget.format` is always `Rgba8UnormSrgb` (not the window surface's format); the view's texture is `round(logical size * scale)` physical px. Painters also
run on frames where the rest of the UI is unchanged (they animate), once per frame per view.
Draw order follows iced's layers: a view is above the quads of its own layer, below text and images of that layer.

### 17.3 Per backend

| Backend | Implementation |
|---|---|
| Native (iced) | iced's `shader` widget: the painter draws into an offscreen texture the size of the view, which the widget composites (premultiplied) in UI order, clipped. |
| Web, WebGPU | Each GPU view node is its own `<canvas>` in the DOM tree. The painter renders to that canvas's surface on the shared device (no copies; DOM z-order and clipping). |
| Web, WebGL2 | The GL context cannot be shared between canvases, so each view canvas gets a 2D context and core's copy path feeds it: the painter draws into an internal `Rgba8UnormSrgb` texture, `present` reads it back (buffer map) and `putImageData`s the pixels, un-premultiplied in linear space (decode, divide by alpha, encode; opaque pixels unchanged). One frame of latency; costs a GPU->CPU readback plus a CPU un-premultiply per view per frame, so keep GPU views small on WebGL2. |

Web as built: `WebBackend::render` (the engine's render overlay, which runs on wasm too) paints the views in the DOM backend's `paint_gpu_views`; it never draws into the main target. Per shown view and frame: physical size = round(`clientWidth/clientHeight` * `devicePixelRatio`); zero size (including `display: none` ancestors) is skipped; `ExtraSurface::for_canvas` is created at the first paint; `resize` (surface, then painter) on a size, scale or format change; then `acquire`, own command encoder, `paint`, submit, `present`. Missing painter: one console warning per name, canvas transparent. The surface goes away with the node. Pointer events use `offsetX/offsetY` and the DOM `buttons` mask (equal to `PointerEvent::LEFT/RIGHT/MIDDLE`); wheel deltas are negated like iced's. Verified: WebGL2 path in headless Firefox (shape pixels, transparent corners, click recolors); the WebGPU path is compile-checked only.

### 17.4 Core: extra surfaces

Core exposes `efeengine_core::render::ExtraSurface` (not a `SurfaceSet`: the caller owns the surfaces):
extra drawing surfaces on the engine's shared device, one per web GPU-view canvas now, one per window
later (`Gpu` carries `instance` and `backend` for this).

```rust
ExtraSurface::for_canvas(&gpu, canvas) -> Result<Self, String>   // wasm32 only
surface.format()                       // fixed for the run
surface.resize(&gpu, (w, h))           // physical px; sets canvas backing size; 0 = not drawn
surface.acquire(&gpu) -> Option<SurfaceFrame>   // { view, format, size }; None = skip this frame
surface.present(&gpu, frame)           // after submitting the commands that drew into frame.view
```

- WebGPU: a real surface on the shared device (storage format: the main surface's if supported, else the first
  non-sRGB, configured with `view_formats` = its sRGB variant; frames are handed out as an sRGB view and `format()` reports the sRGB format; alpha `PreMultiplied`, else `Inherit`, else the first mode the canvas offers: without
  premultiplied support transparent pixels may composite opaque).
- WebGL2: copy path as in 17.3 (format `Rgba8UnormSrgb`). The readback is double-gated: if the previous one
  has not completed when a frame is presented, that frame is re-read at the next `acquire`, so the last
  drawn content always arrives. The path is logged once. Details in `crates/core/structure.md`.
- The iced backend does not need it (textures). `Engine::create_window` will use the same layer later
  (`ExtraSurface::from_surface` is the native hook).

## 18. Open questions

- Gamepad and keyboard focus navigation (d-pad moves focus to the nearest widget): wanted for games,
  native backends can do it, the web needs a small JS focus manager. Proposed for v2 with a
  `FocusNav` signal.
- Data-driven UI files (`.efui`, built into the tree by keys): after the API is stable.
- Localization: text through a `tr("menu.play")` binding once the engine has a string table.
- Animations and transitions: CSS has them, iced and the fallback would need their own; v2.
