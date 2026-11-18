# Building UI with efeengine-ui

A practical guide: how to put menus, settings, HUDs and dialogs into a game, who owns the input,
how scripts and mods get their own widgets, and how to replace the backend. The reference is
[`API.md`](API.md) (design, every type), the plan is [`../../ui-roadmap.md`](../../ui-roadmap.md),
the code layout is [`../../crates/ui/structure.md`](../../crates/ui/structure.md). Running examples:
`examples/coin_dash` (menus), `examples/ui_demo` (every widget, GPU view), `examples/coin_dash_mods`
(a mod that builds UI).

## 1. Set up

```rust
engine.add_plugin(UiPlugin::new())?;      // before the game loop starts
```

`UiPlugin::new()` picks the backend: iced on native (feature `iced`, on by default), HTML elements on
the web, a headless recorder when the engine has no window (tests). Your game crate forwards the
feature, like `examples/ui_demo/Cargo.toml`:

```toml
efeengine-ui = { path = "../../crates/ui", default-features = false }
[features]
default = ["iced"]
iced = ["efeengine-ui/iced"]
```

Add the plugin **before** anything exports the API index (mods and scripts are compiled against it:
the plugin registers the `ui` namespace and the `ui.signal` event, section 6). Game code only uses
`efeengine-ui` types; no iced or DOM type appears anywhere.

## 2. Two ways to build UI

Both end in the same `UiTree` resource, so they mix freely (one layer each, section 5 of API.md).

**The view layer (recommended for game UI).** A function from state to a description (`El`), run every
frame and reconciled against the tree; widgets send *messages*, a system turns them into state changes.
This is `examples/coin_dash/src/menu.rs`:

```rust
fn main_menu(_: &Menu) -> El<MenuMsg> {
    column![
        text("Coin Dash").class("title"),
        button("Play").key("play").on_click(MenuMsg::Play),
        button("Quit").key("quit").on_click(MenuMsg::Quit),
    ]
    .class("panel")
}

engine.add_plugin(UiView::new("main_menu", main_menu).layer(Layer::fill(30).modal()))?;
engine.add_system(Stage::Update, |cx: &mut Ctx| {
    for msg in cx.events::<MenuMsg>().iter().cloned().collect::<Vec<_>>() {
        match msg { MenuMsg::Play => { /* change the state */ } MenuMsg::Quit => cx.commands.quit(), .. }
    }
});
```

Show and hide views with `UiViews::set_enabled(name, bool)`; a view that was never enabled creates
nothing. Keys are `<view>/<keys of keyed ancestors>/<own key>`: `main_menu/play`. Tests and scripts
find widgets by these keys.

**The retained API.** Create nodes on the `UiTree` resource (`ui.layer`, `ui.button(parent, ..)`,
setters, `ui.remove`). Use it for dynamic things the view function would only get in the way of, and
for anything created outside the main loop. Bindings (`Bind::res(..)`) keep text and values in sync
with game state without a view.

Signals (`Click`, `Changed`, `Submit`, `Focus`, `Hover`) are data: read them with `cx.ui_signals()` in a
system, or let a view turn them into messages. The tree stores no closures.

## 3. Themes and classes

A `Theme` maps node kinds and class names to styles (normal, hovered, pressed, focused, disabled).
`UiPlugin::new().theme(my_theme)` replaces the built-in dark theme; classes used by the built-in theme
are `panel`, `title`, `muted`, `danger`. Style a node with classes first and with inline `Style` fields
(`.gap(8.0)`, `.width(Size::Fill)`, `.align(Align::Center)`) for layout. The style subset is small on
purpose: every property exists on every backend (API.md section 5 lists the per-backend notes). Fonts:
`ui.load_font(name, bytes)`, then `Style::font`. A theme change is one journal entry and restyles
everything.

## 4. Input ownership

One rule: the UI gets input first; what it uses, the game does not see.

- The plugin writes `InputState::capture()` (`pointer`, `keyboard`) every frame from the backend (pointer
  over a widget, a text input with focus) **or** a visible modal layer **or** an input event the backend
  consumed. A modal layer (`Layer::fill(z).modal()`) captures everything below it; use it for menus.
- Game systems read input through `cx.game_input()` / `ctx.game_input()` (a captured device reports nothing
  pressed). The raw `cx.input` is unfiltered: use it only for keys the UI must not swallow, like the Esc
  that closes a menu (`coin_dash` does this, otherwise pausing would work once and never unpause).
- A key or button belongs to whoever got its **press**; the release follows. So a click that opened a menu
  never becomes a game click, and a game key held while a menu opens still gets its release.
- Pausing is the game's job: capture only filters input. `coin_dash` gives its game systems
  `.run_if(menu::running)` and switches the built-in `velocity` system off with
  `cx.schedule().set_enabled("velocity", false)` while a menu is open.
