# efescript roadmap

Living document; edit freely. It expands Phase 5 of the repository `ROADMAP.md`.
Based on a code analysis of 2026-11-15 (runtime, assembler and file format). Bugs
marked **(verified)** were reproduced by running code; the rest come from reading it.

## Where efescript stands

_Historical (the analysis that started E0). The legacy assembler and VM it describes were deleted in E3.6; the current state is in `structure.md`._

About 4,100 lines: an x86-flavoured assembler (`nasm_efe`), a register VM
(12 registers, flags, cooperative threads) on `efepages` memory, and a CLI.
`hello.efe` and `call.efe` run; `heap.efe` and `test.efe` panic.

Worth keeping:
- The register-machine design and register names; it maps well to native code later.
- Per-instance state: the runtime has no globals, so one VM per world is possible.
- The assembler's shape: tokens → `PreCompile` (instruction or label) → bytes. That intermediate step is the right place for an IR.
- Access width carried by the operand (`BYTEPTR` … `QWORDPTR`).
- Source positions (`LineColumn`) already travel with tokens.

What has to change, in one line each:
- **No format.** Bytecode is a headerless byte stream loaded at address 0; opcode numbers come from enum order, so adding an opcode silently breaks every compiled file.
- **Everything panics.** The runtime returns no `Result`; about 73 `unwrap`/`todo!`/`panic!` sites are reachable from bytecode. With `panic = abort` a bad script would kill the engine.
- **It cannot stop.** `tick` throws away the result of every instruction, so the `Finish` syscall does nothing **(verified)**, and there is no instruction budget: `jmp 0` runs forever **(verified)**.
- **Unsound ownership.** Threads hold raw pointers into the `ProgramRuntime` that owns them, so moving the runtime dangles them; `compile_from_file` does exactly that.
- **No memory safety for the host.** No bounds, no limit: a script can touch one byte per page and exhaust host memory. Code is writable. The stack starts right after the code and grows without limit.
- **Syscalls are static.** A Rust enum matched by the number in `ra`, with an ad hoc register convention, hard-wired to host stdout, stdin and the file system (arbitrary file read and write).
- **Host-dependent.** Registers are `usize`; the crate does not compile for wasm32 **(verified)**. Memory is big-endian. Add/sub overflow panics in debug builds and wraps in release.
- **Slow.** Every instruction is re-decoded from bytes, and each byte read allocates: about 344 ns per instruction measured (roughly 3 million instructions per second), 50–100× slower than a plain interpreter should be.
- **Missing operations.** No multiply, divide, shifts, signed comparison, sign extension or floating point; `lea` and `js` cannot be assembled.
- **Almost no tests.** 8 tests pass, none executes bytecode or checks assembler output.

## Proposed direction

Do not patch the current VM into shape. Fix the handful of bugs that block testing
(E0), then build a **v1 core** next to it with a real module format, and move the
assembler over. The old byte format is dropped, not migrated: only five example
programs use it.

Design rules for v1 (these keep JIT, AOT and a wasm translator possible):
1. **Fixed widths.** Registers are `u64` on every host. Memory is little-endian. Integer arithmetic wraps; division by zero and out-of-bounds access trap.
2. **A module, not a byte stream.** Magic, version, and sections: types, imports, functions, code, data, exports, names/debug. Everything is validated at load.
3. **Code is not data.** Code lives outside the script's address space and is immutable. Return addresses live on a separate call stack the script cannot write.
4. **Functions are real.** Each has a signature and a frame; jumps stay inside a function and target instruction indices, never byte addresses.
5. **A syscall is a host call.** `call_host <import index>`; imports are `(namespace, name, signature)` resolved at load against a registry filled at runtime. The registry is also the API index: whatever is callable is listed and exported (see "The API index" below).
6. **One calling convention**, written down once, used by the interpreter, host calls, JIT and AOT.
7. **No I/O in the core.** Printing, files and time are host functions supplied by the embedder (the CLI supplies std ones; the engine supplies its own).
8. **No panics from guest input.** Loading returns `LoadError`; running returns `Exit::{Halt, Yield, Budget, Trap}`.

