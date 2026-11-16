# Hybrid parallel scheduling: implementation spec

Status: implementation spec for the plan "hybrid parallel scheduling" (queued structure,
declared access, buffered script writes). User-facing docs go in `docs/THREADING.md`; this file
is the contract the code follows. Breaking changes are allowed; there are no compatibility shims.

**Goal:** a frame run with `Threads::Single` and with `Threads::Fixed(n)` must give a
byte-identical world, and the result must never depend on thread timing.

## 0. Decisions that differ from the plan (and why)

| # | Plan said | Spec says | Why |
|---|-----------|-----------|-----|
| D1 | one module per worker, per-module write buffer | modules are grouped into **lanes**; a lane runs on one worker; the write buffer belongs to the lane | A mod can call another mod's provide (`provides.rs`), which runs the other module's `Instance` inside the caller's call. Two modules linked by imports must not run on different threads. A lane is a connected component of the "imports a namespace owned by" graph. Most modules are alone in their lane, which gives exactly the plan's semantics. |
| D2 | `engine.world.spawn` into the per-module buffer | each module gets a **pre-reserved block of entities** before the script phase | `World::reserve_entity` is atomic, so concurrent reservation hands out ids in timing order. That is not deterministic. Blocks are reserved on the main thread in module-id order, so the ids are the same for every thread count. |
| D3 | `ParCtx::commands().spawn` returns an `Entity` | a par system's `spawn` returns **nothing**; the entity is reserved when the buffers merge, in registration order | Same reason as D2. Exclusive systems (`Ctx::spawn`) keep returning the handle at once. |
| D4 | `engine.random` global state | **per-module** xorshift state | A shared RNG drawn from several threads in parallel gives a timing-dependent sequence. |
| D5 | (not in the plan) | in headless mode `Threads::Auto` means `Single` | Tests and tools stay deterministic and spawn no threads unless they ask for `Fixed(n)`. |
| D6 | (not in the plan) | the script-event cap (4096 per frame) is applied when the lane buffers merge, in lane order | The cap then drops the same events for every thread count. |
| D7 | `ParCtx::get<T: ComponentRef>` | `get<T: ComponentRef + Query>` (`&C` and `&mut C` both qualify) | The access check needs to know whether the borrow is shared or unique, and only `Query::Fetch::for_each_borrow` tells. |
| D8 | `Auto` = the pool size from the core count | `Auto` (not headless) = `global_pool_size()` participants after the pool is initialised (caller + pool-1 helpers). `Message::SetThreads(n)` = `set_threads(Fixed(n))` | Keeps one pool worker free for background jobs; wasm without crossOriginIsolated gets a pool of 1, so Single. |
| D9 | `propagate_transforms` keeps its algorithm | It still writes `GlobalTransform`/`Children` in place where they exist, but inserting a missing `GlobalTransform`/`Children` and removing a stale `Children` now go through `ParCommands` (applied after the `PostUpdate` stage, not inside the system). It also declares `res_mut::<RenderWarnings>()` and builds `Children` in node order (no `HashMap` iteration). | A par system cannot change structure; the order of the queued ops must be deterministic. |
| D10 | `ParCtx` built before the tasks | Each task builds its own `ParCtx` on the thread that runs it and hands back a `ParOutput` (commands + sends) | `ParCtx` holds `RefCell`s, so it does not need to be `Send`. |
| D11 | (not in the plan) | `Engine::textures()` and `Engine::render_frame()` return `Res<'_, T>` guards | `Resources::get` returns a guard now. All engine resources (`Textures`, `RenderFrame`, `RenderWarnings`) are `Send + Sync`, so no main-thread-only store was needed. |
| D12 | the main `ScriptHost` keeps the world and modules; each lane gets a `ScriptHost` | `Scripting` has no main `ScriptHost` any more: it owns `modules: Vec<ModuleSlot>`, the log/event/commands buffers and the pending `FrameView`. Lane hosts exist only inside the script phase (slots moved in as `Vec<Option<ModuleSlot>>` by id, `None` = another lane). `spawn_pool`, `rng` and `pool_size` live in `ModuleSlot` and move with it. | Nothing outside a phase needs a `ScriptHost`, and moving slots out as `Option`s needs no placeholder module. |
| D13 | `run_inits` at load runs outside a frame | `Scripting::run_inits(exec)` runs the same lane machinery with an init-only phase (pools reserved, merge, stats), so an `init` that spawns or sets behaves exactly as in a frame. | One code path; `Engine::run_script_inits` applies the commands at once as before. |
| D14 | per-lane event cap not specified | each lane keeps at most 4096 events locally (memory bound); the global cap of 4096 is applied at the merge in lane order, and `dropped_events` is the sum of both. | Same events dropped for every thread count (D6). |
| D15 | spawn pool overflow grows the pool by `2 * used` | the next pool size is `clamp(2 * (used + overflows), 64, 65536)` | After an overflow, `used` alone would not grow the pool. |
| D16 | warnings via `Scripting::warn_once` | lanes record `(key, text)` pairs; the merge feeds them to `warn_once` in lane order | The `warned` set is shared state; touching it from lanes would be timing-dependent. |
| D17 | `Fixed(n)` bounds helpers per `run`; panics as they happen | The `Executor` keeps one shared counter of helper jobs in flight; `run` sends a helper only while it is below `threads - 1`. Every task runs under `catch_unwind` on both the inline and the parallel path, and the panic of the **lowest task index** is re-raised. | Nested `run` calls (a par batch whose systems call `par_for_each`) each sent `threads - 1` more helpers, so more than `n` threads worked at once. Re-raising the first panic by completion time (or aborting inline) made the result timing-dependent. |

