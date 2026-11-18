# coin_dash_mods

A minimal base game plus content delivered as third-party mods, packed into `.efmod` packages. It tests the mod
system: packages, policy, asset handlers, dependencies, `[meta]`, `reload_mods`, `unload_mods`.

The base game (`src/lib.rs`) is an empty arena: a window, the `SpriteRenderer` and a camera. Everything on screen
comes from the packages.

## Controls

- Move: WASD or the arrow keys. `COIN_DASH_AUTOPLAY=1`: a bot walks to the nearest coin.
- **R**: `reload_mods()` (new mods load, changed ones are replaced, unchanged ones are *skipped*; the engine logs the
  report). **U**: `unload_mods()` (every mod, dependents first; their textures are freed and the sprites that still
  showed them are despawned).
- Logs: `[coin_dash_mods] ...` from the game (the load report, each mod's `[meta]`, textures registered from mod
  assets), `[dash_core] ...`, `[dash_coins] ...` from the mods' own `engine.log` calls.

## Run

```
cargo xtask build coin_dash_mods native      # builds, exports the API index, packs mods/* into target/mods/coin_dash_mods/*.efmod
COIN_DASH_AUTOPLAY=1 target/debug/native      # (or: cargo xtask run coin_dash_mods native)
cargo xtask mods coin_dash_mods              # just re-pack: a running game hot-reloads a package that changed
cargo test -p coin_dash_mods                  # headless; run it on its own (every example's binary is `native`)
```

**Packages, not folders.** `cargo xtask build ... native` also copies the plain mod folders to `target/debug/mods/`
(that is how `script_demo` gets hot reload of sources). This example exists to test packages, so the game points its
`DirSource` at the packed directory instead: `$COIN_DASH_MODS` if set, else `target/mods/coin_dash_mods/` found
relative to the executable. No xtask change was needed. Try `COIN_DASH_MODS=/some/dir` with one `.efmod` truncated:
the load report lists it as FAILED and the game runs with the rest.

On the web the same bytes would come from `fetch`: `BytesSource` (in `src/lib.rs`) turns `(label, bytes)` pairs into a mod
source. The engine has no downloader yet, so the wasm build loads nothing and says so.

## Layout

```
src/lib.rs                      base game: game.* API (texture/score/autoplay), ModPolicy, PngTextures asset handler,
                                orphan sweep, DirSource over the packages, R / U keys, BytesSource
mods/dash_core/                 player, score, rounds; provides pickup/hit/spawn_sprite/player_pos/set_target/round/
  mod.toml                      format_i64 and the event round_started (struct Round)
  src/{main,common,sprites,player,rules}.efa
  assets/player.png
mods/dash_tiles/                [deps: dash_core] ground tilemap read from assets/map.txt, drawn with tiles.png
  src/{main,common}.efa  assets/{map.txt,tiles.png}
mods/dash_enemies/              [deps: dash_core] patrolling enemies; [meta.difficulty] enemy_count / enemy_speed,
  src/{main,common,patrol}.efa  read back by the mod with engine.mods.meta_f64 and logged by the game
  assets/enemy.png
mods/dash_hud/                  [deps: dash_core] a score panel (top right) with a Reset button, built with the `ui` host
  mod.toml                      functions: the mod owns its widgets (`mod:dash_hud/...`), gets button clicks as `ui.signal`
  src/main.efa                  events and cannot see or change any other widget (docs/ui/GUIDE.md, section on mods)
mods/dash_coins/                [deps: dash_core] coins: reads round_started, spins/bobs, reports pickups to dash_core
  src/{main,common,coins}.efa
  assets/coin_sheet.png
tests/headless.rs               packs the folders with the efa/efemod APIs, loads the packages under the game policy,
                                runs frames, checks entities and score, reload (skipped), unload (cleanup),
                                a corrupt package, a broken PNG, the default grant
```

The policy gives each mod only what it needs (`dash_hud` gets `ui`, `engine.events`, `engine.log` and `game`, nothing that touches the world) (`dash_tiles` gets no `engine.world`; a mod the game does not know gets
`engine.log` and `engine.time`). A mod cannot define components, so coins and enemies keep their data in the mod's own
memory; mods share code only through `provide`s (that is why `dash_core.format_i64` exists).
