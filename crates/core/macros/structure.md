# efeengine-macros

Proc-macro crate providing the `#[main]` entry-point attribute, the `#[derive(ScriptType)]` layout derive, the `#[script_component]` attribute, and the `#[derive(Component)]` derive for efeengine games and built-in engine components. `#[main]` rewrites the user's `main` function into platform-specific native and `wasm32` entry points so the same game source builds as a native binary and as a wasm module. All four macros are re-exported by `efeengine-core::prelude` (`pub use efeengine_macros::{main, Component, script_component, ScriptType}`).

## At a glance
| | |
|---|---|
| Path | `crates/core/macros` |
| Kind | proc-macro (`[lib] proc-macro = true`, edition 2024) |
| Depends on (workspace) | none |
| Used by | `efeengine-core` (re-export in `prelude.rs`; derives `ScriptType` / uses `#[script_component]` on its own components); games, plugins, and examples through `efeengine_core::prelude::*` |
| Key external deps | `syn` 3 (`full`, parses `ItemFn`, `DeriveInput`, and helper attributes), `quote` 1 (`quote!` / `quote_spanned!`), `proc-macro2` 1 (`TokenStream`) |
| Targets | host at compile time; generated code has separate `not(target_arch = "wasm32")` and `target_arch = "wasm32"` branches |
| Features | none |
| Tests | none in this crate; exercised via `efeengine-core` doctests (`src/engine/types.rs`) and integration tests (`tests/components_derive.rs`, `tests/world.rs`, `tests/readonly.rs`, etc.) |

## Layout
```text
crates/core/macros
├── Cargo.toml    package metadata, proc-macro = true, deps proc-macro2 / syn (full) / quote
├── structure.md  this file
└── src/lib.rs    `#[main]`, `#[derive(ScriptType)]`, `#[script_component]`, `#[derive(Component)]`
```

## Main concepts

### `#[main]` (`src/lib.rs`)
1. Parses the annotated item as `syn::ItemFn` (attribute arguments are ignored).
2. Renames the function to `__engine_user_main`, keeping its original signature and body.
3. Emits the renamed function plus target-selected wrappers:
   - Native (`not(target_arch = "wasm32")`): `pub fn run() { __engine_user_main() }` (called by binary `src/main.rs` crates wrapping a game library) and `#[allow(dead_code)] fn main() { run() }` (so a standalone binary works directly).
   - Wasm (`target_arch = "wasm32"`): `use wasm_bindgen::prelude::*;`, then:
     - `#[wasm_bindgen] pub fn __engine_start(config: JsValue) -> Result<(), JsValue>`, the real game entry point called explicitly by JS (e.g. `__engine_start({ wasmJsPath: "./pkg/<crate>.js" })`). Calls `efeengine_core::engine::config::__start_from_js(config)?` and then `__engine_user_main()`.
     - `#[wasm_bindgen(start)] pub fn main() {}`, an intentionally empty start function so Web Workers instantiating the same shared-memory wasm module do not start the game loop.

### `#[derive(ScriptType)]` (`src/lib.rs`)
For a `#[repr(C)]` struct with named fields and no generics (other `repr` hints such as `align(16)` may follow `C`), emits `impl ::efeengine_core::engine::types::ScriptType for T`:
- `NAME`: defaults to the Rust ident string; overridden by `#[script(name = "...")]`.
- `layout() -> ::efeengine_core::efevm::StructDesc`: size and alignment from `size_of` / `align_of`, `.doc(...)` from `///` doc comments (each line trimmed and joined with `\n`), `.readonly(bool)` from `#[script(readonly)]`, and one `FieldDesc` per field using `::core::mem::offset_of!(Self, field)` and `<FieldTy as ScriptField>::PRIM` / `::COUNT` plus field `///` doc comments.
- `#[script(entity)]` on a field emits `Prim::Entity` and adds a compile-time `assert!` checking that `<FieldTy as ScriptField>::PRIM` is `Prim::U64`.
- Rejects generics, enums, unions, tuple/unit structs, unknown `#[script(...)]` keys, and missing `#[repr(C)]` at the offending span. Requires `bytemuck::Pod` (and `efeengine_core` in scope; `crates/core/src/lib.rs` uses `extern crate self as efeengine_core;`).

