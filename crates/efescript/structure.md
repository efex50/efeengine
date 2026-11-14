# efescript

efescript is the engine's scripting layer. Today it is an x86-flavoured assembly dialect ("nasm_efe") with a label-resolving bytecode compiler and a small register VM that runs the bytecode inside an `efepages` `Page`. A high-level language front end (`src/compiler`) exists only as a lexer plus empty AST stubs and is not wired to anything. The crate is both a library (`efescript`) and a CLI binary (`src/main.rs`).

## At a glance
| | |
|---|---|
| Path | `crates/efescript` |
| Kind | library + binary (`efescript`) |
| Depends on (workspace) | `efepages` (paged memory `Page`), `proc_macros` (const generators, sibling crate) |
| Used by | nothing else in the workspace (only listed as a workspace member in the root `Cargo.toml`); `strtohx` emits source for it but has no code dependency |
| Key external deps | `num-derive` / `num-traits` (u8/usize <-> enum conversion for opcodes, operand types, syscalls); `text_io` (line reading for the `Readln` syscall) |
| Targets | native only in practice: `Readln` has `#[cfg(target_os = "windows"/"linux")]` branches only (other OSes will not compile), syscalls use stdin/stdout/fs directly, no wasm32 support |
| Features | none |

## Layout
```text
crates/efescript/
├── Cargo.toml            # package efescript 0.2.0, edition 2021
├── README.md             # one line ("# efescipt", UTF-16 encoded)
├── noname.gv             # stray vim graphviz template, unrelated
├── examples/
│   ├── call.efe          # call/ret demo with labels
│   ├── create_e.efe      # stack string builder + Print loop
│   ├── heap.efe          # HeapAlloc syscall (unimplemented)
│   ├── hello.efe         # "hello world!" via push64/push32 + syscall
│   ├── test.efe          # long scratch program (stdin, pointers, jumps)
│   ├── main.sfe          # high-level syntax sketch: `fn main(){}`; nothing reads it
│   ├── out.efec          # empty leftover compiler output
│   └── ominous_worse.ogg # unrelated audio file
├── proc_macros/          # sibling crate, see its structure.md
├── strtohx/              # sibling crate, see its structure.md
└── src/
    ├── lib.rs            # module tree; invokes proc_macros::max_addr!/stack_page_count!
    ├── prelude.rs        # re-exports ProgramRuntime, parse_from_file, parse_str
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
    │   ├── data_funs.rs  # NumToStr enum; get_inner_ptr (pointer operand decoder, lightly duplicated in opcode_funs.rs)
    │   ├── allocator/    # VMAllocator trait + GlobalAllocator: all todo!(), unused
    │   └── program/
    │       ├── mod.rs    # PRegisters, PFlags, ProgramRuntime
    │       ├── init.rs   # loading bytecode, creating the main thread
    │       ├── runtime.rs# run_all_nonstop, tick_all, tick(thread)
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
  -> parse_str_to_instructions  -> Vec<PreCompile>         (nasm_efe/mod.rs: I(Instruction) | L(label))
  -> nasm_compiler::Data: from_pre_compile, r1(), r2(), r2()   (label addresses resolved)
  -> to_program() -> Vec<u8> bytecode                      (nasm_efe/nasm_compiler.rs)
  -> ProgramRuntime::load_from_vec  (copies bytes to Page offset 0, creates main PThread)
  -> run_all_nonstop -> PThread::tick: decode one instruction, execute it, repeat
```
Source format: one instruction per line, `;` starts a comment, `label name` defines a label (any identifier), identifiers/opcodes are case-insensitive, operands separated by a single separator token (a comma in the examples). Numbers may be decimal, `0x`, `0b`, `0o` (`as_usize`).

