# efe-wasm

`efe-wasm` translates a `.wasm` module into a verified `efevm::Module` (roadmap E7.4b; design in `crates/efescript/docs/e7.4-wasm-design.md`, sections 3, 4, and 7). It is pure data with no I/O: wasm bytes and an `ApiIndex` in, `efevm::Module` out. Both `efa` (`efescript pack` / mod-folder builds) and `efeengine-core`'s `wasm-mods` development path use it to compile Rust and C/C++ wasm mods to `.efvm` without the engine's package runtime linking a wasm parser.

## At a glance
| | |
|---|---|
| Path | `crates/efe-wasm` |
| Kind | library (`src/lib.rs`) plus integration tests |
| Depends on (workspace) | `efevm` (path); dev (non-`wasm32`): `efevm-jit` (path) |
| Used by | `efa` (optional `wasm` feature: `mod_folder::{build_mod, to_fallback}`, enabled by `efescript`), `efeengine-core` (optional `wasm-mods` feature, native only: re-exported as `efeengine_core::efe_wasm` and used in `engine::scripting::{cache, packages}`), `examples/coin_dash_wasm` (non-`wasm32`), and `examples/script_demo` (dev-dependency in `tests/wasm_mods_e2e.rs`) |
| Key external deps | `wasmparser` 0.245, `thiserror` 2.0.21; dev: `wat` 1.245, `wasmi` 2.0.2 |
| Targets | native and `wasm32-unknown-unknown` (`cargo check -p efe-wasm --lib --target wasm32-unknown-unknown`) |
| Safety | `#![forbid(unsafe_code)]`; never panics on malformed input (`wasmparser` validates the module) |
| Tests | unit-style integration suites in `tests/` (`smoke`, `numeric`, `control`, `memory`, `reject`, `abi`, `guest`, `hardening`, `fuzz`) |

## Layout
```text
crates/efe-wasm/
├── Cargo.toml             package efe-wasm 0.1.0 (edition 2024); dev-deps `wat`, `wasmi`, native `efevm-jit`
├── README.md              usage snippet, supported wasm subset, test/fuzz commands
├── structure.md
├── src/
│   ├── lib.rs             `TRANSLATOR_VERSION`, `Fallback*`, `TranslateOptions`, `translate()`, import/event resolution, function index layout, `efe.provide` + `Fallback` merging (`build_interface`), sections 12/13/14
│   ├── parse.rs           module-level reader (types, imports, table 0, memory 0, globals, exports, active elements/data, start, `efe.api`/`efe.provide`/`name` custom sections); allow-list rejection of unsupported features
│   ├── func.rs            operator allow-list pre-scan (`scan_body`), `FnCx` stack-to-register compiler, control flow labels, `local.get`/`local.tee` symbolic optimization, host/event lowering (`lower_host`), `emit_prologue`
│   ├── abi.rs             wasm32 ABI lowering of `ApiType` (`wasm_lower`, `wasm_sig`) and `efe.provide` binary record decoder (`decode_provide`)
│   └── error.rs           `TranslateError` and `Loc` (function index, name, byte offset)
└── tests/
    ├── common/            `mod.rs` (`wat` -> `Module`, interpreter/JIT runners), `host.rs` (mock host from `ApiIndex`), `prov.rs` (`efe.provide` record writers), `wgen.rs` (random WAT generator), `fuzz.rs` (differential runner)
    ├── numeric.rs         integer/float operators and trapping/saturating conversions against Rust models
    ├── control.rs         block/loop/if, `br`/`br_if`/`br_table` with values, multi-value, `select`, local hazards, calls, recursion, `call_indirect` traps, names
    ├── memory.rs          loads/stores of all widths, bounds traps, `size`/`grow`/`fill`/`copy`, data segments, start function and `__wasm_call_ctors`
    ├── reject.rs          rejection errors for unsupported features, ABI-breaking imports, table/size limits
    ├── abi.rs             imports and events, `efe.api` hash, `efe.provide` records, sections 12/13/14, mock-host provided calls
    ├── guest.rs           builds and translates the E7.4a Rust and C guest fixtures (`crates/efescript/tests/fixtures/{guest_mod,guest_c}`) and runs them on a mock host
    ├── hardening.rs       components, GC groups/subtyping, block types, dead multi-value blocks, multi-value moves, data/memory limits, malformed-input mutation tests
    ├── fuzz.rs            `fuzz_short` (150 cases by default) and `fuzz_long` (`#[ignore]`) differential fuzzing against `wasmi` on interpreter and JIT
    └── smoke.rs           basic `add` and `sum` WAT smoke test
