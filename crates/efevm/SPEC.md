# efevm v1 specification

Status: draft 4, 2026-11-15 (draft 1 plus the clarifications in "Changes from draft 1" and the changes in "Changes from draft 2" and "Changes from draft 3" at the end). This is roadmap item E1.1 (`crates/efescript/ROADMAP.md`).
`efevm` is the new core VM for efescript. It lives in its own crate (`crates/efevm`)
next to the old VM in `crates/efescript`, which keeps working until the assembler
moves over (E2).

Decisions taken: per-function **virtual registers**, **three-operand** instructions with
no flags, **compact variable-width** file encoding (decoded once at load into a
fixed-width in-memory form), cooperative **coroutines** (fibers).

## 1. Principles

1. Same behaviour on every host, including `wasm32-unknown-unknown`: no `usize` in semantics.
2. Nothing a module does can panic or corrupt the host. Loading returns `LoadError`; running returns `Exit`.
3. Code is not data: instructions live outside the script's memory and are immutable.
4. No I/O in this crate. Everything outside the VM is a host function.
5. No `unsafe` in the interpreter. No global or static state. No dependencies beyond `core`/`alloc`/`std` collections (a small error-derive crate is acceptable).
6. One calling convention (section 6), shared later by JIT and AOT.

## 2. Values and registers

- Value types: `i32`, `i64`, `f32`, `f64`.
- A register is a 64-bit slot (`u64`). Registers are untyped at run time:
  - `i32` operations read the low 32 bits and write a **zero-extended** result;
  - `f32` is stored as its bits in the low 32 bits (upper bits zero); `f64` as its bits.
- Each function declares `nregs` (0–65535). Its registers are `r0 … r(nregs-1)`, private
  to each activation. Parameters arrive in `r0 … r(nparams-1)`; all other registers start at 0.
- Integer arithmetic wraps. Shifts and rotates use the count modulo the bit width.
- Floats follow IEEE 754; NaN payloads are not specified beyond "some NaN".

## 3. Memory

- One linear memory per instance: little-endian bytes, addressed by `u64`, sized in
  64 KiB pages. A module declares `min_pages` and optional `max_pages`; the embedder sets
  a hard cap. Memory starts zeroed.
- Effective address of a load/store = `reg[base] (u64) + offset (u32 immediate)`. If the
  access does not lie fully inside memory (or the addition overflows) → `Trap::OutOfBounds`.
- No alignment requirement.
- Data segments are copied into memory at instantiation (`offset` + bytes); a segment that
  does not fit is a `LoadError`.

## 4. Module

A module contains, in this order of indices:

| Part | Content |
|---|---|
| types | function types `(params: [ValType]) -> (results: [ValType])` |
| imports | host functions: `(namespace: String, name: String, type index)` |
| functions | `type index`, `nregs`, code (instruction list) |
| table | one list of function indices for indirect calls (may be empty) |
| memory | `min_pages`, `max_pages?` |
| globals | `ValType`, mutable flag, initial constant |
| exports | `name → function index` (and `name → global index`) |
| data | `offset`, bytes |
| names (optional) | function, import and register names |
| lines (optional) | per function: instruction index → (file id, line, column) |
| api (optional) | the API index hash and `(namespace, version)` of every namespace the module was assembled against |

Function index space: imports first (`0 … nimports-1`), then module functions. `call`
targets any function index; calling an import is a host call.

## 5. Instructions

Notation: `d` destination register, `a b c` source registers, `imm` immediate, `L` label
(an instruction index inside the same function), `f` function index. `T` ∈ {`i32`, `i64`},
`F` ∈ {`f32`, `f64`}. Every instruction costs 1 unit of budget, except that `memory.copy`, `memory.fill` and `memory.grow` also pay for their size (see "Changes from draft 2", item 1).

**Constants and moves**
- `const.i32 d, imm32` · `const.i64 d, imm64` · `const.f32 d, imm` · `const.f64 d, imm`
- `mov d, a` (copies all 64 bits)

