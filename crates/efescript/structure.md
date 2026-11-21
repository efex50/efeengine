# efescript

`efescript` is the scripting layer and CLI tooling of efeengine. A text assembler (`crates/efa`) lowers `.efa` source to an `efevm` module (`.efvm`); the `efescript` library and CLI assemble, run, disassemble, bundle, pack `.efmod` packages, inspect modules/packages, export and merge API indices, generate Rust and C/C++ guest bindings (`api-export --rust|--c`), and serve an LSP server and a line debugger. The engine core (`efeengine-core`) does not depend on this crate and runs `.efvm` modules on `efevm` directly. The legacy `.efe` VM/format was removed in roadmap E3.6; `.efe`/`.efec` files are rejected with exit code 2. Design notes live in `docs/`.

## At a glance
| | |
|---|---|
| Path | `crates/efescript` |
| Kind | library (`efescript`) plus two binaries: `efescript` (`src/main.rs`, `default-run`) and `efescript-runner` (`src/bin/runner.rs`, runs an embedded module) |
| Depends on (workspace) | `efevm`, `efa` (with `wasm` feature), `efemod` (with `write` feature) |
| Used by | `xtask` (`[dependencies]`); `[dev-dependencies]` of `efevm-jit`, `examples/script_demo`, and `examples/coin_dash_wasm` |
| Key external deps | `toml` 1, `lsp-server` 0.10, `lsp-types` 0.97, `serde_json` 1.0.154, `rodio` 0.23 (optional, `audio` feature); dev-dep `wasmparser` 0.245 (`tests/guest_wasm.rs`) |
| Features | `audio` (`dep:rodio` for real sound playback in `std.play_sound`; needs ALSA headers on Linux) |
| Targets | native (the `std` host, CLI, LSP, debugger, bundler, and packer are native-only; `efevm` and `efa` carry the portable VM/assembler core) |
| Tests | 16 integration test files under `tests/`, plus unit tests in `src/main.rs`, `src/bundle.rs`, `src/host_std.rs`, `src/disasm.rs`, `src/run_v2.rs`, `src/lsp/{text,project}.rs`, and `src/compiler/ast/lexer/mod.rs` |

