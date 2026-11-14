# efescript

efescript is the engine's scripting layer. Today it is an x86-flavoured assembly dialect ("nasm_efe") with a label-resolving bytecode compiler and a small register VM that runs the bytecode inside an `efepages` `Page`. A high-level language front end (`src/compiler`) exists only as a lexer plus empty AST stubs and is not wired to anything. The crate is both a library (`efescript`) and a CLI binary (`src/main.rs`).

## At a glance
| | |
|---|---|
| Path | `crates/efescript` |
| Kind | library + binary (`efescript`) |
| Depends on (workspace) | `efepages` (paged memory `Page`), `proc_macros` (const generators, sibling crate) |
| Used by | nothing else in the workspace (only listed as a workspace member in the root `Cargo.toml`); `strtohx` emits source for it but has no code dependency |
| Key external deps | `num-derive` / `num-traits` (u8/usize <-> enum conversion for opcodes, operand types, syscalls); `rodio` (optional, `audio` feature) |
| Targets | native only in practice: Readln/ReadCon/ReadFs/WriteFs use stdin and the file system directly, no wasm32 support |
| Features | `audio` (off by default): real `PlaySound` playback through `rodio`. Needs the ALSA development headers on Linux (`alsa-lib-devel` / `libasound2-dev`). It was NOT compiled on the dev machine, so it is untested; the default build has no audio and `PlaySound` fails with ra = 1 |

## Layout
```text
crates/efescript/
├── Cargo.toml            # package efescript 0.2.0, edition 2021
├── README.md             # language reference, how to run examples and tests
├── ROADMAP.md            # E0..E7 plan (E0 done)
├── docs/e0-design.md     # design of the E0 work: db/symbols, host, RunResult, CLI, tests
├── noname.gv             # stray vim graphviz template, unrelated
├── examples/             # every example has a header comment with its expected output
│   ├── hello.efe         # `db` + Println: "hello world!"
│   ├── strings.efe       # db, NAME, NAME.len, BYTEPTR [msg], [msg+reg], Printchar loop
│   ├── jumps.efe         # je jne jgt jlt jge jle jz jnz js, taken and not taken
│   ├── recursion.efe     # recursive sum(n) and fib(n) with call/ret and the stack
│   ├── sound.efe         # PlaySound on examples/ominous_worse.ogg (path relative to the cwd)
│   ├── call.efe          # call/ret, ends with Finish(5)
│   ├── test.efe          # reads a line from stdin and echoes it
│   ├── heap.efe          # documented "expected fault" (HeapAlloc is not implemented)
│   ├── create_e.efe      # WriteFs: writes ./haha in the working directory
│   ├── haha, out.efec    # leftovers of earlier runs, kept unchanged
│   ├── main.sfe          # high-level syntax sketch: `fn main(){}`; nothing reads it
│   └── ominous_worse.ogg # audio file used by sound.efe
├── tests/                # integration tests (CaptureHost + run_with_limit)
│   ├── common/mod.rs     # run()/exit_code() helpers
│   ├── examples.rs       # examples run to exact output and RunResult
│   ├── sound.rs          # PlaySound with a recording host and a failing host
│   ├── control_flow.rs   # jumps, loops, nested calls, recursion depth 1000/20000, step limit
│   ├── strings.rs        # db escapes, UTF-8 len, data before/after code, data not executed
│   ├── errors.rs         # ParseErr variants with positions, runtime faults never panic
│   └── cli.rs            # the built binary: exit codes, -o, .efec, no out.efec
├── proc_macros/          # sibling crate, see its structure.md
├── strtohx/              # sibling crate, see its structure.md
└── src/
    ├── lib.rs            # module tree; invokes proc_macros::max_addr!/stack_page_count!
    ├── prelude.rs        # re-exports ProgramRuntime, RunResult, TickSignal, Host, StdHost, CaptureHost, parse_str, parse_from_file
    ├── main.rs           # CLI: assemble + run a .efe file
    ├── ops.rs            # OpCodes enum (bytecode opcodes)
    ├── instruction.rs    # Instruction, Operands, PtrInner, Registers (in-memory form)
    ├── syscalls.rs       # SysCalls enum + number-literal helpers (as_usize, as_u8..as_u64)
    ├── funs.rs           # same helpers plus trim_zeroes, get_db_data
    ├── macros.rs         # reg!, as_be_bytes! macros
    ├── nasm_efe/
    │   ├── mod.rs        # token -> PreCompile (instructions + labels); OperandType; parse_str/parse_from_file
    │   ├── nasm_funs.rs  # opcode/register name tables, Operands::to_int (operand encoding), PtrInner::from_tokens
    │   └── nasm_compiler.rs # label resolution (Data: r1/r2 passes) and final byte emission
    ├── runtime/
    │   ├── mod.rs        # SimpleOperands (operand byte -> class)
    │   ├── host.rs       # Host trait, StdHost, CaptureHost
    │   ├── data_funs.rs  # NumToStr enum; mem_read/mem_write (checked page access); get_inner_ptr
    │   ├── allocator/    # VMAllocator trait + GlobalAllocator: all todo!(), unused
    │   └── program/
    │       ├── mod.rs    # PRegisters, PFlags, ProgramRuntime, RunResult
    │       ├── tests.rs  # lib tests of the VM
    │       ├── init.rs   # loading bytecode, creating the main thread
    │       ├── runtime.rs# run, run_with_limit, tick_all, tick(thread)
    │       └── thread/
    │           ├── mod.rs        # PThread, SyscallSignal
    │           ├── runtime.rs    # fetch/decode (get_operand, get_instruction), tick
    │           ├── opcode.rs     # execution of each opcode
    │           ├── opcode_funs.rs# get_op_data / write_op_data (register, memory access)
    │           └── syscall.rs    # syscall implementations
    ├── compiler/         # high-level language front end (stub)
    │   ├── mod.rs        # empty `Compiler` struct, private empty `AstObjects`
    │   └── ast/          # Ast/Header/Body empty structs, `Primitives`, calc tree types (unused);
    │       └── lexer/mod.rs # working-ish character lexer (LexerTokens) - also used by the nasm parser
    └── debugger/mod.rs   # `Debugger { runtime }` struct, no behaviour
```

