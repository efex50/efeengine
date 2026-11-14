# page_size_macro

`page_size_macro` is a tiny function-like proc-macro crate that generates the `PAGE_SIZE` constant for `efepages`. It reads the environment variable `efepage_size` at compile time and falls back to `4096`. It exists so the page size is a single compile-time constant shared by the paged memory and its users.

## At a glance
| | |
|---|---|
| Path | `crates/efepages/page_size_macro` |
| Kind | proc-macro (no binary) |
| Depends on (workspace) | none |
| Used by | `efepages` (`crates/efepages/Cargo.toml:8`, invoked from `crates/efepages/src/vars.rs`) |
| Key external deps | none (only `proc_macro` from the compiler) |
| Targets | Host-only at build time (proc-macros are compiled for the host). The generated code is target-independent. |
| Features | none |

## Layout
```text
crates/efepages/page_size_macro/
├── Cargo.toml     # package `page_size_macro`, edition 2021, [lib] proc-macro = true, no deps
└── src/lib.rs     # the single exported macro `page_size`
```

## Main concepts
- **`page_size!()`** (`src/lib.rs:4-9`): the only item. It ignores its input token stream, reads `efepage_size` with `std::env::var`, defaults to `"4096"`, and returns the tokens of `pub const PAGE_SIZE:usize = <value>;`. The text is parsed with `String::parse` into a `TokenStream` and the result is `unwrap`ped.
- **Usage**: `efepages/src/vars.rs` contains `use page_size_macro::page_size;` and `page_size!();`, so `efepages::vars::PAGE_SIZE` is defined in that module.
- **Build-time only**: the environment variable is read while the consuming crate is compiled. Cargo does not track it as an input, so changing it does not trigger a rebuild on its own; run `cargo clean -p efepages` (or touch the crate) after changing it.
- **Value is spliced as-is**: whatever string is in `efepage_size` becomes Rust source. A non-numeric value makes the macro's `parse().unwrap()` panic during compilation, and an expression such as `4096 * 2` would be accepted.

```rust
// crates/efepages/src/vars.rs
use page_size_macro::page_size;
page_size!();                // expands to: pub const PAGE_SIZE: usize = 4096;
```

```sh
efepage_size=8192 cargo build -p efepages   # then rebuild dependents if needed
```

## Public API
- `page_size!()` - function-like proc macro; takes no meaningful input and expands to `pub const PAGE_SIZE: usize = N;`.

## Status and known gaps
- The environment variable is read as an unvalidated string (`src/lib.rs:7`). Non-numeric values panic at compile time with an unhelpful error (`src/lib.rs:9`).
- There is no check that the page size is a power of two or non-zero. A value of `0` would make `efepages` divide by zero at runtime (`page/funs.rs` `start / self.pagesize`).
- A commented-out alternative using `env!("efearena_size")` remains at `src/lib.rs:6`; it is not used.
- No tests in this crate.
