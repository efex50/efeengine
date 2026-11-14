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

## Building

Native builds compile SDL3 from source, so you need a C compiler and CMake
(`sudo dnf install gcc gcc-c++ cmake`).

```sh
cargo build --workspace
cargo run -p test_example --bin native
```

## History

The original repositories (realgameengine, efescript, calc) were merged with their
full `main` history. Each "Import ..." commit is a merge whose second parent is the
old repository's `main`, so `git log --oneline <import-commit>^2` lists its history.
Files in those old commits sit at their original paths (e.g. `src/lib.rs`, not
`crates/core/src/lib.rs`).
