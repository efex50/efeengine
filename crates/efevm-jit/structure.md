# efevm-jit

`efevm-jit` compiles verified `efevm` modules to native code with Cranelift 0.137.2 (roadmap E7, phases J1-J6) and runs fibers through the `efevm::backend::Backend` seam. It is observably identical to the interpreter (SPEC draft 8): same `Exit` sequence, results, memory, globals, `resume_units`, trap kind and location, backtraces, and fiber registers/location/backtrace while suspended. The one allowed difference is the payload of NaN results of float arithmetic.

## At a glance
| | |
|---|---|
| Path | `crates/efevm-jit` |
| Kind | library (`src/lib.rs`), plus a benchmark example (`examples/bench.rs`) and integration tests |
| Depends on (workspace) | `efevm` (`[dev-dependencies]`: `efescript`, `efa`) |
| Used by | `efeengine-core` (optional, feature `jit`), `efevm` (dev-dependency for `EFEVM_TEST_BACKEND=jit`), `efe-wasm` (dev-dependency) |
| Key external deps | `cranelift-codegen`, `cranelift-frontend`, `cranelift-jit`, `cranelift-module`, `cranelift-native` pinned to `=0.137.2` (only for `not(target_arch = "wasm32")`) |
| Features | none |
| Targets | native 64-bit hosts with a Cranelift ISA. On `wasm32` the library body is `#![cfg(not(target_arch = "wasm32"))]`, so it is empty and Cranelift dependencies are not built. |
| Safety | all `unsafe` lives here (calling generated code, raw context pointers, `JITModule::free_memory`); every block has a `// SAFETY:` comment. `efevm` stays `#![forbid(unsafe_code)]`. |
| Tests | 8 integration test files in `tests/` (`calls`, `debugger`, `examples`, `fuzz`, `limits`, `memory`, `ops`, `smoke`) plus shared helpers in `tests/common/` |

## Public API
```rust
pub enum OptLevel { None, Speed }
pub struct JitOptions { pub opt_level: OptLevel /* Speed */, pub max_native_stack: usize /* 1 MiB */, pub verify: bool /* cfg!(debug_assertions) */ }
pub struct JitCode;                       // Send + Sync + Debug; owns executable memory (freed on Drop)
pub fn compile(&efevm::Module, &JitOptions) -> Result<Arc<JitCode>, JitError>;
pub struct JitBackend<H>;                 // impl<H: 'static> Backend<H>; JitBackend::new(Arc<JitCode>)
pub fn enable<H: 'static>(&mut Instance<H>, &JitOptions) -> Result<(), JitError>;   // compile + set_backend
pub enum JitError { UnsupportedHost(String), Unsupported { func: u32, op: Op }, Codegen { func: u32, message: String }, Module(String), TooManyRegisters { func: u32, nregs: u32 } }  // Display + Error
pub const MAX_JIT_REGISTERS: u32 = 4096;  // a function with more registers is refused (the module stays interpreted)
// diagnostics (process-wide AtomicU64 counters): native_runs(), native_stack_fallbacks(), native_stack_resume_fallbacks()
```
`JitBackend::run` checks that `cx.module.data()` matches the compiled module pointer (`JitCode` keeps a clone, so the pointer cannot be reused), that `stack.func`/`stack.pc` is a block leader, and that the outermost function index is a compiled module function (not an import and in range of `code.entries`). Otherwise it delegates to `run_interpreter`.

