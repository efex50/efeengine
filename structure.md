# efeengine structure

efeengine is a Rust game engine that is being assembled from four earlier projects:
an engine core (window, renderer, message loop), a scripting language with its own
VM, the paged memory that VM runs on, and an expression calculator. This file maps
the repository. Every crate has its own `structure.md` with the details.

## Repository layout

```text
efeengine/
├── Cargo.toml              workspace: lists every crate below, shared release profile
├── Cargo.lock
├── rust-toolchain          pins nightly (needed for wasm threads: -Z build-std)
├── .cargo/config.toml      wasm32 rustflags: atomics, shared memory, TLS/heap exports
├── build.py                main entry point: pick an example, build/run it native or web
├── scripts/
│   ├── web.py              wasm build → wasm-bindgen → assemble dist/<example>/
│   ├── web.sh              thin wrapper around web.py
│   └── serve.py            local HTTP server with COOP/COEP headers
├── templates/web/          page shell copied into every web build (index.html, main.js, _headers)
├── crates/
│   ├── core/               efeengine-core: the engine
│   │   └── macros/         efeengine-macros: #[main] entry-point macro
│   ├── efescript/          efescript: `.efa` assembler, CLI, LSP and debugger on efevm (scripting toolchain)
│   │   ├── proc_macros/    build-time constants for efescript
│   │   └── strtohx/        dev tool: text → push instructions for the removed .efe format
│   ├── efevm/              efevm: v1 core VM (verified .efvm modules, interpreter, fibers); the v2 path runs on it
│   ├── efepages/           efepages: sparse paged memory used by the VM
│   │   └── page_size_macro/ build-time PAGE_SIZE constant
│   └── calc/               calc: expression parser / printer / simplifier (+ CLI)
├── examples/
│   ├── test_example/       sample game (native binary + wasm library)
│   ├── script_demo/        scripting example: API index export, .efa scripts, end-to-end test (native binary + wasm library)
│   └── input_test/         manual input test: logs InputState changes (native binary + wasm library)
├── docs/                   design notes (design-io-and-loop.md: the input system and engine loop)
├── dist/                   generated web builds (gitignored)
└── .github/workflows/      CI: cargo build + cargo test
```

## Crates

