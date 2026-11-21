# proc_macros

Tiny proc-macro crate in `crates/efescript/proc_macros`. It exposes two function-like macros that expand to `pub const` definitions so values can be fixed at compile time of a consuming crate. Originally used by the legacy `.efe` VM in `efescript`, it is no longer depended on after `.efe` removal (roadmap E3.6), though the crate remains in the workspace.

## At a glance
| | |
|---|---|
| Path | `crates/efescript/proc_macros` |
| Kind | proc-macro (`[lib] proc-macro = true`) |
| Depends on (workspace) | none |
| Used by | nothing in the workspace (`efescript` dropped the dependency in E3.6) |
| Key external deps | none (only compiler-provided `proc_macro`) |
| Targets | host proc-macro (target-independent generated `usize` consts) |
| Features | none |
| Tests | none |

## Layout
```text
crates/efescript/proc_macros/
├── Cargo.toml    package proc_macros 0.1.0, edition 2021, [lib] proc-macro = true, no deps
├── src/lib.rs    the two proc macros (`max_addr!`, `stack_page_count!`, 28 lines)
└── structure.md  this file
```

## Main concepts
Both macros ignore their input `TokenStream` and format a `pub const` declaration that is parsed back into a `TokenStream`:

- `max_addr!()` expands to `pub const MAX_ADDR_SIZE:usize = 281474976710656;` (`2_usize.pow(48)`).
- `stack_page_count!()` expands to `pub const STACK_PAGE_COUNT:usize = N;`, reading `N` from the `STACK_PAGE_COUNT` environment variable at macro-expansion time (default `"2"`). Non-numeric values print the `ParseIntError` to stderr and panic, aborting compilation. Because `std::env::var` is read without `rerun-if-env-changed`, Cargo does not track changes to `STACK_PAGE_COUNT` automatically.

Legacy usage (before E3.6 removed the dependency from `efescript`):
```rust
proc_macros::max_addr!();
proc_macros::stack_page_count!();
```

## Public API
- `#[proc_macro] pub fn max_addr(_i: TokenStream) -> TokenStream` -> `pub const MAX_ADDR_SIZE: usize`
- `#[proc_macro] pub fn stack_page_count(_i: TokenStream) -> TokenStream` -> `pub const STACK_PAGE_COUNT: usize`

## Status and known gaps
- Unused anywhere in the workspace since `efescript` removed the legacy `.efe` VM in E3.6.
- An older `efearena_size` design remains as commented-out lines in `src/lib.rs`.
- No `cargo:rerun-if-env-changed` tracking for `STACK_PAGE_COUNT`; no tests.
