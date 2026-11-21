# efeengine structure

efeengine is a Rust game engine targeting both native desktop (SDL3 + wgpu / SDL_Renderer) and the web (`wasm32-unknown-unknown` + WebGPU / WebGL2 + Web Workers). It brings together an engine core, an `efecs` (`hecs` fork) ECS, a cross-platform UI stack (`efeui-core`, `efeengine-ui`, `efeengine-world-ui`), an external editor + IPC link + `.efproj` project format, and a sandboxed scripting/modding stack (`efevm`, optional Cranelift JIT `efevm-jit`, `.efa` assembler/linker `efa`, `.wasm` translator `efe-wasm`, guest SDK `efe-guest`, `.efmod` packager `efemod`, and the `efescript` toolchain). This file maps the repository; every crate under `crates/` has its own `structure.md` with the details.

## Repository layout

```text
efeengine/
├── Cargo.toml                  workspace: lists every crate below, shared optimize profile
├── Cargo.lock
├── rust-toolchain              pins nightly (needed for wasm threads: -Z build-std)
├── .cargo/config.toml          wasm32 rustflags: atomics, shared memory, TLS/heap exports
├── build.py                    main Python entry point: pick an example, build/run native or web
├── xtask/                      Rust build/run/serve/mod/api/project automation (`cargo xtask`)
├── scripts/
│   ├── web.py                  wasm build → wasm-bindgen → assemble dist/<example>/
│   ├── web.sh                  thin wrapper around web.py
│   ├── serve.py                local HTTP server with COOP/COEP headers
│   ├── remote.py               remote build helper
│   └── sync_efescript.py       efescript sync helper
├── templates/web/              page shell copied into every web build (index.html, main.js, coi-sw.js, embed.html, _headers)
├── crates/
│   ├── core/                   efeengine-core: the engine
│   │   └── macros/             efeengine-macros: #[main], #[script_component], #[derive(Component, ScriptType)]
│   ├── efecs/                  efecs (lib `hecs`): forked hecs 0.11.2 with dynamic components & change-detection ticks
│   │   └── macros/             hecs-macros: #[derive(Bundle, DynamicBundleClone, Query)]
│   ├── ui-core/                efeui-core: engine-agnostic UI tree, styles, themes, bindings, diffing
│   ├── ui/                     efeengine-ui: engine UI adapter (web DOM, native iced, Draw2D fallback, debug/editor UI)
│   ├── world-ui/               efeengine-world-ui: world-space UI panels/billboards drawn via Draw2DOverlay
│   ├── editor/                 efeengine-editor (`efe-editor`): standalone native iced editor application
│   ├── editor-link/            efeengine-editor-link: IPC/socket bridge between a running game and efe-editor
│   ├── project/                efeengine-project: `.efproj` XML project format, validation and scaffolding
│   ├── efevm/                  efevm: v1 core VM (verified `.efvm` modules, register interpreter, fibers, budget, host imports)
│   ├── efevm-jit/              efevm-jit: optional Cranelift JIT backend for efevm (native only)
│   ├── efa/                    efa: `.efa` assembler, linker, and mod-folder builder (optional `.wasm` translation)
│   ├── efemod/                 efemod: `.efmod` mod package reader/writer, manifest, components, dependency resolution
│   ├── efe-wasm/               efe-wasm: WebAssembly (`.wasm`) → `.efvm` module translator for Rust/C mods
│   ├── efe-guest/              efe-guest: `no_std` Rust SDK + `include/efe_guest.h` C header for WASM guest mods
│   ├── efescript/              efescript: scripting toolchain CLI (`run`, `build`, `dis`, `bundle`, `pack`, `api`, `api-export`, `lsp`, `debug`)
│   │   ├── proc_macros/        legacy proc-macros for removed `.efe` VM (unused)
│   │   └── strtohx/            legacy dev helper for removed `.efe` format (unused)
│   ├── calc/                   calc: expression lexer/parser/printer/evaluator/simplifier (+ optional CLI), used by `efa`
│   └── efepages/               efepages: standalone sparse paged byte memory
│       └── page_size_macro/    build-time PAGE_SIZE constant for efepages
├── examples/                   11 example games/demos (native bin + wasm lib): coin_dash*, custom_renderer, editor_demo, input_test, multi_window, script_demo, test_example, ui_demo
├── docs/                       design notes and specs (rendering, threading, scenes, editor, UI, JIT, mod loading, etc.)
├── dist/                       generated web builds (gitignored)
└── .github/workflows/          CI: cargo build + cargo test
```

