# efescript

The scripting layer of efeengine: an x86-flavoured assembly dialect (`.efe`) and a small register VM
that runs it. See `structure.md` for the code layout, `docs/e0-design.md` for the design and
`ROADMAP.md` for what comes next.

## v2: `.efa` assembly on efevm

v2 is the current path. A text assembly language (`.efa`) is assembled into an `efevm`
module (`.efvm`: verified bytecode with a register interpreter, fibers and an instruction
budget, see `crates/efevm/SPEC.md`), and `efescript` runs it. Assembly errors point at the
file, line and column with a caret under the span. A trap prints a backtrace with the source
line of every frame. `dis` turns a module back into `.efa` text that assembles to the same bytes.

Code: `src/asm/` (assembler), `src/disasm.rs`, `src/run_v2.rs`, `src/host_std.rs`, and the VM in
`crates/efevm`. Design: `docs/e2-design.md`. Full syntax reference: `docs/efa-syntax.md`. Readable guide to every instruction: `docs/efa-instructions.md`.

### CLI

```text
efescript run   <file.efa|file.efvm> [--budget N]
efescript build <file.efa> -o <out.efvm>
efescript dis   <file.efvm|file.efa> [-o out.efa]
efescript <file.efa> [--budget N]      (shorthand for run)
```

- `run` assembles if needed and runs the exported `main`. `--budget N` limits the total number
  of instructions (unlimited by default).
- `build` writes the encoded module to `-o`.
- `dis` prints the module as `.efa` (to stdout, or to `-o`). Names come from the `names`
  section when present; otherwise `f0`, `r0`, `data0`, ... are used.

Exit codes for v2 programs:

- `main` returns nothing: 0. `main` returns one `i32`: that value. `std.exit(code)`: that code.
- 2: a load or assembly error (missing file, diagnostics, no exported `main` or a wrong `main`
  signature, an unresolved import).
- 3: a trap (the trap and its backtrace go to stderr), or the `--budget` ran out.

### Language reference (short)

The full reference is `docs/efa-syntax.md`. The mnemonics are the `efevm` ones, listed in
`crates/efevm/OPCODES.md`.

- **Lexical rules.** `;` starts a comment. A dotted name is one token (`i64.add`, `std.println`,
  `msg.len`). Integers: decimal, `0x`, `0b`, `0o`, `_` separators, optional `-`. Floats: `1.5`,
  `1e9`, `inf`, `nan`, `nan:0x7fc00001`. Characters: `'a'`, `'\n'`. Strings: `"..."` with the
  escapes `\n \t \r \0 \\ \" \xNN`. A statement ends at the end of its line; blocks use `{` `}`.
- **Top-level items.** `import std.println(i64, i64) -> i64`, `memory MIN [MAX]` (64 KiB pages),
  `const NAME = 10`, `global [mut] NAME: i64 = 0`, `table [f1, null, f2]`, data
  (`NAME db "text", 10`, `dw`, `dd`, `dq`, `NAME resb 64`), and `[export] fn NAME(a: i64) -> i64 { ... }`.
  Types are `i32 i64 f32 f64`.
- **Functions, registers, labels.** Parameters are registers `r0`, `r1`, ... named by the
  parameter list. `reg a, b` declares more named registers. Raw `rN` registers can always be used.
  Labels are `name:` and are local to the function. Every function must end in `ret`, `jmp`,
  `br_table` or `unreachable`; the assembler does not add one.
- **Instructions.** The `efevm` mnemonic with the destination first: `i64.add d, a, b`,
  `i64.load d, base, off`, `i64.store src, base, off`.
- **Conveniences** (each expands to plain instructions):
  - immediates in source positions: `i64.sub n, n, 1`; `const` and data names work too;
  - call sugar: `call f(a, b) -> x` or `-> (x, y)`, where `f` is a function or an import;
    `call_indirect (i64) -> i64, idx(a) -> x` for indirect calls;
  - returns: `ret`, `ret a`, `ret (a, b)`;
  - loads and stores with an optional offset; the base may be a data name;
  - `br_table idx, [a, b, c], default`;
  - globals by name: `global.get d, NAME`, `global.set NAME, a`.
