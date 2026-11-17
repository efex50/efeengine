# efecs: efeengine's fork of hecs

- Upstream: https://github.com/Ralith/hecs, tag `v0.11.2`, commit `c9d641251b6574c3ba157a9b4dca9046731b04a4`.
- License: MIT OR Apache-2.0, Copyright (c) Benjamin Saunders and hecs contributors (`LICENSE-MIT`, `LICENSE-APACHE`, kept unchanged).
- Imported as a plain snapshot (`git read-tree`), without upstream history.

The package is named `efecs`, but the library name stays `hecs`, so upstream code, docs and tests work unchanged. `efeengine-core` depends on it as `hecs = { package = "efecs", path = "../efecs" }` and re-exports it as `efeengine_core::hecs`. Plugin crates should use that re-export, not crates.io `hecs`.

## Changes from upstream

1. Packaging only: renamed the package to `efecs`, `publish = false`, removed the nested `[workspace]`, `[profile.release]` and `.github/`; the macros crate's dev-dependency points at the renamed package. No source changes.

2. `ComponentId` keys and dynamic (runtime-registered) components.
   - Every place that keyed components by `TypeId` (archetype type lists and index, the world's archetype set, bundles' `with_ids`/`with_static_ids`, `TypeInfo`, `EntityBuilder`, `CommandBuffer`, column batches, `Fetch::for_each_borrow`, the derive macros) now uses `ComponentId::{Rust(TypeId), Dynamic(DynamicId { id, generation })}`. `Archetype::component_types`, `EntityRef::component_types` and `EntityBuilder::component_types` yield `ComponentId`; `Archetype::has_dynamic(TypeId)` became `has_component(ComponentId)`. The no-op `TypeIdMap` hasher is unchanged and still used for `TypeId`-keyed maps.
   - A component is registered at runtime with `World::register_dynamic(id: u32, DynamicInfo { size, align }, tracked)`. Dynamic components are plain bytes (`size` a multiple of `align`, `align` a power of two up to 4096, size up to 1 GiB, zero allowed): column storage honours the alignment, copies come from caller `&[u8]` with `ptr::copy_nonoverlapping`, and dropping is a no-op. They are accessed with `insert_dynamic`, `remove_dynamic`, `has_dynamic`, `get_dynamic` / `get_dynamic_mut` (dynamically borrow-checked `DynRef` / `DynRefMut` guards), `query_dynamic(&[ids])` and `Archetype::dynamic_column`; typed queries and `World::get::<&T>` never see them, but `EntityRef::component_types` lists them and `despawn`, `clear` and `take` handle them.
   - `unregister_dynamic` removes the component from every entity (archetype order, relative order kept) and forgets the id. Registering the same `u32` again yields a new `generation`, so archetypes built for the old layout are never reused (they stay behind as empty archetypes). Bundles or column batches carrying dynamic `TypeInfo`s (e.g. from `World::take` in another world) are checked against the registry before anything is mutated and panic on a mismatch, because the byte copies would otherwise be out of bounds.
3. Change ticks.
   - Design: opt-in per component, with no storage and no work for untracked components. `World::track_changes::<T>()` / `track_changes_id(ComponentId)` (or `tracked = true` at dynamic registration) gives every archetype column of that component an array of `ComponentTicks { added, changed }` next to its data, with the same capacity as the column; the arrays are allocated, grown, swap-removed, moved between archetypes and freed together with the component storage, and are guarded by the column's `AtomicBorrow`. Enabling tracking on a component that already exists retrofits tick arrays on existing archetypes with both ticks set to the current `change_tick`, so every existing value looks added and changed once. Tracking cannot be disabled.
   - `World::change_tick()` starts at `Tick(1)` and only moves with `increment_change_tick()`. Spawning, inserting and moving stamp new components with it (`added = changed = now`; replacing keeps `added`). `Tick` comparison is wrap-aware; `check_change_ticks` / `maybe_check_change_ticks` clamp stale ticks (the Bevy design, see `THIRD_PARTY_NOTICES.md`).
   - Writes: a `&mut T` fetch stamps `changed = this_run` eagerly for every row it yields (one `Option` branch; untracked columns skip it), which also covers `World::get::<&mut T>`, `query_one`, `query_mut` and views. The new `Mut<T>` query item stamps lazily, only on `DerefMut` / `set_changed` / `into_inner`, and offers `is_added` / `is_changed` / `bypass_change_detection`. Mutation through `Archetype::get::<&mut T>` columns and `World::get_unchecked` has no tick available and does not stamp.
   - Filters: `Changed<T>` and `Added<T>` are query types with item `()` that require `T`, take a shared borrow of its column and compare the row's tick against the query window. On an untracked column they match every entity that has `T`. They are not applied inside `Option`, `Satisfies` or the right-hand side of `With`/`Without`; inside `Or` a present side contributes its filter. They cannot be combined with `&mut T` / `Mut<T>` of the same component in one query (borrow conflict).
   - The `Fetch` trait gained `execute(archetype, state, ticks: QueryTicks)`, an associated `const HAS_FILTER` and an `unsafe fn filter(&self, row) -> bool` (default `true`; tuples AND, `Or` ORs). Iteration only calls `filter` when `HAS_FILTER` is true, so plain queries compile to the old loop. For queries with row filters `len()` / `size_hint()` count the matching rows (O(rows)).
   - Query windows default to `(Tick(0), world.change_tick())`; `with_ticks(last_run, this_run)` on `QueryBorrow`, `QueryMut`, `QueryOne`, `ViewBorrow` and `PreparedQueryBorrow`, and `World::query_since(last_run)`, set them. `World::get_at::<T>(entity, now)` / `EntityRef::with_tick(now)` are `get` with an explicit stamp tick (used by parallel systems). Inspection: `component_ticks`, `is_changed::<T>`, `is_added::<T>`.

## Pulling upstream fixes

```sh
git fetch --depth 1 https://github.com/Ralith/hecs tag vX.Y.Z
git diff c9d641251b6574c3ba157a9b4dca9046731b04a4 FETCH_HEAD   # review upstream changes, apply by hand
```
Update the commit above and this list after each merge.