## Crates

| Crate (package) | Path | Kind | What it does | Details |
|---|---|---|---|---|
| `efeengine-core` | `crates/core` | lib (rlib + cdylib) | The engine: windowing, wgpu / SDL_Renderer graphics, `efecs` (`hecs`) `World`, `TypeRegistry`, `Commands`, platform-neutral input (SDL3 native, DOM web, headless), fixed-timestep hybrid sequential/parallel schedule, scenes/prefabs/schema reflection, cross-platform thread pool, `RenderFrame` (`Renderer2D`/`Renderer3D`/`CustomRenderer` + overlays), and the `efevm` scripting/mod runtime (optional `efa`, `jit`, `wasm-mods`). | [crates/core/structure.md](crates/core/structure.md) |
| `efeengine-macros` | `crates/core/macros` | proc-macro | `#[main]` (native `fn main` / `run()` or wasm `__engine_start`), `#[script_component]`, `#[derive(Component)]`, and `#[derive(ScriptType)]`. | [crates/core/macros/structure.md](crates/core/macros/structure.md) |
| `efecs` (`hecs`) | `crates/efecs` | lib (`no_std` + `alloc`) | Forked `hecs` 0.11.2 ECS extended with runtime-registered dynamic components (`ComponentId`) and opt-in per-component change-detection ticks. Re-exported by `efeengine-core` as `hecs` / `ecs`. | [crates/efecs/structure.md](crates/efecs/structure.md) |
| `hecs-macros` | `crates/efecs/macros` | proc-macro | `#[derive(Bundle)]`, `#[derive(DynamicBundleClone)]`, and `#[derive(Query)]` for `efecs`. | [crates/efecs/macros/structure.md](crates/efecs/macros/structure.md) |
| `efeui-core` | `crates/ui-core` | lib | Engine-agnostic UI model: generational `WidgetId`, `UiTree`, `Style`, `Theme`, `UiSignal`, `UiBindings`, CSS/DOM planning, `HeadlessModel`, and declarative `El` view reconciliation. | [crates/ui-core/structure.md](crates/ui-core/structure.md) |
| `efeengine-ui` | `crates/ui` | lib | Engine adapter for `efeui-core`: `UiPlugin`, web DOM backend, native `iced` backend, Draw2D fallback backend, bundled fonts / `text_kit`, debug menu, and in-game editor UI widgets. | [crates/ui/structure.md](crates/ui/structure.md) |
| `efeengine-world-ui` | `crates/world-ui` | lib | World-space UI panels and billboards extracted in `PostUpdate` and rendered via `Draw2DOverlay`. | [crates/world-ui/structure.md](crates/world-ui/structure.md) |
| `efeengine-editor` | `crates/editor` | lib + bin (`efe-editor`) | Standalone native `iced` editor application: project/scene management, build runner, and live connection to a running game over `efeengine-editor-link`. | [crates/editor/structure.md](crates/editor/structure.md) |
| `efeengine-editor-link` | `crates/editor-link` | lib | IPC/socket protocol bridge embedded in a game to expose scene inspection, editing, and gizmos to `efe-editor` (no-op on `wasm32`). | [crates/editor-link/structure.md](crates/editor-link/structure.md) |
| `efeengine-project` | `crates/project` | lib | `.efproj` XML project manifest reader/writer, validator, and new-project scaffolder. | [crates/project/structure.md](crates/project/structure.md) |
| `efevm` | `crates/efevm` | lib | Core bytecode VM: verified `.efvm` modules, register interpreter, fibers, fuel budget, host imports, `ApiIndex`, and pluggable execution backend trait; wasm32-compatible. | [crates/efevm/structure.md](crates/efevm/structure.md) |
| `efevm-jit` | `crates/efevm-jit` | lib | Optional Cranelift JIT backend (`JitBackend`) for `efevm` on native targets (delegates to interpreter when unsupported/debugged). | [crates/efevm-jit/structure.md](crates/efevm-jit/structure.md) |
| `efa` | `crates/efa` | lib | `.efa` compiler run by the engine and toolchain: assembler (`asm`), linker (`link`), and mod-folder builder (`mod_folder`), with optional `.wasm` translation via `efe-wasm` (`wasm` feature). | [crates/efa/structure.md](crates/efa/structure.md) |
| `efemod` | `crates/efemod` | lib (`write` feature for packer) | `.efmod` mod package container (7z reader + canonical writer), `Manifest` (`[meta]`, versions, dependencies), and mod-declared `components.toml` parsing. | [crates/efemod/structure.md](crates/efemod/structure.md) |
| `efe-wasm` | `crates/efe-wasm` | lib | Translates WebAssembly (`.wasm`) guest modules into verified `.efvm` modules so mods can be written in Rust or C and run sandboxed on `efevm`. | [crates/efe-wasm/structure.md](crates/efe-wasm/structure.md) |
| `efe-guest` | `crates/efe-guest` | lib (`no_std` on `wasm32`) | Guest SDK for writing WASM mods in Rust (allocator, `provide!` / `provide_event!` / `scratch!` macros, `Entity`, `GuestStruct`) plus `include/efe_guest.h` for C/C++ mods. | [crates/efe-guest/structure.md](crates/efe-guest/structure.md) |
| `efescript` | `crates/efescript` | lib + bin | Scripting toolchain CLI (`run`, `build`, `dis`, `bundle`, `pack`, `api`, `api-export` including Rust/C guest bindgen, `lsp`, `debug`) and `std` host namespaces on `efevm`. | [crates/efescript/structure.md](crates/efescript/structure.md) |
| `proc_macros` | `crates/efescript/proc_macros` | proc-macro | Legacy proc-macros (`MAX_ADDR_SIZE`, `STACK_PAGE_COUNT`) from the removed `.efe` VM; unused in the workspace. | [crates/efescript/proc_macros/structure.md](crates/efescript/proc_macros/structure.md) |
| `strtohx` | `crates/efescript/strtohx` | bin | Legacy dev helper that converts text into `.efe` `push*` instructions; unused since `.efa` `db` string literals replaced `.efe`. | [crates/efescript/strtohx/structure.md](crates/efescript/strtohx/structure.md) |
| `calc` | `crates/calc` | lib + optional bin `cli` | Expression front end (lexer, Pratt parser, infix printer, checked `i128`/`f64`/`bool` evaluator, constant folder). Used by `efa` for `.efa` constant expressions. | [crates/calc/structure.md](crates/calc/structure.md) |
| `efepages` | `crates/efepages` | lib | Standalone sparse paged byte memory (unwritten memory reads as zeros); no longer depended on by `efescript`. | [crates/efepages/structure.md](crates/efepages/structure.md) |
| `page_size_macro` | `crates/efepages/page_size_macro` | proc-macro | Emits compile-time `PAGE_SIZE` from `efepage_size` env var (default `4096`) for `efepages`. | [crates/efepages/page_size_macro/structure.md](crates/efepages/page_size_macro/structure.md) |
| `xtask` | `xtask` | bin | Workspace automation CLI (`cargo xtask`) for native/web builds, serving, API export, mod packing, script checking, and project commands. | — |

