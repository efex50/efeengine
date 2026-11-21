# efecs

`efeengine`'s fork of `hecs` 0.11.2 (upstream commit `c9d641251b6574c3ba157a9b4dca9046731b04a4`), packaged as `efecs` while keeping `[lib] name = "hecs"` so upstream code, docs, and tests work unchanged. Beyond upstream `hecs`, it adds `ComponentId`-keyed archetypes with runtime-registered plain-bytes ("dynamic") components for scripting/mods and opt-in per-component change detection ticks (`Tick`, `ComponentTicks`, `Changed<T>`, `Added<T>`, `Mut<T>`).

## At a glance
| | |
|---|---|
| Path | `crates/efecs` (package `efecs`, library name `hecs`) |
| Kind | library (`rlib`, `#![no_std]` with `alloc`, optional `std`) |
| Depends on (workspace) | `hecs-macros` (`crates/efecs/macros`, optional via `macros` feature) |
| Used by | `efeengine-core` (`crates/core`, imported as `hecs` and re-exported as `efeengine_core::hecs` and `efeengine_core::ecs`); `hecs-macros` dev-dependency; `tests/no-std-test-crates/macros` |
| Key external deps | `hashbrown` 0.17 (`default-hasher`, `inline-more`), `spin` 0.12 (`mutex`, `spin_mutex`, `lazylock`), `foldhash` 0.2, optional `serde` 1.0 (`column-serialize` / `row-serialize`). Dev-deps: `criterion`, `rand`, `trybuild`, `serde` (`derive`), `serde_test`, `bincode` |
| Features | `default = ["std"]`, `std`, `macros` (`hecs-macros`), `column-serialize` (`serde`), `row-serialize` (`serde`) |
| Targets | native and `wasm32-unknown-unknown` (`no_std` + `alloc` compatible; `std` enables `QueryCache` in `query.rs`) |
| Tests | unit tests in `src/` (47: `batch.rs` 7, `borrow.rs` 3, `change_tracker.rs` 1, `command_buffer.rs` 6, `entities.rs` 10, `entity_builder.rs` 2, `query.rs` 1, `serialize/column.rs` 2, `serialize/row.rs` 2, `tick.rs` 3, `world.rs` 10); integration tests in `tests/` (`tests.rs` 68, `dynamic.rs` 21, `ticks.rs` 28, `derive.rs` 1 `trybuild` runner over 8 pass + 3 compile-fail cases in `tests/derive/`) |

## Layout
```text
crates/efecs
├── Cargo.toml                  package `efecs`, `[lib] name = "hecs"`, features (`std`, `macros`, `column-serialize`, `row-serialize`)
├── FORK.md                     upstream base commit and summary of fork changes (dynamic components, change ticks)
├── README.md / CHANGELOG.md    upstream hecs docs and changelog
├── structure.md                this file
├── benches/bench.rs            Criterion benchmarks (requires `macros` feature)
├── examples/                   `cloning.rs`, `ffa_simulation.rs`, `format.rs`, `serialize_to_disk.rs` (`column-serialize`), `transform_hierarchy.rs`
├── macros/                     `hecs-macros` proc-macro crate (`Bundle`, `DynamicBundleClone`, `Query` derives)
├── src/
│   ├── lib.rs                  `#![no_std]`, tuple helper macros, module list, public re-exports and `#[doc(hidden)]` macro support exports
│   ├── archetype.rs            `Archetype`, `ComponentId` (`Rust` / `Dynamic`), `DynamicId`, `TypeInfo`, `TypeIdMap`, `ArchetypeColumn`, `ArchetypeColumnMut`, `DynColumn`, tick column storage
│   ├── batch.rs                `ColumnBatchType`, `ColumnBatchBuilder`, `ColumnBatch`, `BatchWriter`, `BatchIncomplete`
│   ├── borrow.rs               `AtomicBorrow` (lock-free shared/unique borrow state per archetype column)
│   ├── bundle.rs               `Bundle`, `DynamicBundle`, `DynamicBundleClone`, `DynamicClone`, `MissingComponent`, query-satisfaction helpers
│   ├── change_tracker.rs       `ChangeTracker<T>`, `Changes<'a, T>` (value-diffing change tracker using `Previous<T>` components)
│   ├── command_buffer.rs       `CommandBuffer` (deferred spawn, insert, remove, despawn, and dynamic component writes)
│   ├── dynamic.rs              runtime-registered byte components: `DynamicInfo`, `DynamicError`, `DynRef`, `DynRefMut`, `World` dynamic CRUD and `query_dynamic`
│   ├── entities.rs             `Entities` allocator, `Entity`, `EntityMeta`, `Location`, `ReserveEntitiesIterator`, `NoSuchEntity`
│   ├── entity_builder.rs       `EntityBuilder`, `BuiltEntity`, `EntityBuilderClone`, `BuiltEntityClone`
│   ├── entity_ref.rs           `EntityRef`, `Ref`, `RefMut`, `ComponentRef`, `ComponentRefShared` (with optional stamp tick via `with_tick`)
│   ├── query.rs                `Query`, `QueryShared`, `Fetch`, `Access`, `Mut`, `Changed`, `Added`, `Or`, `With`, `Without`, `Satisfies`, `QueryBorrow`, `QueryIter`, `QueryMut`, `Batch`, `BatchedIter`, `PreparedQuery`, `View`, `PreparedView`, `ViewBorrow`
│   ├── query_one.rs            `QueryOne` (single-entity query borrow and `with_ticks`)
│   ├── serialize/
│   │   ├── mod.rs              gated by `column-serialize` or `row-serialize`
│   │   ├── column.rs           `column::{SerializeContext, DeserializeContext, serialize, serialize_satisfying, deserialize, ...}`
│   │   └── row.rs              `row::{SerializeContext, DeserializeContext, serialize, serialize_satisfying, deserialize, ...}`
│   ├── take.rs                 `TakenEntity` (moves components out of a `World` into another via `DynamicBundle`)
│   ├── tick.rs                 `Tick`, `ComponentTicks`, `QueryTicks`, `CHECK_TICK_THRESHOLD`, `MAX_CHANGE_AGE`
│   └── world.rs                `World`, `Component`, `ComponentError`, `QueryOneError`, `Iter`, `ArchetypesGeneration`, `SpawnBatchIter`, `SpawnColumnBatchIter`
└── tests/
    ├── tests.rs                core ECS integration tests
    ├── dynamic.rs              dynamic component registration, CRUD, borrowing, querying, and unregister tests
    ├── ticks.rs                change tick tracking, `Changed`/`Added` filters, `Mut`, wrapping/clamping, and `with_ticks` tests
    ├── derive.rs               `trybuild` test harness (gated on `macros`)
    ├── derive/                 compile-pass and compile-fail derive fixtures (`enum_query`, `generics`, `nested_query`, `no_prelude`, etc.)
    └── no-std-test-crates/     excluded workspace crate checking `#![no_std]` + `macros` compilation
