# efa

`efa` is the `.efa` compiler and mod-folder builder that the engine runs at load time: the assembler (`asm`), the linker (`link`), and the mod-folder builder (`mod_folder`, including optional `.wasm` mod translation behind the `wasm` feature). It was moved out of `efescript` in E10 so that games can compile `.efa` sources without the `efescript` toolchain. The design is in `crates/efescript/docs/e10-design.md`.

## At a glance
| | |
|---|---|
| Path | `crates/efa` |
| Kind | library (`efa`); `fuzz/` is a separate `cargo-fuzz` crate (`efa-fuzz`), excluded from the root workspace |
| Depends on (workspace) | `efevm`, `efemod` (reader only, never the `write` feature), `calc` (`default-features = false`: constant expressions), `efe-wasm` (optional, feature `wasm`) |
| Key external deps | `toml` 1 (reads `mod.toml`) |
| Used by | `efeengine-core` (optional feature `efa`, enabled by default, re-exported as `efeengine_core::efa`; `wasm-mods` enables `efa/wasm`), `efescript` (enables `efa/wasm`, re-exports `asm` and `link`), `efevm-jit` (`dev-dependencies`), and `efa-fuzz` (`crates/efa/fuzz`) |
| Features | `wasm` (`dep:efe-wasm`; enables `.wasm` translation in `mod_folder::build_mod` and `mod_folder::to_fallback`) |
| Targets | `wasm32-unknown-unknown` as well as native; the file-system helpers (`FsSource`, `MAX_FILE_BYTES`, `list_assets`, `collect_files`) are native-only |
| Tests | unit tests in `src/asm/{lexer.rs, parser.rs, diag.rs}`, `src/link/tests.rs`, and `src/mod_folder.rs`; integration tests `tests/const_expr.rs` and `tests/never_panics.rs`; fuzz target `fuzz/fuzz_targets/build_mod.rs` |

## Layout
```text
crates/efa
├── Cargo.toml           package efa 0.1.0, edition 2021; optional feature `wasm`
├── src/
│   ├── lib.rs           `pub mod asm, link, mod_folder`; `VERSION` (the engine's disk cache key includes it)
│   ├── asm/             the assembler (moved from efescript with limits and `read_bytes`)
│   │   ├── lexer.rs, parser.rs, ast.rs   tokens, syntax tree (`use`, `pub`, `provide`, `struct`, `scratch`, `import event` / `provide event`, `ConstExpr`)
│   │   ├── diag.rs      diagnostics (`Diagnostic`, `Diagnostics`, `did_you_mean`): file, line, column and a caret under the span
│   │   ├── source.rs    `FileSource` (`read`, `read_bytes`, `display_name`, `list`: the engine hashes a mod folder from it), `MemSource`, `FsSource` (native: `new`, `for_file`, `with_prefix`, `MAX_FILE_BYTES`; refuses escapes and symlinks out of the root)
│   │   ├── lower/       one file as `asm::Unit`: `mod.rs` (`declare`, `resolve`, `define`), `func.rs`, `value.rs`, `consts.rs` (constant expressions, `Fold`: lazy evaluation of consts, struct layouts and data sizes)
│   │   └── mod.rs       `parse`, `assemble`, `assemble_with`, `assemble_program`, `assemble_programs`; `AsmOptions { api, max_use_depth, max_nesting }`
│   ├── link/            the linker: `mod.rs` (`Unit`, `Decls`, `Resolved`, `SymbolMap`, `Declared`, `StaticValue`, `link`, `LinkOptions`, `struct_consts`), `layout.rs` (phase 1: indices, data, scratch, memory, program-wide checks, `interface`), `tests.rs`
│   └── mod_folder.rs    mod folders: `read_mod_toml`, `ModToml`, `BuildToml`, `ProvideToml`, `ProvideEventToml`, `inside`, `BuildLimits`, `PreparedMod`, `BuiltMod`, `prepare_mod`, `to_fallback` (`wasm` feature), `finish_mod`, `build_mod`, `LimitedSource`, `asm_options`, `mod_index`, `list_assets` (native)
├── tests/
│   ├── const_expr.rs    constant expressions: positions, precedence, names, typing, errors and spans, other files
│   └── never_panics.rs  mutated seed folders, long `use` chains and deep nesting: errors, never panics
└── fuzz/
    ├── Cargo.toml       separate `efa-fuzz` crate (`[workspace]`, excluded from root workspace)
    └── fuzz_targets/build_mod.rs   cargo-fuzz target for `build_mod` and `assemble_program`
```