### Examples (`examples/`)

All 11 examples build as a `lib` (for wasm) plus a `native` binary:
- `test_example` — minimal sample game running the engine loop.
- `input_test` — manual input test logging keyboard, mouse, wheel, text, touch, focus, and resize changes.
- `custom_renderer` — template for a custom game `Renderer` with a stub WGSL shader.
- `multi_window` — multi-window rendering and UI demonstration.
- `ui_demo` — `efeengine-ui` showcase and integration tests.
- `editor_demo` — scene/editor integration demo.
- `script_demo` — scripting & modding demo (`.efa` scripts, mod folders, WASM mod end-to-end tests, `efeengine-editor-link`).
- `coin_dash` — full sample game using `efeengine-core`, `efeengine-ui`, and `efeengine-world-ui`.
- `coin_dash_scripts` — `coin_dash` variant driven by `.efa` scripts.
- `coin_dash_mods` — `coin_dash` variant loading `.efmod` mod packages.
- `coin_dash_wasm` — `coin_dash` variant loading Rust/WASM mods via `efe-wasm` and `efe-guest`.

## How the crates depend on each other

```text
examples/* ──┬─► efeengine-core ──► efeengine-macros
             │        │
             │        ├──► efecs (`hecs`) ──► hecs-macros
             │        ├──► efevm
             │        ├──► efemod ──► efevm
             │        ├──► efa (default feature `efa`) ──► efevm, efemod, calc, efe-wasm (feature `wasm`)
             │        ├──► efevm-jit (native feature `jit`) ──► efevm
             │        └──► efe-wasm (native feature `wasm-mods`) ──► efevm
             │
             ├─► efeengine-ui ──► efeui-core, efeengine-core
             ├─► efeengine-world-ui ──► efeengine-core, efeengine-ui
             └─► efeengine-editor-link ──► efeengine-core

efeengine-editor ──► efeengine-editor-link, efeengine-project

xtask ──► efescript ──► efa (with `wasm`), efevm, efemod (with `write`), efe-wasm

efe-guest                (standalone guest SDK, depended on by external WASM mod crates)
efepages ──► page_size_macro   (standalone)
proc_macros, strtohx           (standalone legacy)
```

