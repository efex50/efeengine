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
│   ├── efescript/          efescript: v2 `.efa` assembler and CLI on efevm, legacy assembler + VM (scripting layer)
│   │   ├── proc_macros/    build-time constants for efescript
│   │   └── strtohx/        dev tool: text → push instructions for .efe programs
│   ├── efevm/              efevm: v1 core VM (verified .efvm modules, interpreter, fibers); the v2 path runs on it
│   ├── efepages/           efepages: sparse paged memory used by the VM
│   │   └── page_size_macro/ build-time PAGE_SIZE constant
│   └── calc/               calc: expression parser / printer / simplifier (+ CLI)
├── examples/
│   ├── test_example/       sample game (native binary + wasm library)
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
| `efescript` | `crates/efescript` | lib + bin | The scripting layer, with two paths. v2 (current): the `.efa` assembler (`src/asm`) lowers assembly text to an `efevm` module, and the CLI runs it (`efescript run`), builds it (`build`) or prints it back (`dis`); the legacy assembler is kept until E3.6. Legacy: an x86-flavoured assembler ("nasm_efe") and a register VM (12 registers, flags, syscalls, threads) run `.efe` files inside one `efepages` memory (`efescript file.efe`). The high-level language is not written yet (lexer and empty AST stubs only). | [crates/efescript/structure.md](crates/efescript/structure.md) |
| `proc_macros` | `crates/efescript/proc_macros` | proc-macro | Emits `MAX_ADDR_SIZE` (2^48) and `STACK_PAGE_COUNT` (from a build-time env var) for efescript. Neither is used yet. | [structure.md](crates/efescript/proc_macros/structure.md) |
| `strtohx` | `crates/efescript/strtohx` | bin | Dev helper: converts text into `push64/32/16/8 0x…` lines to paste string constants into `.efe` programs. | [structure.md](crates/efescript/strtohx/structure.md) |
| `efevm` | `crates/efevm` | lib | the v1 core VM: verified modules (.efvm), register interpreter, fibers, budget, host imports; no dependencies, wasm32-compatible | [crates/efevm/structure.md](crates/efevm/structure.md) |
| `efepages` | `crates/efepages` | lib | Sparse paged byte memory (page index → fixed-size buffer); unwritten memory reads as zeros. It holds the VM's code, data and stack. | [crates/efepages/structure.md](crates/efepages/structure.md) |
| `page_size_macro` | `crates/efepages/page_size_macro` | proc-macro | Emits `PAGE_SIZE` from the build-time env var `efepage_size` (default 4096). | [structure.md](crates/efepages/page_size_macro/structure.md) |
| `calc` | `crates/calc` | lib + optional bin | Parses infix/prefix/postfix arithmetic into a tree, prints it in any notation, and simplifies it with variables. Integer (`i128`) and float (`f64`) numbers. The `cli` feature adds a clap command-line tool. | [crates/calc/structure.md](crates/calc/structure.md) |
| `test_example` | `examples/test_example` | lib + bin `native` | Sample game: creates an `Engine`, adds a world object, runs the game loop. Built as a native binary or a wasm library for the web. | — |
| `input_test` | `examples/input_test` | lib + bin `native` | Manual input test: an `Update` system logs key, mouse, wheel, text, touch, focus and size changes. Run it natively and in the browser to check both backends. | — |

## How the crates depend on each other

```text
test_example ──┐
input_test ───┴► efeengine-core ──► efeengine-macros

efescript ──► efepages ──► page_size_macro
    ├──────► proc_macros
    └──────► efevm

calc            (standalone)
strtohx         (standalone)
```

The engine and the scripting side are **not connected yet**: `efeengine-core` does not
depend on `efescript` or `calc`, and nothing depends on `calc`. The intended next
step is for the engine to host the efescript VM (scripts ticking inside the frame
loop and talking to the engine through the system schedule and `Message`s).

## Build and run

`./build.py` (Python, standard library only; `python build.py` on Windows) is the main entry point:

| Command | Effect |
|---|---|
| `./build.py` | Interactive: choose example, native or web, build or run |
| `./build.py run <example> native` | Build and start the native (SDL3) binary (e.g. `input_test`) |
| `./build.py run <example> web [--port N]` | Build to wasm, assemble `dist/<example>/`, serve it on `http://localhost:8080/` with COOP/COEP headers |
| `./build.py dist <example>` | Release web build ready to upload to a static host |
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
- **Engine:** the renderer is a single fixed triangle and `hecs` (planned for the Phase 3 game world) is not used yet. The input system, schedule, fixed-step loop and headless mode are in; gamepads are polled once per frame into `GamepadState` (read through `cx.gamepads`), compile-checked only. The native window and browser input paths are only compile-checked (use `input_test` by hand); the headless tests cover the loop and `InputState`.
- **efescript:** many inputs panic instead of returning errors. Several opcodes and syscalls are `todo!()`. The `Finish` syscall's result is ignored. Threads hold raw pointers into the runtime. There is no high-level language yet.
- **efepages:** reads and writes that end exactly on a page boundary across several pages lose the last page's data, and `set_max_pages` is `todo!()`.
- **calc:** integer division by zero panics, and trailing tokens are silently ignored.