- **Data.** Every data item defines `NAME` (its address) and `NAME.len` (its size in bytes).
  Items are laid out from address 1024 in definition order, each 8-byte aligned.
- **Host functions** (namespace `std`, from `src/host_std.rs`):

  | Import | Meaning |
  |---|---|
  | `std.print(i64, i64)` | write `len` bytes at `ptr` |
  | `std.println(i64, i64)` | the same, then a newline |
  | `std.print_i64(i64)` · `std.print_f64(f64)` · `std.print_char(i32)` | write a number / one character |
  | `std.read_line(i64, i64) -> i64` | read a line into `ptr` (at most `cap` bytes); returns the length |
  | `std.play_sound(i64, i64) -> i32` | play the file at the path; 0 = ok |
  | `std.exit(i32)` | stop the program with this exit code |

Hello world:

```text
; hello.efa
import std.println(i64, i64)          ; pointer, length

msg db "hello world!"

export fn main() {
    call std.println(msg, msg.len)
    ret
}
```

Sum of n, n-1, ..., 1 (a loop; `examples/recursion.efa` has the recursive version):

```text
export fn sum(n: i64) -> i64 {
    reg acc, t
    const.i64 acc, 0
loop:
    i64.eqz t, n
    br_if t, done
    i64.add acc, acc, n
    i64.sub n, n, 1
    jmp loop
done:
    ret acc
}
```

### v2 examples

Run from the crate directory (`crates/efescript`), so relative paths resolve:
`cargo run -q -p efescript -- run examples/hello.efa`. Each file has its expected output in its header comment.

- `examples/hello.efa`: one import, one string, one call (`hello world!`).
- `examples/strings.efa`: `db` strings and escapes, `NAME.len`, byte loads, a slice, `dw`/`dd`/`dq` tables.
- `examples/jumps.efa`: every comparison and branch form on i64 and i32, `eqz`, `br_if`, `br_ifz`, `jmp`, `br_table`, `select`.
- `examples/recursion.efa`: recursive sum, factorial and fibonacci with call sugar.
- `examples/floats.efa`: f64 and f32 arithmetic, comparisons, conversions, reinterpret, a float global.
- `examples/sound.efa`: `std.play_sound` on `examples/ominous_worse.ogg`; `main` returns the status (0 or 1).

### v2 tests

```text
cargo test -p efescript
```

## Legacy path (`.efe`, nasm_efe and the old runtime)

Everything from here to the end of this file is the legacy path, kept unchanged until E3.6
removes it. Its exit codes are listed under "Run" below.

## Run

From the crate directory (`crates/efescript`), so relative paths such as the one in `sound.efe` resolve:

```text
cargo run -q -p efescript -- examples/hello.efe      # assemble and run
cargo run -q -p efescript -- examples/hello.efe -o /tmp/hello.efec   # assemble only
cargo run -q -p efescript -- /tmp/hello.efec         # run precompiled bytes
cargo run -q -p efescript --features audio -- examples/sound.efe     # real sound (needs ALSA headers)
```

Flags: `-o <file>` write bytecode instead of running, `-d` dump the runtime afterwards, `-r` print memory usage.
Exit code: the program's `Finish` code (0 when it simply ends), 2 for a missing file or assembly error, 3 for a runtime fault.

Examples (each has its expected output in the header comment): `hello`, `strings`, `jumps`, `recursion`,
`call`, `sound`, `test` (reads stdin), `heap` (expected fault), `create_e` (writes `./haha`; run it from a scratch directory).

## Test

```text
cargo test -p efescript
```

Lib tests plus the integration tests in `tests/` (examples, sound, control flow, strings, errors, CLI).
They capture output with `CaptureHost`; no audio device is needed.

## Language reference

