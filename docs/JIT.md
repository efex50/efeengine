# Native script compilation (Cranelift): design and roadmap

Status: planned. Nothing here is built. The interpreter (`crates/efevm`) stays the portable path, including on the web.

## Decision

- Scripts compile to native machine code with Cranelift on native targets.
- Compiled code has **one** way to reach the outside: the host functions that `efevm` already defines (`HostRegistry`, `Imports`, `Caller`). It has no other syscalls, no direct engine pointers, no globals.
- Compiled code is loaded by a separate native-only crate. The engine crates do not link to it; they only see `efevm` hosts.
- **Cranelift is the standard backend**, for both JIT and AOT. It works with `efevm` through the same host table.
- **Granularity: whole functions only.** A function is either compiled or runs in the interpreter. Fibers and coroutines stay interpreted. A function that uses an unsupported instruction falls back to the interpreter without failing the mod.
- **AOT is optional.** The default is JIT with a cache (below). AOT shared libraries are a feature flag for shipping.
- **Linker for AOT: rust-lld**, shipped with the Rust toolchain. mold is not used.
- `efevm` drops `#![forbid(unsafe_code)]`. Unsafe is allowed only where it is needed: the executable-memory boundary and the call into generated code. Those go in the JIT crate, and `efevm`'s interpreter stays safe Rust.

Reading of "no compiled code goes in engine": the engine crate (`crates/core`) never contains or links generated code. Generated code is produced at mod load time into process memory by `efevm-jit`, and that crate is only reachable through the `efevm` module API.

## Trust model

Generated machine code is not sandboxed by the hardware, so the safety rules are enforced by the compiler we emit:

1. **Linear memory only.** Every guest load and store is emitted with a bounds check against the instance's memory size, the same check the interpreter does (`Trap::OutOfBounds`). No raw host pointers in guest code.
2. **Calls only through a host table.** A host call is an indirect call through a fixed table of `extern "C"` trampolines built from `HostRegistry`. The table is read-only after load. Guest code receives no other function addresses.
3. **Fuel and depth.** Back edges and calls decrement a fuel counter and check `max_call_depth`, so a loop cannot hang the host. Same limits as `efevm`'s `limits`.
4. **Traps unwind through the host.** A trap returns an error code from the trampoline; the JIT never unwinds across generated frames.
5. **Same semantics as the interpreter.** Integer wrap, shifts modulo width, `f32`/`f64` bit layout (SPEC section 2). The interpreter is the reference; JIT output is tested against it.

Consequence to accept: a Cranelift codegen bug is a host-memory bug, not a guest trap. That is why the JIT is native-only, behind a feature, and the interpreter remains the default until the JIT passes the same test suite.

## Pipeline

```
mod source ──► efescript ──► efevm Module (bytecode, unchanged)
                                   │
                    native build   ▼
                          efevm-jit: Module ──► Cranelift IR ──► machine code
                                   │                              (exec memory)
                                   ▼
                    efevm Instance (same API as interpreter), calls go to host table
```

- The `efevm` module format does not change. The JIT consumes the same decoded module, so mods remain one artifact.
- `efevm-jit` exposes a backend that implements the same entry points as the interpreter (`Instance::call`, fibers). Fibers need a separate decision (see open questions).

## Roadmap

### J0: boundary (no codegen yet)

- [ ] J0.1 **[S] Unlock `efevm` unsafe.** Replace `#![forbid(unsafe_code)]` with `#![deny(unsafe_code)]`; the interpreter keeps a `#![deny]` so no unsafe creeps in by accident. _Done when_ `cargo test -p efevm` passes unchanged.
- [ ] J0.2 **[S] Backend trait.** `efevm` defines the execution entry points as a trait (`ExecBackend`) that the interpreter implements; `efevm-jit` implements the same trait. _Done when_ the existing `efevm` tests run through the trait.
- [ ] J0.3 **[S] Host table export.** A function that turns `Imports<H>` into a flat table of `(namespace, name, FuncType, trampoline)` the JIT can index. _Done when_ a test lists every import of `script_demo` in the table.

### J1: Cranelift spike

- [ ] J1.1 **[M] Compile one function.** `efevm-jit` compiles a single integer-only `Function` (arithmetic, branches, calls to other module functions) with `cranelift-jit`; a test calls it and checks the result against the interpreter for 1000 random inputs.
- [ ] J1.2 **[S] Measure.** Compile time per function and run time against the interpreter on a fib and a loop benchmark. Record in this file.
- _Done when_ the spike matches the interpreter on all tests and the numbers are recorded. If the speedup is small on the real script workload, stop here and keep the interpreter.