**Integer (for each T)**
- `T.add d,a,b` `sub` `mul` `and` `or` `xor` `shl` `shr_s` `shr_u` `rotl` `rotr`
- `T.div_s` `T.div_u` `T.rem_s` `T.rem_u`: divisor 0 → `Trap::DivideByZero`; `div_s` of `MIN / -1` → `Trap::IntegerOverflow`; `rem_s` of `MIN % -1` = 0.
- `T.clz d,a` `T.ctz d,a` `T.popcnt d,a` `T.eqz d,a`
- Comparisons, result `i32` 0 or 1: `T.eq` `ne` `lt_s` `lt_u` `gt_s` `gt_u` `le_s` `le_u` `ge_s` `ge_u`

**Float (for each F)**
- `F.add` `sub` `mul` `div` `min` `max` `copysign` (three operands)
- `F.abs` `neg` `sqrt` `ceil` `floor` `trunc` `nearest` (two operands)
- Comparisons, result `i32`: `F.eq` `ne` `lt` `gt` `le` `ge`

**Conversions** (two operands)
- `i32.wrap_i64` · `i64.extend_i32_s` · `i64.extend_i32_u`
- `T.extend8_s` · `T.extend16_s` · `i64.extend32_s`
- `T.trunc_F_s` · `T.trunc_F_u`: NaN → `Trap::InvalidConversion`; out of range → `Trap::IntegerOverflow`
- `T.trunc_sat_F_s` · `T.trunc_sat_F_u` (saturating, NaN → 0, never trap)
- `F.convert_T_s` · `F.convert_T_u` · `f32.demote_f64` · `f64.promote_f32`
- `i32.reinterpret_f32` · `i64.reinterpret_f64` · `f32.reinterpret_i32` · `f64.reinterpret_i64`

**Select**
- `select d, cond, a, b`: `d = if reg[cond] as i32 != 0 { a } else { b }`

**Memory**
- `i32.load d, base, off` · `i64.load` · `f32.load` · `f64.load`
- `i32.load8_s` `i32.load8_u` `i32.load16_s` `i32.load16_u`
- `i64.load8_s` `i64.load8_u` `i64.load16_s` `i64.load16_u` `i64.load32_s` `i64.load32_u`
- `i32.store src, base, off` · `i64.store` · `f32.store` · `f64.store`
- `i32.store8` `i32.store16` · `i64.store8` `i64.store16` `i64.store32`
- `memory.size d` (pages, i64) · `memory.grow d, pages` (old size in pages, or -1 as i64 on failure)
- `memory.copy dst, src, len` · `memory.fill dst, value, len` (trap if either range is out of bounds; `copy` handles overlap)

**Globals**
- `global.get d, g` · `global.set g, a` (setting an immutable global is a `LoadError` at verification)

**Control**
- `jmp L` · `br_if a, L` (taken when `reg[a] as i32 != 0`) · `br_ifz a, L`
- `br_table a, [L0 … Ln-1], Ldefault` (index `reg[a] as u32`)
- `call f, base` · `call_indirect type, a, base` (table index in `reg[a]`; out of range or unset → `Trap::UndefinedElement`; wrong type → `Trap::IndirectCallTypeMismatch`)
- `ret base`
- `yield` (suspends the fiber: `Exit::Yield`; resuming continues after it)
- `unreachable` (`Trap::Unreachable`) · `nop`

Running past the last instruction of a function is impossible: the verifier requires
every function to end in `ret`, `jmp`, `br_table` or `unreachable`.

Opcode numbers are explicit constants in the source (`#[repr(u16)]` with a written
discriminant on every variant) and are never reused or renumbered once released. The
opcode table is generated into `OPCODES.md` by a test.

## 6. Calling convention

`call f, base` (the callee has `P` parameters and `R` results):
- arguments are the caller's registers `base … base+P-1`;
- after the call, results are in the caller's registers `base … base+R-1`;
- the verifier requires `base + max(P, R) <= nregs` of the caller;
- no other caller register is changed by the call.

`ret base` returns the callee's registers `base … base+R-1` as results.

Host functions use the same window: they receive `args: &[u64]` (length `P`) and write
`results: &mut [u64]` (length `R`).

The call stack (return positions and register frames) is held by the VM, outside script
memory. Its depth limit is set by the embedder; exceeding it → `Trap::StackOverflow`.

## 7. Execution, fibers and exits

