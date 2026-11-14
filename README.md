# efeengine

Monorepo for the efe game engine.

| Path | Crate | What it is |
|---|---|---|
| `crates/core` | `efeengine-core` | Engine core: SDL3 (native) / web window, wgpu renderer, message queue |
| `crates/core/macros` | `efeengine-macros` | `#[main]` entry-point macro (native + wasm) |
| `crates/efescript` | `efescript` | Scripting language: assembler + bytecode VM (`proc_macros`, `strtohx` inside) |
| `crates/efepages` | `efepages` | Paged memory used by the efescript VM (`page_size_macro` inside) |
| `crates/calc` | `calc` | Expression parser / printer / simplifier |
| `examples/test_example` | `test_example` | Example game (native binary + wasm) |

The core crate is named `efeengine-core` rather than `core` because a crate called
`core` would clash with Rust's built-in `core` library.

## Branches

- `main`: the merged `main` branches of all projects.
- `world`: `main` plus realgameengine's `world` branch (hecs ECS, thread pool, window
  events). It uses nightly Rust (`rust-toolchain`) and wasm threads
  (`.cargo/config.toml`: atomics, shared memory, `-Z build-std`), and links the
  system SDL3 instead of building it from source.

## Building and running

Everything goes through `./build.py` (Python 3.9+, standard library only; on Windows use
`python build.py`). Run it without arguments to be asked which example to use, whether to
run it native or on the web, and whether to run or just build:

```sh
./build.py                                   # interactive
./build.py run   test_example native [--release] [--static]
./build.py run   test_example web    [--release] [--port 8080]
./build.py build test_example native|web [--release]
./build.py dist  test_example                # web publish folder
./build.py list | check | test | doctor
./build.py dev linux|web|windows|<triple>    # rust-analyzer target (.vscode/settings.json)
./build.py clean [test_example]
```

Add `--dry-run` to print the commands instead of running them. Native builds link the
system SDL3 (`sudo dnf install SDL3-devel`); `--static` builds SDL3 from source instead and
needs a C compiler and CMake (`sudo dnf install gcc gcc-c++ cmake`). `./build.py doctor`
reports what is missing.

### Rust alternative: `cargo xtask`

The same commands are also available as a Rust tool (`xtask/`, using clap, dialoguer and
tiny_http), so nothing but cargo is needed. `build.py` keeps working as before.

```sh
cargo xtask                                  # interactive (arrow keys)
cargo xtask run test_example native [--release] [--static]
cargo xtask run test_example web [--port 8080] [--host 0.0.0.0]
cargo xtask serve test_example               # serve dist/ again without rebuilding
cargo xtask build | dist | list | check | test | doctor | dev | clean
cargo xtask --help                           # every command and flag
```

## Web

Examples can be built to wasm and served in the browser (nightly Rust with `rust-src`
and the `wasm32-unknown-unknown` target, plus `wasm-bindgen-cli` matching `Cargo.lock`):

```sh
./build.py run test_example web [--release] [--port 8080]
./build.py dist test_example           # release build, publish-ready
```

`scripts/web.py` (also `scripts/web.sh`) still works directly:
`scripts/web.sh build|serve|dist <example> [--release] [--port N]`.

Output goes to `dist/<example>/` (gitignored). The page needs the
`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`
headers for wasm threads: `scripts/serve.py` sends them locally, and `_headers` covers
Netlify / Cloudflare Pages (GitHub Pages cannot set headers). See
[templates/web/README.md](templates/web/README.md).

## History

The original repositories (realgameengine, efescript, calc) were merged with their
full `main` history. Each "Import ..." commit is a merge whose second parent is the
old repository's `main`, so `git log --oneline <import-commit>^2` lists its history.
Files in those old commits sit at their original paths (e.g. `src/lib.rs`, not
`crates/core/src/lib.rs`).
