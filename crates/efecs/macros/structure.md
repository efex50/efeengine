# hecs-macros

Procedural macro crate for `efecs` (`hecs`), providing `#[derive(Bundle)]`, `#[derive(DynamicBundleClone)]`, and `#[derive(Query)]`. It is an optional dependency of `efecs` enabled via its `macros` feature, which re-exports all three derive macros at the `hecs` crate root.

## At a glance
| | |
|---|---|
| Path | `crates/efecs/macros` (package `hecs-macros`) |
| Kind | proc-macro (`[lib] proc-macro = true`) |
| Depends on (workspace) | none (`[dev-dependencies]` uses `hecs = { package = "efecs", features = ["macros"], path = ".." }` for doctests) |
| Used by | `efecs` (`crates/efecs`, optional dep via `macros` feature); `crates/efecs/tests/no-std-test-crates/macros` |
| Key external deps | `syn` 3.0 (`proc-macro`, `parsing`, `printing`, `derive`, `clone-impls`, `visit-mut`), `quote` 1.0, `proc-macro2` 1.0 |
| Features | none |
| Targets | runs on host at compile time; generated code targets native and `wasm32` (`#![no_std]` compatible via `::core` and `::hecs` paths) |
| Tests | doctests in `src/lib.rs`; tested end-to-end by `crates/efecs/tests/derive.rs` (`trybuild`: 8 pass + 3 compile-fail cases in `crates/efecs/tests/derive/`) and `crates/efecs/tests/no-std-test-crates/macros` |

## Layout
```text
crates/efecs/macros
├── Cargo.toml          package `hecs-macros`, `proc-macro = true`, `syn` / `quote` / `proc-macro2` deps, `efecs` dev-dep
├── LICENSE-APACHE      upstream Apache-2.0 license
├── LICENSE-MIT         upstream MIT license
├── structure.md        this file
└── src/
    ├── lib.rs          proc-macro entry points: `derive_bundle`, `derive_dynamic_bundle_clone`, `derive_query`
    ├── common.rs       `struct_fields` and `member_as_idents` helpers for named, tuple, and unit structs
    ├── bundle.rs       `#[derive(Bundle)]` codegen (`DynamicBundle` + `Bundle` impls)
    ├── bundle_clone.rs `#[derive(DynamicBundleClone)]` codegen (`DynamicBundleClone::put_with_clone`)
    └── query.rs        `#[derive(Query)]` codegen (`Query` + `Fetch` impls and companion `*Fetch` / `*State` types)
```

## Generated code & fork adaptations

- **`#[derive(Bundle)]` (`src/bundle.rs`)**:
  - Accepts named, tuple, and unit structs; rejects enums and unions. Adds `: ::hecs::Component` bounds to generic type parameters.
  - Emits `unsafe impl ::hecs::DynamicBundle` (`has`, `key`, `with_ids`, `type_info`, `put`) and `unsafe impl ::hecs::Bundle` (`with_static_ids`, `with_static_type_info`, `get`).
  - Adapted for `efecs`: sorts and caches `::hecs::ComponentId` arrays (`::hecs::ComponentId::of::<T>()`) via `::hecs::spin::Lazy` for non-generic structs instead of raw `TypeId`.
- **`#[derive(DynamicBundleClone)]` (`src/bundle_clone.rs`)**:
  - Accepts structs; rejects enums and unions. Adds `: ::core::clone::Clone` bounds to generic type parameters.
  - Emits `unsafe impl ::hecs::DynamicBundleClone` calling `f` with `::hecs::TypeInfo::of::<T>()` and `::hecs::DynamicClone::new::<T>()`, then `core::mem::forget` on moved fields.
- **`#[derive(Query)]` (`src/query.rs`)**:
  - Accepts structs and enums with **exactly one lifetime parameter and no type parameters** (replaces that lifetime with `'static` when resolving field `::hecs::Query` types).
  - Emits a `const _: () = { ... }` block containing `{Ident}Fetch`, `{Ident}State`, `impl<'a> ::hecs::Query`, and `unsafe impl ::hecs::Fetch`. For enums without any fieldless variant, adds a `__HecsDanglingFetch__` sentinel variant (reserving that variant name).
  - Adapted for `efecs`: `Fetch::execute` takes `ticks: ::hecs::QueryTicks`, `Fetch::for_each_borrow` passes `::hecs::ComponentId`, and `Fetch::HAS_FILTER` / `Fetch::filter` propagate per-field tick filter behavior (ORed across struct/variant fields in `HAS_FILTER`, ANDed in `filter`).

## Public API
- `#[derive(Bundle)]` (`derive_bundle`)
- `#[derive(DynamicBundleClone)]` (`derive_dynamic_bundle_clone`)
- `#[derive(Query)]` (`derive_query`)

All generated paths qualify `::hecs` and `::core` (plus `::hecs::alloc` and `::hecs::spin`), so the consumer crate must have `hecs` in scope (whether from `efecs` directly or `efeengine_core::hecs` aliased as `hecs`).