```rust
pub enum Exit {
    Done,                 // the fiber's entry function returned; results are available
    Yield,                // `yield` executed, or a host function asked to yield
    OutOfBudget,          // budget used up; resume to continue
    Trap(Trap),           // the fiber is dead; the instance stays usable
}
pub enum Trap {
    OutOfBounds, DivideByZero, IntegerOverflow, InvalidConversion, Unreachable,
    StackOverflow, UndefinedElement, IndirectCallTypeMismatch,
    Host(String),         // a host function failed
}
```

- An `Instance<H>` owns memory, globals and its fibers. `H` is the embedder's host state.
- A **fiber** is one coroutine: its own call stack and registers; memory and globals are shared.
- API shape:
  ```rust
  let module = Module::decode(bytes)?;            // or built with ModuleBuilder; always verified
  let mut reg = HostRegistry::<H>::new();
  reg.define_namespace("env", 1, "game", "Console output.")?;
  reg.register(HostFn::new("env", "print").doc("Print `text`.").param("text", ApiType::Str),
      |cx, args, results| { let s = cx.read_str(args.str(0)?)?; … Ok(HostOk::Continue) })?;
  let mut inst = Instance::new(&module, &reg.imports(), Limits::default())?; // name + type + versions
  let fiber = inst.spawn("main", &[])?;                                  // export name + arguments
  match inst.resume(fiber, &mut host, 10_000) { Exit::Done => inst.results(fiber), … }
  ```
- Host functions are registered in a `HostRegistry` (namespace, name, typed and named
  parameters and results, doc); `HostRegistry::imports()` is the only producer of a non-empty
  `Imports`, and `HostRegistry::index()` is the same table as an API index
  (`crates/efevm/API_INDEX.md`). A host function receives typed `Args`/`Results` (by logical
  parameter index; a `str` is one parameter, two slots) and a `Caller` with access to `&mut H`,
  the instance memory (checked `read`/`write`, `read_bytes`, `read_str`, `read_struct`,
  `read_u32` … helpers) and globals. It returns `Ok(HostOk::Continue)`, `Ok(HostOk::Yield)`
  or `Err(Trap::Host(..))`. It cannot call back into the VM in v1.
- Fibers are scheduled by the embedder (resume whichever it wants); the VM never switches by itself.
- `Limits`: max memory pages, max call depth, max register stack size, max fibers (defaults in "Changes from draft 2", item 2).
- `Instance::call` runs one export to completion in a single resume ("Changes from draft 2", item 3).

## 8. Verification (at load, before anything runs)

- All indices in range: types, functions, imports, globals, table entries, exports, register numbers `< nregs`.
- Every label is a valid instruction index of the same function.
- Call windows fit (section 6); `call_indirect` type index valid.
- `global.set` only on mutable globals; data segments and memory limits consistent.
- Every function ends in a terminator; `br_table` has a default.
- Import resolution: every import must exist in `Imports` with an identical type, and every
  namespace recorded in the module's `api` section must exist with the same version.

Because registers are untyped 64-bit slots and memory is bounds-checked, a verified
module cannot break host memory safety even if it mixes types.

## 9. Binary format (`.efvm`, version 1)

- Header: bytes `EFVM`, then `u16` little-endian version (`1`).
- Then sections: `id: u8`, `size: uleb`, payload. Known ids in ascending order, each at
  most once: 1 types, 2 imports, 3 functions, 4 table, 5 memory, 6 globals, 7 exports,
  8 data, 9 code, 10 names, 11 lines, 12 api. Unknown ids ≥ 128 are skipped (custom sections).
- Integers are LEB128 (`uleb` unsigned, `sleb` signed). Strings are `uleb` length + UTF-8.
  Floats are fixed little-endian IEEE bytes.
- Code section: for each function, `uleb` instruction count, then instructions.
- Instruction: `opcode` as `uleb`, then operands in the order of section 5: registers,
  function/type/global indices and labels as `uleb`; `imm32`/`imm64` as `sleb`;
  load/store offsets as `uleb`; `br_table` as `uleb` count then that many labels then the default.
- A decoder must reject: truncated input, over-long LEB values, sections out of order,
  trailing bytes, counts that exceed the remaining input (no pre-allocation by untrusted counts).

### 9.1 Section payloads (added in draft 2)

