# efescript roadmap

Living document; edit freely. It expands Phase 5 of the repository `ROADMAP.md`.
Based on a code analysis of 2026-11-15 (runtime, assembler and file format). Bugs
marked **(verified)** were reproduced by running code; the rest come from reading it.

## Where efescript stands

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
5. **A syscall is a host call.** `call_host <import index>`; imports are `(namespace, name, signature)` resolved at load against a registry filled at runtime.
6. **One calling convention**, written down once, used by the interpreter, host calls, JIT and AOT.
7. **No I/O in the core.** Printing, files and time are host functions supplied by the embedder (the CLI supplies std ones; the engine supplies its own).
8. **No panics from guest input.** Loading returns `LoadError`; running returns `Exit::{Halt, Yield, Budget, Trap}`.

## Roadmap

### E0: Make the current code testable (small)
- [ ] E0.1 Remove debug noise: `dbg!` on every token (`nasm_efe/mod.rs:211`, `:288`), `dbg!` in `Println`, the whole-memory dump on an unknown opcode.
- [ ] E0.2 Fix the assembler bugs that make the examples wrong:
  - `[rbp+8]` assembles as `[rbp+rbp]`: `PtrInner::from_tokens` reads `t[0]` instead of `t[2]` (`nasm_funs.rs:67-79`) **(verified)**. This is why `hello.efe` says the form "doesn't work".
  - Hex literals containing `b` (`0x20b`, `0x0b`) are rejected: the `0b` prefix test uses `contains` (`funs.rs:7`) **(verified)**.
  - Forward jumps near address 255 are encoded one byte off, because label resolution guesses jump sizes in three passes instead of iterating to a fixed point **(verified)**.
  - An undefined label panics; a duplicate label silently takes the last definition **(verified)**.
  - Decimal numbers are rejected in `push32 5` and panic in `[8]`.
- [ ] E0.3 Fix the decoder bugs: first static inside a pointer reads one byte instead of `len` (`data_funs.rs`) **(verified)**; `[reg-reg]` adds; sub-register masks use `|` instead of `&`; `mov al, x` does not mask to 8 bits.
- [ ] E0.4 Make `tick` return the instruction's result so `Finish` ends the program with its exit code.
- [ ] E0.5 Golden tests: each example assembles to known bytes and runs to a known output; one test per fixed bug. Fix the two `efepages` bugs with tests: multi-page reads and writes ending on a page boundary lose the last page, and a zero-length read at a page boundary returns a whole page **(both verified)**.
- [ ] E0.6 Stop the CLI writing `./out.efec` on every run (make it `-o <file>`), and let it run a precompiled file.

### E1: v1 core VM (embeddable, safe)
- [ ] E1.1 **Specification first.** One document: value types (`i32`, `i64`, `f32`, `f64`), registers, instruction list with exact semantics, traps, calling convention, module layout. Everything after this implements it.
- [ ] E1.2 `Vm`, `Module`, `Instance` types that own their data (indices instead of raw pointers; safe to move and store in a world). No `unsafe` in the interpreter.
- [ ] E1.3 Linear memory: one little-endian `Vec<u8>` per instance with a configurable maximum, bounds-checked loads and stores, `memory.size` / `memory.grow`. `efepages` stays available as an optional sparse backend, not the default.
- [ ] E1.4 Pre-decoded execution: decode once at load into an instruction array; the loop runs over that array. Target: under 10 ns per instruction in release.
- [ ] E1.5 `run(budget) -> Exit`: `Halt(code)`, `Yield`, `Budget` (resumable), `Trap(kind)`. Typed traps: out-of-bounds, division by zero, stack overflow, unreachable, bad host call.
- [ ] E1.6 Complete integer and control set: mul, signed and unsigned div and rem, shifts, rotates, not, neg, count-leading/trailing-zeros, popcount, sign and zero extension, signed and unsigned comparisons, conditional select, jump table, indirect call through a function table. Arithmetic sets no hidden flags; comparisons produce a value or feed the branch directly.
- [ ] E1.7 Floating point: `f32` and `f64` arithmetic, comparisons and conversions.
- [ ] E1.8 Builds and passes its tests on `wasm32-unknown-unknown`; CI checks it.
- [ ] E1.9 Tests: instruction-level tests for every opcode, trap tests, a budget/resume test, and a fuzz target that feeds random bytes to the loader and must never panic.

### E2: Module format and assembler v2
- [ ] E2.1 Binary module format (`.efec` v1): magic, version, section table, checksum. A loader with a verifier: jump targets, register and type use, stack depth per function, import signatures.
- [ ] E2.2 Assembler v2 on the existing token → IR → bytes shape: `fn name(params) -> result` blocks, labels as `name:`, `.data` / `.rodata` with string literals (retires `strtohx`), constants, negative and character literals, comments anywhere, `import` declarations.
- [ ] E2.3 Real diagnostics: an error type with `Display`, file, line, column and a source snippet; no panics on any input.
- [ ] E2.4 Debug section: function and import names, and an instruction → source line table.
- [ ] E2.5 A disassembler (`efescript dis file.efec`) and round-trip tests: assemble → disassemble → assemble gives identical bytes.

### E3: Host calls (the syscall redesign)
- [ ] E3.1 `HostRegistry`: `register(namespace, name, signature, function)` at any time. Functions receive a host context `&mut H` chosen by the embedder, plus typed arguments, and return a value or a trap.
- [ ] E3.2 Type descriptions for signatures: scalars, entity handles, `(pointer, length)` slices and strings in guest memory, and `#[repr(C)]` structs with named fields. Shared with the engine's type registry (repository roadmap 3.4).
- [ ] E3.3 Import resolution at load, with clear errors for a missing name or a signature mismatch. Namespaces carry a version.
- [ ] E3.4 Safe guest-memory access helpers for host functions (checked read and write of slices, strings and structs).
- [ ] E3.5 A `std` host library in a separate crate or feature: print, read line, files, time. Used by the CLI; not linked into the engine. File access is opt-in and limited to a directory.
- [ ] E3.6 Remove the old numbered `syscall` instruction and the `SysCalls` enum.

### E4: API manifest and tooling
- [ ] E4.1 Manifest export from a registry: functions with parameter names, types and docs; struct layouts; namespaces and versions; a content hash. Human-readable text first (JSON), binary later if size matters.
- [ ] E4.2 Modules record the manifest hash they were built against; the loader reports incompatibilities by name.
- [ ] E4.3 The assembler reads a manifest to check `import` declarations and call arity at build time.
- [ ] E4.4 Language server for the assembly language: diagnostics, go-to-label, completion and hover for host functions from the manifest, reloading when the manifest file changes.
- [ ] E4.5 Debugger: breakpoints, single step, register and memory view over the debug section (replaces the empty `debugger` module).

### E5: Engine integration (repository roadmap 5.4–5.5)
- [ ] E5.1 The engine registers its host functions (world, commands, input, time, events, log) and exports the manifest with `cargo xtask api-export`.
- [ ] E5.2 `Script` component and `ScriptSystem` with a per-frame budget.
- [ ] E5.3 Mods: load and unload modules, mods registering functions for other mods, per-mod budgets and namespace permissions, hot reload on native.

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