### Bytecode and operand encoding
- Instruction = 1 opcode byte followed by 0-2 operands. Opcode 0 is never valid and is treated as end of program (`get_instruction` returns `None`).
- Operand = type byte (`OperandType`): `Static=0`, `RA=1..RD=4`, `RBP=5`, `RSP=6`, `R1..R6=7..12`, `AL,AH,BL,BH,CL,CH,DL,DH=13..20`, `Pointer=0x16`, `BYTEPTR..QWORDPTR` after it, `NULL=0xff`. Registers are just the type byte.
- `Static`: `0x00, len, bytes` where bytes are the big-endian value with leading zeroes trimmed (value 0 has `len 0`).
- Pointer operands (`[...]`): type byte, inner items (register byte or static), optional `+`/`-` ASCII byte and a second item, then the same type byte again as terminator. Inner forms: `[x]`, `[reg+n]`, `[reg-n]`, `[n-reg]`, `[reg+reg]`, `[reg-reg]` (`PtrInner`).
- Jumps/calls are emitted as `opcode, 0x00, len, addr bytes`: the label operand becomes a `Static` absolute byte offset into the program. Because address size can change the instruction length, labels are resolved in several passes (`r1`, then `r2` twice).
- All multi-byte integers (pushes, memory loads/stores, return addresses, Static values) are big-endian.

### Instruction set (`ops.rs`)
| Group | Mnemonics (opcode numbers in enum order) |
|---|---|
| Data | `mov`=1; `lea`=15 (parsed, but executing it is `todo!()`) |
| Arithmetic | `add8/16/32/64`=2-5, `sub8/16/32/64`=6-9 (result truncated to the width, plain `+`/`-`, so overflow panics in debug builds) |
| Logic | `or`=10, `xor`=11, `and`=12, `nand`=13, `nor`=14; full `usize` width |
| Stack | `push8/16/32/64`=20-23, `pop8/16/32/64`=24-27 |
| Compare | `cmp`, `test` |
| Jumps | `jmp`, `je`, `jne`, `jgt`, `jlt`, `jle`, `jge`, `jz`, `jnz`, `js` |
| Calls | `call`, `ret`, `syscall`, `nop`=255 |

Numbers follow the enum: `Mov`=1 counting up to `Lea`=15, `Push8` is pinned to 20, then counting up (`Cmp`=28 ... `Ret`/`SysCall` last); `Nop`=255.

### Registers and flags
`PRegisters` (all `usize`): `ra rb rc rd` (general), `rbp`, `rsp`, `r1..r6`. 8-bit sub-registers `al ah bl bh cl ch dl dh` address the low byte and byte 1 of `ra/rb/rc/rd` for reads and writes via `mov`/arithmetic. `PFlags`: `carry equals greater lesser greatereq lessereq zero negative sign`. Only `cmp` (equals/greater/lesser/greatereq/lessereq) and `test` (zero/lesser/greater/negative) set flags, and both reset all flags first. `carry` and `sign` are never set (so `js` never jumps). `cmp`/`test` compare as unsigned `usize`.

### Memory model
There is a single `efepages::page::Page` for everything. The bytecode is written at offset 0 (`program.write(0, bytes)`), and the main thread's `rsp` starts at `program_size + 2`, so the stack grows upward right after the code in the same address space. `push` writes at `rsp` then increments; `pop` decrements then reads; `call` pushes the return address as 8 big-endian bytes. Memory is read/written through `Page::read(addr, len)` / `Page::write(addr, Vec<u8>)`; `-r` on the CLI reports `page.len() * PAGE_SIZE`. Because code and data share the page, nothing prevents writes over code. `proc_macros` generates `MAX_ADDR_SIZE` (2^48) and `STACK_PAGE_COUNT` (env `STACK_PAGE_COUNT` at build time, default 2) constants in `lib.rs`, but nothing uses them yet.

### Threads and scheduling
`ProgramRuntime { counter, program: Page, program_size, threads: Vec<PThread>, stdin/stdout/stderr: Vec<u8> }`. `init_program` creates exactly one `PThread` (id 0). Each `PThread` has its own `counter` (program counter), registers and flags, and raw pointers (`*mut Page`, `*mut Vec<u8>`) to the runtime's page and std buffers. `run_all_nonstop` round-robins `tick()` over all threads and stops as soon as any thread's `tick` returns `None` (decoded opcode byte 0). There is no API to spawn more threads. `tick_all` and `tick(i)` allow host-driven stepping.