```

## Public API
- `pub const TRANSLATOR_VERSION: u32 = 1` (included in external cache keys such as `ScriptCache::translate_wasm`).
- `pub fn translate(wasm: &[u8], index: &ApiIndex, opts: &TranslateOptions) -> Result<Module, TranslateError>`
- `TranslateOptions { fallback: Fallback, names: bool, require_efe_api: bool }` (`Default`: empty `Fallback`, `names: true`, `require_efe_api: false`).
- `Fallback { provides: Vec<FallbackProvide>, events: Vec<FallbackEvent>, scratch: Option<u32> }` with `FallbackProvide { name, export, doc, params, results }` and `FallbackEvent { name, payload, doc }`.
- `TranslateError` (`Invalid`, `Unsupported`, `TooLarge`, `Import`, `UnknownNamespace`, `ApiHash`, `Section`, `Provide`, `Verify`) and `Loc { func: Option<u32>, name: Option<String>, offset: Option<usize> }`.

## How a module is translated
1. **Parse & subset check (`parse.rs`, `func.rs`).** `parse::parse` reads module sections via an allow-list (rejecting components, GC `rec`/subtypes/struct/array types, shared/descriptor types, SIMD `v128`, exceptions/tags, multi-memory, `memory64`, custom page sizes, imported memory/table/globals, multiple or non-`funcref`/initialized tables, tables larger than `MAX_TABLE = 1 << 20`, passive/declarative elements, passive data, and extended-const). `func::scan_body` checks every operator and local type so unsupported constructs fail with a named error before `wasmparser::Validator` runs with only the subset's `WasmFeatures` (`MUTABLE_GLOBAL | SATURATING_FLOAT_TO_INT | SIGN_EXTENSION | REFERENCE_TYPES | MULTI_VALUE | BULK_MEMORY | FLOATS`).
2. **`efe.api` check (`lib.rs`).** Multiple `efe.api` sections must hold the same 8-byte little-endian hash, which must match `index.hash()` (`ApiHash` otherwise). Missing `efe.api` is permitted unless `opts.require_efe_api` is set.
3. **Imports (`lib.rs`, `abi.rs`).** Each import module must be `<ns>@<ver>` with `ns` present in `index` (else `UnknownNamespace`) and consistent version per namespace. Event imports (`<ev>.send|count|read`) are validated against `index.event(ns, ev)` and sorted into section 14 entries; `lower_host` lowers them inline to `engine.events.send|count|read` calls with the event id constant. Function imports are checked against `abi::wasm_sig` (`ptr`, `&Struct`, `str`, `bytes` lower to `i32` / `(i32, i32)`, `entity` to `i64`); `ptr` results emit `i32.wrap_i64`.
4. **Function index space (`lib.rs`).** Ordered as `[host imports][wasm defined functions][wrappers for imports referenced by table 0 or exports][provide $abi wrappers]`.
5. **Bodies (`func.rs`).** Registers are laid out as locals `r0..r(P+L-1)`, two scratch registers (`nlocals`, `nlocals + 1`), and operand stack slots from `sbase = nlocals + 2` (failing with `TooLarge` if register index `> 65534`). `local.get` and `local.tee` push symbolic `Ent::Local(x)` entries that materialize only when needed (before `x` is overwritten, before calls/branches, or at block boundaries). `br` copies carried values with ascending moves; `br_table` uses per-target trampolines when moves or `ret` are needed; unreachable code after `br`/`br_table`/`return`/`unreachable` is skipped up to the matching `else`/`end`.
6. **Sections & lazy initialization (`lib.rs`, `func.rs`).** Copies memory 0 (`max` capped at 65536 pages), globals, table 0 active elements, active data segments (bounds-checked against initial memory), function/global exports, names (`opts.names`), section 12 (`api_ref`), section 13 (`interface`), and section 14 (`event_ref`), then verifies via `ModuleBuilder::finish`. `abi::decode_provide` and `merge_fallback` combine `efe.provide` records with `opts.fallback` (rejecting disagreements), verify `efe_scratch` is an exported immutable `i32` global, copy referenced `StructDesc`s from `index`, and generate `<export>$abi` wrappers (`i32.wrap_i64` for `str`, `bytes`, `&Struct` parameters). If a `start` function or `__wasm_call_ctors` export exists (both must be defined `() -> ()` functions), an extra mutable `i32` `initialized` global is appended and `emit_prologue` is prepended to exported functions so initializers run once (`0 -> 1 -> 2`).

## Notes
- Traps map 1:1 (out-of-bounds memory, divide by zero, integer overflow, invalid float-to-int conversion, `unreachable`, undefined/mismatched `call_indirect`, stack exhaustion). `memory.grow` wraps efevm's `i64` result to `i32`.
- Custom sections other than `efe.api`, `efe.provide`, and `name` (such as DWARF, `producers`, `target_features`) are ignored. Unknown `efe.provide` record kinds (e.g. future kind 3) are skipped; importing a mod's own namespace yields `UnknownNamespace`.
- `tests/common/wgen.rs` NaN-canonicalises float outputs after NaN-producing operations so `wasmi`, efevm interpreter, and efevm JIT compare bit-for-bit.
- Because clang/lld put `.custom_section.*` globals into data segments rather than wasm custom sections, C mods pass their provides via `TranslateOptions::fallback` (tested in `tests/guest.rs`).
