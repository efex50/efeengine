# coin_dash_scripts

`examples/coin_dash` with its gameplay in efescript. Same game: collect the coins, dodge the three patrolling
enemies, a bigger round when the coins are gone. The rules are `.efa` source files that the engine compiles at run
time with `Engine::load` (through the `ScriptCache`).

## Controls

- Move: WASD or the arrow keys.
- Coin: +1 score. Enemy touch: score reset, back to the start, one second of immunity.
- `COIN_DASH_AUTOPLAY=1`: the player script walks to the nearest coin (a demo without a keyboard).
- There is no text rendering yet: events go to the log, prefixed `[coin_dash_scripts]` (`score 3`,
  `hit by enemy, score reset`, `round 2`) plus a status line once a second.

## Run

```
cargo xtask run coin_dash_scripts native          # builds, copies scripts/ next to the binary, runs
COIN_DASH_AUTOPLAY=1 cargo xtask run coin_dash_scripts native
cargo test -p coin_dash_scripts                   # headless; run it on its own (every example's binary is `native`)
```

**Hot reload**: the root script is watched with `Engine::watch_source`. Run against the sources and edit them:

```
EFE_SCRIPT_DIR=$PWD/examples/coin_dash_scripts/scripts target/debug/native
```

Edit any `.efa` there (say `PLAYER_SPEED` in `common.efa`): within 0.25 s the log says `recompiled and
reloaded`, the world stays, `init` runs again (it adopts the coins already on the field), and a file that does not
compile is logged and the old version keeps running. Without `EFE_SCRIPT_DIR` the scripts come from `scripts/` next
to the executable, and when that is missing from the copy embedded with `include_str!` (the only source on the web).

## Layout

```
Cargo.toml
src/lib.rs                  window + SpriteRenderer, PNG textures, the Player/Coin/Patrol component types,
                            the game.* host functions, spawning the ground/camera/player/enemies, loading the scripts
scripts/coin_dash_scripts.efa   root: only `use`s the others; one module named after it
scripts/common.efa          constants, vector helpers, a log formatter (no number formatting in the engine yet)
scripts/player.efa          player_update: keyboard movement, autoplay bot
scripts/enemy.efa           patrol_update: walk a lane, turn at the ends, flip the sprite
scripts/coin.efa            coin_update: spin (switches UV frames of the sheet) and bob
scripts/rules.efa           init / frame: rounds (spawns coins), pickup, score, hit and reset, status line
assets/*.png                copies of the coin_dash art (embedded with include_bytes!)
tests/headless.rs           compile, run, autoplay scores, hit, hot reload, embedded sources
```

Which part is Rust and which is script:

| Rust | script |
|---|---|
| window, renderer, textures, ground tiles, camera follow | pickup, score rules, hit/reset, rounds, status line |
| `Player`/`Coin`/`Patrol` component types (scripts cannot declare components) | player movement, bot, patrol, coin spin/bob |
| `game.score_add/score/score_reset/autoplay/texture` | spawning coins (`engine.world.spawn/insert`, `engine.script.attach`) |
| `Script` on the player and enemies (as `script_demo` does) | |

`cargo xtask check-scripts coin_dash_scripts` assembles every file against the game's API index.