LEB widths: every `uleb` is a `u32` unless marked `u64`; `imm32` is an `i32` `sleb`, `imm64` an
`i64` `sleb`. A LEB is over-long (rejected) if it uses more than `ceil(bits/7)` bytes (5 for 32-bit,
10 for 64-bit) or if its last byte sets bits that do not fit the type (for `sleb`: the unused
bits must equal the sign bit). Zero-padded encodings within the maximum length are accepted.
`ValType` is one byte: `0` i32, `1` i64, `2` f32, `3` f64. Booleans/flags are one byte `0` or `1`.
`vec(X)` = `uleb` count, then that many `X`.

| id | section | payload |
|---|---|---|
| 1 | types | `vec(functype)`; functype = `vec(ValType)` params, `vec(ValType)` results |
| 2 | imports | `vec(import)`; import = `string` namespace, `string` name, `uleb` type index |
| 3 | functions | `vec(func)`; func = `uleb` type index, `uleb` nregs (≤ 65535) |
| 4 | table | `vec(uleb u64)`; `0` = unset entry, otherwise function index + 1 (at most 2^32) |
| 5 | memory | flag byte (`0` no max, `1` has max), `uleb` min_pages, then `uleb` max_pages if flag is 1 |
| 6 | globals | `vec(global)`; global = `ValType`, mutable flag, init: `sleb` i32 / `sleb` i64 / 4 bytes f32 / 8 bytes f64 |
| 7 | exports | `vec(export)`; export = `string` name, kind byte (`0` function, `1` global), `uleb` index |
| 8 | data | `vec(segment)`; segment = `uleb u64` offset, `vec(byte)` bytes |
| 9 | code | `vec(body)`, count equal to the functions count; body = `vec(instruction)` |
| 10 | names | `vec(fname)` then `vec(rname)`; fname = `uleb` function index, `string`; rname = `uleb` function index, `uleb` register, `string` |
| 11 | lines | `vec(string)` file names, then `vec(flines)`; flines = `uleb` function index, `vec(entry)`; entry = `uleb` instruction index, `uleb` file id, `uleb` line, `uleb` column |
| 12 | api | 8 bytes index hash (`u64` little-endian), then `vec(ns)`; ns = `string` namespace, `uleb` version |

- A missing section means "empty" (memory: `min_pages` 0, no max). A functions section without a
  code section (or the other way round with a non-zero count) is rejected.
- The encoder writes a section only when it is non-empty (memory: when `min_pages != 0` or a max
  is set; api: whenever the module has one, even with no namespaces), so encoding is
  deterministic: `decode(encode(m)) == m`.
- The section `size` must equal exactly the bytes its payload uses. Custom sections (id ≥ 128) may
  appear anywhere and are skipped; ids 0 and 13–127 are rejected.
- The verifier rejects an api section whose namespace names are not identifiers joined by dots
  or appear twice (`VerifyError::ApiNamespace`).
- Register operands are `uleb` values ≤ 65535 (larger is a decode error). Function indices in
  names/lines use the full function index space (imports first).
- Instruction operands follow section 5 exactly, e.g. `select d, cond, a, b` is four register
  `uleb`s, `i32.store src, base, off` is two registers then the `uleb` offset, `const.f32` is the
  register then 4 raw bytes, `global.set g, a` is the global index then the register,
  `call_indirect type, a, base` is type index, register, register.

In memory, instructions are decoded once into a fixed-size form (for example
`struct Inst { op: Op, a: u32, b: u32, c: u32 }` with 64-bit constants and `br_table`
targets in side tables). That form is an implementation detail, not part of this spec.

## 10. Tests required for E1

- One or more tests per instruction, including every trap condition and the boundary
  values (`MIN / -1`, shift counts ≥ width, NaN conversions, memory edge addresses).
- Calls: recursion (factorial/fibonacci), mutual recursion, indirect calls, stack overflow trap.
- Budget: an endless loop returns `OutOfBudget`, and resuming continues correctly.
- Fibers: two fibers interleaved through `yield` sharing memory; host-requested yield.
- Host calls: arguments, results, memory access, host trap.
- Verifier: one rejecting test per rule in section 8.
- Binary: encode → decode round trip for every instruction; a "never panics" test that
  feeds truncated and randomly mutated modules to `Module::decode`.