## Roadmap

### E0: Make the current code testable (small)
- [x] E0.1 Remove debug noise: `dbg!` on every token (`nasm_efe/mod.rs:211`, `:288`), `dbg!` in `Println`, the whole-memory dump on an unknown opcode.
  _Done:_ no `dbg!` is left in `src`; unknown opcodes fault with a message instead of dumping memory.
- [x] E0.2 Fix the assembler bugs that make the examples wrong:
  - `[rbp+8]` assembles as `[rbp+rbp]`: `PtrInner::from_tokens` reads `t[0]` instead of `t[2]` (`nasm_funs.rs:67-79`) **(verified)**. This is why `hello.efe` says the form "doesn't work".
  - Hex literals containing `b` (`0x20b`, `0x0b`) are rejected: the `0b` prefix test uses `contains` (`funs.rs:7`) **(verified)**.
  - Forward jumps near address 255 are encoded one byte off, because label resolution guesses jump sizes in three passes instead of iterating to a fixed point **(verified)**.
  - An undefined label panics; a duplicate label silently takes the last definition **(verified)**.
  - Decimal numbers are rejected in `push32 5` and panic in `[8]`.
  _Done:_ all listed bugs fixed; labels use a fixed 4-byte encoding so no size guessing remains; undefined/duplicate symbols are `ParseErr` with a position. Added the `db` string system and `NAME.len` (see `docs/e0-design.md`).
- [x] E0.3 Fix the decoder bugs: first static inside a pointer reads one byte instead of `len` (`data_funs.rs`) **(verified)**; `[reg-reg]` adds; sub-register masks use `|` instead of `&`; `mov al, x` does not mask to 8 bits.
  _Done:_ all four fixed with lib tests; also `lea` and `js` now assemble and run, add/sub wrap.
- [x] E0.4 Make `tick` return the instruction's result so `Finish` ends the program with its exit code.
  _Done:_ `tick` returns a `TickSignal`; `run`/`run_with_limit` return `RunResult::{Finished(u8), EndOfProgram, LimitReached, Fault}`; Finish ends the program with its code, and invalid bytecode or syscalls are faults instead of panics.
- [x] E0.5 Golden tests: each example assembles to known bytes and runs to a known output; one test per fixed bug. Fix the two `efepages` bugs with tests: multi-page reads and writes ending on a page boundary lose the last page, and a zero-length read at a page boundary returns a whole page **(both verified)**.
  _Done:_ `tests/examples.rs` runs hello, strings, jumps, recursion, call and heap to exact output and `RunResult`; `tests/{sound,control_flow,strings,errors,cli}.rs` and the lib tests cover each fixed bug. Byte-exact golden files of the assembled output are not stored, only behaviour is asserted. The `efepages` boundary bugs are fixed (`cargo test -p efepages`).
- [x] E0.6 Stop the CLI writing `./out.efec` on every run (make it `-o <file>`), and let it run a precompiled file.
  _Done:_ `-o <file>` writes the bytecode and does not run; `.efec` files run directly; no `./out.efec` any more; missing file/assembly error exits 2, runtime fault 3 (`tests/cli.rs`).