### Syntax
- One statement per line; `; comment` anywhere. Mnemonics and registers are case-insensitive.
- Labels: `label NAME` or `NAME:`. Jumps and calls take a label or a register; a label is also an immediate (`mov r1, _loop`).
- Numbers: decimal, `0x1f`, `0b101`, `0o17`, negative immediates (`-1`).
- Two operands need a real comma: `mov ra, 5`.
- Pointers: `[rbp+8]`, `[r1-4]`, `[r1+r2]`, `[msg]`, `[msg+r1]`, with a width prefix `BYTEPTR`, `WORDPTR`, `DWORDPTR` or `QWORDPTR`: `mov bl, BYTEPTR [msg+r1]`.

### Registers
`ra rb rc rd` (general), `rbp`, `rsp` (stack pointer), `r1`..`r6`; 8-bit parts `al ah bl bh cl ch dl dh`
(low byte and byte 1 of `ra`..`rd`). All 64-bit (`usize`), memory is big-endian.

### Instructions
`mov`, `lea`, `add8/16/32/64`, `sub8/16/32/64`, `or`, `xor`, `and`, `nand`, `nor`, `cmp`, `test`,
`push8..64`, `pop8..64`, `jmp`, `je`, `jne`, `jgt`, `jlt`, `jle`, `jge`, `jz`, `jnz`, `js`, `call`, `ret`, `syscall`, `nop`.
`cmp a, b` compares unsigned and sets equals/greater/lesser/ge/le; `test a` sets zero (a == 0) and sign (top bit set).
There is no multiply or divide. `call` pushes an 8-byte return address on the stack and `ret` pops it.
The stack starts after the program (code and data) and grows upward.

### Syscalls
Number in `ra`, arguments in `rb rc rd r1`.

| ra | Name | Arguments | Result |
|---|---|---|---|
| 0 | Printchar | `rb` = character | prints it (UTF-8) |
| 1 | Print | `rb` = pointer, `rc` = length | prints the bytes and a trailing space |
| 2 | Println | `rb` = pointer, `rc` = length | prints the bytes and a newline |
| 3 | NumToString | `rb` = number, `rc` = 1 for signed, `rd` = 1 hex / 2 binary / 3 octal, else decimal | text written at `rsp`; `rb` = pointer, `rc` = length; `rsp` advances |
| 4 | StringToNum | `rb` = pointer, `rc` = length | number in `ra` |
| 5 | Readln | - | stdin line at `rsp`; `rb` = pointer, `rc` = length |
| 6 | ReadCon | `rd` = delimiter byte | like Readln, up to the delimiter |
| 7 | ReadFs | `rb`/`rc` = path | file contents written at `rsp` |
| 8 | WriteFs | `rb`/`rc` = data, `rd`/`r1` = path | writes the file |
| 12 | PlaySound | `rb` = path pointer, `rc` = path length | `ra` = 0 ok, 1 failed (needs the `audio` feature) |
| 5050 | Finish | `rb` = exit code | ends the program |

HeapAlloc (9), Write (10), Flush (11) are not implemented and end the run with a fault. An unknown number is a fault too.

### Strings
`NAME db item, item, ...` defines bytes; an item is a string literal (UTF-8, escapes `\n \t \r \0 \\ \" \xNN`) or a number 0-255.
It creates two symbols: `NAME` (address of the first byte) and `NAME.len` (length in bytes, so `"çay"` is 4).
Symbols work as immediates and inside pointers. The data is placed after the code, behind a zero byte that ends the program,
so running off the end of the code never executes data. Where the `db` line sits in the file does not matter.

```
msg db "hello world!", 10      ; 13 bytes

mov ra, 2                      ; Println
mov rb, msg
mov rc, msg.len
syscall

mov r1, 0
mov bl, BYTEPTR [msg+r1]       ; first byte, 'h'
mov ra, 5050                   ; Finish
mov rb, 0
syscall
```

### Embedding
```rust
use efescript::prelude::*;
let cap = CaptureHost::shared();
let mut rt = ProgramRuntime::with_host(Box::new(cap.clone()));
rt.load_from_vec(parse_str(src).unwrap());
let result = rt.run_with_limit(1_000_000);   // Finished(u8) | EndOfProgram | LimitReached | Fault(String)
println!("{}", cap.borrow().out_string());
```
Implement the `Host` trait (`write_out`, `play_sound`) to route output and sound elsewhere.