## 1. Configuration

```rust
// crates/core/src/engine/config.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Threads {
    /// Pool size from the core count (`worker_count`), or `EFE_THREADS`. Headless: Single.
    /// wasm without crossOriginIsolated: Single.
    #[default] Auto,
    /// `n` participants in total (the calling thread plus n-1 pool workers). Fixed(1) == Single.
    Fixed(usize),
    /// Everything on the calling thread; the pool is never touched.
    Single,
}
```

- `EngineConfig.threads: Threads` replaces `Option<usize>`. `validate` rejects `Fixed(0)`.
- `StartupConfig.threads: Option<usize>` (JS) stays as it is. `merged_with_startup` maps
  `Some(n)` to `Fixed(n)` when the engine config says `Auto`.
- `Engine::threads(&self) -> usize`: the current parallelism (number of participants).
- `Engine::set_threads(&mut self, Threads)`, and `Message::SetThreads(n)`, change it at runtime.
  If needed they grow the global pool (`set_global_pool_size`) to at least `n - 1` workers.
  They never shrink it below what it already is. Pool creation and growth are allowed in headless mode
  when `Fixed(n>1)` is asked for.

## 2. Executor (`crates/core/src/engine/parallel.rs`, new)

```rust
pub(crate) struct Executor { threads: usize }
impl Executor {
    /// Run every task once and return when all are done. Tasks may borrow the caller's stack.
    /// The calling thread works on tasks too (it never blocks on a channel), so this works on
    /// the wasm main thread (no Atomics.wait) and nested calls from a worker never deadlock.
    pub(crate) fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]);
    /// Split 0..len into chunks and call f(range) per chunk, in parallel.
    pub(crate) fn for_chunks(&self, len: usize, chunk: usize, f: &(dyn Fn(Range<usize>) + Sync));
}
```

Algorithm of `run`:
1. If `threads == 1` or `tasks.len() <= 1`, run the tasks inline in order, each under `catch_unwind` (all tasks run; the panic of the lowest index is re-raised). This is also the small-batch fast path.
2. Otherwise build `Arc<Shared { next: AtomicUsize, done: AtomicUsize, len, tasks: *mut [...] (lifetime erased), panic: Mutex<Option<Box<dyn Any + Send>>> }>`.
   Send up to `k = min(threads - 1, len - 1)` helper jobs to `GLOBAL_POOL` (`Jobs::Job`). A helper is sent only if the executor's shared `Arc<AtomicUsize>` of helpers in flight can be CAS-incremented while below `threads - 1`; a helper decrements it when its loop exits. So nested `run` calls never push the participants over `threads`. Each helper loops:
   `i = next.fetch_add(1)`; if `i >= len`, stop; otherwise run task `i` under `catch_unwind`, keep the `(index, payload)` with the lowest index, and do `done.fetch_add(1, Release)`.
3. The calling thread runs the same loop. Then it spins until `done == len` (Acquire).
   It uses `spin_loop`; on native it calls `thread::yield_now` every 64 spins.
   Then it re-raises the stored panic (lowest task index) with `resume_unwind`.