## Main concepts

### Pipeline
```text
.efe text
  -> LexerTokens::string_to_token + trim_spaces           (compiler/ast/lexer)
  -> parse_str_to_instructions -> instructions, labels, `db` data  (nasm_efe/mod.rs)
  -> one sizing pass (addresses of every label and data name), one emit pass (nasm_compiler.rs)
  -> Vec<u8>: code, one 0x00 end marker, then the data bytes
  -> ProgramRuntime::load_from_vec  (copies bytes to Page offset 0, creates main PThread)
  -> run / run_with_limit -> PThread::tick: decode one instruction, execute it, repeat
```

### Source format
One statement per line; `; comment` anywhere; case-insensitive mnemonics. Labels: `label NAME` or `NAME:`. Data: `NAME db "text", 10, 0xff`. Two operands are separated by a real comma. Numbers: decimal, `0x`, `0b`, `0o`, negative immediates. Details and the register/syscall tables are in `README.md`.

### Strings and symbols
- `NAME db item, ...` (string literal with `\n \t \r \0 \\ \" \xNN` escapes, UTF-8 bytes, or numbers 0-255) defines `NAME` (address) and `NAME.len` (byte length). Labels and data names share one namespace; a duplicate or undefined name is a `ParseErr`, never a panic.
- Symbols are usable as immediates (`mov rb, msg`, `mov rc, msg.len`, `mov r1, _label`) and inside pointers (`BYTEPTR [msg]`, `[rbp+msg]`, `[msg+r1]`); jumps/calls take a label or a register.
- All data goes after the last instruction in definition order, preceded by one `0x00` byte (the end-of-program marker), wherever the `db` line is in the source.
- Every symbol reference is encoded as a static operand with a fixed 4-byte value (`00 04 b3 b2 b1 b0`, big-endian), so instruction sizes never depend on addresses (no multi-pass size guessing). Programs larger than 4 GiB are rejected (`ProgramTooLarge`).

### Bytecode and operand encoding
- Instruction = 1 opcode byte followed by 0-2 operands. Opcode 0 is never valid and means end of program.
- Operand = type byte (`OperandType`): `Static=0`, `RA=1..RD=4`, `RBP=5`, `RSP=6`, `R1..R6=7..12`, `AL,AH,BL,BH,CL,CH,DL,DH=13..20`, `Pointer=0x16`, `BYTEPTR..QWORDPTR` after it, `NULL=0xff`.
- `Static`: `0x00, len, bytes` big-endian, leading zeroes trimmed for plain numbers, fixed 4 bytes for symbols.
- Pointer operands (`[...]`): type byte, inner items, optional `+`/`-` and a second item, then the type byte again. Forms: `[x]`, `[reg+n]`, `[reg-n]`, `[n-reg]`, `[reg+reg]`, `[reg-reg]`; `x`/`n` may be a symbol.
- All multi-byte integers are big-endian.