### J2: full instruction set

- [ ] J2.1 **Memory ops** with bounds checks (trust rule 1).
- [ ] J2.2 **Host calls** through the table (trust rule 2), including `Caller` memory access from host code.
- [ ] J2.3 **Fuel and depth** (trust rule 3).
- [ ] J2.4 **Traps** as error codes (trust rule 4).
- [ ] J2.5 **Differential tests.** Every opcode in `OPCODES.md` runs in both backends and results match, including traps.
- _Done when_ the whole `efevm` test suite passes with the JIT backend selected.

### J3: integration

- [ ] J3.1 **[S] Feature flag.** `jit` feature on `efevm-jit` and on the game; default off. Web builds never enable it.
- [ ] J3.2 **[S] Load-time choice.** The mod loader picks the JIT when the feature is on and the target is native, else the interpreter.
- [ ] J3.3 **Hot reload.** A changed mod recompiles its functions; fits with E6 (hot-patching) later.
- _Done when_ a mod runs natively with the JIT on and in the interpreter with the feature off, with identical output.

## AOT: shipped mods as shared libraries

JIT stays for development: a mod changed in the editor is compiled and run in memory at once. AOT is for shipping and for load-time speed:

1. Cranelift (`cranelift-object`) writes a relocatable object for the mod.
2. A linker turns it into a shared library: `.so` on Linux, `.dylib` on macOS, `.dll` on Windows.
3. The engine loads it at runtime with `libloading` (`dlopen` on Unix) and resolves one entry point, which takes the host table from `efevm`.

Linker: **rust-lld**, shipped with the Rust toolchain, so nothing extra to install. It works on all three platforms with the same flags. The editor finds it through the toolchain (`rustc --print sysroot`), not on PATH.

Plugin contract:
- The mod exports one C-ABI entry point and a version number. The engine refuses a mod whose version or `efevm` API hash does not match.
- The mod receives the host table as a pointer argument and calls nothing else. Enforce this at build time: after linking, check the library's undefined symbols (`nm -u` / `objdump`) and reject any that are not the host table symbol or the allowed libc subset. This turns trust rule 2 into a build check.
- Link with no third-party libraries.

Reload:
- `dlclose` is unreliable (thread-local storage, static destructors), so a reload loads the new build under a new file name and drops the old handle later.
- Scripts keep no pointers into their library across a reload; state lives in guest memory, which the host copies.

Trust: a loaded `.so` runs native code in the engine's process, so the rules above (linear memory, host table, fuel) are the only protection. Treat AOT mods like JIT mods: the same review gate before untrusted mods run.

## Cache

Compiled code is cached on disk, for both JIT and AOT, so a mod is compiled once per change.

- **Key:** hash of (module bytes, `efevm` API hash, Cranelift version, target triple, backend flags). Any change produces a new key, so a stale entry is never loaded.
- **Location:** the user cache directory (`dirs` crate), one folder per key. The editor and the game share it.
- **Contents:** the JIT code as a relocatable blob with its relocations (JIT) or the shared library (AOT).
- **Integrity:** entries store their own hash. A mismatch deletes the entry and recompiles.
- **Eviction:** size limit (default 256 MB), oldest first. Deleting the folder is always safe.
- **Failure:** if the cache is unreadable or unwritable, compile in memory and continue.

## Milestones added

- [ ] J4.1 **[S] Function-level JIT.** Only whole functions are compiled; unsupported functions use the interpreter.
- [ ] J4.2 **[M] Cache** as above, with a test that a changed module byte gives a new key and a corrupted entry is recompiled.
- [ ] J4.3 **[M] AOT feature** (`aot`): Cranelift object, rust-lld link, C-ABI entry point, undefined-symbol check.
- _Done when_ a mod loads in the editor from cache on the second run without compiling, and the AOT build of the same mod gives the same output as the interpreter.

## Open questions

- **Fibers (coroutines).** Decided: fibers stay interpreted; only plain functions are compiled (see Decision).
- **Fuel cost.** Fuel checks slow loops. Recommendation: measure in J1.2 before deciding the granularity (per back edge vs per call).
- **Platform.** Cranelift targets x86_64 and aarch64 here. Other targets stay on the interpreter.
- **Memory growth.** If a guest grows memory, the JIT must reload the base pointer after every host call. Recommendation: reload after every call and grow.
- **Security review.** Because generated code is native, the trust rules above need a code review before J3 ships to users with untrusted mods.