- Web: DOM events on UI elements never reach the canvas; key events from inside the UI layer are ignored by
  the engine's key handling. Native: the iced backend answers `Captured` per event. The behaviour is the
  same and tested through the headless backend (`t.capture()`).

## 5. Testing

```rust
let mut t = UiTest::new(|e| { setup(e); menu::setup(e, false); });
t.step(2);
t.click("main_menu/play");
t.step(1);
assert_eq!(t.resource::<Menu>().screen, Screen::Playing);
```

`UiTest` is a headless engine with the plugin: `click`, `set_value`, `set_checked`, `type_text`, `submit`,
`hover`, `text(key)`, `capture()`, key events through `t.engine.inject(..)`. The headless backend checks the
journal is consistent on every `step`. `examples/coin_dash/tests/menu.rs` is a complete example (menus, pause
with Esc, the game frozen behind a menu, input not leaking). GPU tests (`-- --ignored`) render a tree
offscreen to a PNG: `examples/coin_dash/tests/native_screenshot.rs`.

## 6. Scripts and mods

Scripts (`.efa`) and mods build UI through host functions in the namespace `ui`. They are registered by
the plugin in the same registry as the engine's functions, so they are in the API index (`.efapi`), the
docs and the type checker like any other. Reference: `engine.api_index()` or the exported index; here is the
shape.

### Handles, status codes

A widget is an `i64` handle (positive, unique across modules). Creating functions return the handle or a
**negative status**; the others return a status (0 = ok):

| status | meaning |
|---|---|
| 1 | bad handle: unknown, removed, forged, or another module's; also `find` with no match |
| 2 | wrong widget kind (`set_text` on a slider, a child under a text) |
| 3 | the mod's node limit is reached |
| 4 | denied (changing a mount point, a modal layer for a mod) |
| 5 | bad argument (empty or taken key, unknown anchor) |

A violation never traps and never panics: the call returns the status, the first refusal of each kind per
module is logged, and `UiScripts::violations()` keeps the last 64 for tests.

### The functions