### E1: v1 core VM (embeddable, safe)
_Done 2026-11-15 in the new crate `crates/efevm` (spec: `crates/efevm/SPEC.md`): 372 tests, about 3.1 ns per instruction. E1.8 stays open until CI checks wasm32 (a local check passes)._
- [x] E1.1 **Specification first.** One document: value types (`i32`, `i64`, `f32`, `f64`), registers, instruction list with exact semantics, traps, calling convention, module layout. Everything after this implements it.
- [x] E1.2 `Vm`, `Module`, `Instance` types that own their data (indices instead of raw pointers; safe to move and store in a world). No `unsafe` in the interpreter.
- [x] E1.3 Linear memory: one little-endian `Vec<u8>` per instance with a configurable maximum, bounds-checked loads and stores, `memory.size` / `memory.grow`. `efepages` stays available as an optional sparse backend, not the default.
- [x] E1.4 Pre-decoded execution: decode once at load into an instruction array; the loop runs over that array. Target: under 10 ns per instruction in release.
- [x] E1.5 `run(budget) -> Exit`: `Halt(code)`, `Yield`, `Budget` (resumable), `Trap(kind)`. Typed traps: out-of-bounds, division by zero, stack overflow, unreachable, bad host call.
- [x] E1.6 Complete integer and control set: mul, signed and unsigned div and rem, shifts, rotates, not, neg, count-leading/trailing-zeros, popcount, sign and zero extension, signed and unsigned comparisons, conditional select, jump table, indirect call through a function table. Arithmetic sets no hidden flags; comparisons produce a value or feed the branch directly.
- [x] E1.7 Floating point: `f32` and `f64` arithmetic, comparisons and conversions.
- [ ] E1.8 Builds and passes its tests on `wasm32-unknown-unknown`; CI checks it.
- [x] E1.9 Tests: instruction-level tests for every opcode, trap tests, a budget/resume test, and a fuzz target that feeds random bytes to the loader and must never panic.

### E2: Module format and assembler v2
- [ ] E2.1 Binary module format (`.efec` v1): magic, version, section table, checksum. A loader with a verifier: jump targets, register and type use, stack depth per function, import signatures.
  _Mostly done in E1:_ `efevm`'s `.efvm` format (magic, version, ordered sections, LEB128) and its load-time verifier cover this, and v2 writes and runs `.efvm`. There is no checksum section yet. Left open for that.
- [x] E2.2 Assembler v2 on the existing token → IR → bytes shape: `fn name(params) -> result` blocks, labels as `name:`, `.data` / `.rodata` with string literals (retires `strtohx`), constants, negative and character literals, comments anywhere, `import` declarations.
  _Done:_ `src/asm` (lexer, parser, lowering to `efevm::ModuleBuilder`) for `.efa` sources (`docs/e2-design.md`, `docs/efa-syntax.md`). It covers `fn` blocks, `name:` labels, `import`, `const`, globals, a function table, data (`db`/`dw`/`dd`/`dq`/`resb`, with `NAME`/`NAME.len`) laid out from address 1024, immediates as sources, and call/ret sugar. There is no separate `.data`/`.rodata` split, and data is writable. String literals make `strtohx` unnecessary, but that crate is not deleted yet. Six examples are in `examples/*.efa`, and the CLI commands are `efescript run|build|dis`.
- [x] E2.3 Real diagnostics: an error type with `Display`, file, line, column and a source snippet; no panics on any input.
  _Done:_ `asm::Diagnostic`/`Diagnostics` print file:line:col, a snippet and a note (with "did you mean"). The assembler recovers per line and caps output at 100 diagnostics. efevm verify errors map back to the source statement. Fuzz tests (`tests/v2_never_panics.rs`) show it does not panic on mutated sources or `.efvm` bytes.
- [x] E2.4 Debug section: function and import names, and an instruction → source line table.
  _Done:_ the `names` section holds function, import and register names. The `lines` section has file, line and column for every instruction. Traps print a backtrace with function names and source lines (`run_v2::trap_report`, `tests/v2_backtrace.rs`).
- [x] E2.5 A disassembler (`efescript dis file.efec`) and round-trip tests: assemble → disassemble → assemble gives identical bytes.
  _Done:_ `src/disasm.rs` and `efescript dis`. The round trip assemble → disassemble → assemble gives identical bytes apart from the `lines` section, tested for every example (`tests/v2_roundtrip.rs`).

### The API index (rule for E3–E5 and everything after)

