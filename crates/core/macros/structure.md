# efeengine-macros

A tiny proc-macro crate that provides the `#[main]` attribute for efeengine games. It rewrites the user's `main` function into a platform-specific entry point so the same game source builds as a native binary and as a wasm module. It is re-exported by `efeengine-core` (`pub use efeengine_macros::main`), so games normally get it from `efeengine_core::prelude::*`.

## At a glance
| | |
|---|---|
| Path | `crates/core/macros` |
| Kind | proc-macro (`[lib] proc-macro = true`) |
| Depends on (workspace) | none |
| Used by | `efeengine-core` (re-export); games such as `examples/test_example` through the prelude |
| Key external deps | `syn` 2 (`full`, parses the `ItemFn`), `quote` (generates tokens), `proc-macro2` (declared; not imported directly in the source) |
| Targets | the macro runs on the host at compile time; its output has separate native and `wasm32` branches |
| Features | none |

## Layout
```text
crates/core/macros
├── Cargo.toml    package metadata, proc-macro = true, deps syn/quote/proc-macro2
└── src/lib.rs    the single `#[proc_macro_attribute] pub fn main`
```

## Main concepts

### What `#[main]` does (`src/lib.rs`)
1. Parses the annotated item as `syn::ItemFn` (the attribute arguments are ignored).
2. Renames the function to `__engine_user_main`, keeping the original signature and body.
3. Emits the renamed function plus wrappers selected with `cfg`:
   - Native (`not(target_arch = "wasm32")`): `fn main() { __engine_user_main() }`, a normal binary entry point. The return value is passed through as the tail expression, so a `()` return is what works in practice.
   - Wasm (`target_arch = "wasm32"`): `use wasm_bindgen::prelude::*;`, then
     - `#[wasm_bindgen] pub fn __engine_start() { __engine_user_main() }`, the real start of the game; JS must call it explicitly.
     - `#[wasm_bindgen(start)] pub fn main() {}`, an empty start function. The in-code comment says it inits without starting the game so that workers do not start it.

### Why the wasm split matters
Web Workers in `efeengine-core` instantiate the same wasm module with shared memory. A start function that ran the game would run it in every worker, so the game start is moved to the explicit `__engine_start()` export that the page calls once.

## Public API
- `#[main]` (attribute macro, no arguments), applied to the game's `fn main()`.
- Generated items: `__engine_user_main` (renamed user fn), native `main`, wasm exports `__engine_start` and `main` (wasm-bindgen start).
- On wasm the expansion refers to `wasm_bindgen` by crate name (`use wasm_bindgen::prelude::*;`), so `wasm_bindgen` must resolve in the module where `#[main]` is used. The game example only depends on `efeengine-core`, relying on the prelude glob import (`pub use wasm_bindgen` on wasm) to bring it into scope.

## Platform notes
Native: standard `fn main`. Wasm: build the game crate as `cdylib` with the root `.cargo/config.toml` flags (atomics, shared memory) and nightly `build-std`; load the wasm-bindgen output, then call `__engine_start()`. The macro crate itself needs no special setup.

## Status and known gaps
- Attribute arguments are silently ignored; async or generic `main` is not handled specially.
- The wasm `main()` start function is intentionally empty (`src/lib.rs`, last block); no panic hook or logger setup in the macro (the panic hook is installed in `Engine::new` on wasm).
- The generated `main` and `use wasm_bindgen::prelude::*` are not hygienic and can collide with user items of the same name; the user's function name is replaced by `__engine_user_main`.
- `proc-macro2` is declared but unused directly; no tests.