`layer(name, z, anchor, modal)`, `column(parent)`, `row(parent)`, `text(parent, text)`,
`button(parent, label)`, `checkbox(parent, label, checked)`, `slider(parent, min, max, value)`,
`text_input(parent, text)`, `image(parent, texture, fit)`, `remove(w)`, `set_key(w, key)`, `find(key)`,
`set_text`, `get_text(w, buf)`, `set_value`, `value`, `set_checked`, `checked`, `set_visible`, `set_enabled`,
`set_hover_signals`, `set_class`, `add_class`, `remove_class`, `set_size`, `set_padding`, `set_gap`,
`set_font_size`, `set_background(w, 0xRRGGBBAA)`, `set_text_color`, `node_count`. Anchors: 0 fill, 1 center,
2 top-left, 3 top, 4 top-right, 5 left, 6 right, 7 bottom-left, 8 bottom, 9 bottom-right. Changes are queued
and applied at the end of the frame (`PostUpdate`, `LATE + 15`), in module order and call order, so a
widget created in a frame appears in the same frame's sync. Not offered to scripts: GPU views (they need a
Rust painter), styles beyond the list above (use classes from the game's theme).

### Who may do what

| | game script (not a mod) | mod |
|---|---|---|
| owner of its nodes | `Owner::Plugin("script:<name>")` | `Owner::Mod("<mod id>")` |
| layer and node keys | as written | prefixed `mod:<mod id>/` |
| change or remove | its own nodes and any node found with `find` | only nodes it created |
| `find` | any keyed node in the tree | its own keys, plus mount points the game exposed |
| mount points | n/a | good as a **parent** (create widgets under them), nothing else |
| modal layer | yes | no, unless `UiScripts::allow_modal(true)` |
| layer `z` | any | clamped to `0..=100` (`set_z_range`) |
| node limit | none | 256 (`set_max_nodes`, `set_max_nodes_for(mod, n)`) |

On top of that the mod needs the namespace in its **grant** like any other host function: `ui` (and
`engine.events` to read the signal) in `[permissions]` and in the game's `ModPolicy`; without it the mod
does not load (`ModError::NotGranted`). Nodes of a mod are removed when the mod unloads or is replaced by a
reload (the new version builds them again in `init`). The game configures all of this with the `UiScripts`
resource:

```rust
let ui = engine.resources().get::<UiScripts>().unwrap().clone();
ui.expose("hud/slot");              // mods may add widgets under this keyed node
ui.set_max_nodes(64);
```

### Signals as script events

The event `ui.signal` (payload struct `UiSignal { widget: i64, kind: u32, value: f32 }`) carries what the
user did to the module's own widgets, readable **in the same frame** as the input through the normal
`engine.events.count / read`. `kind`: 1 click, 2 changed (checkbox 0 or 1, slider value, text input length),
3 submit, 4 focus gained, 5 focus lost, 6 hover enter, 7 hover leave. The text of a text input is read with
`ui.get_text`. Events are broadcast like every script event, but the handle names the module that owns the
widget (`(module id + 1) << 32 | n`): a module only ever matches its own handles, and the only thing another
module could learn from an event is that some widget of someone else changed.

```text
import ui.layer
import ui.column
import ui.button
import event ui.signal
...
export fn init() {
    reg layer, col, btn, no
    const.i32 no, 0
    call ui.layer(s_hud, s_hud.len, 20, 4, no) -> layer       ; z 20, top right
    call ui.column(layer) -> col
    call ui.button(col, s_reset, s_reset.len) -> btn
    global.set reset_w, btn
    ret
}
export fn frame(dt: f32) {
    ; for each ui.signal event: if widget == reset_w and kind == 1 -> game.score_reset
}
```

The complete mod is `examples/coin_dash_mods/mods/dash_hud/src/main.efa` (a score panel with a Reset button),
tested in `examples/coin_dash_mods/tests/headless.rs`; the rules above are tested in
`crates/ui/tests/scripts.rs` (forged and foreign handles, mount points, limits, unload).

Why host functions and not a script-side view function: a whole tree would cross the VM boundary every frame
and grants are simpler per node (API.md 16.6). Scripts that want a view-like loop can keep their own state
and call `set_text` / `set_visible` when it changes.

## 7. Replacing the backend or writing one

The game never sees the backend, so it can be replaced without touching game code. A backend implements
`UiBackend` (API.md section 13): the plugin calls `sync(tree, changes, theme, cx)` once per frame with the
frame's journal, `input` for every raw input event (native), `drain_signals` once per frame, `capture`,
and, on native, `gpu_init` / `gpu_resize` / `render`.

To write one:

1. Start from `HeadlessBackend` (`crates/ui/src/headless.rs`): it is the smallest correct backend and
   shows the journal rules. Copy its consistency checks into your tests.
2. Apply the journal against the **final** tree: `Created` builds a node, `Moved` entries name parents whose
   children you reorder like `tree.children(parent)`, `Removed` arrives children first, property entries
   (`Kind`, `Style`, `Classes`, `Visible`, `Enabled`, `Layer`) mean "read the new state from the tree".
   Resolve styles with `tree.resolve(id, WidgetState)` (kind, classes, inline, then state styles).
3. Produce signals as `UiSignal { widget, key: None, owner: Owner::Game, kind }`; the plugin fills key and
   owner, drops stale, hidden, disabled and ill-typed ones, sanitizes values and writes them into the tree
   (value widgets edit locally and report `Changed`, the tree is written back before game systems run).
4. Report capture: `Filter::Captured` per consumed pointer or key event (native) and `capture()` for the
   rest. A visible modal layer is added by the plugin.
5. Meet the shared behaviour list: value widgets, text input focus and IME (`take_text_input`), images
   (`UiBackendCx.images`), fonts (`UiChange::Font`), hover and focus styles, GPU views (`set_painters`).
   A style property or node kind a backend cannot draw is a bug in the backend or a reason to drop it from
   the API (rule 2 of the roadmap), never a silent difference.
6. Plug it in with `UiPlugin::with_backend(MyBackend::new())`. Tests stay on the headless backend; run the
   conformance parts that need your target (a browser, a GPU) in smoke tests of your own, like
   `crates/ui/tests/native_gpu.rs` and `examples/ui_demo/tests/native_screenshot.rs`.

The planned fallback (taffy layout + glyphon text, no iced) is described in
[`FALLBACK_NATIVE.md`](FALLBACK_NATIVE.md); it is exactly such a backend.

## 8. Decisions made for scripts and mods (Phase 4)

- **Namespace `ui`, not `engine.ui`:** the owner `engine` is reserved for the engine core; the UI crate
  registers as owner `efeengine-ui`. The functions are in the index like all others.
- **Queue and apply, handles local to a module:** lanes run in parallel on a snapshot and the tree is a
  resource, so host functions validate against a small shared table and queue operations; the apply step
  sorts by module id and call order (deterministic). A handle is `(module id + 1) << 32 | n` and never a
  `WidgetId`; checking ownership is a table lookup, not a tree walk.
- **Signals text:** the event payload is fixed size (16 bytes); text is read with `get_text` from a cache that
  the signal delivery and the script's own `set_text` keep current.
- **Mount points** are the only way for a mod to put widgets inside game-owned layout, and the game opts in
  per node. A mod can never edit or remove a game widget.
- **Limits are per mod, configured by the game** (`UiScripts`), default 256 nodes, no modal layers.
- **Fullscreen:** `Commands::set_fullscreen(bool)` (core): SDL window on native, the browser Fullscreen API
  on the web (needs a user gesture there; a refusal is ignored), a no-op for the headless window.
