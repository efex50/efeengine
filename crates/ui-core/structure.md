# efeui-core

The engine-agnostic half of the UI (design: `docs/ui/API.md`). Split out of `efeengine-ui` so the model can get other adapters (a Bevy one is planned) next to the efeengine one.

## At a glance
| | |
|---|---|
| Path | `crates/ui-core` (package `efeui-core`) |
| Kind | library (rlib) |
| Depends on (workspace) | nothing |
| External deps | `smallvec` (node classes), `thiserror` (`TreeError`). Nothing engine, renderer or windowing related, ever (`cargo tree -p efeui-core` shows only those) |
| Used by | `efeengine-ui` (`crates/ui`), which re-exports every module under the same path |
| Targets | native and `wasm32-unknown-unknown` |
| Tests | unit tests in `src/` (15: `css.rs` 10, `dom_plan.rs` 5), `tests/tree.rs` (14), `tests/theme.rs` (5), `tests/review.rs` (9, journal consistency through `HeadlessModel`), `tests/read_only.rs` (10, read-only / multiline inputs, `blur`, headless focus & capture, `follow_end`, `ellipsize` / `Truncated`) |

## Layout
```text
crates/ui-core
├── Cargo.toml
├── README.md
├── structure.md
├── src
│   ├── lib.rs        module list and the top-level re-exports (the same names `efeengine_ui` re-exports)
│   ├── id.rs         WidgetId (generational), Owner (Game / Plugin / Mod)
│   ├── style.rs      Style (all fields Option, merge, chained setters), Color, Edges, Size, Border, Direction, Justify, Align, TextAlign, FontId
│   ├── node.rs       NodeKind, KindTag, ImageFit, ImageId (opaque image key), ClassId, Anchor, Layer, Node
│   ├── journal.rs    UiChange (Created, Removed, Moved, Kind, Style, Classes, Visible, Enabled, HoverSignals, Layer, Theme, Font, Blur)
│   ├── tree.rs       UiTree (slots, keys, owners, layers, classes, fonts, journal, builders, setters, theme, blur), NodeRef, StyleMut, TreeError, FontData, PendingBind
│   ├── theme.rs      Theme, StateStyles, WidgetState, resolution, built-in dark theme
│   ├── signal.rs     UiSignal, UiSignalKind, UiValue, PointerEvent, filter_signals
│   ├── bindings.rs   Bind<T>, BindCx (type-keyed host values), UiBindings, BindUpdate, TextSrc; the engine constructors `Bind::res` / `Bind::query` are an adapter extension
│   ├── capture.rs    UiCapture { pointer, keyboard }
│   ├── css.rs        pure CSS generation (class names, declarations, base and theme state stylesheets)
│   ├── dom_plan.rs   pure parts of a DOM backend (element tag, class list, inline declarations, gpu-view auto declarations, layer/backdrop declarations, style diff)
│   ├── headless.rs   HeadlessModel (journal recorder and consistency checker) and HeadlessHandle; adapters wrap the model in their backend trait
│   └── view/         el.rs (El, IntoChildren, constructors incl. `selectable_text` / `gpu_view`, `column!` / `row!`), reconcile.rs (tree diff), mod.rs
└── tests/            tree.rs, theme.rs, review.rs, read_only.rs
```

## Boundary with an adapter
The adapter owns: the backend trait and the frame loop (when to call `take_changes`, `filter_signals`, `UiBindings::evaluate`, `reconcile_root`), input events, textures (it maps `ImageId` to its own handle), clipboard, windows, scripting and the runtime of views (`UiView`). The core never calls into an adapter. A few items are `pub` and `#[doc(hidden)]` only because the adapter needs them (`PendingBind`, `UiTree::pending_binds`, `take_removed`, `reconcile::{Table, validate, reconcile_root}`, `view::el::Handlers`).
