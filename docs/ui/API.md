# efeengine-ui: the common UI API

> **Status:** Design, not implemented\
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
- A visible `modal` layer captures all pointer and keyboard input below it, even outside its
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
- **Web:** a node's inline `Style` is written to its element's `style` (one CSS property per set
  field, mapped by the table below); kinds, classes and state styles come from the generated theme
  stylesheet (section 6). A style change writes only the changed properties of that element.
- Left out on purpose in v1: margins (use gap and padding), wrapping, absolute positioning inside
  layers, per-corner radius, shadows, transforms, animations. Each can be added when all three
  backends support it.

| Property | CSS | iced | taffy / fallback |
|---|---|---|---|
| direction | `flex-direction` | `row` / `column` | `flex_direction` |
| justify, align | `justify-content`, `align-items` | `align_x` / `align_y` on the container | `justify_content`, `align_items` |
| gap, padding | `gap`, `padding` | `spacing`, `padding` | `gap`, `padding` |
| width/height Fill, Portion | `flex-grow` | `Length::Fill`, `FillPortion` | `flex_grow` |
| min/max | `min-width` ... | `min_width` / `max_width` (check heights in the spike) | `min_size`, `max_size` |
| background, border, radius | `background`, `border`, `border-radius` | `container::Style` | quad pipeline |
| text color, size, bold, align | `color`, `font-size`, `font-weight`, `text-align` | `text` style | glyphon attrs |
| opacity | `opacity` | per-widget color alpha (check in the spike) | multiply alpha |

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
  state style (hovered/pressed/focused/disabled) of each of those on top.
- The engine ships a default theme (dark, readable at 1x and 2x). `UiTree::set_theme` replaces it;
  the change is one journal entry and backends restyle everything.
- **Web:** the theme becomes one generated stylesheet: `.efui-k-button`, `.efui-c-panel`,
  `:hover`, `:active`, `:focus-visible`, `[disabled]`. The template ships the base reset CSS. A game
  may add its own CSS file on the web as an escape hatch; it is documented as web-only.