## Layout
```text
crates/efescript
├── Cargo.toml           package efescript; bins efescript and efescript-runner
├── README.md            user guide: CLI, exit codes, host functions, language, tools
├── ROADMAP.md           E-items and status notes
├── docs/
│   ├── efa-syntax.md         full `.efa` syntax reference
│   ├── efa-instructions.md   guide to every instruction (`efevm` mnemonics)
│   ├── e0-design.md          original design (the removed `.efe` path; history)
│   ├── e2-design.md          `.efa` assembler and v2 runner
│   ├── e3-design.md          host registry, std host, API index, LSP, debugger, engine integration
│   ├── e7-cranelift-design.md Cranelift JIT backend design notes
│   ├── e7.4-wasm-design.md   guest wasm ABI, binding generators, custom sections
│   ├── e8-design.md          multi-file `.efa` modules, linker, and `.efmod` packages
│   └── e10-design.md         engine loading API, script cache, mod sources
├── examples/            `.efa` programs with expected output in header comments (`hello`, `strings`,
│   │                    `jumps`, `recursion`, `floats`, `sound` + `ominous_worse.ogg`, `exit`, `echo`,
│   │                    `write_file`), plus legacy `main.sfe`
├── src/
│   ├── main.rs          CLI: `run`, `build`, `dis`, `bundle`, `api`, `api-export` (std host, `.efmod`/`.efvm`, or `--rust`/`--c` bindings), `pack`, `inspect`, `lsp`, `debug`, and `<file.efa>` shorthand
│   ├── lib.rs           public module exports (re-exports `efa::{asm, link}`)
│   ├── prelude.rs       common re-exports (`CaptureHost`, `Host`, `StdHost`)
│   ├── host.rs          `Host` trait (output, sound), `StdHost` (process stdio + optional `rodio`), `CaptureHost` (tests)
│   ├── host_std.rs      `std_registry(&StdOptions)`: `HostRegistry` of `std`, `std.time`, `std.fs`; `StdEnv<H>`, `StdOptions`
│   ├── run_v2.rs        `load_path`/`load_path_with`/`load_bytes`, `run`/`run_with`/`run_embedded`, trap reports
│   ├── disasm.rs        `.efvm` -> `.efa` disassembler (`disassemble`, `disassemble_data`); uses `names` section
│   ├── pack.rs          mod packaging: `pack_dir` -> `Packed` (`.efmod` + `.api` `ApiIndex` fragment), `read_api_input` (`.efapi`/`.efmod`), and re-exports from `efa::mod_folder`
│   ├── bindmodel.rs     language-neutral binding model built from `ApiIndex` (`Node` tree, padded `Layout`, `wasm_lower`)
│   ├── rustgen.rs       `generate(&ApiIndex)` for `api-export --rust` (`#![no_std]` `wasm32` bindings using `efe-guest` + `efe.api` custom section)
│   ├── cgen.rs          `generate(&ApiIndex)` for `api-export --c` (`efe_api.h` header for clang `wasm32` + `EFE_API_IMPLEMENTATION` `efe.api` section)
│   ├── bundle.rs        standalone executables: runner + module bytes + 16-byte `EFVMBNDL` trailer
│   ├── debugger/mod.rs  `Debugger<H>`: line-oriented `efescript debug` command loop over `efevm` stepping hooks
│   ├── lsp/             `Server::handle(Message) -> Vec<Message>`, `run_stdio` / `run_stdio_code`
│   │   ├── mod.rs       protocol loop, capabilities, `definition`, `completion`, `hover`, index reload
│   │   ├── analysis.rs  diagnostics and symbol tables from the last AST that parsed
│   │   ├── project.rs   multi-file programs: buffer-first `BufSource`, program root from nearest `mod.toml` `[build] entry`, per-file diagnostics
│   │   └── text.rs      UTF-16/line-column text helpers
│   ├── bin/runner.rs    `efescript-runner`: runs the module embedded in its own executable
│   └── compiler/        high-level language frontend stub (`mod.rs`, `ast/mod.rs`, `ast/lexer/mod.rs`), not wired to anything
├── proc_macros/         sibling workspace crate (legacy compile-time const macros; not a dependency)
├── strtohx/             sibling workspace crate (legacy dev tool for removed `.efe` format)
└── tests/
    ├── api_index.rs         std registry vs index, `api`/`api-export`, assembling with `--api`
    ├── api_merge.rs         repeated `--api` flags merged via `ApiIndex::merge`
    ├── asm_lower.rs         `.efa` assembler/lowering tests (ops, immediates, call sugar, `ret`, data, memory, globals)
    ├── bindgen.rs           `rustgen`/`cgen` golden tests (`fixtures/guest_test.*`), determinism, CLI `--rust`/`--c`, name collision/error checks
    ├── guest_common/mod.rs  shared `efe.provide` custom section decoder for guest wasm tests
    ├── guest_wasm.rs        builds Rust (`fixtures/guest_mod`) and C (`fixtures/guest_c/mod.c`) wasm guests and checks ABI with `wasmparser` (skips if toolchain missing)
    ├── fixtures/            `guest_test.efapi`, `guest_test.api.rs`, `guest_test.efe_api.h`, `guest_mod/`, `guest_c/mod.c`
    ├── pack.rs              `pack_dir`, `.efmod` determinism, manifest checks, `.efapi` fragment output, CLI `pack`/`inspect`/`api-export`
    ├── v2_backtrace.rs      trap backtraces with source lines
    ├── v2_bundle.rs         `bundle` and embedded execution
    ├── v2_cli.rs            CLI options, exit codes, `.efe`/`.efec` rejection
    ├── v2_debugger.rs       debugger commands on scripted stdin
    ├── v2_diagnostics.rs    assembler diagnostics
    ├── v2_events.rs         `import event` and `provide event` (`efa-syntax.md` section 17)
    ├── v2_examples.rs       every `examples/*.efa` against expected output and exit code
    ├── v2_lsp.rs            LSP protocol tests (`Server::handle` and stdio binary)
    ├── v2_multifile.rs      multi-file `.efa` programs (`use`, visibility, imports, interface, structs, scratch, `FsSource`)
    ├── v2_multifile_tools.rs  LSP, debugger, and `run` on multi-file programs
    ├── v2_never_panics.rs   deterministic mutation fuzzing on assembler, decoder, disassembler
    ├── v2_ported.rs         ported legacy examples (`exit.efa`, `echo.efa`, `write_file.efa`) and legacy rejection
    └── v2_roundtrip.rs      `build` -> `dis` -> `build` byte-identical round trip across `examples/*.efa`
