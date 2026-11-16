# efeengine UI Roadmap

> **Status:** Proposal / roadmap idea\
> **Date:** 2026-11-15\
> **Scope:** Engine editor UI and game-facing UI
>
> This document records an architectural direction, not a commitment to
> implement every item. Keep it in the project roadmap and revise it as
> platform support and engine requirements become clearer.

## 1. Direction

Use two complementary UI libraries for different audiences:

-   **egui --- developer-facing UI:** engine editor, debugging,
    inspection, profiling, and development tools.
-   **iced --- end-user/game-facing UI:** menus, settings, inventory,
    dialogs, forms, HUD-adjacent interfaces, and other polished
    application-style game interfaces.

Do not make either library a mandatory dependency of the engine core.
Keep the engine's public platform and rendering abstractions independent
from the chosen UI toolkit.

## 2. Intended responsibilities

### egui: developer environment

Potential uses:

-   Scene, entity, and component inspectors.
-   Runtime variable and resource inspection.
-   Debug console and log viewer.
-   Profiler and performance overlays.
-   Asset browser and development utilities.
-   Engine/editor settings and quick prototypes.

The editor may use egui directly. Game builds should not need to include
egui unless a game explicitly opts into it.

### iced: end-user interfaces

Potential uses:

-   Main menus and pause menus.
-   Settings and configuration screens.
-   Inventory, lists, forms, and dialogs.
-   Notifications and other interactive panels.
-   Themeable, styleable game interfaces.
-   More application-like interfaces that benefit from a structured
    widget and state model.

Games should be able to opt into iced rather than inheriting it as an
unavoidable engine dependency.

## 3. Proposed crate and feature boundaries

Illustrative structure; names can be adjusted to match the existing
workspace:

``` text
efeengine
├── efeengine-core
├── efeengine-ui
│   └── iced integration / game-facing UI support
└── efeengine-editor
    └── egui integration / developer tools
```

Keep dependencies optional and separated by crate or Cargo feature. For
example:

``` toml
[features]
ui-iced = ["dep:iced"]
ui-egui = ["dep:egui"]
```

This is illustrative only: feature names and dependency wiring should
follow the actual workspace design. Prefer putting editor-only
dependencies in the editor crate instead of exposing them through the
runtime crate.

## 4. Integration principles

1.  **Keep the core toolkit-agnostic.** `efeengine-core` should not
    expose iced- or egui-specific types in its fundamental APIs.
2.  **Keep the two UI runtimes independent.** Do not force iced and egui
    into one shared widget tree, state model, or event system.
3.  **Share engine services, not widget implementations.** Both
    integrations may consume common window, input, timing, asset, and
    rendering services where appropriate.
4.  **Avoid assuming that sharing wgpu objects is sufficient.**
    Explicitly manage render-pass ordering, surface
    acquisition/presentation, texture lifetime and registration,
    clipping, DPI scaling, and input-coordinate conversion.
5.  **Keep game UI optional.** A game should be able to use iced,
    another UI solution, or its own UI without taking a dependency on
    the editor.
6.  **Preserve escape hatches.** Advanced games may need custom
    rendering or UI systems outside the built-in integrations.

## 5. Roadmap phases

### Phase 0 --- Validate requirements and platform support

-   [ ] Define which targets the editor must support.
-   [ ] Define which targets game-facing UI must support: desktop,
    WebAssembly, Android, and iOS as applicable.
-   [ ] Test the selected iced version and renderer on each intended
    target; distinguish stable support from experimental or unavailable
    support.
-   [ ] Test egui on the editor's intended targets.
-   [ ] Record minimum supported Rust, wgpu, and toolkit versions.
-   [ ] Build small proof-of-concept applications before committing to
    public APIs.

**Exit criteria:** A platform compatibility matrix exists, and both
libraries have been tested on the targets relevant to their
responsibilities.

### Phase 1 --- egui developer tooling proof of concept

-   [ ] Render an egui panel inside the engine/editor window.
-   [ ] Route mouse, keyboard, text, focus, and resize events correctly.
-   [ ] Validate DPI scaling and high-resolution displays.
-   [ ] Add a minimal inspector or runtime statistics panel.
-   [ ] Verify rendering order and resource lifetime with the engine's
    wgpu renderer.

**Exit criteria:** A useful developer panel runs without coupling egui
to the engine core.

### Phase 2 --- iced game UI proof of concept

-   [ ] Render a minimal iced interface through the intended
    engine/window/rendering integration.
-   [ ] Validate widget input, focus, text input, clipboard needs, and
    window resizing.
-   [ ] Prototype a menu/settings screen with a custom theme.
-   [ ] Evaluate fonts, images, scaling, clipping, and accessibility
    needs.
-   [ ] Measure startup cost, binary size, memory use, and frame-time
    impact.

**Exit criteria:** A representative game-facing interface works on the
required target platforms with acceptable integration and performance.

### Phase 3 --- Public integration APIs

-   [ ] Define how games create, update, and render their UI runtime.
-   [ ] Define how UI receives platform input and emits
    actions/messages.
-   [ ] Define how UI accesses engine assets and localization, if
    needed.
-   [ ] Document threading and lifecycle rules.
-   [ ] Keep toolkit-specific APIs in their integration crates.
-   [ ] Add examples and API documentation.

**Exit criteria:** A game can opt into the iced integration without
depending on editor tooling or leaking toolkit types into core engine
APIs.

### Phase 4 --- Editor and game workflow

-   [ ] Expand egui tooling: inspector, asset browser, console, and
    profiler as needed.
-   [ ] Add examples of iced menus, settings, dialogs, and themed
    widgets.
-   [ ] Document how to replace or bypass the built-in game UI.
-   [ ] Add integration tests and platform smoke tests.
-   [ ] Revisit the architecture based on real game and editor usage.

**Exit criteria:** Developer tooling and game-facing UI have clear,
independently maintainable workflows.

## 6. Risks and open questions

-   **Platform compatibility:** iced's support and maturity may differ
    across desktop, WebAssembly, Android, and iOS. Verify the exact
    versions and renderer rather than assuming parity.
-   **Renderer integration:** both toolkits' wgpu integration must
    coexist with the engine's render graph and surface lifecycle.
-   **Binary size and compile time:** optional dependencies and crate
    boundaries should prevent unused UI systems from entering game
    builds.
-   **Input ownership:** define how the engine decides whether game
    input or UI input consumes an event.
-   **DPI and coordinate systems:** standardize logical pixels, physical
    pixels, scaling, and pointer coordinates.
-   **Text and localization:** evaluate font loading, text shaping, IME,
    clipboard, and localization requirements early.
-   **Editor versus runtime:** editor UI and shipped game UI have
    different lifecycle, performance, and distribution requirements;
    keep them separate.

## 7. Current architectural decision

  -----------------------------------------------------------------------
  Area                                Preferred direction
  ----------------------------------- -----------------------------------
  Engine core                         UI-toolkit agnostic

  Developer environment / editor      egui

  End-user / game-facing UI           iced

  Rendering integration               Engine-owned integration boundary,
                                      likely using the existing wgpu
                                      stack

  Dependency policy                   Optional and separated; no
                                      mandatory UI toolkit in core

  Alternative/custom UI               Must remain possible
  -----------------------------------------------------------------------

**Guiding principle:** egui serves the people building and debugging
games; iced serves the people playing them. The engine owns the boundary
and lifecycle, while each toolkit keeps its own UI model.