- **iced:** the theme becomes style functions per kind and class (`button::Style` for each state).
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
  `order::INPUT + 10` in `PreUpdate` and fills the `UiSignals` resource (a readable-only
  `Events<UiSignal>`; core's `Ctx::send` is only readable next frame). Read it with `cx.ui_signals()`
  (trait `UiCtxExt`, returns a copy).
- Disabled and invisible widgets send nothing.
- Signals are data. The tree stores no callbacks (rule from the roadmap); helpers make matching short:

```rust
fn menu(cx: &mut Ctx) {
    for s in cx.ui_signals() {
        match s.key_kind() {
            ("menu/play", UiSignalKind::Click) => cx.send(StartGame),
            ("menu/volume", UiSignalKind::Changed(UiValue::F32(v))) => cx.res_mut::<Audio>().volume = *v,
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
pub struct UiCapture { pub pointer: bool, pub keyboard: bool }
```

- Updated each frame by the backend: `pointer` when the pointer is over a widget or a modal layer
  is visible, `keyboard` when a text input has focus (or a modal layer is visible).
- `InputState` gains filtered helpers (`cx.game_input()`) that return "not pressed" for captured
  devices. Raw input stays available for systems that want it.
- **Native** backends see raw `InputEvent`s before the game systems (a core hook, section 12) and
  answer captured or not per event.
- **Web:** DOM events on UI elements do not reach the canvas. The backend sets `pointer` from
  `pointerenter`/`pointerleave` on widgets and `keyboard` from focus. Moving focus from the canvas
  into the UI must not trigger the engine's focus-loss input reset (`web_backend.rs`): the web
  backend tells the window backend which elements belong to the UI.

## 11. Text, fonts and images

- **Fonts:** `FontId` registry in the UI plugin. `ui.load_font(name, bytes)`. Web: `FontFace` from
  the bytes. iced: `font::load`. Fallback: cosmic-text font database. `efeengine-ui` bundles one OFL
  font (listed in `THIRD_PARTY_NOTICES.md`) as the native default; the web default is the browser's
  sans-serif unless the game loads a font.
- **Text input on native** needs IME composition events. The engine's `InputEventKind` has
  `Text(String)` but no composition event yet: add `TextEditing { text, cursor, len }` from SDL3, and
  a `set_text_input_area(rect)` on the window trait so IME popups appear next to the field.
- **Images** reference engine `TextureHandle`s. Web: the backend turns the decoded RGBA into an
  `ImageBitmap` / object URL once per texture version. iced: `image::Handle::from_rgba`. Fallback:
  its own GPU texture table fed by the same `TextureChanges` the game renderer gets.
- **World-space UI** (labels and bars over entities) is out of v1: on the web HTML always draws above
  the canvas, so these would be positioned from the camera each frame or drawn by the game renderer.

## 12. Engine integration (changes in efeengine-core)

1. **Overlay render pass:** `Renderer` is a single slot today. Add an overlay list that renders into
   the same surface texture after the main renderer, before present:
   `engine.add_render_overlay(impl RenderOverlay)` with `init`, `resize`, `render(gpu, target, frame)`.
2. **Input pre-filter:** `engine.set_input_filter(impl InputFilter)`, called for each raw
   `InputEvent` before `InputState::apply`, returning `Captured` / `Pass`. Captured events still reach
   `InputState` but are flagged, which the filtered helpers use.
3. **Main-thread backend slot:** the backend holds non-`Send` objects (web-sys elements, iced
   renderer). It lives in the engine next to the renderer, not in resources, and is driven from the
   frame: `sync` after `PostUpdate`, `render` in the overlay pass, input in the pre-filter.
4. **IME and text input area** on the window trait (section 11).
5. **Web:** a `uiLayerId` in the startup config (default `"ui"`), and the template's `#ui` element
   above the canvas inside `#stage` (so fullscreen includes it).

## 13. Backend trait

```rust
pub trait UiBackend: 'static {
    fn init(&mut self, cx: &mut UiBackendCx);                          // GPU/DOM resources
    fn input(&mut self, ev: &InputEvent, tree: &UiTree) -> Filter;     // native only; web: no-op
    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], theme: &Theme, cx: &mut UiBackendCx);
    fn drain_signals(&mut self, out: &mut Vec<UiSignal>);
    fn capture(&self) -> UiCapture;
    fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView) {}   // native only
    fn resize(&mut self, logical: (f32, f32), scale: f32) {}
}

pub enum UiChange {
    Created(WidgetId), Removed(WidgetId), Moved { id: WidgetId, parent: WidgetId, index: u32 },
    Kind(WidgetId), Style(WidgetId), Classes(WidgetId), Visible(WidgetId), Enabled(WidgetId),
    Layer(WidgetId), Theme, Font(FontId),
}
```

- The journal names what changed, the backend reads the new state from the tree. A frame with no
  changes costs nothing in `sync`.
- `UiPlugin` picks the backend: web on wasm, iced on native by default, the fallback with the
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
#[derive(Clone, Debug)]
enum MenuMsg { Play, Volume(f32), Name(String), Fullscreen(bool) }

fn menu_view(s: &MenuState) -> El<MenuMsg> {
    column![
        text("Coin Dash").class("title"),
        button("Play").on_click(MenuMsg::Play),
        slider(0.0..=1.0, s.volume).step(0.05).on_change(MenuMsg::Volume),
        checkbox("Fullscreen", s.fullscreen).on_toggle(MenuMsg::Fullscreen),
        text_input(&s.name).placeholder("Player name").max_len(16).on_input(MenuMsg::Name),
        text(format!("Best: {}", s.best)),
    ]
    .class("panel")
    .gap(12.0)
}

// One view = one layer, built from one state resource.
engine.add_plugin(UiView::new("menu", menu_view).layer(Layer::centered(10)));

// update = a normal system reading the typed messages.
engine.add_system(Stage::Update, |cx: &mut Ctx| {
    for m in cx.events::<MenuMsg>() {
        let s = cx.res_mut::<MenuState>();
        match m {
            MenuMsg::Play => s.start = true,
            MenuMsg::Volume(v) => s.volume = *v,
            MenuMsg::Name(n) => s.name = n.clone(),
            MenuMsg::Fullscreen(f) => s.fullscreen = *f,
        }
    }
});
```

- `El<Msg>` is a lightweight description: node kind, classes, inline style, key, children, and the
  message constructors (`on_click(Msg)`, `on_change(fn(f32) -> Msg)`, `on_input(fn(String) -> Msg)`,
  `on_submit`, `on_toggle`, `on_focus`). It is the same node set and `Style` as sections 4 and 5.
- `Msg: Clone + Send + Sync + 'static`. Messages are delivered as `Events<Msg>`, registered by
  `UiView` (no `add_event` needed).
- The state is any resource (`UiView::new` takes `fn(&S) -> El<Msg>` for resource `S`). For views
  over several resources or queries, `UiView::with(|cx: &ViewCx| -> El<Msg>)` gets read access to
  resources and the world.
- No bindings are needed: the view reads the state directly. Bindings (section 8) remain for the
  retained API.

### 16.2 Reconciler

Each frame the view runs (see 16.3), the reconciler compares the new `El` tree with the previous
one, node by node, and turns the differences into retained-tree operations:

| Difference | Tree operation |
|---|---|
| Same position (or same key), same kind | update only changed properties (text, value, classes, style, enabled, visible) |
| Different kind | remove the old node, create the new one |
| New child / missing child | create / remove |
| Keyed child moved | `move_to` (the node keeps its id, so focus, cursor, scroll offset survive) |

- **Identity:** children are matched by `.key(..)` if they have one, otherwise by position. Lists
  whose items can be inserted, removed or reordered must use keys (inventory rows, chat lines).
  Keys only need to be unique among siblings in the view layer; the reconciler turns them into
  tree-wide keys under the view's layer (`menu/<path>`), so scripts and tests can still find nodes.
- **Cost:** proportional to the size of the view, plus tree operations only for what changed. A
  menu of a few hundred nodes is cheap to rebuild every frame. The journal stays empty when nothing
  changed, so backends do no work.
- **Messages:** the reconciler keeps a table widget id → message constructor, replaced on every
  run. A `UiSignal` for a widget in the table becomes a `Msg` (`Click` → the `on_click` message,
  `Changed(F32(v))` → `on_change(v)`, ...) and is sent as `Events<Msg>` in the same frame
  (`order::INPUT + 10`, like signals). Widgets without a handler send nothing.
- **Value widgets:** as in section 7, the backend shows the edit immediately and the message is sent.
  The view returns the value from the state on its next run: if `update` stored it, nothing changes;
  if `update` clamped or rejected it, the reconciler writes the corrected value.

### 16.3 When the view runs

- Default: every frame, in `PostUpdate` at `order::LATE + 10` (after game systems changed the
  state, before render extract).
- `UiView::new(..).when(changed::<MenuState>())` runs it only when the resource changed. This needs
  change ticks on resources (the efecs fork has them for components only): the UI work adds a
  `changed` tick to resources written through `res_mut` / `Access` write.
- `UiView::new(..).enabled(false)` / `set_enabled` hides the layer and stops running the view
  (pause menus that exist only while paused).
- A view that panics is a bug in game code like any system panic; the reconciler applies nothing
  from a half-built tree.

### 16.4 Mixing layers

- A view owns its layer and every node in it: retained-API changes to those nodes are overwritten on
  the next run. To combine both, put the retained or script-owned UI in another layer, or give the
  view a `slot(key)` element: an empty container the reconciler never touches, whose children are
  managed through the retained API (for example a mod panel inside the game's pause menu).
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

## 17. Open questions

- Gamepad and keyboard focus navigation (d-pad moves focus to the nearest widget): wanted for games,
  native backends can do it, the web needs a small JS focus manager. Proposed for v2 with a
  `FocusNav` signal.
- Data-driven UI files (`.efui`, built into the tree by keys): after the API is stable.
- Localization: text through a `tr("menu.play")` binding once the engine has a string table.
- Animations and transitions: CSS has them, iced and the fallback would need their own; v2.