```

## Public API and fork extensions

### Core ECS (upstream `hecs` surface)
- **World & entities**: `World` (spawn, `spawn_batch`, `spawn_column_batch`, `despawn`, `insert`, `insert_one`, `remove`, `remove_one`, `take`, `clear`, `reserve_entity`, `reserve_entities`, `flush`), `Entity`, `EntityRef`, `Ref`, `RefMut`, `TakenEntity`.
- **Bundles & deferred commands**: `Bundle`, `DynamicBundle`, `DynamicBundleClone`, `EntityBuilder`, `EntityBuilderClone`, `CommandBuffer`, `ColumnBatchType` / `ColumnBatchBuilder` / `ColumnBatch`.
- **Queries & views**: `Query`, `QueryShared`, `QueryBorrow`, `QueryMut`, `QueryOne`, `PreparedQuery`, `View`, `PreparedView`, `ViewBorrow`, combinators `Or`, `With`, `Without`, `Satisfies`, and `Batch` / `BatchedIter`.
- **Serialization & legacy diff tracker**: `serialize::{column, row}` (feature-gated) and `ChangeTracker<T>`.
- **Derive macros** (when `macros` feature is enabled): `Bundle`, `DynamicBundleClone`, `Query` re-exported from `hecs_macros`.

### Fork additions (see `FORK.md`)
- **`ComponentId` & dynamic components (`archetype.rs`, `dynamic.rs`)**:
  - `ComponentId::{Rust(TypeId), Dynamic(DynamicId { id, generation })}` replaces raw `TypeId` keys in archetypes, `TypeInfo`, `EntityRef::component_types`, `EntityBuilder`, `CommandBuffer`, `ColumnBatch`, and `Fetch::for_each_borrow`.
  - `World::register_dynamic(id, DynamicInfo { size, align }, tracked)` registers plain-bytes Pod components (`align` power of two <= `MAX_DYNAMIC_ALIGN` 4096, `size` multiple of `align` <= `MAX_DYNAMIC_SIZE` 1 GiB, `0` allowed for tags). `unregister_dynamic` removes the component from all entities and bumps generation so stale archetypes are never reused.
  - Accessors: `insert_dynamic`, `remove_dynamic`, `has_dynamic`, `get_dynamic`, `get_dynamic_mut` (`DynRef` / `DynRefMut`), `query_dynamic(&[ids])`, and `Archetype::dynamic_column` (`DynColumn`). Typed queries ignore dynamic components; `despawn`, `clear`, and `take` handle them.
- **Opt-in change detection ticks (`tick.rs`, `query.rs`, `world.rs`)**:
  - `World::track_changes::<T>()` / `track_changes_id(ComponentId)` (or `tracked = true` on `register_dynamic`) allocates per-column `ComponentTicks { added, changed }` arrays alongside component data, retrofitting existing archetypes. Untracked components pay no tick storage or write cost.
  - `World::change_tick()` starts at `Tick(1)` and advances only via `increment_change_tick()`. Wrap-aware comparison and clamping (`check_change_ticks`, `maybe_check_change_ticks`, `CHECK_TICK_THRESHOLD`, `MAX_CHANGE_AGE`).
  - `&mut T` fetches stamp `changed` eagerly; `Mut<'a, T>` stamps lazily on `DerefMut`, `set_changed`, or `into_inner`, and exposes `is_added`, `is_changed`, `last_changed`, and `bypass_change_detection`.
  - Row filters `Changed<T>` and `Added<T>` (implementing `Fetch::HAS_FILTER` and `Fetch::filter`) compare column ticks against `QueryTicks { last_run, this_run }`, configured via `with_ticks(last_run, this_run)` or `World::query_since(last_run)`. Explicit stamp overrides: `World::get_at::<T>(entity, now)` and `EntityRef::with_tick(now)`. Inspection: `World::component_ticks`, `is_changed::<T>`, `is_added::<T>`.