## Layout
```text
crates/efevm-jit/
├── Cargo.toml                  # package `efevm-jit` (edition 2024, v0.1.0), Cranelift 0.137.2 on non-wasm32, dev-deps `efescript` & `efa`
├── README.md                   # usage snippet, security note, design/benchmark pointers
├── structure.md                # this file
├── src/
│   ├── lib.rs                  # `#![cfg(not(target_arch = "wasm32"))]` gate and public re-exports
│   ├── compile.rs              # OptLevel, JitOptions, JitError, JitCode, MAX_JIT_REGISTERS, compile(), per-function codegen (`Fc`)
│   ├── ctx.rs                  # JitCtx (repr C) + field offsets, TableEntry, Rt (Rust-side run state), trap and suspend codes
│   ├── helpers.rs              # `extern "C"` helpers called from generated code (traps, save/resume, stack_low, grow/copy/fill, h_host_call)
│   └── backend.rs              # JitBackend, enable(), run(), rebuild() of FiberStack, process-wide diagnostic counters
├── examples/
│   └── bench.rs                # ns/instruction, interpreter vs JIT (count loop, recursive fib, memory loop, f64 loop)
└── tests/
    ├── common/
    │   ├── mod.rs              # test harness (`Pair`, `Cfg`, `diff*`, `cross`, `snap`, `fuzz_case`)
    │   ├── prog.rs             # recursive test module with host yields, traps, memory and indirect calls
    │   └── randmod.rs          # SplitMix64 random verified-module generator for differential fuzzing
    ├── smoke.rs                # counting loop under all budget sets
    ├── ops.rs                  # arithmetic/compare/convert ops x edge inputs, trap kinds, select/br_table/globals/consts
    ├── memory.rs               # loads and stores x offsets x boundary/overflowing bases, memory.size/grow/copy/fill, globals
    ├── calls.rs                # test program under 8 budget sets, trap kinds, cross-backend resume, `step` mixed with JIT
    ├── limits.rs               # `max_call_depth`/`max_registers` traps, tiny native stack fallbacks, `MAX_JIT_REGISTERS` refusal
    ├── debugger.rs             # verifies `efescript::debugger::Debugger` drops the JIT and never enters native code
    ├── fuzz.rs                 # J6 differential fuzzing (`fuzz_short` 150 cases, `#[ignore]` `fuzz_long` and `fuzz_stats`)
    └── examples.rs             # runs `crates/efescript/examples/*.efa` with the real `std` host on both backends