| Crate (package) | Path | Kind | What it does | Details |
|---|---|---|---|---|
| `efeengine-core` | `crates/core` | lib (rlib + cdylib) | The engine. `Engine` owns the window, logger, wgpu graphics context, a simple world, input state, resources and a system schedule, and runs a fixed-timestep frame loop (`frame`: poll input, handle messages, `Startup` / `PreUpdate` / `FixedUpdate` / `Update` / `PostUpdate` / `RenderExtract` / `Shutdown` stages, render). A platform-neutral input system (keyboard, mouse, wheel, touch, text, window events) is fed by an SDL3 backend on native and DOM listeners on the web; a headless window makes the loop testable without SDL or a GPU. Control `Message`s (start/stop/kill, title, logging) travel on an engine-owned flume queue and a global one for JS and workers. A thread pool uses OS threads natively and Web Workers on wasm. Rendering is currently one hard-coded animated triangle. | [crates/core/structure.md](crates/core/structure.md) |
| `efeengine-macros` | `crates/core/macros` | proc-macro | `#[main]`: turns the game's `main` into a native `fn main`, or on wasm into an exported `__engine_start()` that the web page calls. | [crates/core/macros/structure.md](crates/core/macros/structure.md) |
| `efescript` | `crates/efescript` | lib + bin | The scripting toolchain on `efevm`: the `.efa` assembler (`src/asm`, with `--api` checks against a game's `.efapi`), the CLI (`run`, `build`, `dis`, `bundle`, `api`, `api-export`, `lsp`, `debug`) and the `std` host (`std`, `std.time`, `std.fs`) on `efevm::HostRegistry`. The legacy `.efe` assembler and VM were removed (E3.6). The high-level language is not written yet (lexer and empty AST stubs only). | [crates/efescript/structure.md](crates/efescript/structure.md) |
| `proc_macros` | `crates/efescript/proc_macros` | proc-macro | Emits `MAX_ADDR_SIZE` (2^48) and `STACK_PAGE_COUNT` (from a build-time env var) for efescript. Neither is used yet. | [structure.md](crates/efescript/proc_macros/structure.md) |
| `strtohx` | `crates/efescript/strtohx` | bin | Dev helper: converts text into `push64/32/16/8 0x…` lines to paste string constants into `.efe` programs. | [structure.md](crates/efescript/strtohx/structure.md) |
| `efevm` | `crates/efevm` | lib | the v1 core VM: verified modules (.efvm), register interpreter, fibers, budget, host imports; no dependencies, wasm32-compatible | [crates/efevm/structure.md](crates/efevm/structure.md) |
| `efepages` | `crates/efepages` | lib | Sparse paged byte memory (page index → fixed-size buffer); unwritten memory reads as zeros. It holds the VM's code, data and stack. | [crates/efepages/structure.md](crates/efepages/structure.md) |
| `page_size_macro` | `crates/efepages/page_size_macro` | proc-macro | Emits `PAGE_SIZE` from the build-time env var `efepage_size` (default 4096). | [structure.md](crates/efepages/page_size_macro/structure.md) |
| `calc` | `crates/calc` | lib + optional bin | Parses infix/prefix/postfix arithmetic into a tree, prints it in any notation, and simplifies it with variables. Integer (`i128`) and float (`f64`) numbers. The `cli` feature adds a clap command-line tool. | [crates/calc/structure.md](crates/calc/structure.md) |
| `test_example` | `examples/test_example` | lib + bin `native` | Sample game: creates an `Engine`, adds a world object, runs the game loop. Built as a native binary or a wasm library for the web. | — |
| `script_demo` | `examples/script_demo` | lib + bin `native` | Scripting example: a component and a host namespace (`game.score_add`), `.efa` scripts and a mod, all assembled against the exported API index. `cargo test -p script_demo` is the end-to-end test. | — |
| `input_test` | `examples/input_test` | lib + bin `native` | Manual input test: an `Update` system logs key, mouse, wheel, text, touch, focus and size changes. Run it natively and in the browser to check both backends. | — |

## How the crates depend on each other

```text
test_example ──┐
input_test ────┼► efeengine-core ──► efeengine-macros
script_demo ───┘        │
                        └──► efevm ◄── efescript

script_demo ┄┄ (dev-dependency, end-to-end test) ┄┄► efescript

efepages ──► page_size_macro      (standalone; no longer used by efescript)
proc_macros, calc, strtohx        (standalone)
```

The engine runs scripts on `efevm` itself (`engine::scripting` in `efeengine-core`). It does not
depend on `efescript`, which is the toolchain (assembler, CLI, LSP, debugger). `calc` is standalone.

## Build and run

`./build.py` (Python, standard library only; `python build.py` on Windows) is the main entry point:

| Command | Effect |
|---|---|
| `./build.py` | Interactive: choose example, native or web, build or run |
| `./build.py run <example> native` | Build and start the native (SDL3) binary (e.g. `input_test`) |
| `./build.py run <example> web [--port N]` | Build to wasm, assemble `dist/<example>/`, serve it on `http://localhost:8080/` with COOP/COEP headers |
| `./build.py dist <example>` | Release web build ready to upload to a static host |
| `cargo xtask api-export <example>` | Build natively and write the script API index to `target/efapi/<example>.efapi` (see `crates/core/structure.md`, scripting) |
| `./build.py check` / `test` / `doctor` / `clean` | Type-check, run tests, report missing tools, clean |
| `./build.py dev <linux\|web\|windows>` | Point rust-analyzer at a target (`.vscode/settings.json`) |

`cargo xtask` (the `xtask` crate) is the Rust port with the same commands, e.g. `cargo xtask run input_test native` or `cargo xtask run input_test web`.

Requirements:
- **Nightly Rust** with `rust-src` and the `wasm32-unknown-unknown` target. `rust-toolchain` selects nightly automatically.
- **Native:** system SDL3 development files (`SDL3-devel` / `libsdl3-dev`). Alternatively `--static` builds SDL3 from source, which needs cmake, a C compiler and the X11/Wayland, GL and ALSA headers.
- **Web:** `wasm-bindgen-cli` at the same version as `wasm-bindgen` in `Cargo.lock` (`./build.py doctor` prints the install command). The page must be served with COOP/COEP headers for wasm threads; `scripts/serve.py` does this locally and `templates/web/_headers` does it on Netlify/Cloudflare Pages.

## Where things stand

The crate `structure.md` files list known gaps with file references. The largest:
- **Web:** the wasm build starts through `__engine_start({...})`, initialises graphics (WebGPU, or WebGL2 on desktop Linux by default) before the frame loop and handles resize, focus, keyboard, pointer, wheel and text input; see `crates/core/structure.md` for remaining gaps.
- **Engine:** the renderer is a placeholder (one triangle per `(Transform, MeshHandle)`), the world is `hecs`, and scripts run in the `Update` stage. The input system, schedule, fixed-step loop and headless mode are in; gamepads are polled once per frame into `GamepadState` (read through `cx.gamepads`), compile-checked only. The native window and browser input paths are only compile-checked (use `input_test` by hand); the headless tests cover the loop and `InputState`.
- **efescript:** the `compiler/` front end is a stub; there is no high-level language yet. Toolchain gaps are listed in `crates/efescript/structure.md`.
- **efepages:** reads and writes that end exactly on a page boundary across several pages lose the last page's data, and `set_max_pages` is `todo!()`.
- **calc:** integer division by zero panics, and trailing tokens are silently ignored.