- A benchmark (not a pass/fail test) of a counting loop, reporting ns per instruction;
  target under 10 ns in release.
- The crate builds for `wasm32-unknown-unknown`.

## Changes from draft 1

Draft 2 records what was built. None of these changes alters an instruction's semantics; they
pin down points draft 1 left open.

1. **Binary payloads (new 9.1).** Draft 1 named the sections but not their byte layout. Section
   9.1 now fixes every payload, the ValType bytes, the LEB width of every field and the
   "over-long LEB" rule. Missing sections mean empty, and the encoder omits empty sections, so
   encoding is deterministic. Register operands are at most 65535 in the file. Table entries are
   `uleb u64` holding index + 1, so every `u32` index can be represented, and a value above
   2^32 is rejected.
2. **Errors.** `LoadError` has four kinds: `Decode { offset, error: DecodeError }`,
   `Verify { func, inst, error: VerifyError }` (one `VerifyError` variant per rule of section
   8), `Link(LinkError)` for import resolution and the memory cap at `Instance::new`, and
   `Build(BuildError)` from `ModuleBuilder`. `Instance::spawn` returns `SpawnError`:
   `NoSuchExport`, `NotAFunction`, `ImportEntry` (an export that is an import cannot be a
   fiber entry), `ArgCount`, `TooManyFibers` and `StackOverflow`.
3. **Verifier rules made explicit.** These are additions to section 8:
   - `nregs >= nparams`;
   - `ret base` needs `base + R <= nregs`;
   - operand fields an instruction does not use must be zero, so the in-memory form is
     canonical;
   - a 32-bit global's init has its upper 32 bits zero;
   - name and line entries refer to existing functions, registers, instructions and files;
   - export names are unique across both kinds;
   - data segments must fit in `min_pages` (checked at verification, so a module that
     verifies always instantiates).
4. **Fiber lifecycle.** Resuming a fiber that is `Done` returns `Exit::Done` again, and a
   trapped fiber returns the same `Exit::Trap` again. An unknown or removed `FiberId` returns
   `Exit::Trap(Trap::Host("invalid fiber"))`. `FiberId` carries a generation, so stale ids
   are detected. `Instance::remove_fiber` frees a slot. Finished fibers count toward
   `max_fibers` until they are removed. `Instance::status` returns `Ready`, `Suspended`,
   `Done` or `Trapped(trap)`. The trap location and backtrace of a trapped fiber are
   available through `Instance::trap_location` / `Instance::backtrace`.
5. **Budget.** Budget 0 executes nothing and returns `OutOfBudget`. An instruction that traps
   still costs 1, and so does a host call. (Superseded by draft 3, item 1: `memory.copy`, `memory.fill` and `memory.grow` now cost in proportion to their size.) `Instance::resume_counted` also returns the number of instructions executed.
6. **Limits.** `max_call_depth` counts live activations including the entry function. A call
   that would exceed it traps with `StackOverflow`; with 0, `spawn` fails. `max_registers` is
   the total register slots of one fiber across all its frames. A register stack that cannot be
   allocated is also a `StackOverflow`. The defaults of draft 2 (16384 pages, depth
   10_000, 2^20 registers, 1024 fibers) are superseded by draft 3, item 2.
7. **Host boundary.** Values crossing into the VM keep section 2's invariant: `i32`/`f32` spawn
   arguments, host results and globals set through `Caller::set_global` /
   `Instance::set_global` have their upper 32 bits cleared according to the declared type. Host
   result slots start at 0, so a result the host does not write is 0. A host function can call
   `Caller::fiber()` to learn which fiber is calling. `Instance::set_global` (embedder) may set
   immutable globals; `Caller::set_global` (host function) may not.
8. **Small operand details.**
   - `call_indirect` and `br_table` read their index register as `u32`.
   - `memory.grow` reads its page count as `u64`.
   - `select`, `br_if` and `br_ifz` test the low 32 bits.

## Changes from draft 2

Draft 3 makes these three changes.

