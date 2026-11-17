# Threading

The engine runs a frame on several threads and still gives a **byte-identical world** for
`Threads::Single` and `Threads::Fixed(n)`. This page is the user guide; the contract the code
follows is `docs/PARALLEL_SPEC.md`.

```rust
let cfg = EngineConfig { threads: Threads::Fixed(4), ..EngineConfig::default() };
engine.set_threads(Threads::Single);   // also `Message::SetThreads(n)`
engine.threads();                      // participants now (the calling thread counts)
```

| `Threads` | Meaning |
|---|---|
| `Auto` (default) | window mode: the global pool size (`global_pool_size()`: the caller plus the pool's helpers; `EFE_THREADS` overrides the core count). Headless: `Single`. |
| `Fixed(n)` | `n` participants in total (the calling thread plus `n - 1` pool workers). `Fixed(1)` is `Single`. `Fixed(0)` is rejected by `validate`. The pool grows if needed and never shrinks. The bound holds for nested batches too (a par system calling `par_for_each`): the executor counts helper jobs in flight and never has more than `n - 1`. |
| `Single` | everything on the calling thread; the pool is not touched. |

## The three rules

1. **Structure goes through `Commands`.** A parallel system never spawns, despawns, inserts or
   removes directly. It calls `ctx.commands()`; the queued ops are applied after the stage.
2. **Rust data goes through declared `Access`.** A parallel system declares every component it
   reads or writes and every resource it reads or writes. Systems whose declarations conflict
   are never run at the same time.
3. **Scripts are buffered and applied in a fixed order.** Script writes go to a per-lane buffer
   and are merged on the main thread in lane order, never in timing order.

## Par systems

```rust
engine.add_par_system(
    Stage::Update,
    Access::new().named("patrol").read::<Transform>().write::<Velocity>().write::<Patrol>(),
    |ctx: &ParCtx| {
        ctx.par_for_each::<(&Transform, &mut Velocity, &mut Patrol), _>(64, |(t, v, p)| { /* ... */ });
    },
);
```

`ParCtx` has `query`, `query_one`, `get`, `par_for_each(batch, f)`, `res`/`res_mut` (and `try_`
variants), `events::<T>()`, `send(ev)` (buffered, replayed in order), `commands()`, plus `time`,
`input`, `gamepads`, `window`, `types`. `world_unchecked()` is an escape hatch: nothing is
checked against the `Access`, hecs still checks borrows at run time (a conflicting borrow
panics, it is never unsound).

* Component access is checked in debug builds against the declaration, and panics with the
  system's name (`Access::named`). Resource access is checked in release builds too.
* `ParCommands::spawn` returns nothing: the entity is created when the buffers are merged. Use an
  exclusive system (`Ctx::spawn`) if you need the handle at once.
* Resources, events and components must be `Send + Sync + 'static`; a par system closure must be `Send`.
* Anything that needs the window, the renderer or scripting stays an exclusive system
  (`add_system`, `&mut Ctx`). coin_dash is an example: `player_input`, `patrol`,
  `animate_coins` and `collisions` are par systems, the camera and the status line are exclusive.

Built in: `velocity_system` is a par system (`read Velocity, write Transform`, batches of 1024);
`propagate_transforms` is a par system (structural changes through commands); `render_extract`
and the script system are exclusive.

Systems can be disabled, gated by a run condition, added late and removed; see [SYSTEMS.md](SYSTEMS.md).
A disabled system is left out of the batches (it does not split one) and keeps its change tick.

## How batches are built

Per stage, once (rebuilt lazily when a system is added or its order changes), as a pure
function of the registered systems:

* sort the systems by `(order, registration index)`. Every system has an order number
  (`i32`, default `order::USER` = 0; set with `engine.add_system(..).order(n)`). Equal orders
  keep registration order; the registration index counts over all stages. See
  `efeengine_core::prelude::order` for the named constants (`INPUT`, `EARLY`, `SCRIPTS`, `USER`,
  `PHYSICS`, `TRANSFORM`, `LATE`);
* walk the sorted list;
* an exclusive system closes the current batch and forms a batch of its own;
* a par system joins the current par batch if its `Access` does not conflict with any member;
  otherwise it closes the batch and starts a new one. There is no look-ahead and no
  reordering after the sort, so conflicting systems always keep their sorted order. Par systems
with different order numbers may still share a batch if their accesses do not conflict.

Two accesses conflict when one writes what the other reads or writes (component or resource).
Read/read never conflicts. The members of a batch run through the executor, one task each; the
calling thread takes tasks too and never blocks on a channel. When the batch ends, every member's
commands are appended to the stage's command list and its event sends are replayed, in
sorted (execution) order. The merged order is therefore the order a sequential run produces.
If tasks panic, all tasks still run and the panic of the lowest task index is re-raised, with one thread or many.

## Scripts: lanes

A module can call another module's `provide`, which runs the other module's instance inside the
caller's call, so two modules linked by imports must not run on different threads. Modules are
grouped into **lanes**: connected components of the graph "A imports a namespace owned by B".
Inside a lane modules are sorted by id; lanes are sorted by their smallest id. Most modules are
alone in a lane. A lane runs on one worker, in order: budget reset and pending `init`s, then
`frame(dt)` of each module, then the `entry` of each `Script` entity of each module.

During the script phase every lane works on a read-only snapshot of the world:

* `engine.world.get`/`has`/iteration read the snapshot, plus the lane's own writes
  (read-your-own-writes). Other lanes see the start-of-phase values.
* `engine.world.set` is buffered. Insert, remove, despawn and `script.attach` are queued as
  commands and become visible after the stage, as before. Events sent are readable next frame.
* Merge on the main thread, in lane order: writes in call order, then `Script.state` updates,
  then commands, then events (the 4096-per-frame cap is applied here), then logs.
* A write to an `(entity, component)` pair that an earlier lane already wrote this frame is a
  **write conflict**: the later lane (higher id) wins and `write_conflicts` is counted, with one
  warning per frame. Several writes by one lane are not conflicts.
* `engine.random` is per module (seeded from the engine seed and the module name).

With one thread the lanes run in sequence through the same code, with the same buffering.

## Change ticks

The world has a change tick (`world.change_tick()`, `engine.change_tick()`). Components that are
tracked (`engine.track_changes::<T>()`; `Transform` and `GlobalTransform` are) remember the tick
they were added and last changed at, and `Changed<T>` / `Added<T>` filters compare those with the
tick at which the querying system last ran. Every system has a `last_run` (`Tick::new(0)` before
its first run, so the first run sees everything as new) and a `this_run`.

* **Exclusive systems**: the schedule calls `world.increment_change_tick()` right before the
  system runs; that is its `this_run` (`ctx.this_run()`, `ctx.last_run()`). Use `ctx.query::<Q>()`
  (not `ctx.world.query`) so filters and `&mut T` stamps use the system's ticks.
* **Par batches**: before any task starts, the members get their ticks in **sorted system order**
  (one `increment_change_tick` per member, in `members` order). The assignment never depends on
  completion order, so component ticks are identical for `Single` and `Fixed(n)`
  (`tests/parallel.rs` compares them entity by entity). `ParCtx::query`, `query_one` and
  `par_for_each` and `get` (through `World::get_at`) carry the system's `(last_run, this_run)`.
* **Commands**: `Engine::apply_commands` bumps the tick once before applying, so everything applied
  after a stage (Rust commands, buffered script writes and inserts) is newer than every system
  of that stage. Every system, the one that queued it included, sees it as added/changed on its
  next run. A system's own direct writes are not new for itself.
* **Scripts**: the script system is exclusive, so the world is frozen at its `this_run`.
  `engine.world.tick()` returns it; `engine.world.changed(entity, component, since)` is 1 if the
  component was added or changed after `since`, 0 if not, 1 for an untracked component, and
  -1 / -2 / -3 for a stale entity / missing component / unknown id. A script stores `tick()` and
  passes it next frame. Buffered `set`s are applied after the stage, so a module's own writes
  show as changed on the next frame.
* The world clamps old ticks once per frame (`maybe_check_change_ticks`), so wrapping is safe.
* **Resources** carry a `changed` tick too (`Resources::changed_tick::<T>()`, set on `insert`).
  `ResMut` stamps its system's `this_run` on **`DerefMut`**, not on the borrow, so a read-only
  `res_mut` is not a change; `Resources::get_mut` / `get_or_insert_with` (a bare `&mut T`) count
  immediately, stamped with the tick the engine keeps in sync (`Resources::set_tick`: the
  exclusive system's `this_run`; `Engine::insert_resource` / `resources_mut` bump the world tick).
  `Ctx::res_changed::<T>()` / `ParCtx::res_changed::<T>()` mean "changed since this system last
  ran" (own writes included, like components); `Engine::res_changed::<T>(since)` takes an explicit
  tick. `Resources::check_ticks` clamps old resource ticks once per frame next to the world's.

## Spawn pools

Entity ids are handed out in a fixed order, so `engine.world.spawn` in a script takes an entity
from a pool reserved for its module before the script phase, in module-id order. The pool starts
at 64; the next size is `clamp(2 * (used + overflows), 64, 65536)`. Unused pooled entities are
despawned after everything else is merged. If the pool is empty, `spawn` returns the null handle
(bits 0, stale for every API), counts a `spawn_overflow` and logs one warning per frame and
module; the pool is larger next frame.

## ScriptStats

`Engine::script_stats()` returns the numbers of the last script phase:
`write_conflicts`, `spawn_overflows`, `lanes`, `dropped_events`.

## Determinism: the guarantee and what voids it

A frame's result does not depend on the thread count or on timing, provided that:

* par systems touch only what they declare, and write only their own query items in place
  (`par_for_each` closures must not touch other shared data);
* host functions (`register_host_fn`) are `Send + Sync` and **order-independent**. They now run
  on worker threads, possibly at the same time. State they share must change in a way that does
  not depend on the call order (an atomic add, a max, a set), or go through the host's buffers.
  A host function that pushes to a shared `Vec` or draws from a shared RNG voids the guarantee
  (see `tests/script_lanes.rs`, which sorts such results);
* `world_unchecked` is not used to read data another system of the same batch writes. Misuse
  panics through the hecs borrow check but stays sound.

`crates/core/tests/parallel.rs` hashes a 2k-entity world after 300 frames with `Single` and 20
times with `Fixed(4)` (par systems, exclusive systems, script modules that conflict and spawn).

## Web

* The main thread takes work itself (the executor's calling thread runs tasks), and never uses
  `Atomics.wait`, so it works on the browser main thread.
* Without `crossOriginIsolated` (no `SharedArrayBuffer`) the pool has size 1, so `Auto` is
  `Single`.
* Headless `Auto` is `Single` on every target.

Benchmark: `cargo test -p efeengine-core --release --test parallel_bench -- --ignored --nocapture`.