### `#[script_component]` (`src/lib.rs`)
Attribute macro `#[script_component(name = "...", tracked, readonly)]` for a struct with named fields and no generics:
- Rejects generics, enums, unions, tuple/unit structs, and any user-supplied `#[repr(...)]`.
- Strips `Clone`, `Copy`, `Pod`, `Zeroable`, and `ScriptType` from the user's own `#[derive(...)]` lists so they are not derived twice, then emits `#[repr(C)]`, `#[derive(::core::clone::Clone, ::core::marker::Copy, ::efeengine_core::bytemuck::Pod, ::efeengine_core::bytemuck::Zeroable, ::efeengine_core::prelude::ScriptType)]`, and `#[bytemuck(crate = "::efeengine_core::bytemuck")]` (so games need no direct `bytemuck` dependency).
- Forwards `#[script(name = ...)]` and `#[script(readonly)]` to `ScriptType`.
- Emits `impl ::efeengine_core::engine::types::ComponentInfo for T` with `NAME`, `TRACKED`, `SCRIPT = true`, and `register(engine)`: calls `engine.current_owner()`, `engine.register_component_owned::<Self>(&owner)?`, and `engine.track_changes::<Self>()` when `TRACKED` is true, returning `Ok(Some(id))`.

### `#[derive(Component)]` (`src/lib.rs`)
Derive macro with optional `#[component(name = "...", tracked)]` (`readonly` is rejected; only `name` and `tracked` are accepted):
- Works on any type (structs, tuple/unit structs, enums, generics).
- Emits `impl ComponentInfo` with `SCRIPT = false` and `register(engine)`: calls `engine.check_registration_open(type_name::<Self>())?`, calls `engine.track_changes::<Self>()` when `TRACKED` is true, and returns `Ok(None)`.

## Public API
- `#[main]` — attribute macro transforming `fn main()` into native `run()` + `main()` and wasm `__engine_start(config)` + empty `#[wasm_bindgen(start)] main()`.
- `#[derive(ScriptType)]` with helper `#[script(name = "...", readonly, entity)]`.
- `#[script_component]` / `#[script_component(name = "...", tracked, readonly)]` — attribute macro generating `#[repr(C)]`, `Clone`/`Copy`/`Pod`/`Zeroable`/`ScriptType`, and `ComponentInfo` (`SCRIPT = true`).
- `#[derive(Component)]` with helper `#[component(name = "...", tracked)]` — implements `ComponentInfo` (`SCRIPT = false`).
- Generated items from `#[main]`: `__engine_user_main`, native `pub fn run()` and `fn main()`, wasm exports `__engine_start` and `main`.
- On wasm, `#[main]` expands `use wasm_bindgen::prelude::*;`, so `wasm_bindgen` must be in scope where `#[main]` is used (`efeengine_core::prelude::*` re-exports `wasm_bindgen` on wasm32).

## Platform notes
- Native: builds a normal `pub fn run()` and `fn main()`.
- Wasm: build the game crate as `cdylib` with the workspace `.cargo/config.toml` atomics/shared-memory flags and nightly `-Z build-std`; load the wasm-bindgen output and call `__engine_start({ ... })` once from JS.

## Status and known gaps
- `#[main]` ignores attribute arguments and does not handle `async` or generic `main`.
- The wasm `#[wasm_bindgen(start)] pub fn main()` is intentionally empty; panic-hook and logger setup happen in `Engine::with_config` rather than the macro.
- The generated `main`, `run`, and `use wasm_bindgen::prelude::*` items are non-hygienic and can collide with user items of the same name; the user's function is renamed to `__engine_user_main`.