4. **Soundness invariant (write it in a `// SAFETY:` comment):** a task pointer is dereferenced only by
   the thread that claimed index `i < len`. `run` returns only after every claimed task has finished.
   A helper that starts late finds `next >= len` and touches only the `Arc`'d atomics, never the erased pointer.
   Every task is `Send`. Each `&mut` task is claimed by exactly one thread, through the `fetch_add`.
   This is the only `unsafe` in the scheduler.
5. Progress never depends on the pool: if every worker is busy with something else, the calling thread runs all the tasks.

## 3. Resources (`resources.rs`)

- Storage becomes `HashMap<TypeId, RwLock<Box<dyn Any + Send + Sync>>>`. `insert`, `get_mut`,
  `remove` and `get_or_insert_with` take `&mut self`. `get` and `get_mut` use `RwLock::get_mut` / `into_inner`, so exclusive code pays no lock cost. A `get(&self)` that returns `&T` cannot exist with a lock, so:
  - `get(&self) -> Option<Res<'_, T>>` (a read guard that derefs to `T`), and
  - `get_mut(&mut self) -> Option<&mut T>` (no lock),
  - plus the crate-internal `pub(crate) fn read<T>(&self) -> Option<Res<T>>` and `write<T>(&self) -> Option<ResMut<T>>`, which use `try_read`/`try_write`, for `ParCtx`.
  A poisoned or contended lock panics with the type name. Contention cannot happen when the access declarations are correct.
