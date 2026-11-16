# efecs: efeengine's fork of hecs

- Upstream: https://github.com/Ralith/hecs, tag `v0.11.2`, commit `c9d641251b6574c3ba157a9b4dca9046731b04a4`.
- License: MIT OR Apache-2.0, Copyright (c) Benjamin Saunders and hecs contributors (`LICENSE-MIT`, `LICENSE-APACHE`, kept unchanged).
- Imported as a plain snapshot (`git read-tree`), without upstream history.

The package is named `efecs`, but the library name stays `hecs`, so upstream code, docs and tests work unchanged. `efeengine-core` depends on it as `hecs = { package = "efecs", path = "../efecs" }` and re-exports it as `efeengine_core::hecs`. Plugin crates should use that re-export, not crates.io `hecs`.

## Changes from upstream

1. Packaging only: renamed the package to `efecs`, `publish = false`, removed the nested `[workspace]`, `[profile.release]` and `.github/`; the macros crate's dev-dependency points at the renamed package. No source changes.

Planned (ROADMAP Phase 3): runtime component types (`TypeInfo` with a custom id, size, alignment, for mod-defined components) and per-column change ticks (`Mut<T>`, `Changed<T>`, `Added<T>`).

## Pulling upstream fixes

```sh
git fetch --depth 1 https://github.com/Ralith/hecs tag vX.Y.Z
git diff c9d641251b6574c3ba157a9b4dca9046731b04a4 FETCH_HEAD   # review upstream changes, apply by hand
```
Update the commit above and this list after each merge.