## Main concepts
- **One input abstraction.** The assembler reads everything through `FileSource`. The engine passes `FsSource` (native files), `MemSource` (tests and in-memory mods), and wrappers: `LimitedSource` (limits on files and bytes, counting each path once) and the recording wrapper in core's disk cache.
- **Limits before recursion.** `AsmOptions::max_use_depth` and `max_nesting` turn over-limit input into a diagnostic. The nesting check runs before any recursive parsing. `BuildLimits` (defaults: 32 MiB of files read, 1024 files, use depth 64, nesting 128) apply to mod-folder builds.
- **Folder builds are split.** `prepare_mod` reads `mod.toml` (`read_mod_toml`, which parses `[mod]`, `[dependencies]`, `[permissions]`, `[build]`, `[meta]` -> `Manifest::meta`, `[[provide]]`, and `[[provide_event]]`), validates the manifest, and checks the `[build]` paths (`entry` or `wasm`, `assets`, and `api`, which is checked with `inside()` and otherwise ignored). The engine compiles the entry between `prepare_mod` and `finish_mod`, so the compile can go through its cache. `finish_mod` runs `efemod::check_module` and reads the assets as bytes, refusing invalid asset names and the reserved names `manifest.json`, `module.efvm`, `engine` and `engine/*`. `build_mod` runs all three steps.
- **Wasm mod folders (feature `wasm`).** When `[build] wasm` is set or `[build] entry` ends in `.wasm`, `build_mod` reads the `.wasm` bytes and translates them with `efe_wasm::translate` using `to_fallback` (mapping `[[provide]]`, `[[provide_event]]`, and `[build] scratch` into `efe_wasm::Fallback`) instead of running the `.efa` assembler. Without the `wasm` feature, `.wasm` mod folders return an error.
- **Constant expressions** (efa-syntax 18). `parser::const_operand` converts the efa tokens of an expression (in the positions `memory`, `const`, `global`, data values, struct counts, `scratch`, load/store offsets, `const.T` values) to `calc` tokens whose byte offsets are synthetic (`k..k+1`) and runs `calc::Parser::parse_expr`, which stops at the first token that cannot continue it. The AST keeps `Operand::Expr(Box<ConstExpr>)` = the `calc::Expr` + `spans[k]` (the efa span of token `k`); a lone literal or name stays `Int`/`Float`/`Char`/`Name`, so its output is byte-identical to before. A negative literal after an operand is split into `-` and the number. Evaluation: `lower::consts::Fold` evaluates consts, struct layouts and data sizes on demand in `declare` (cycles reported at the closing use), the names a const reads come from `env.values` (API consts, struct consts including `S.id` when `component` is set, `alias.NAME` of used files), and a const that needs a data address or event id is "late" (computed in `resolve`, never `pub`). Other files' `pub` consts are known at declare time because `link::declare_all` declares a unit's `Unit::deps()` first and passes their `Decls` (`link::Declared`, `link::static_values`). The folded `calc::Value` becomes a `Val` (floats as `f64` text) and is narrowed by `type_val` at the use site.
- **Diagnostics name the file.** `FsSource::with_prefix` sets the display name of a mod folder, so errors name the folder.

## Tests
`cargo test -p efa` runs the unit tests in `lexer.rs`, `parser.rs`, `diag.rs`, `link/tests.rs` and `mod_folder.rs`, plus `tests/const_expr.rs` and `tests/never_panics.rs`. The assembler's end-to-end tests are in `crates/efescript/tests`. `cargo fuzz run build_mod` from `crates/efa/fuzz` needs `cargo-fuzz`. The engine's load tests are in `crates/core/tests/load.rs`.
