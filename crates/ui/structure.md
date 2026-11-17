# efeengine-ui

The common UI API of the engine (design: `docs/ui/API.md`, sections 1 to 15; plan: `ui-roadmap.md`, Phase 1 retained layer). A retained `UiTree` resource with a change journal, a style subset and themes, signals, bindings, the `UiBackend` trait, a headless backend and the `UiTest` helper. The web backend (DOM) is real (Phase 2); iced and fallback come later; no UI toolkit type appears in the API. The Elm-style view layer (section 16) is the `view` module, built on top of the public tree API.

## At a glance
| | |
|---|---|
| Path | `crates/ui` |
| Kind | library (rlib) |
| Depends on (workspace) | `efeengine-core` (Engine, Plugin, Ctx, Resources, Events, `InputEvent`, `TextureHandle`) |
| External deps | `smallvec` (node classes), `thiserror` (`TreeError`) |
| Used by | `examples/ui_demo` |
| Targets | native and `wasm32-unknown-unknown` (both check clean) |
| Tests | `tests/tree.rs` (14), `tests/theme.rs` (5), `tests/plugin.rs` (22), `tests/settings.rs` (2), `tests/view.rs` (22); all headless, no window or GPU |

## Layout
```text
crates/ui
├── Cargo.toml
├── README.md
├── structure.md
├── src
│   ├── lib.rs        module list and re-exports (everything public is importable from the crate root)
│   ├── id.rs         WidgetId (generational), Owner (Game / Plugin / Mod)
│   ├── style.rs      Style (all fields Option, `merge` / `overlaid`, chained setters), Color, Edges, Size, Border, Direction, Justify, Align, TextAlign, FontId
│   ├── node.rs       NodeKind, KindTag, ImageFit, ClassId, Anchor, Layer, Node (read accessors)
│   ├── journal.rs    UiChange
│   ├── tree.rs       UiTree (slots, keys, owners, layers, classes, fonts, journal, builders, setters, theme), NodeRef (builder handle), StyleMut, TreeError
│   ├── theme.rs      Theme, StateStyles, WidgetState, resolution, built-in dark theme
│   ├── signal.rs     UiSignal, UiSignalKind, UiValue, UiCtxExt (`cx.ui_signals()`), filter_signals
│   ├── bindings.rs   Bind<T>, BindCx, UiBindings resource, BindUpdate, TextSrc
│   ├── capture.rs    UiCapture (alias of core's `InputCapture`)
│   ├── backend.rs    UiBackend trait, Filter, UiBackendCx
│   ├── css.rs        pure CSS generation (class names, declarations, base and theme state stylesheets), target independent
│   ├── dom_plan.rs   pure parts of the web backend (element tag, class list, inline declarations, style diff), native unit tests
│   ├── web.rs        WebBackend (wasm32 only): DOM elements, delegated listeners, capture
│   ├── headless.rs   HeadlessBackend (journal recorder and consistency checker) and HeadlessHandle
│   ├── plugin.rs     UiPlugin (resources and four main-thread systems)
│   ├── view/         the Elm-style view layer: mod.rs (docs: key scheme, frame order), el.rs (El, IntoChildren, constructors, `column!` / `row!`), reconcile.rs (tree diff), plugin.rs (UiView, UiViews, ViewCx, RunCondition, `changed`)
│   └── testing.rs    UiTest
└── tests/            see above
```

## Main concepts