### Syscalls
`syscall` reads the number from `ra`. Numbers follow `SysCalls` enum order, `Finish = 5050`, `RaylibEx1 = 2550`. An unknown number panics (`from_usize(..).unwrap()`).

| ra | Name | Inputs | Behaviour / outputs |
|---|---|---|---|
| 0 | Printchar | `rb` = char | `print!` of `rb as u8 as char` |
| 1 | Print | `rb` = ptr, `rc` = len | `print!("{} ", s)` (note trailing space), UTF-8 required |
| 2 | Println | `rb` = ptr, `rc` = len | `dbg!` of the bytes, then `println!` |
| 3 | NumToString | `rb` = number, `rc` = 1 signed else unsigned, `rd` = 1 hex / 2 bin / 3 oct else dec | writes text at `rsp`; `rb` = ptr (old `rsp`), `rc` = len; `rsp` += len |
| 4 | StringToNum | `rb` = ptr, `rc` = len | parses with `as_usize` (supports `0x/0b/0o`); result in **`ra`** (doc comment says `rb`); panics on bad input |
| 5 | Readln | - | reads a line (`text_io`), writes at `rsp`; `rb` = ptr, `rc` = len; `rsp` += len |
| 6 | ReadCon | `rd` = delimiter byte | `read_until` on stdin, same outputs as Readln (delimiter included) |
| 7 | ReadFs | `rb` = path ptr, `rc` = path len | reads the file, writes it at `rsp`, `rsp` += file length; does not return a pointer in a register despite its doc |
| 8 | WriteFs | `rb`/`rc` = data ptr/len, `rd`/`r1` = path ptr/len | `fs::write` |
| 9 | HeapAlloc | `rb` = size | `todo!()` (panics) |
| 10 | Write | - | `todo!()` |
| 11 | Flush | - | `todo!()` |
| 2550 | RaylibEx1 | `rb`/`rc` = title | reads string then `todo!()` |
| 5050 | Finish | `rb` = exit code | returns `SyscallSignal::Finish(code)`, which is currently discarded (see gaps) |

### CLI (`src/main.rs`)
`efescript <file.efe> [-d|-r]`
1. no argument: prints (Turkish) help and exits with code 1.
2. assembles the file with `parse_from_file(...).unwrap()`.
3. always writes the bytecode to `./out.efec` in the current directory.
4. loads and runs with `run_all_nonstop`.
5. `-d` prints the `Debug` form of the `ProgramRuntime` (including the page); `-r` prints `ram usage :N bytes` (page count x `PAGE_SIZE`).
Note the assembler currently leaves `dbg!` output of every token on stderr (nasm_efe/mod.rs:211).

### Example (`examples/hello.efe`, trimmed)
```asm
mov rbp,rsp              ; rbp = start of the stack (data area)
push64 0x68656c6c6f20776f ; bytes "hello wo" pushed big-endian at rsp
push32 0x726c6421        ; "rld!"  -> stack now holds "hello world!" (12 bytes)
mov ra, 2                ; syscall 2 = Println
mov rb, rbp              ; pointer to string
mov rc, 12               ; length
syscall                  ; prints: hello world!
```
Running it assembles to a few dozen bytes and prints `hello world!`. `examples/call.efe` shows labels with `call _test` / `ret`.

## Public API
`lib.rs` exposes `pub mod prelude`, `nasm_efe`, `compiler`, `debugger`; `ops`, `instruction`, `syscalls`, `funs`, `macros` (macros are `#[macro_export]` and so still reachable) and `runtime` are `pub(crate)`.
- `efescript::prelude::{ProgramRuntime, parse_from_file, parse_str}`.
- `parse_str(impl Into<String>) -> Result<Vec<u8>, ParseErr>`: assembly text -> bytecode. `parse_from_file(path)` does the same after reading the file (panics if the file cannot be read or is not UTF-8). `ParseErr` variants: `NoData, WrongToken, WrongOpCode, WrongOperand, ErrLabelName, EOF, WrongPtrInner`.
- `ProgramRuntime`: `new()`, `load_from_vec(Vec<u8>)`, `load_from_file(path)` (loads raw bytecode, e.g. an `.efec`), `compile_from_file(path)` (an associated constructor: assembles a file and returns a ready `ProgramRuntime`), `run_all_nonstop()`, `tick_all()`, `tick(thread_idx)`, `print_program_slice(start, len)`; public fields `program` (`Page`), `threads`, `counter`, `stdin/stdout/stderr`.
- `PThread` (in the crate-private `runtime` module but reachable through `ProgramRuntime::threads`): public `registers`, `flags`, `counter`, `tick()`, `start()`.
- Host embedding sketch: `let mut p = ProgramRuntime::new(); p.load_from_vec(parse_str(src)?); p.run_all_nonstop();`.

