# wasm_mods: script_demo mods written in Rust and C

Two twin mods that use the script_demo API for real, compiled to wasm and run on efevm through the
translator `efe-wasm` (roadmap E7.4b, design `crates/efescript/docs/e7.4-wasm-design.md`). They are
not under `mods/` on purpose: `cargo xtask mods`, `check-scripts` and the demo's `load_mods`
only know `.efa` mod folders. `tests/wasm_mods_e2e.rs` builds, translates, packs, loads and runs both.

| | `rust_mod` (id `wasm_rust`) | `c_mod` (id `wasm_c`) |
|---|---|---|
| source | `src/lib.rs`, `no_std` cdylib on `efe-guest` | `mod.c` on `efe_guest.h` |
| bindings (generated, committed) | `src/api.rs` | `efe_api.h` |
| heap | `Vec`, `String` | static arrays |
| provides | in the wasm (`provide!`, section `efe.provide`) | in `mod.toml` (`[[provide]]`, clang cannot emit the section for wasm) |

What they do: `frame` 1 calls `game.score_add(sum of squares 1..=5)`, `inventory.give("sword", &Item)`
and reports the results with `game.report`; `frame` 2 reads the `inventory.given` event. They provide
`sum_squares(n: i32) -> i64`, `checksum(name: str) -> i64` and `boom(code: i32) -> i32` (traps on 13: a Rust
panic, or `EFE_TRAP()` in C). Both depend on the `.efa` mod `mods/inventory` (that is where the event
and the `str` + `&Item` function come from), so the bindings are generated from the game's index merged
with inventory's.

## Build by hand

From the repo root. Rust (needs the `wasm32-unknown-unknown` target; `RUSTFLAGS` overrides the repo's
web-build flags in `.cargo/config.toml`, which the translator rejects):

```sh
RUSTFLAGS=-Cdebuginfo=0 cargo build --release --target wasm32-unknown-unknown \
    --manifest-path examples/script_demo/wasm_mods/rust_mod/Cargo.toml
# -> rust_mod/target/wasm32-unknown-unknown/release/wasm_rust_mod.wasm
```

C (clang with the wasm32 target and a `wasm-ld`; with rustup's lld add `-fuse-ld=<sysroot>/lib/rustlib/<host>/bin/gcc-ld/wasm-ld`):

```sh
cd examples/script_demo/wasm_mods/c_mod
clang --target=wasm32 -nostdlib -O2 -mbulk-memory -I../../../../crates/efe-guest/include -I. \
    -Wl,--no-entry -Wl,--export=efe_scratch mod.c -o mod.wasm
```

## Regenerate the bindings

When the game's API or `mods/inventory` changes (the test `bindings_are_up_to_date` fails):

```sh
cargo xtask api-export script_demo                      # the game's .efapi
efescript pack examples/script_demo/mods/inventory --api script_demo.efapi -o inventory.efmod
efescript api-export --rust script_demo.efapi inventory.efmod -o examples/script_demo/wasm_mods/rust_mod/src/api.rs
efescript api-export --c    script_demo.efapi inventory.efmod -o examples/script_demo/wasm_mods/c_mod/efe_api.h
```

or `EFE_BLESS=1 cargo test -p script_demo --test wasm_mods_e2e bindings` (rewrites both files from the same index).

## mod.toml

`[mod]`, `[dependencies]` and `[permissions]` are as in the `.efa` mods. `[build] wasm` names the wasm
file (design doc section 8). `[[provide]]` lists the provided functions with the index spelling of
types, and `[build] scratch` is the size of the scratch region; for C they are the translator's
`Fallback` (design 4.4), for Rust they only document what the section says (the test checks both agree).
`efescript pack` does not read any of this yet: `read_mod_toml` rejects `[[provide]]` and `[build] wasm`,
and packing, the dev-time loading of a `.wasm` folder and the cache are E7.4c. Until then the test
does it by hand: translate with `efe_wasm::translate`, wrap with `efemod::Package::write`, load with
`Engine::load_input(LoadInput::Package { .. })`.

## Test

```sh
python3 scripts/remote.py test -p script_demo --test wasm_mods_e2e                  # interpreter
python3 scripts/remote.py test -p script_demo --features jit --test wasm_mods_e2e   # and the JIT
```

A missing wasm32 toolchain or clang prints `SKIPPED` and the test passes.