### Tree (`tree.rs`, `node.rs`, `id.rs`)
- `WidgetId { index, generation }`: Copy, Hash, stale ids are rejected everywhere (`get` returns `None`, setters return `false`, `remove` is a no-op). `to_bits` / `from_bits` pack it into a `u64` for scripts.
- Roots are **layers**: `ui.layer(name, Layer)` makes a root container with key `name` (`Layer { z, modal, anchor, offset }`, helpers `Layer::fill(z)`, `centered`, `top_left`, ..., `.modal()`, `.offset(x, y)`). An existing name returns the same id and updates the data (`UiChange::Layer`). `layers()` lists them by z; `modal_active()` is true while a visible modal layer exists.
- Creation: `ui.column(parent)`, `row`, `container`, `scroll`, `text(parent, &str | String | Bind<String>)`, `button`, `checkbox`, `slider(parent, range, value)`, `text_input`, `image`, or `create(parent, NodeKind)` (panics on a stale parent) / `try_create` (returns `Result`). They return a `NodeRef` with chained setters (`.class`, `.classes`, `.key`, `.visible`, `.enabled`, `.hover_signals`, `.style`, every `Style` field as a method, `.placeholder`, `.max_len`, `.password`, `.step`) and `.id()`. `ui.node(id)` gives a `NodeRef` for an existing node.
- Setters on existing nodes (each writes one journal entry, none when nothing changes): `set_text`, `set_value(id, impl Into<UiValue>)` (slider clamped, text cut to `max_len`, wrong type returns `false`), `set_kind(id, NodeKind)` (same `KindTag` only), `edit_kind`, `set_image`, `set_visible`, `set_enabled`, `set_class`, `set_classes`, `add_class`, `remove_class`, `set_style`, `edit_style`, `style_mut(id)` (guard, one entry on drop), `move_to(id, parent, index)`, `remove(id)`, `remove_owner(&Owner)`, `set_theme`, `load_font`. `set_key` returns `Err(KeyTaken)`; the builder `.key` panics on a duplicate.
- Reads: `get`, `contains`, `len`, `iter`, `parent`, `children`, `kind`, `text_of`, `value`, `visible`, `enabled`, `is_shown` / `is_enabled` (include ancestors), `find(key)`, `key_of`, `owner_of`, `layer_of`, `layers`, `subtree`, `nodes_of(&Owner)`, `class_names`, `has_class`, `theme`, `resolve`.
- Owners: nodes get the tree's current owner (`set_owner`, `with_owner`; default `Owner::Game`). `Owner::from_engine_owner(engine.current_owner())` maps a plugin context. `remove_owner` removes every node of an owner with its subtrees (also foreign children under them).
- Class names are interned (`class_id`, `class_name`).

### Journal (`journal.rs`)
`UiChange { Created, Removed, Moved { id, parent, index }, Kind, Style, Classes, Visible, Enabled, HoverSignals, Layer, Theme, Font }`. `take_changes()` hands the frame's entries to the plugin and coalesces `Moved` to one entry per node with its final parent and index (valid against the final tree; backends place by final tree order). Coalescing the tree does: no property entry for a node created this frame (`Created` covers it); created and removed in the same frame leaves nothing; `Removed` is written for every node of the subtree, children first, and drops the node's earlier entries and `Moved` entries into it; a setter that changes nothing writes nothing. So a backend never sees an entry for an id that is gone, and `Created` is always before later entries of that id.

### Style and themes (`style.rs`, `theme.rs`)
`Style` is `Copy` with `Option` fields; `merge(&over)` / `overlaid(&over)` put set fields on top. `Theme { base, kinds, classes }` with `Theme::dark()` (the default), `Theme::empty()`. Resolution (`UiTree::resolve(id, WidgetState)` or `Theme::resolve`): `base`, kind normal, each class normal in order, inline style; then the state styles (hovered, pressed, focused, disabled; active ones only, later wins) of the kind and of each class on top of that, so a state style also beats the inline style (the literal reading of the design doc). `disabled` is added from the node's own or an ancestor's `enabled == false`.

### Signals (`signal.rs`)
`UiSignal { widget, key, owner, kind }`, `UiSignalKind { Click, Changed(UiValue), Submit(String), Focus(bool), Hover(bool) }`, `UiValue { Bool, F32, Text }`, `signal.key_kind()` -> `(&str, &UiSignalKind)`. `filter_signals(tree, raw)` is what the plugin runs on the backend's signals: drops stale, hidden, disabled, below-a-visible-modal-layer and ill-typed ones (`Focus(false)` is only dropped when stale) (and `Hover` without `.hover_signals()`), sanitizes `Changed` values and writes them into the tree without a journal entry, fills key and owner from the node. The result is put into the readable buffer of core's `Events<UiSignal>` (registered by the plugin) with `Ctx::send_batch_now` at `INPUT + 10`, so every later system of the same frame reads it with `cx.events::<UiSignal>()` or `cx.ui_signals()` (trait `UiCtxExt`, an iterator over the same events; collect it first if the loop needs `cx` mutably). They are dropped at the next frame's event update.

