# strtohx

Small standalone command-line helper (not a library). It converts text arguments into legacy efescript assembly push instructions (`push64`/`push32`/`push16`/`push8 0x...`) representing UTF-8 string bytes for the stack. It was written as a developer tool for authoring string constants in the old `.efe` format, which was removed from `efescript` in roadmap E3.6 (replaced by `.efa` with `db` string literals).

## At a glance
| | |
|---|---|
| Path | `crates/efescript/strtohx` |
| Kind | binary (`src/main.rs`) |
| Depends on (workspace) | none |
| Used by | nothing in the workspace / build (legacy tool for the removed `.efe` format) |
| Key external deps | none |
| Targets | native CLI |
| Features | none |
| Tests | none |

## Layout
```text
crates/efescript/strtohx/
├── Cargo.toml    package strtohx 0.1.0, edition 2021, no dependencies
├── src/main.rs   entire CLI tool (125 lines)
└── structure.md  this file
```

## Main concepts
Usage: `strtohx "hello world" "second string"` (each argument is processed in turn; passing no arguments panics with `"not enough args"`).

For each argument, `handle_part` iterates over its UTF-8 bytes and greedily splits them into chunks of 8, 4, 2, and 1 bytes (`K8`, `K4`, `K2`, `K1`) via `ayıkla`. Each chunk is reversed into a zero-padded `[u8; 8]` by `as_le_bytes!` and converted via `usize::from_le_bytes`, producing a big-endian hex literal formatted in uppercase (`push64 0x...`, `push32 0x...`, `push16 0x...`, `push8 0x...`), followed by `total len is N`.

Example output:
```text
$ strtohx "hello world"
------
input str:hello world

push64 0x68656C6C6F20776F
push16 0x726C
push8 0x64

total len is 11
```

## Public API
None; binary-only crate (CLI arguments -> stdout).

## Status and known gaps
- Legacy tool for the removed `.efe` format (E3.6); `.efa` supports string literals directly via `db`.
- No arguments -> `panic!("not enough args")` rather than a usage message.
- No newline escaping or output-file flag; no tests.
- `as_le_bytes!` is misleadingly named (it pops/reverses bytes into an 8-byte array so `usize::from_le_bytes` yields the big-endian integer); invalid chunk size panics in `ayıkla`.