The engine runs scripts on `efevm` directly (`engine::scripting` in `efeengine-core`), compiles `.efa` via `efa`, optionally translates `.wasm` mods via `efe-wasm` (`wasm-mods`), and optionally JIT-compiles via `efevm-jit` (`jit`). It does not depend on `efescript`, which is the developer toolchain (CLI, bindgen, LSP, debugger).

## Build and run

`./build.py` (Python, standard library only; `python build.py` on Windows) and `cargo xtask` provide the main build and run commands:

| Command | Effect |
|---|---|
| `./build.py` | Interactive: choose example, native or web, build or run |
| `./build.py run <example> native` | Build and start the native (SDL3) binary (e.g. `coin_dash`, `input_test`) |
| `./build.py run <example> web [--port N]` | Build to wasm, assemble `dist/<example>/`, serve it on `http://localhost:8080/` with COOP/COEP headers |
| `./build.py dist <example>` | Release web build ready to upload to a static host |
| `cargo xtask api-export <example>` | Build natively and write the script API index to `target/efapi/<example>.efapi` |
| `cargo xtask mods <example>` | Pack `examples/<example>/mods/*` into `target/mods/<example>/` as `.efmod` packages |
| `cargo xtask check-scripts <example>` | Export the API index, then assemble every `.efa` script of the example against it |
| `./build.py check` / `test` / `doctor` / `clean` | Type-check, run tests, report missing tools, clean |
| `./build.py dev <linux\|web\|windows>` | Point rust-analyzer at a target (`.vscode/settings.json`) |

Requirements:
- **Nightly Rust** with `rust-src` and the `wasm32-unknown-unknown` target (`rust-toolchain` selects nightly automatically).
- **Native:** system SDL3 development files (`SDL3-devel` / `libsdl3-dev`), or `--static` to build SDL3 from source (needs cmake, a C compiler, and X11/Wayland, GL, ALSA headers).
- **Web:** `wasm-bindgen-cli` matching `wasm-bindgen` in `Cargo.lock` (`./build.py doctor` prints the install command). Served with COOP/COEP headers for wasm threads (`scripts/serve.py` locally; `templates/web/_headers` on static hosts).

## Where things stand

Each crate's `structure.md` lists current details and known gaps. High-level summary:
- **Engine & ECS:** `efeengine-core` runs on forked `efecs` (`hecs`) with dynamic components and change-detection ticks, fixed-timestep hybrid sequential/parallel schedules, scenes/prefabs/schema reflection, multi-window `RenderFrame` (`Renderer2D`/`Renderer3D`/`CustomRenderer` + overlays), and native SDL3 / web DOM / headless backends.
- **UI:** `efeui-core` + `efeengine-ui` have working web DOM, native `iced`, and Draw2D fallback backends, plus `efeengine-world-ui` for world-space panels.
- **Editor & Projects:** `efeengine-project` (`.efproj`), `efeengine-editor-link` (IPC), and `efeengine-editor` (`efe-editor`) are in the workspace for standalone editing alongside in-game editor panels.
- **Scripting & Mods:** `efevm` (with optional `efevm-jit` Cranelift backend), `efa`, `efemod`, `efe-wasm`, `efe-guest`, and `efescript` support `.efa` assembly and Rust/C `.wasm` guest mods. `efescript`'s high-level `compiler/` front end is still a stub.
- **Standalone / legacy:** `efepages` (`set_max_pages` is `todo!()`), `efescript/proc_macros`, and `efescript/strtohx` remain standalone/legacy crates.