### Bindings (`bindings.rs`)
`Bind<T>` is a `Send + Sync` function `Fn(&BindCx) -> Option<T>`: `Bind::res(|s: &R| ..)`, `Bind::query::<Component>(entity, |c| ..)` (reads the world), `Bind::with(|cx: &BindCx| ..)` (resources, world, time). Register with `ui.bind_text / bind_value / bind_checked / bind_visible / bind_enabled / bind_class / bind_image`, or pass a `Bind<String>` to `ui.text(..)` / `ui.button(..)`. The registrations wait in the tree until the bindings system moves them into the `UiBindings` resource (`absorb`), which also drops the bindings of removed widgets. `evaluate` returns only the values that changed since last time; applying them is a normal tree change (journal entry only if the node really changed). A `None` result leaves the property alone. `evaluate` takes `&self` (last values behind a mutex per entry): the plugin borrows `UiBindings` in place instead of removing and reinserting it every frame, and only writes it (`absorb`, which moves the tree out of its slot for the call) on frames where the tree has new registrations or removed ids, so `res_changed::<UiBindings>()` is quiet otherwise.

### Backend and plugin (`backend.rs`, `headless.rs`, `plugin.rs`)
`UiBackend: 'static` (not `Send`): `init`, `input(&InputEvent, &UiTree) -> Filter`, `sync(tree, changes, theme, cx)`, `drain_signals`, `capture`, `resize`. `render(gpu, target)` comes with the core overlay pass. `UiPlugin::new()` uses the headless backend (and inserts `HeadlessHandle`), `UiPlugin::with_backend(b)` any other, `.theme(t)` replaces the dark theme. The backend sits in an `Rc<RefCell<..>>` shared by four main-thread systems (`add_system`): `ui_input` (PreUpdate, `INPUT + 5`: raw events to `backend.input`, resizes to `backend.resize`), `ui_signals` (PreUpdate, `INPUT + 10`: drain, filter, write back, `send_batch_now` the signals, `InputState::set_capture` = backend `capture()` OR visible modal layer OR input events `backend.input` answered `Filter::Captured` this frame: key and text count as keyboard, mouse, wheel and touch as pointer), `ui_bindings` (PostUpdate, `LATE + 10`, borrows `UiBindings` in place), `ui_sync` (PostUpdate, `LATE + 20`: `take_changes`, `sync` only when not empty).
`HeadlessBackend` keeps no widgets: it records every change, checks consistency (created id new and alive with a live parent; removed id known and gone; other changes name a known live id; moved parent exists) and collects violations as strings (`HeadlessHandle::errors()`). `HeadlessHandle` also injects signals, sets the capture flag and reads the recorded journal and batch sizes.