### Instruction set (`ops.rs`)
| Group | Mnemonics |
|---|---|
| Data | `mov`, `lea` (effective address of a pointer operand) |
| Arithmetic | `add8/16/32/64`, `sub8/16/32/64` (wrapping, result truncated to the width) |
| Logic | `or`, `xor`, `and`, `nand`, `nor` |
| Stack | `push8/16/32/64`, `pop8/16/32/64` |
| Compare | `cmp` (equals/greater/lesser/ge/le, unsigned), `test` (zero, sign = top bit) |
| Jumps | `jmp`, `je`, `jne`, `jgt`, `jlt`, `jle`, `jge`, `jz`, `jnz`, `js` |
| Calls | `call`, `ret`, `syscall`, `nop` |

There is no multiply, divide, shift or signed comparison.

### Registers and flags
`PRegisters` (all `usize`): `ra rb rc rd rbp rsp r1..r6`. `al ah bl bh cl ch dl dh` are byte 0 / byte 1 of `ra..rd`; writes are masked to 8 bits. `cmp` and `test` reset all flags first.

### Memory model
One `efepages::page::Page` holds code, data and stack. Bytes are loaded at offset 0 and `rsp` starts at `program_size + 2` (after code and data), growing upward. `push` writes then increments, `pop` decrements then reads, `call` pushes an 8-byte return address, `ret` pops it; popping below the stack base is a fault ("stack underflow"). Syscall memory access is checked (`mem_read`/`mem_write` return `Err`, which becomes a fault). Nothing prevents writing over code.

### Host interface (`runtime/host.rs`)
```rust
pub trait Host {
    fn write_out(&mut self, bytes: &[u8]);                       // Printchar / Print / Println
    fn play_sound(&mut self, path: &str) -> Result<(), String>;  // PlaySound
}
```
- `StdHost` (default of `ProgramRuntime::new()`): writes to stdout; `play_sound` uses `rodio` with the `audio` feature, otherwise returns `Err("... built without the `audio` feature")`.
- `CaptureHost { out: Vec<u8>, sounds: Vec<String> }`: records everything. `CaptureHost::shared()` returns an `Rc<RefCell<CaptureHost>>` that itself implements `Host` (`Rc<RefCell<H>>: Host`), so a test can pass `Box::new(cap.clone())` to `ProgramRuntime::with_host` and read `cap.borrow()` afterwards.

### Running and `RunResult`
`run()` and `run_with_limit(max_instructions)` return `RunResult::{Finished(u8), EndOfProgram, LimitReached, Fault(String)}`. `tick` returns a `TickSignal::{Continue, End, Finish(code), Fault(msg)}`. Faults (bad opcode byte, unknown or unimplemented syscall, bad memory access, stack underflow, invalid Printchar value, ...) are printed to stderr as `efescript: runtime error: ...` and returned; the VM does not panic on bytecode input. Threads: `run_*` round-robins over `threads`, but only the main thread exists (no spawn API). The `run_all_nonstop` wrapper is kept.

### Syscalls
Number in `ra`, arguments in `rb rc rd r1`. Numbers follow the `SysCalls` enum order, `Finish = 5050`, `RaylibEx1 = 2550`. An unknown number is a fault.

| ra | Name | Inputs | Behaviour / outputs |
|---|---|---|---|
| 0 | Printchar | `rb` = char (Unicode scalar) | writes it UTF-8 encoded; invalid scalar = fault |
| 1 | Print | `rb` = ptr, `rc` = len | writes the bytes plus a trailing space |
| 2 | Println | `rb` = ptr, `rc` = len | writes the bytes plus `\n` |
| 3 | NumToString | `rb` = number, `rc` = 1 signed else unsigned, `rd` = 1 hex / 2 bin / 3 oct else dec | writes text at `rsp`; `rb` = ptr, `rc` = len; `rsp` += len (save/restore `rsp` yourself before `ret`) |
| 4 | StringToNum | `rb` = ptr, `rc` = len | result in **`ra`**; bad input is a fault |
| 5 | Readln | - | reads a stdin line (newline stripped), writes at `rsp`; `rb` = ptr, `rc` = len |
| 6 | ReadCon | `rd` = delimiter byte | `read_until` on stdin (delimiter kept), same outputs |
| 7 | ReadFs | `rb`/`rc` = path | reads the file at `rsp`, `rsp` += length (no pointer register is set) |
| 8 | WriteFs | `rb`/`rc` = data, `rd`/`r1` = path | `fs::write` |
| 9 | HeapAlloc | - | not implemented: fault |
| 10, 11 | Write, Flush | - | not implemented: fault |
| 12 | PlaySound | `rb`/`rc` = path | `ra` = 0 ok / 1 failed (error on stderr); never faults |
| 2550 | RaylibEx1 | - | not implemented: fault |
| 5050 | Finish | `rb` = exit code | ends the run with `Finished(rb as u8)` |