Every function a script can call is in **one table: the API index**. It lives in
`efevm` (`HostRegistry`), so the engine, the CLI and mods all fill the same one.

- **Registering is the only way in.** There is no path that makes a host function
  callable without adding its index entry (namespace, name, parameter names and
  types, results, doc text, the registering owner: `std`, `engine`, or a mod name).
  An engine system or a mod that adds a host call therefore updates the index
  by construction; nothing is kept in sync by hand.
- **Types too.** Structs and components that cross the boundary are registered
  with their field layout (name, offset, type, size, alignment) and appear in the
  same index.
- **Exported like a PDB.** The index is written to a versioned file, `<game>.efapi`
  (JSON, with a content hash). Every game built with the engine gets one:
  `cargo xtask api-export` writes it at build time, `cargo xtask build`/`dist`
  place it next to the game, and the running engine can write it again
  (`Engine::export_api_index`) so functions registered by loaded mods are in it.
- **Tools read only the file.** The assembler (import checks), the language
  server (completion, hover, signatures) and the debugger use the `.efapi` file
  and never link the game. A mod developer needs the game's `.efapi`, not its source.
- **Checked by a test.** A test fails when a host function is callable but
  missing from the index, or has an entry without a doc line.

### E3: Host calls (the syscall redesign)
- [x] E3.1 `HostRegistry` in `efevm`: `register(namespace, name, signature, function)` at any time. Functions receive a host context `&mut H` chosen by the embedder, plus typed arguments, and return a value or a trap. It replaces direct use of `Imports`: a registry produces the `Imports` an instance is linked with, and is the API index above.
  _Done:_ `efevm::HostRegistry<H>` (`crates/efevm/src/api/registry.rs`): `define_namespace`, `register(HostFn, f)`, `replace`, `register_type`, `unregister_owner`, `imports()`, `index()`. Host functions get `&mut Caller<H>`, typed `Args`/`Results` by logical parameter and return `Result<HostOk, Trap>`. `Imports` can no longer be filled by hand (the old `Imports::func` is deleted); `HostRegistry::imports()` is the only producer, so the registry is the index. Tests: `crates/efevm/tests/api.rs` (`registry_functions_are_callable`, `imports_are_exactly_the_index`) and registry unit tests; all efevm tests were moved onto the registry. Design: `docs/e3-design.md` section 2.
- [x] E3.2 Type descriptions for signatures: scalars, entity handles, `(pointer, length)` slices and strings in guest memory, and `#[repr(C)]` structs with named fields. Shared with the engine's type registry (repository roadmap 3.4). Each parameter has a name and each function a doc string, because both go into the index.
  _Done:_ `ApiType` (`i32 i64 f32 f64 bool entity ptr str bytes &Struct`), `HostFn` with named parameters/results and a required doc, `StructDesc`/`FieldDesc`/`Prim` with layout validation (`crates/efevm/src/api/types.rs`). The engine's type registry produces the same `StructDesc` (derive `ScriptType`, repository roadmap 3.4). Tests: `api::types` unit tests, `crates/core/tests/api_index.rs::every_registered_component_is_in_the_index_with_its_layout`.
- [x] E3.3 Import resolution at load, with clear errors for a missing name or a signature mismatch. Namespaces carry a version.
  _Done:_ `Instance::new` resolves imports by `namespace.name` and lowered signature and checks the namespace versions recorded in the module (section 12). New `LinkError::{UnknownNamespace, NamespaceVersion, ApiHash}`; `ImportTypeMismatch` shows both signatures; `Imports::check_module` lists every problem by name. Tests: `crates/efevm/tests/api.rs::{link_checks_namespace_versions, check_module_lists_every_problem}`, `crates/efescript/tests/api_index.rs::run_v2_reports_link_problems_by_name`.