### UiTest (`testing.rs`)
`UiTest::new(|engine| ..)` (headless engine, 60 Hz, `UiPlugin::new()`), `step(n)` (panics on journal errors), `click`, `set_value`, `set_checked`, `type_text`, `submit`, `focus`, `hover`, `signal`, `text(key)`, `value(key)`, `id(key)`, `ui()`, `ui_mut()`, `resource::<T>()`, `resource_mut::<T>()`, `capture()` (core's `InputState::capture()`), `changes()`, `backend()`, and the public `engine`. Helpers only queue a signal in the headless backend; the next `step` delivers it through the plugin's normal path.

### View layer (`view/`, API.md section 16)
- `El<Msg>` (`el.rs`): kind data, classes, inline `Style` (all setters chained through the same `style_fields!` macro as `NodeRef`), key, visible / enabled, children, and handlers `on_click(Msg)`, `on_change` / `on_toggle` / `on_input` / `on_submit` / `on_focus` (boxed `Fn + Send + Sync` closures), `map(f)`. Kind setters `.placeholder .max_len .password .step`. Constructors `text button button_with checkbox slider text_input image container row column scroll slot`; `.child(x)` / `.children(x)` take an `El` or any iterable of `El` (`IntoChildren`); macros `column![..]`, `row![..]` (`#[macro_export]`: `efeengine_ui::column`, import explicitly, std has a `column!` too).
- `UiView<Msg>` (`plugin.rs`): `new(name, Fn(&S) -> El)`, `with(name, Fn(&ViewCx) -> El)`, `.layer`, `.when(RunCondition)` (repeated calls OR), `.enabled`. Plugin `efeengine-ui/view/<name>`, depends on `efeengine-ui`, registers `Events<Msg>` and the `UiViews` resource (`set_enabled`, `is_enabled`, `toggle`). Nodes are owned by `Owner::Plugin(<plugin name>)`, the layer key is `<name>`.
- Systems per view: `ui_view_messages` (PreUpdate, `INPUT + 11`: signals of widgets in the handler table become `Msg` via `send_batch_now`, and mark the view dirty so it runs this frame under `.when`) and `ui_view` (PostUpdate, `LATE + 11`, after bindings at `LATE + 10` and before sync at `LATE + 20`: enabled check, condition, build the whole `El`, `validate` duplicate keys, then `tree.with_owner(..)` { layer, visible, `reconcile_root` }, replace the handler table).
- `reconcile.rs`: diffs the `El` against the tree (not a previous `El`). Per parent: keyed children match the node with the full key (parent must be the same, else that node is removed and recreated), unkeyed ones match an unkeyed child at the same index; kind mismatch or leftovers are removed (before creating, so keys are free); then in order create / `move_to` when the child at index i differs / update kind data, classes, style, visible, enabled. Slots are updated but their children are not visited. Key scheme: `<view>/<keys of keyed ancestors>/<own key>`, unkeyed elements add nothing.
- Frame order: INPUT+10 signals, INPUT+11 messages, Update (game reads `Events<Msg>`), LATE+10 bindings, LATE+11 views, LATE+20 sync.

### Fonts
`UiTree::load_font(name, bytes)` registers a `FontId` and journals `Font(id)`; `font`, `font_by_name`, `fonts`. No font file is bundled yet (the native default font and `THIRD_PARTY_NOTICES.md` entry come with the native backend).

### Web backend (`web.rs`, `dom_plan.rs`, `css.rs`; API.md sections 5, 6, 10, 12)
`UiPlugin::new()` uses `WebBackend` on wasm32 (`UiPlugin::headless()` always headless). Layer element: startup config `uiLayerId` (default `ui`, created in the canvas's parent if missing; marked `data-efui-layer`). One element per node (`data-efui-id` = `WidgetId::to_bits`), inline style = resolved normal style diffed against the last written declarations, `<style id="efui-base">` and `<style id="efui-theme">` (hover, active, focus, disabled with `!important`) in `<head>`. Journal applied against the final tree: `Created` builds the element, parents named by `Created` / `Moved` get their DOM children ordered like `tree.children`. Delegated listeners on the layer element (kept in the backend, removed on drop) produce the signals; capture from pointerover / pointerout plus `:hover` and from `document.activeElement`. Modal layers get a transparent backdrop element. `UiBackendCx.images` carries RGBA for `Image` nodes (drawn once into a canvas). Core side: `web_impls::is_editable_target` also ignores key / composition events from inside `[data-efui-layer]`.

## Not done yet
- Web: fonts (`UiChange::Font` is ignored), textures replaced after the widget was built do not refresh, no IME work beyond the browser's own, no browser test automation (checked by hand with `examples/ui_demo`).
- Script host functions (API.md section 9, Phase 4); `Owner::Mod` and per-owner queries exist.
- Native backends (iced, fallback) and `render(gpu, target)`.
- Native IME and focus navigation.
- View layer: moves across parents recreate the node; reorder uses one `move_to` per out-of-place child (no longest-increasing-subsequence minimization); no `hover_signals` on `El`.