```

## ABI
One Cranelift function per module function, default call conv of the host ISA (so Rust calls it as `extern "C"`):
`fn(ctx: *mut JitCtx, window: *mut u64, mode: i32) -> i32`.
- `window`: `max(np, nr)` u64s: arguments on entry in mode 0, results on a normal return.
- `mode`: 0 normal call, 1 resume. Status: 0 returned, 1 suspended, 2 trapped, and 3 host-yield (only returned by import thunks to their direct caller, which turns it into status 1).
- One thunk per import (same signature) calls `ctx.host_call` (`h_host_call::<H>`, set per run; the code is H-independent). Direct calls, indirect calls and imports are therefore uniform.
- `JitCtx` fields: `mem_base`, `mem_len`, `globals`, `budget`, `depth` (live activations = interpreter `frames.len() + 1`), `max_depth`, `regs_used` (sum of `max(nregs, np)` over live activations) and `max_regs`, `stack_limit`, `table`/`table_len` (`TableEntry { code, canon, size, is_host }`), `host_call`, `rt` (`*mut Rt`), `resume_inner`, `suspend_kind`, trap record. Generated code reads and writes fields at `offset_of!` offsets.
- Registers are Cranelift `Variable`s of type `I64`; 32-bit ops `ireduce` and `uextend`; f32 is bitcast through `I32`. Non-parameter registers start at 0. `mem_base`/`mem_len` are cached in variables and reloaded after every call, thunk call and `memory.grow`; globals are always accessed through `ctx.globals`.
- One Cranelift block per efevm leader (`Module::blocks`). Leader entry: `if ctx.budget <= 0 { suspend(budget, pc) }; ctx.budget -= cost`. Bulk ops charge extra units inside their helpers (`h_grow`, `h_copy`, `h_fill`).
- Traps are explicit branches to cold blocks calling `h_trap(kind, func, inst)` and returning 2. Division checks, trunc range checks, bounds checks (`base + off` and `+ size` overflow, then `<= mem_len`) are all explicit; the hardware-trapping forms are never reachable.
- Calls: depth check (`depth + 1 > max_depth`), then register check (`regs_used + callee size > max_regs`), both `StackOverflow` at the call instruction, in the interpreter's order; imports are not checked (as in the interpreter). Window registers go to a stack slot, the callee runs, status 0 loads `base..base+nr`. `call_indirect` checks table bounds / null (`UndefinedElement`) and canonical type (`IndirectCallTypeMismatch`), then branches on `is_host` for the checks.

## Suspension and resume
- Every suspension point (a leader whose budget check fails, `yield`, a host call returning `Yield`, a callee returning 1, native stack low) stores all registers into a stack slot and calls `h_save_inner` (innermost: next pc) or `h_save_frame` (callers: return pc and window base), then returns 1. Each native frame on the way out appends its activation, innermost first, to `Rt::saved`.
- `JitBackend::run` (`backend.rs`) then calls `rebuild`: it lays the activations out in `FiberStack` exactly as the interpreter does (outermost first, `fp`s back to back, `max(nregs, np)` registers each, `frames` for callers, `func/pc/fp` for the innermost). So interpreter and JIT can resume each other's fibers.
- Resume always enters the outermost activation with mode 1. `h_resume_frame` copies the next activation's registers (`Rt::cursor`) into a slot and tells whether it is the innermost. The entry then dispatches with a `Switch`: innermost jumps to the leader block at `pc` (budget is charged there as usual); a caller jumps to its call site (return pc - 1) in mode 1, which re-checks depth and registers (restoring the accounting), re-stores the window and calls the callee in mode 1, then continues like a normal return. No instruction is re-executed, no budget is charged for the re-entry.
- Native stack: each function entry compares the stack pointer with `ctx.stack_limit` (address of a local in `run` minus `max_native_stack`). Below it: mode 0 suspends with the callee's fresh activation at pc 0 (before charging); mode 1 sets `Rt::keep` (activations from `saved.len()` on stay untouched in the `FiberStack`, the front ones are updated in place). `run` then finishes the resume with `run_interpreter` and the remaining counter, so deep recursion behaves exactly like the interpreter. The thread needs headroom of `max_native_stack` plus what host functions use.
- Done: results are copied from the Rust-side window to `stack.results`. Trap: `stack.trace` = `Rt::trace` (innermost fault first, then each caller's call instruction appended by `h_trap_frame`), `Exit::Trap` from `Rt::pending` (a host `Err` is stored there as is).

## Ops
All 181 ops of `Op::ALL` are lowered; `JitError::Unsupported` exists but is never returned. Notes: `min/max/nearest/copysign/sqrt/ceil/floor/trunc` map to Cranelift's wasm-semantics instructions; `abs`/`neg` work on the integer bits like the interpreter; trapping truncs use the interpreter's range constants then `fcvt_to_*_sat`; `rem_s` divides by 1 when the divisor is -1 (result 0).

## Tests (`tests/`)
| File | What |
|---|---|
| `common/mod.rs`, `common/prog.rs` | harness (`Pair`, `diff*`, `cross`, `snap`), the recursive test program with host yields, traps, memory and indirect calls |
| `smoke.rs` | counting loop under all budget sets |
| `ops.rs` | every arithmetic/compare/convert op x edge inputs (0, -1, MIN, MAX, shifts >= width, NaN, +-0, +-inf, range edges of every trunc, dirty upper halves), trap kinds, select/br_table/globals/consts |
| `memory.rs` | every load and store x offsets x boundary and overflowing bases, `memory.size/grow` with remap, copy/fill charging, globals |
| `calls.rs` | the program under 8 budget sets, trap kinds, cross-backend resume at every suspension point (JIT/interpreter alternating by pattern), `step` mixed with JIT resumes |
| `limits.rs` | `max_call_depth` and `max_registers` traps at the same location, deep recursion past a tiny native stack (several limits, including 0), the program with a tiny native stack |
| `debugger.rs` | the efescript debugger drops a JIT installed on its instance and never runs native code |
| `fuzz.rs`, `common/randmod.rs` | J6 differential fuzzing, see below |
| `examples.rs` | every `crates/efescript/examples/*.efa` with the real `std` host, both backends, 5 budget sequences |

Run: `cargo test -p efevm-jit` (debug: about a minute; Cranelift itself is slow in debug).

### J6: differential testing
- **efevm's own tests on the JIT.** `crates/efevm/tests/common/mod.rs::new_instance` is `Instance::new` plus, with `EFEVM_TEST_BACKEND=jit`, `efevm_jit::enable`. All integration tests of `efevm` (except `backend.rs`, which tests the seam itself) create their instances through it, so `EFEVM_TEST_BACKEND=jit cargo test -p efevm --tests` runs about 300 tests with the same expectations on the JIT (efevm has a dev-dependency on efevm-jit; a module with a function over `MAX_JIT_REGISTERS` registers stays interpreted). First run found the register-count bug below.
- **Random modules.** `tests/common/randmod.rs` builds verified modules with `ModuleBuilder` from a seed (SplitMix64): 2-5 functions of 4 types, a table with matching and mismatching entries, 6 mutable and immutable globals of all four types, memory with random max, the host functions of `prog::registry` (host yields, host errors, a host that grows memory, host global access). Statements: all 181-op groups (integer ops of every width, comparisons, conversions, float ops over inputs like NaN, -0, infinities and range edges), loads and stores of every width in and out of bounds, `memory.size/grow/copy/fill`, globals, `select`, if/else, bounded loops, `br_table`, direct, indirect and host calls, `yield`, early `ret`, `unreachable`. Termination: loops have constant trip counts, direct calls go to higher indexes, a fuel global bounds function entries. Half the modules are "safe" (divisors forced odd, addresses masked) to reach deep code; the rest trap often. Random `Limits` (call depth, registers, memory pages), random Cranelift `opt_level`.
- **NaN payloads** (SPEC: "some NaN") are the one allowed difference, so after a float op that can produce a NaN the generator emits `select d, d != d, CANON_NAN, d`; everything else, including the NaN-ness and sign handling, is compared bit for bit.
- **Comparison.** `common::fuzz_case(seed)` runs the case on the interpreter (reference), on the JIT, and on a third instance that switches between JIT and interpreter at random resumes (2/3 JIT), all with the same random budget sequence (0, 1, 2, small, up to 2000, unlimited), and compares after every resume: exit, units used, fiber status, results, trap location and backtrace, fiber location/backtrace/registers while suspended, memory (hash), all globals, the host call log.
- **Runs.** `cargo test -p efevm-jit --test fuzz` runs `fuzz_short` (150 seeded cases; `EFEVM_FUZZ_CASES`, `EFEVM_FUZZ_SEED`). Long run: `EFEVM_FUZZ_SECS=600 EFEVM_FUZZ_SEED=1 cargo test -p efevm-jit --release --test fuzz -- --ignored fuzz_long --nocapture` (also `EFEVM_FUZZ_ITERS`, `EFEVM_FUZZ_TRACE=1` prints each seed); it prints failing seeds and a histogram of how programs ended. Replay a seed: `EFEVM_FUZZ_SEED=<seed> EFEVM_FUZZ_CASES=1 cargo test -p efevm-jit --test fuzz fuzz_short`.
- **Result (release, this machine):** `fuzz_long` seed 100000, 823 s, 9225 cases, 771 914 resumes, 65.4 M units compared: 0 differences. Ends: 1885 Done, 7 kinds of trap (div by zero 667, out of bounds 1904, indirect type mismatch 1817, stack overflow 1805, undefined element 450, host error 116, overflow 143, invalid conversion 137, unreachable 179), 122 cut off at the resume cap.
- **Bugs found:** (1) a function with more than 65 533 registers panicked inside Cranelift (block-parameter overflow); fixed by refusing functions over `MAX_JIT_REGISTERS` (4096) with `JitError::TooManyRegisters`; regression test `limits::a_function_with_too_many_registers_is_refused`. (2) The first fuzz runs showed NaN payload differences of float results (interpreter canonical NaN, JIT propagating the input payload): allowed by the SPEC, handled in the generator as above, not a code change.
- **`Grant::jit` default stays `false`.** Fuzzing was clean for 14 minutes and 9k programs, but that is one machine, x86-64 only, and not coverage-guided; the JIT is unsafe code that runs untrusted mods. Recommendation: flip it only after a much longer run (hours, several seeds, ideally also on aarch64) and a cargo-fuzz target with coverage feedback.

## Benchmark (`cargo run -p efevm-jit --release --example bench`, this machine, x86-64)
| Program | interpreter ns/instr | JIT ns/instr |
|---|---|---|
| count loop, unlimited budget | 3.24 | 0.47 |
| count loop, budget 10000 per resume | 3.28 | 0.49 |
| recursive fib(32), unlimited | 7.45 | 1.26 |
| recursive fib(32), budget 10000 | 7.22 | 1.41 |
| memory loop (load + store, bounds checks), unlimited | 3.35 | 0.33 |
| memory loop, budget 10000 | 3.40 | 0.34 |
| f64 mul/add loop, unlimited | 3.11 | 0.95 |
| f64 mul/add loop, budget 10000 | 3.06 | 0.97 |

Compile time (Cranelift, `opt_level` speed): 0.5-1.1 ms for these small modules.

Unit counts are asserted equal between the two.

## Known limits
- Functions with more than 4096 registers are not compiled (the whole module then stays on the interpreter).
- All registers of a function are live at every leader (the cold suspend paths read them), which limits register allocation; register typing and merged budget checks are J7.
- Compile time is per whole module at `enable`; no lazy compilation. Functions with very large `nregs` get a large native frame and a large cold path per suspension point.
- A panic in a host function aborts the process (it unwinds through generated code, which has no unwind info); the interpreter would propagate it.
- Only exercised on x86-64. Not yet fuzzed with coverage feedback (the J6 generator is seeded and not coverage-guided).
- NaN payloads of float arithmetic results are not guaranteed identical to the interpreter's (SPEC: "some NaN").