- [x] E3.4 Safe guest-memory access helpers for host functions (checked read and write of slices, strings and structs).
  _Done:_ `Caller::{read_bytes, read_str, write_bytes, read_struct, write_struct, read_/write_u32/u64/f32/f64}` with checked arithmetic; negative pointers or lengths are out of bounds. Tests: `crates/efevm/tests/api.rs::caller_memory_helpers`, `crates/core/tests/scripting.rs::guest_out_of_bounds_pointer_traps_and_the_engine_keeps_running`.
- [x] E3.5 A `std` host library in a separate crate or feature: print, read line, files, time. Used by the CLI; not linked into the engine. File access is opt-in and limited to a directory. `host_std.rs` moves onto the registry, so `std` is in the index like any other namespace.
  _Done:_ `efescript::host_std::std_registry(&StdOptions)` registers `std`, `std.time` and `std.fs` (owner `std`) with parameter names and docs; the lowered signatures of the old `std` functions are unchanged. `std.fs` is always in the index but denied (-2) unless `--fs-root DIR` is given; `..`, absolute paths and symlinks out of the root are denied. It is a module of the efescript CLI crate, which the engine never depends on (no separate crate). Tests: `host_std` unit tests (fs escapes, time), `tests/v2_ported.rs` (`write_file.efa`).
- [x] E3.6 Remove the old numbered `syscall` instruction and the `SysCalls` enum (the legacy `.efe` assembler and VM go with them; port any example worth keeping to `.efa`).
  _Done:_ `src/nasm_efe`, `src/runtime`, `ops.rs`, `instruction.rs`, `syscalls.rs`, `funs.rs`, `macros.rs`, the `.efe` examples and their tests are deleted; efescript no longer depends on efepages or proc_macros. `Host`/`StdHost`/`CaptureHost` moved to `src/host.rs`. Ported: `call.efe` → `exit.efa`, `test.efe` → `echo.efa`, `create_e.efe` → `write_file.efa`; `heap.efe` was dropped. `.efe`/`.efec` files exit 2 with a message. Tests: `tests/v2_ported.rs`. Not ported as tests: legacy-only checks (deep recursion to depth 20000, data placed between code, unknown syscall numbers) have no v2 counterpart.

### E4: API index file and tooling
- [x] E4.1 Index export from a registry to `.efapi`: functions with parameter names, types and docs; struct layouts; namespaces, versions and owners; a format version and a content hash. JSON first, binary later if size matters. A reader for the same file (`efescript api <file>` prints it). The format is specified in `crates/efevm/API_INDEX.md`.
  _Done:_ `ApiIndex::{to_json, from_json, hash}` (`crates/efevm/src/api/{index,json}.rs`, hand-written JSON, no dependencies), format in `crates/efevm/API_INDEX.md`. `efescript api FILE.efapi` prints it, `efescript api-export` writes the std index. The reader verifies format, version, hash and every `signature`, and never panics (mutation test). The reader accepts a hash computed over the file text or over its canonical re-serialisation, so reformatted files still load. Tests: `crates/efevm/tests/api.rs`, `crates/efescript/tests/api_index.rs::{json_round_trip_keeps_the_hash, api_export_cli_matches_the_library_index, api_cli_prints_every_function}`.