1. **Bulk memory cost and budget debt.** Budget is now in units. Every instruction costs 1 as
   before; in addition
   - `memory.copy` and `memory.fill` cost `1 + len / 256` (integer division of the `u64`
     length register);
   - `memory.grow` costs `1 + (pages * 65536) / 256`, that is `1 + pages * 256`, computed with
     saturating arithmetic. It is charged for the requested size even when the grow fails
     and returns -1.
   - A bulk op that traps (out of bounds) costs only 1 and creates no debt.

   If the cost is more than the budget left, the operation still completes, the remaining
   budget becomes 0, and the excess (cost minus what was left) becomes a per-fiber **debt**.
   The resume returns `Exit::OutOfBudget` right after that instruction, so the fiber is resumable.
   At the start of every later `resume`, before anything executes, the debt is paid:
   if `budget <= debt`, then `debt -= budget` and the resume returns `OutOfBudget` having
   executed nothing; otherwise `budget -= debt`, the debt is 0 and the rest of the budget
   runs normally. Budget 0 therefore never pays debt but never executes either. A cost exactly
   equal to the budget left creates no debt. A fiber with an absurd grow (for example `u64::MAX`
   pages) owes about 2^64 units, so it is effectively dead; the embedder can still remove it.
   `Instance::resume_counted` keeps counting executed **instructions**, not units: the extra
   units of bulk operations and debt payments are not included, so a resume that executes
   2 instructions and pays 10 units of debt reports 2.
2. **Lower default limits and stack release.** `Limits::default()` is now
   `max_memory_pages = 1024` (64 MiB), `max_call_depth = 4096`, `max_registers = 262144` (per
   fiber) and `max_fibers = 256`. When a fiber finishes (`Done`) or traps, its register stack,
   call stack and scratch buffer are freed at once; `results`, `status`, `trap_location` and
   `backtrace` keep working, and resuming the fiber returns the same `Exit` as before.
3. **`Instance::call`.** `call(&mut self, export: &str, args: &[u64], host: &mut H, budget: u64)
   -> Result<Vec<u64>, CallError>` spawns the export, resumes it once with `budget`, returns the
   results on `Done`, and removes the fiber in every case. `CallError` has the variants
   `Spawn(SpawnError)`, `Trap { trap, location: Option<(u32, u32)>, backtrace: Vec<(u32, u32)> }`
   (copied from `trap_location` / `backtrace` before the removal), `OutOfBudget` (including
   debt) and `Yield`. It implements `Debug`, `Clone`, `PartialEq`, `Eq`, `Display` and
   `std::error::Error`.
4. **`Instance::resume_units`.** `resume_units(&mut self, fiber: FiberId, host: &mut H, budget: u64)
   -> (Exit, u64)` is `resume` that also returns the budget units consumed by that call:
   executed instructions, plus the extra units of bulk operations, plus debt paid at entry. It
   never exceeds `budget`; an over-budget bulk op reports the whole remaining budget (the rest
   is debt, paid and counted by later resumes). Budget 0, a `Done`/`Trapped` fiber or an invalid
   id report 0. Summing it over resumes gives the total cost, so an embedder can keep a total
   budget across yield/resume loops.

## Changes from draft 3

Draft 4 adds the host registry and the API index (efescript roadmap E3/E4,
`crates/efescript/docs/e3-design.md`). The binary format version stays `1`: every draft 3 file
is still valid and means the same.

1. **Host functions only through the registry.** `Imports` can no longer be filled directly;
   `HostRegistry::imports()` builds it (a snapshot of `Arc`s) together with the namespace
   versions and the index hash. `Imports::new()` is the empty set. `Imports::names()` lists
   what is callable and `Imports::check_module(&Module)` returns every link problem at once.
2. **Section 12 `api`** (9, 9.1): the index hash and `(namespace, version)` pairs. Modules
   without it link exactly as before.
3. **Linking.** `Instance::new` also checks the namespace versions of section 12.
   `LinkError` gains `UnknownNamespace { namespace }`, `NamespaceVersion { namespace, module,
   host }` and `ApiHash { module, host }` (informational, only listed by `check_module` after
   real errors); `ImportTypeMismatch` now carries both signatures as text (`module`, `host`).
   A different index hash alone is not an error.
4. **Debugger access.** For a `Ready` or `Suspended` fiber, `Instance::fiber_location` (the next
   instruction), `fiber_registers` (the current frame) and `fiber_backtrace` (innermost first,
   callers at their `call` instruction). `resume` with budget 1 executes exactly one instruction
   (if no budget debt is owed).