- Every resource must be `Send + Sync + 'static` (a breaking change: `Engine::insert_resource<T: Send + Sync + 'static>`).
  If an engine resource is legitimately `!Send` (check `Textures`, `RenderFrame`, `RenderWarnings` and the examples' resources), give the engine a separate main-thread-only store reachable only from `Ctx`. Record what you did.

## 4. Events (`events.rs`)

- `AnyEvents` gains `Send + Sync`. Event types must be `Send + Sync + 'static` (a breaking change on `add_event`, `send` and the like).
- Par systems read the readable buffer (`ParCtx::events::<T>()`). They never send directly: `ParCtx::send(ev)` pushes a `Box<dyn FnOnce(&mut EventRegistry) + Send>` into the system's own buffer. The buffer is replayed when the batch merges.

## 5. Access and ParCtx (`crates/core/src/engine/parallel.rs` or `access.rs`; re-export from `schedule` and the prelude)

```rust
#[derive(Clone, Default, Debug)]
pub struct Access { name: Option<&'static str>, read: Vec<TypeId>, write: Vec<TypeId>,
                    res: Vec<TypeId>, res_mut: Vec<TypeId>, names: HashMap<TypeId, &'static str> }
impl Access {
    pub fn new() -> Self;
    pub fn named(self, name: &'static str) -> Self;      // shown in panics; default: type_name of the closure
    pub fn read<T: Component>(self) -> Self;
    pub fn write<T: Component>(self) -> Self;            // implies read
    pub fn res<T: Send + Sync + 'static>(self) -> Self;
    pub fn res_mut<T: Send + Sync + 'static>(self) -> Self; // implies res
    pub fn conflicts(&self, other: &Access) -> bool;     // w/w or r/w on a component or a resource
}
```

```rust
pub struct ParCtx<'a> {
    world: &'a World,                 // private: go through the checked wrappers
    pub types: &'a TypeRegistry,
    pub time: &'a Time,
    pub input: &'a InputState,
    pub gamepads: &'a GamepadState,
    pub window: &'a WindowState,
    resources: &'a Resources,
    events: &'a EventRegistry,
    access: &'a Access,
    commands: RefCell<Commands>,      // per-system, per-run buffer (ParCtx is never shared between threads)
    sends: RefCell<Vec<Box<dyn FnOnce(&mut EventRegistry) + Send>>>,
    exec: &'a Executor,
}
impl<'a> ParCtx<'a> {
    pub fn query<Q: Query>(&self) -> QueryBorrow<'_, Q>;   // debug: checked against access
    pub fn query_one<Q: Query>(&self, e: Entity) -> QueryOne<'_, Q>; // debug: checked
    pub fn get<'b, T: ComponentRef<'b>>(&'b self, e: Entity) -> Result<T::Ref, ComponentError>; // debug: checked
    pub fn world_unchecked(&self) -> &World;               // escape hatch; hecs still checks borrows at runtime
    pub fn res<T: Send + Sync + 'static>(&self) -> Res<'_, T>;       // panics if not declared or missing
    pub fn res_mut<T: Send + Sync + 'static>(&self) -> ResMut<'_, T>;// panics if not declared res_mut
    pub fn try_res / try_res_mut  -> Option<..>             // same checks; None if the resource is missing
    pub fn events<T: Send + Sync + 'static>(&self) -> &Events<T>;
    pub fn send<T: Send + Sync + 'static>(&self, ev: T);
    pub fn commands(&self) -> RefMut<'_, ParCommands>;     // see below
    pub fn par_for_each<Q, F>(&self, batch: u32, f: F)
        where Q: Query, for<'q> Q::Item<'q>: Send, F: Fn(Q::Item<'_>) + Send + Sync;
}
```

- **Debug access check** (`cfg(debug_assertions)`): `<Q::Fetch as hecs::Fetch>::for_each_borrow(|id, unique| ..)`.
  A unique borrow needs `id ∈ write`. A shared borrow needs `id ∈ read ∪ write`.
  Otherwise panic: `par system `{name}` accesses `{type}` ({read|write}) without declaring it in its Access`.
  The type name comes from `Access.names`, or is shown as `TypeId(..)` for an undeclared type.
  Resource checks run in release builds too.
- **ParCommands**: a wrapper over `Commands` with a spawn that cannot reserve ids (D3).
  `spawn(bundle)` returns `()` and records `WorldOp::SpawnDeferred(EntityBuilder)`.
  It also has `despawn`, `insert`, `insert_bundle`, `remove`, `insert_raw`, `remove_raw` and the engine-control methods (`quit`, `set_title`, …).
  `Commands::apply` handles `SpawnDeferred` with `world.spawn(builder.build())`.
  Ops apply in order, so a deferred spawn gets its id at apply time, in the merged order: deterministic.
- `par_for_each`: `let mut q = self.query::<Q>()` (checked); `let batches: Vec<Batch<Q>> = q.iter_batched(batch).collect()`.
  If there is one batch, or `exec.threads == 1`, run inline. Otherwise build one `&mut dyn FnMut()` task per batch and call `exec.run`.
  The hecs batches are disjoint, so in-place writes are safe.
  `f` must not touch other systems' data. Only the query items are written in place.

## 6. Schedule (`schedule.rs`)

```rust
pub type System = Box<dyn FnMut(&mut Ctx<'_>)>;                       // exclusive, main thread
pub type ParSystem = Box<dyn FnMut(&ParCtx<'_>) + Send>;
enum Entry { Exclusive(System), Par { access: Access, f: ParSystem } }
Engine::add_system(stage, f)                         // unchanged
Engine::add_par_system(stage, access: Access, f: impl FnMut(&ParCtx<'_>) + Send + 'static) -> &mut Self
```

**Batch building**, done once per stage. It is cached and rebuilt lazily (dirty flag) when a system is added or its order changes. It is a pure function of the registered systems. *Update: every system has an order number; the entries are first sorted by `(order, registration index)` (see `docs/THREADING.md`), and everything below that says "registration order" means this sorted order.*
- Walk the entries in sorted order. `Exclusive` closes the current batch and becomes a batch on its own.
- A `Par` entry joins the current par batch if it does not conflict with any member. Otherwise it closes the batch and starts a new one.
  (No look-ahead and no reordering, so systems that conflict always keep their registration order.)
- Expose `pub(crate) fn batches(&self, stage) -> Vec<BatchInfo>` (or a test hook) for the unit tests.

**Running a stage:**
- Exclusive batch: build `Ctx` as today and call it.
- Par batch: build one `ParCtx` per member, all sharing `&World`, `&Resources` and `&EventRegistry`.
  Run the members through `Executor::run`, one task each. Afterwards, in registration order, append each member's `Commands` to the stage `commands` and replay its `sends` into the `EventRegistry`.
  Merging at the end of each batch, rather than at the end of the stage, keeps the order of all queued ops equal to the registration order, and that is the order a sequential run would produce.
- The `Commands` are applied after the stage, as today.
- The Executor lives on `Engine`. `run_stage` passes it to `Schedule::run`.

**Built-in systems:**
- `velocity_system`: par, `Access::new().named("velocity").read::<Velocity>().write::<Transform>()`, with `par_for_each` and a batch size of 1024.
- `propagate_transforms`: par. Its access is `read Transform, Parent; write GlobalTransform` (and `Children`, if such a component exists). Keep its algorithm. If it needs structural changes, they go through commands.
- `render_extract`: stays exclusive.
- `script_system`: stays exclusive. It runs its own lanes in parallel inside (§7).

## 7. Scripts: lanes, a snapshot and buffered writes (`scripting/`)

### 7.1 Send audit (step 2)
- Host functions are already `Arc<dyn Fn + Send + Sync>` (`efevm/src/imports.rs:184`) and `register_host_fn` already needs `Send + Sync`. Keep it that way.
- Add compile-time assertions (`const _: fn() = || { fn s<T: Send>() {} s::<..>(); };`) for:
  `efevm::Instance<ScriptHost>: Send`, `efevm::Module: Send + Sync`, `HostRegistry<ScriptHost>: Sync`,
  `efevm::Imports<ScriptHost>: Sync`, and `ScriptHost: Send`.
  Remove any `Rc`, `RefCell` or `Cell` that blocks them in efevm or in the scripting code.
- Check every example and test host closure. They must compile with these bounds, and ones that share state must use `Arc` with atomics or a `Mutex`.

### 7.2 Lanes
- `lanes: Vec<Vec<ModuleId>>` is computed from the loaded modules with union-find.
  Module A and module B share a lane if A imports a function from a namespace whose owner is module B, or the other way round (the provides graph).
  Inside a lane the modules are sorted by id. The lanes are sorted by their smallest id.
  Recompute after every load, unload or reload (or lazily each frame; it is cheap).
- A lane runs on one worker, in order:
  1. budget reset and pending `init`s for its modules;
  2. for each module, by id: `frame(dt)`;
  3. the `entry` of each `Script` entity of that module, in entity order (the same order as today).

### 7.3 The lane host
`ScriptHost` remains the `H` of the VM. During the script phase each lane gets its own `ScriptHost` value:
- `frame: Arc<FrameView>`: the read-only snapshot shared by all lanes.
  It holds `world: World` (moved in with `mem::take`), `types`, `time`, `input`, `events_in` and `event_sizes`.
  The world goes back with `Arc::try_unwrap` after every lane host has been dropped or reset. That must succeed; `expect` it with a clear message.
  This keeps the scripting side free of `unsafe`.
- `modules`: the lane's `ModuleSlot`s, moved out of the main host and back afterwards.
  A lookup by `ModuleId` finds only lane members. A call to a module outside the lane returns `RunError::OtherLane(name)`. This is a new variant and should not happen, because lanes are closed under imports.
- `writes: Vec<(Entity, u32 /*component id*/, Vec<u8>)>` and an index `HashMap<(Entity, u32), usize>` pointing to the latest entry for the read-back.
- `commands: Commands`: insert, remove, despawn and `script.attach`, as today.
- `spawn_pool: HashMap<ModuleId, VecDeque<Entity>>`, the pre-reserved ids (D2). `engine.world.spawn` pops from the pool of `current` and queues `Commands::spawn_reserved(e)` (an insert of an empty bundle, so the entity stays alive).
  If the pool is empty, spawn returns the null handle (bits 0, which every API treats as stale), counts `spawn_overflow`, and logs a warning once per frame per module.
- `script_state: Vec<(Entity, i64)>`: the new `Script.state` values, applied afterwards.
- `events_out`, `logs` and `dropped_events`, per lane.
- The RNG moves into `ModuleSlot.rng` (D4). Its seed is a mix of the engine seed and the module name. `engine.random.seed` seeds the current module's state only.
- `current`, as today.

### 7.4 Semantics, as seen by scripts
- `engine.world.get`, `has` and the iteration helpers read the snapshot. `get` first looks up `(entity, component)` in the lane's `writes` index. A lane sees its own writes, read-your-own-writes; other lanes see the start-of-phase values.
- `engine.world.set` checks `alive` and `has` against the snapshot (structure is frozen during the phase, so the check still holds when the write is applied). It then appends to `writes`. It returns the same status codes as before.
- insert, remove, despawn and attach are queued as before. They become visible after the stage, as before.
- Events sent are readable next frame, as before.

### 7.5 Merge, after all lanes have finished (main thread, `&mut World` back)
1. Lanes in lane order, then each lane's `writes` in call order: `entry.write(&mut world, e, &bytes)`.
   If an `(e, c)` pair was already written by an *earlier lane* this phase, `write_conflicts += 1`, and the later write wins.
   Log at debug level, plus a single warning per frame when the count is above 0.
   Several writes to the same `(e, c)` within one lane are not conflicts.
2. `Script.state` updates, in lane order.
3. Lane `Commands` are appended to the stage commands, in lane order.
4. Events are merged in lane order, applying the 4096 cap (D6). Logs are merged in lane order.
5. Unused spawn-pool entities are queued as despawns, in module-id order, *after* everything else. (They were reserved, so the next flush brings them to life empty; the despawn frees them in a deterministic order.)
   Pool size for the next frame: `clamp(2 * used, 64, 65536)`. The initial size is 64.

### 7.6 Stats
`pub struct ScriptStats { pub write_conflicts: u64, pub spawn_overflows: u64, pub lanes: usize, pub dropped_events: usize }`, last frame, from `Scripting::stats()` (and `Engine::script_stats()`).

### 7.7 Thread count
Lanes run through the engine `Executor`, one task per lane. With 1 thread they run in sequence through the same code path, with the same buffering, so the results are identical.

### 7.8 What host closures must respect
Host functions now run on worker threads, possibly at the same time as each other. They must be `Send + Sync`.
For determinism, a game host function that mutates shared state must do something order-independent, such as an atomic add, or go through the host's buffers.
Document this in THREADING.md.
Remove the methods `ScriptHost::world_mut` and `world_and_commands`.
Instead, `ScriptHost` offers `world() -> &World` (the snapshot), `set_component(e, id, bytes) -> Result<(), CompError>` (buffered), `commands_mut()` and `spawn() -> Option<Entity>` (from the pool).

## 8. Tests

- **Unit tests** (schedule/parallel):
  - batch builder: conflicting systems keep their order, non-conflicting systems share a batch, an exclusive system closes a batch, and r/r on the same component does not conflict;
  - `Access::conflicts`;
  - an undeclared query or resource panics with the system name (`#[should_panic]`, gated with `#[cfg(debug_assertions)]` for the query check);
  - the executor runs every task exactly once with 1, 2 and 8 threads, re-raises a panic, and handles nested `run`;
  - the merge order of `Commands` and events follows registration order, not completion order (give a task a sleep to force a different completion order).
- **Script tests:**
  - read-your-own-writes;
  - another lane sees the snapshot value;
  - `write_conflicts == 1` when two modules set the same component of the same entity, and the higher module id wins;
  - modules joined by provides share a lane;
  - spawn ids are equal for Single and Fixed(4);
  - pool overflow returns the null handle and counts it.
- **`crates/core/tests/parallel.rs`, the determinism test:** a world with about 2k entities;
  par systems (velocity through `par_for_each`, plus 3 custom par systems on disjoint and overlapping components, one using a resource and one spawning or despawning through commands);
  exclusive systems;
  and at least two script modules that `set` the same component and spawn entities.
  Run 300 frames with `Threads::Single` and with `Threads::Fixed(4)`, hash the world each time, and assert the hashes are equal.
  Repeat 20 times.
  World hash: entities sorted by `to_bits`, and per entity every registered component's bytes in ascending component id, plus `Script.state`. Use a fixed hasher (FNV or SipHash with fixed keys, not `RandomState`).
- **Benchmark, `crates/core/tests/parallel_bench.rs`:** an `#[ignore]` test.
  It sets up 10k entities with `Transform`, `Velocity` and `Sprite`, plus two extra par systems with some per-entity math (for example, a 64-step loop).
  It prints the ms per frame over 300 frames for Single, Fixed(2), Fixed(4) and Fixed(available_parallelism).
  Run it with `cargo test -p efeengine-core --release --test parallel_bench -- --ignored --nocapture`.

## 9. Docs
- `docs/THREADING.md`: the three rules (structure goes through `Commands`; Rust data through declared `Access`; scripts are buffered and applied in a fixed order), the determinism guarantee and what voids it (host closures with order-dependent side effects; `world_unchecked` misuse panics but stays sound), how batches are built, lanes, spawn pools, the stats, and the web notes (the main thread takes work itself, no `Atomics.wait`, and no crossOriginIsolated means Single).
- `ROADMAP.md`: a new item, ticked. `crates/core/structure.md`: the new files. This spec: keep §0 up to date with any new deviation.