- [x] E4.2 Modules record the index hash they were built against (plus the imports they use, which they already list); the loader reports incompatibilities by name, not just "hash differs".
  _Done:_ `.efvm` section 12 `api` (index hash + version of every namespace imported), written by the assembler with `--api`; the linker checks versions and `Imports::check_module` reports incompatibilities by function and namespace name, adding the hash only as context. Tests: `crates/efevm/tests/api.rs::{api_section_round_trips, api_section_rejections}`, `examples/script_demo/tests/e2e.rs` (section 12 hash equals the exported index's).
- [x] E4.3 The assembler reads a `.efapi` file (`--api <file>`) to check `import` declarations and call arity at build time, with "did you mean" for unknown names.
  _Done:_ `efescript build|run --api FILE`, `asm::assemble_with(src, file, &AsmOptions { api })`: unknown imports with "did you mean", signature mismatches shown with parameter names, `import ns.name` without a signature, struct constants `S.size`, `S.align`, `S.<field>`, `S.id`. Tests: 11 diagnostics tests in `src/asm/lower/tests.rs`, `tests/api_index.rs::{assemble_an_example_against_an_index_read_from_json, run_and_build_with_api_through_the_cli}`.
- [x] E4.4 Language server for the assembly language (`efescript lsp`): diagnostics, go-to-label, completion and hover for host functions from the `.efapi` file, reloading when the file changes.
  _Done:_ `efescript lsp [--api FILE]` (`src/lsp/`, `lsp-server` + `lsp-types`): diagnostics, go to definition, completion of host functions/struct constants/mnemonics/local symbols, hover; finds a single `*.efapi` in the workspace root without `--api`; reloads it when its modification time changes and republishes diagnostics. Tests: `tests/v2_lsp.rs` (11, including the binary over stdio). Not checked with a real editor; definition/hover are single-file and use the last AST that parsed.
- [x] E4.5 Debugger: breakpoints, single step, register and memory view over the debug section (replaces the empty `debugger` module). Host calls are shown by index name.
  _Done:_ `efescript debug FILE [--api FILE]` (`src/debugger/`, a line debugger on stdin/stdout): `break LINE|FILE:LINE|FUNC`, `delete`, `step` (resume with budget 1), `next`, `continue`, `regs` (with register names), `mem`, `bt`, `list`, `where`, `quit`; import calls are shown by index name and parameter names (`call std.println(text = r0..r1)`). efevm gained `Instance::{fiber_location, fiber_registers, fiber_backtrace}`. Tests: `tests/v2_debugger.rs` (9, including the CLI), `crates/efevm/tests/api.rs::single_step_debugger_hooks`. There is no DAP (editor) protocol, only the command line.
- [x] E4.6 The index test: every callable host function has an index entry with a doc line; run for the `std` registry and for the engine registry.
  _Done:_ `crates/efescript/tests/api_index.rs` (std registry) and `crates/core/tests/api_index.rs` (engine registry): every name in `imports().names()` has an index entry with a non-empty doc and the same lowered type, and the other way round; the JSON round trip keeps the hash; every engine component is in the index with its layout.

### E5: Engine integration (repository roadmap 5.4–5.5)
Needs the world, `Commands` and type registry of repository roadmap 3.2–3.4; those are pulled forward as part of this work.
- [x] E5.1 The engine registers its host functions (world, commands, input, time, events, log) in the registry and exports the index with `cargo xtask api-export`; `Engine::export_api_index` does the same at runtime. Registered components appear in the index with their layouts.
  _Done:_ the engine registers `engine.world`, `engine.input`, `engine.time`, `engine.events`, `engine.log`, `engine.mods` (27 functions, `crates/core/src/engine/scripting/api_engine.rs`); games add their own with `Engine::define_namespace`/`register_host_fn`. `Engine::register_component` is the one public path that registers a component in the type registry and in the index. `Engine::api_index()`/`export_api_index()`; `cargo xtask api-export <example>` runs the game with `EFE_API_EXPORT`. Tests: `crates/core/tests/api_index.rs`, `examples/script_demo/tests/e2e.rs::the_native_binary_writes_the_index_with_efe_api_export`.
- [x] E5.2 `Script` component and `ScriptSystem` with a per-frame budget.
  _Done:_ `Script { module, entry, state }` component and the script system (first in `Update`): `entry(entity, dt, state) -> state` per entity, optional `init`/`frame` exports, one instance per module (one VM per world), per-module per-frame budget. Tests: `crates/core/tests/scripting.rs::{script_moves_its_entitys_transform, budget_exhaustion_skips_the_module_but_not_the_frame, a_script_with_an_entry_that_runs_out_skips_the_modules_other_scripts}`.
- [x] E5.3 Mods: load and unload modules, mods registering functions for other mods (added to the index under the mod's name, removed on unload), per-mod budgets and namespace permissions, hot reload on native.
  _Done:_ `Engine::load_mod(bytes, ModOptions)` with namespace-prefix permissions and budget, `unload_module`; `engine.mods.export` registers a mod function under the mod's namespace (owner = the mod, parameter names from the names section), callable by other modules, removed from the index on unload (callers then get a clean trap); hot reload with `watch_module` (native only, polls the file's modification time). Tests: `crates/core/tests/scripting.rs::{mod_exports_appear_in_the_index_are_callable_and_vanish_on_unload, disallowed_namespace_and_link_errors_are_reported_by_name, hot_reload_swaps_the_module_and_resets_script_state}`, `examples/script_demo/tests/e2e.rs::a_mod_exports_a_function_that_another_script_calls`. A mod can register layout-only structs only from Rust (`Engine::register_struct`), not from script code.
- [x] E5.4 Every game build ships its index: `cargo xtask build`/`dist` write `<game>.efapi` next to the game, and an example game shows a script calling an engine function checked against that file.
  _Done:_ `cargo xtask build|run <example> native` writes `<example>.efapi` next to the binary and assembles `examples/<example>/scripts/*.efa` against it into `scripts/*.efvm`; `dist` copies the `.efapi` into `dist/<example>/`. `examples/script_demo` runs `mover.efa`, which moves entities through `engine.world.get/set` and calls the game's own `game.score_add`. Tests: `examples/script_demo/tests/e2e.rs::mover_script_assembled_against_the_exported_index_moves_entities` (headless). Not run: the SDL window (`cargo xtask run script_demo native`) and the web dist (wasm-bindgen-cli is not installed); both are compile-checked only. Scripts marked `; needs-mod:` are not assembled by xtask.

### E6: High-level language
- [ ] E6.1 Language design: syntax, type system, how host types and imports appear. (Today only a symbol lexer and empty AST types exist; `main.sfe` is `fn main(){ }`.)
- [ ] E6.2 Lexer with keywords, strings, numbers, comments and multi-character operators; parser to a real AST.
- [ ] E6.3 Type checker, then code generation to the same IR the assembler uses.
- [ ] E6.4 Language server support for the language, driven by the manifest.

### E7: Compilers
- [ ] E7.1 A compiler-friendly IR: functions, basic blocks, virtual registers, lowered to the 12-register form by a register allocator.
- [ ] E7.2 AOT: modules to native code, and to wasm for the web build, with the same calling convention and import resolution.
- [ ] E7.3 JIT on native. Not possible in the browser, which keeps the interpreter or AOT-to-wasm.
- [ ] E7.4 wasm → efescript translator, so Rust, C and others can target the VM. Needs E1.6, E1.7 and E3 complete.

## Order and decisions

Suggested order: **E0 → E1 → E2 → E3**, then E4 and E5 together, then E6, then E7.
E0 is a few days of work; E1–E3 are the redesign and should not start before the
specification in E1.1 is agreed.

Open decisions:
- **Keep the x86-style two-operand form** (`add64 ra, rb`) or move to three operands (`add ra, rb, rc`)? Three operands are simpler to verify and to compile; two operands are what exists.
- **Register count.** 12 fixed registers, or per-function virtual registers in the module with 12 only as an assembler convention? Virtual registers make E7 and the wasm translator much easier.
- **Fixed-width instructions** (simpler, faster decode, larger files) or compact variable-width with a per-function index?
- **Threads.** Keep cooperative VM threads as coroutines (useful for scripts that wait), or drop them until needed?
- **Embed a wasm runtime instead?** If wasm becomes an input format (E7.4), `wasmi` or `wasmtime` would give Rust mods sooner but bypass efescript. This roadmap assumes one VM for everything.
- **`efepages`.** Default memory becomes a flat vector; keep `efepages` as an optional backend, or retire it after fixing its bugs?