```

`lib.rs` exports `prelude`, `compiler`, `debugger`, `host`, `host_std`, `disasm`, `run_v2`, `bundle`, `lsp`, `pack`, `bindmodel`, `rustgen`, `cgen`, and `pub use efa::{asm, link}` (whose implementation lives in `crates/efa`).

## Main concepts
- **Pipeline.** `.efa` text goes through `efa::asm` (lexer and parser per file of the `use` graph) and `efa::link` (two-phase linking into `efevm::ModuleBuilder`) to an `efevm::Module`. `run_v2` checks imports against `host_std`'s `HostRegistry`, creates an `Instance`, and runs export `main` under an optional instruction budget. `disasm` prints a module back as `.efa`.
- **Host functions.** `host_std::std_registry` registers three v1 namespaces (owner `std`):

  | Namespace | Functions |
  |---|---|
  | `std` (v1) | `print(text)`, `println(text)`, `print_i64(v)`, `print_f64(v)`, `print_char(c)`, `read_line(buf) -> (len)`, `play_sound(path) -> (status)`, `exit(code)` |
  | `std.time` (v1) | `now_ms() -> (ms)` (monotonic since `StdEnv` creation), `unix_ms() -> (ms)` |
  | `std.fs` (v1) | `read_file(path, buf) -> (len)`, `write_file(path, data) -> (status)` |

  `std.fs` is always registered but denied unless `fs_root` (`--fs-root`) is configured; denied calls return `-2` (`IO_ERROR` is `-1`). Paths must be relative without `..` and are resolved inside the canonicalized root.
- **API index.** `efescript api FILE` prints an index; `api-export [-o FILE]` writes the `std` index (or a `.efmod`/`.efvm` index with `FILE` and optional `--id ID` for bare `.efvm`). `build`, `run`, `debug`, `pack`, and `lsp` accept repeatable `--api` inputs (`.efapi` or `.efmod`), merged via `ApiIndex::merge`.
- **Guest bindings (`--rust` / `--c`).** `bindmodel` validates and converts a merged `ApiIndex` into a sorted namespace tree (`Node`), padded struct `Layout`s, and wasm32 ABI types (`docs/e7.4-wasm-design.md`). `rustgen` emits a `no_std` Rust module targeting `efe-guest` (nested namespace modules, `raw` `wasm_import_module = "<ns>@<ver>"` externs, safe wrappers, `#[repr(C)]` structs with `size_of`/`align_of`/`offset_of!` assertions, event modules, and the `efe.api` custom section). `cgen` emits `efe_api.h` for `clang --target=wasm32` (with `EFE_API_IMPLEMENTATION` emitting `efe.api`). Invoked via `efescript api-export --rust|--c INPUT... [-o FILE]`.
- **Mod packages.** `pack DIR` reads `DIR/mod.toml`, merges CLI `--api` inputs with `[build] api`, calls `efa::mod_folder::build_mod` and `Package::write`, writes `<id>-<version>.efmod` (or `-o FILE`), and writes the mod's API fragment `<out>.efapi` (`efemod::api_fragment`) alongside it. `inspect` dumps a `.efmod` or `.efvm`'s manifest, dependencies, permissions, interface, imports, events, API section, and assets.
- **Standalone executables.** `bundle` appends an encoded `.efvm` and a 16-byte trailer (`[module_len: u64 LE][b"EFVMBNDL"]`) to `efescript-runner` (or `--runner PATH` / current exe). When started with an embedded module, `run_embedded()` executes it with `std.fs` denied and ignores CLI arguments.
- **LSP & Debugger.** `lsp` runs a synchronous `lsp-server` loop over stdio with buffer-first multi-file analysis (`BufSource`), diagnostics, go-to-definition, completion, hover, and mtime/size-based `.efapi` reload. `debug` runs `Debugger` over stdin/stdout (`break`, `delete`, `step`, `next`, `continue`, `regs`, `mem`, `bt`, `list`, `quit`) with the JIT backend explicitly disabled (`instance.set_backend(None)`).
- **Exit codes.** `run`: `main`'s return value or `std.exit` code; `2` for CLI/load/assembly/API-index errors or `.efe`/`.efec` input; `3` for VM trap or exhausted `--budget`.

## Known gaps
- `std.fs.write_file` checks paths before writing, so concurrent symlink swaps are not prevented (TOCTOU).
- `std.print_char` traps on an invalid Unicode scalar value; `std.play_sound` returns `1` on non-UTF-8 paths or playback errors.
- `dis` -> `asm` without `--api` drops section 12 (API hash/version provenance); with the matching `--api` index the round trip is byte-identical (excluding `lines`).
- LSP `definition` and `completion` fall back to the last parsed AST when the current buffer has syntax errors.
- `compiler/` is an unused stub; there is no high-level language frontend yet.
- `bundle` executables are unprotected raw payloads; on macOS bundled binaries must be re-signed.
