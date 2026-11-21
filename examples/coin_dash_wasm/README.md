# coin_dash_wasm

`coin_dash` whose entire gameplay runs in **two WebAssembly mods** — one in **Rust** (`mods/dash_rust`) and one in **C** (`mods/dash_c`) — compiled to `.wasm`, translated in-process with `efe-wasm` into verified `efevm` modules, wrapped as `.efmod` packages in memory, and executed under a `ModPolicy`.

The Rust host keeps only what mods cannot do: windowing, `SpriteRenderer`, textures/assets, camera smoothing, component layouts (`Player`, `Coin`, `Patrol`, `CoinSpawned`), the `game.*` host API & events, the UI (`menu.rs` + `DebugMenuPlugin` + `WorldUiPlugin`), and loading the mods.

> **Web build:** not supported (`efe-wasm` translation and local `.wasm` mod loading are native-only; a wasm32 build shows a warning banner in the UI).

## Controls

- **Move**: `WASD` or arrow keys (`COIN_DASH_AUTOPLAY=1` skips the main menu and steers toward the nearest coin).
- **Pause / Back**: `Esc` (toggles pause menu, or backs out of Settings).
- **Debug menu**: `F5` (FPS + log capture via `DebugMenuPlugin`).
- **HUD**: drag with the left mouse button.

## Build the wasm mods

Requires a Rust toolchain with `wasm32-unknown-unknown` (`rustup target add wasm32-unknown-unknown`), `clang` with the `wasm32` target, and `wasm-ld` (either on `PATH` or from rustup's `gcc-ld`).

```sh
python3 examples/coin_dash_wasm/build_mods.py
```

Outputs `target/coin_dash_wasm/wasm/dash_c.wasm` and `target/coin_dash_wasm/wasm/dash_rust.wasm` (skips cleanly with a message if a toolchain is missing).

## Run

```sh
python3 examples/coin_dash_wasm/build_mods.py
cargo run -p coin_dash_wasm
# or with autoplay:
COIN_DASH_AUTOPLAY=1 cargo run -p coin_dash_wasm
```

`build_mods.py` also copies each `.wasm` into its mod folder (`mods/<name>/<name>.wasm`, git-ignored): that is where `[build] wasm` in `mod.toml` points, and `build.py` / `cargo xtask` pack the folders from there. This PC may have no clang, so `python3 examples/coin_dash_wasm/build_mods.py --remote` builds on the remote machine and fetches the two files. Run it before `./build.py --remote run coin_dash_wasm native`.

Override the wasm lookup directory with `COIN_DASH_WASM=/path/to/wasm_dir`. If the `.wasm` files have not been built, the game still opens and shows `"mods not built: run build_mods.py"` in the UI.

## Tests

Run on the remote build server (has `clang` and `wasm32-unknown-unknown`):

```sh
python3 scripts/remote.py test -p coin_dash_wasm
```

Regenerate committed guest bindings (`mods/dash_c/efe_api.h` and `mods/dash_rust/src/api.rs`) if `game.*` or `dash_c` provides change:

```sh
EFE_BLESS=1 cargo test -p coin_dash_wasm --test headless bindings_are_up_to_date
```

## Layout

| Path | Role |
|---|---|
| `src/lib.rs` | Host API (`game.*`, `game.coin_spawned`, `game.patrol_turn`), `Player`/`Coin`/`Patrol` components, textures, camera, world-UI queue, `ModPolicy`, in-process `efe-wasm` translation + `.efmod` wrapping |
| `src/menu.rs` | Main menu, Settings, Pause, Game Over, draggable HUD (`Score`, `Round`, `Coins left`, `Lives`), missing-wasm banner |
| `src/main.rs` | Native binary entry point calling `coin_dash_wasm::run()` |
| `mods/dash_c/mod.c` | C mod (`clang --target=wasm32`): enemy patrol + `game.patrol_turn` events, coin spin/bob animation, reads `game.coin_spawned`, provides `next_lane`, `coin_phase`, `check_coin_spot`, `provide_calls` |
| `mods/dash_c/mod.toml` | Manifest + `[[provide]]` / `[build] scratch` fallback metadata for `efe-wasm` |
| `mods/dash_c/efe_api.h` | Generated C bindings (committed) |
| `mods/dash_rust/src/lib.rs` | Rust `#![no_std]` mod (`efe-guest`): ground tiles, player + shadow, movement + autoplay, rounds, coin/enemy spawning (calling `dash_c`), pickups, score, hits, lives/game-over, restart |
| `mods/dash_rust/src/api.rs` | Generated Rust bindings against `game` + `dash_c` (committed) |
| `build_mods.py` | Stdlib-only script building both mods to `target/coin_dash_wasm/wasm/` |
| `tests/headless.rs` | Binding freshness, missing-wasm UI, menu/pause/game-over flow, headless gameplay + autoplay + C provide verification |