### CLI (`src/main.rs`)
```text
efescript <file.efe>          assemble and run
efescript <file.efec>         run precompiled bytes (selected by the .efec extension)
efescript <file.efe> -o out   assemble to the file `out`, do not run
  -d  print the runtime state after the run     -r  print the memory usage
  -h, --help
```
Exit code: the Finish code; 0 at the end of the program (or when the step limit is hit, which the CLI does not set); 2 for a missing file, bad arguments or an assembly error (`efescript: file: ParseErr(...)` on stderr); 3 for a runtime fault; 1 when no file is given. Nothing is written to the working directory unless `-o` is used.

## Public API
`lib.rs` exposes `pub mod prelude`, `nasm_efe`, `compiler`, `debugger`; `ops`, `instruction`, `syscalls`, `funs`, `macros` and `runtime` are `pub(crate)`.
- `efescript::prelude::{ProgramRuntime, RunResult, TickSignal, Host, StdHost, CaptureHost, parse_str, parse_from_file}`.
- `parse_str(impl Into<String>) -> Result<Vec<u8>, ParseErr>`; `parse_from_file(path)` reads the file first (`ParseErr::Io` instead of a panic). `ParseErr` (in `efescript::nasm_efe`): `NoData, WrongToken, WrongOpCode, WrongOperand, ErrLabelName, EOF, WrongPtrInner, UndefinedSymbol, DuplicateSymbol, BadString, BadNumber, Expected, ProgramTooLarge, Io`; most carry a `LineColumn`.
- `ProgramRuntime`: `new()`, `with_host(Box<dyn Host>)`, `load_from_vec(Vec<u8>)`, `load_from_file(path)`, `compile_from_file(path)`, `run()`, `run_with_limit(n)`, `run_all_nonstop()`, `tick_all()`, `tick(thread_idx)`, `print_program_slice(start, len)`; public fields `program` (`Page`), `threads`, `counter`.
- Embedding: `let cap = CaptureHost::shared(); let mut p = ProgramRuntime::with_host(Box::new(cap.clone())); p.load_from_vec(parse_str(src)?); let r = p.run_with_limit(1_000_000);`.

## Tests
`cargo test -p efescript`: lib tests (lexer, assembler, VM; one per fixed bug) plus `tests/*.rs` (see Layout). All use `CaptureHost`; none needs an audio device. Run examples from the crate directory so `examples/ominous_worse.ogg` resolves.

## Fixed in E0 (no longer bugs)
`dbg!` noise; `[rbp+8]` right operand; hex literals containing `b`; decimal literals in `push32 5` / `[8]`; label sizing near address 255; undefined and duplicate labels; first static in a pointer reading one byte; `[reg-reg]`; sub-register masks; `mov al, x` masking; `lea` and `js` unusable; `Finish` ignored; `tick` result discarded; unknown opcode/syscall, bad UTF-8, `StringToNum` and `subN` underflow panics; `Println`/`Print` debug output; CLI always writing `./out.efec`; `parse_from_file` panics; moving the runtime after `load_*` (runtime_can_move_after_load test).

## Status and known gaps
- High-level compiler is not implemented: `compiler/mod.rs` has an empty `Compiler`; `compiler/ast/*` types are unused skeletons; `examples/main.sfe` is never read. The `debugger` module is a struct without methods.
- Not implemented syscalls (faults): `HeapAlloc`, `Write`, `Flush`, `RaylibEx1`. `ReadFs` sets no result register.
- `ProgramRuntime::load_from_vec_new` is `todo!()`; `ProgramRuntime::compile_from_file` and `load_from_file` still `unwrap()` (panic on a missing file; the CLI does not use them); `runtime/allocator` is all `todo!()` and unused; a few `todo!()` remain in `nasm_funs.rs` for operand kinds that are never built.
- `cmp`/`test` compare as unsigned; no multiply, divide, shifts or signed comparison. `carry` is never set.
- The format is headerless and opcode numbers follow enum order (replaced by the v1 module format in E2); memory is big-endian and registers are `usize`.
- Code is writable and the stack has no limit other than memory; `STACK_PAGE_COUNT` / `MAX_ADDR_SIZE` are unused.
- The `audio` feature is untested (see Features). No wasm/other-OS support.