## Status and known gaps
- High-level compiler is not implemented: `compiler/mod.rs` only has an empty `Compiler` struct; `compiler/ast/*` types (`Ast`, `Header`, `Body`, `Primitives`, `AstCalcObj*`) are unused skeletons; `examples/main.sfe` (`fn main(){}`) is never read. The lexer works on text but `get_str` (string literals) is unimplemented/unused. The `debugger` module is a struct with no methods.
- Finish syscall is ignored: `PThread::tick` discards the `HandleSignal` returned by `handle_opcodes` (`runtime/program/thread/runtime.rs:177`), so `Finish(code)` (and `Todo`/`Err` signals) have no effect; programs only stop on an opcode byte 0 (running past the end of the code). Several examples still use `mov ra,3 / syscall` as "exit", which is now NumToString.
- `lea` is parsed and encoded but `todo!()` at decode (`thread/runtime.rs:153`); `Operands::String`, `OperandType::String`/`NULL` operand building and `SimpleOperands::PushPop` are `todo!()`.
- Raw pointers: `PThread` stores `*mut Page` and `*mut Vec<u8>` (null by default) filled in `init_program` (`init.rs`); they are dereferenced in `unsafe` blocks and also alias `&mut self.threads` via a raw-pointer cast in `program/runtime.rs`. Moving the `ProgramRuntime` after `load_*` would leave dangling pointers (the pointers target fields of the struct itself).
- Panics on bad input throughout: unknown/odd syntax in the assembler (`unwrap()` on the token iterator, a missing trailing EOF), invalid UTF-8 in print/read syscalls, unknown syscall number, unknown opcode byte (`thread/runtime.rs:78`), subtraction underflow in `subN`, `Operands::Null` write.
- Assembler bug: in `PtrInner::from_tokens` the right-hand operand is read from `t[0]` instead of `t[2]` (`nasm_efe/nasm_funs.rs:67`), so `[rbp+8]`-style pointers are not built correctly; `examples/hello.efe` marks this form "çalışmıyor" (not working).
- Pointer arithmetic bugs at runtime: `PtrInner::ExtReg` (`reg-reg`) adds instead of subtracting (`opcode_funs.rs:240`); sub-registers used inside a pointer use `r | 0xff` instead of `r & 0xff` (`opcode_funs.rs:261`); `get_inner_ptr` reads a single byte for the first static operand (`data_funs.rs`).
- `js` is never taken (`sign` flag never set), and `Js` is missing from the jump list in `nasm_compiler.rs` (its label would not be resolved).
- `Print`/`Println` syscall quirks: trailing space and `dbg!` output; `HeapAlloc`, `Write`, `Flush`, `RaylibEx1` are `todo!()`; `ReadFs` leaves no result register; `StringToNum` returns in `ra` although documented as `rb`.
- `ProgramRuntime::load_from_vec_new` is `todo!()`; `runtime/allocator` (`VMAllocator`, `GlobalAllocator`) is all `todo!()` and unused; `reg!` macro and many helper fns (`get_db_data`, `as_u8..as_u64`) are unused.
- `STACK_PAGE_COUNT` / `MAX_ADDR_SIZE` generated constants are not used; there is no stack bounds check.
- Debug leftovers: `dbg!` calls in the parser and in `Println`; the CLI always overwrites `./out.efec`.
- No wasm/other-OS support (see Targets); tests are mostly print-style (`lexer` token dump, stdout flush experiments in `main.rs`).
