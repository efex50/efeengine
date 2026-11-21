# Implicit State Machines in `efeengine/core`

This report catalogs all identified implicit state machines in `crates/core`, detailing their implicit states, state-driving variables, transitions, associated failure modes/risks, and architectural refactoring suggestions.

---

## Table of Contents
1. [Engine Lifecycle, Scheduling & Concurrency](#1-engine-lifecycle-scheduling--concurrency)
2. [Windowing & OS Backends](#2-windowing--os-backends)
3. [Input Subsystem & Peripherals](#3-input-subsystem--peripherals)
4. [Rendering & GPU Subsystem](#4-rendering--gpu-subsystem)
5. [Scripting, Mod Management & VM ABI](#5-scripting-mod-management--vm-abi)
6. [Core Systems, Events, Commands & Clocks](#6-core-systems-events-commands--clocks)
7. [Architectural Recommendations](#7-architectural-recommendations)

---

## 1. Engine Lifecycle, Scheduling & Concurrency

### 1.1 Main Engine Execution & Lifecycle
- **Status**: resolved. One enum, `Lifecycle { Created, Running, Paused, ShuttingDown, Terminated }` (`crates/core/src/engine.rs`), replaces `EngineStatus` and the `started` / `shut_down` flags (`Engine.state.scripting.api_open` is separate and unchanged).
- **Transitions**:
  - First `frame()`: `Created` -> `Running`, closes API registration, runs `Stage::Startup` (skipped when killed first).
  - `Message::Stop`: `Running` -> `Paused`. A paused frame polls window events and handles messages but runs no stage, does not render and does not advance game time (`Time::track` only remembers the clock, so resuming has no large delta). The loop runners keep looping.
  - `Message::Start`: `Paused` -> `Running`.
  - `Message::Kill`, `EngineCommand::Quit` or `WindowEvent::CloseRequested`: any live state -> `ShuttingDown`; `abort_if_killed()` runs `Stage::Shutdown` once and sets `Terminated`. Later `frame()` calls are no-ops.

---

### 1.2 Frame Pipeline Stage Gating & Interruption
- **Location**: `crates/core/src/engine.rs:1070–1085, 1180–1238`
- **Implicit States**: 12 sequential pipeline phases:
  `ApiLockCheck` $\to$ `TimeAdvanceAndEvents` $\to$ `WindowPoll` $\to$ `GamepadPoll` $\to$ `StartupStage` $\to$ `PreUpdate` $\to$ `FixedUpdate` $\to$ `Update` $\to$ `PostUpdate` $\to$ `RenderExtract` $\to$ `WorldTick` $\to$ `RenderPresent`.
- **State Variables**: 7 consecutive `if self.abort_if_killed() { return; }` checkpoints in `frame()`.
- **Transitions & Triggers**: If any stage or command sets `EngineCommand::Quit` or `Message::Kill`, `abort_if_killed()` executes `shutdown_once()`, sets `Lifecycle::Terminated`, and returns early.
- **Risks**: Manual checkpoint repetition is fragile. If a new stage or plugin hook is added without an explicit checkpoint, killed engines will execute subsequent stages in an invalid state.
- **Refactoring Suggestion**:
  Convert `frame()` into an iterator or pipeline runner over an explicit `FrameStep` enum returning `ControlFlow::Continue` or `ControlFlow::Break`.

---

### 1.3 Schedule Batch Compilation & Cache
- **Location**: `crates/core/src/engine/schedule.rs:255–322, 460–467, 490–509`
- **Implicit States**:
  - `Dirty / Unsorted`: `dirty == true`, `batches_valid == false`.
  - `SortedOnly / BatchesStale`: `dirty == false`, `batches_valid == false` (or `built_for != active`).
  - `Valid / Cached`: `dirty == false`, `batches_valid == true`, `built_for == active`.
- **State Variables**:
  - `StageSystems.dirty: bool`
  - `StageSystems.batches_valid: bool`
  - `StageSystems.built_for: Vec<bool>`
  - `StageSystems.batches: Vec<BatchInfo>`
- **Transitions & Triggers**:
  - `push()`, `remove()`, `order()` set `dirty = true`.
  - `set_enabled()`, `run_if()` set `batches_valid = false`.
  - `ensure_sorted()` sorts and sets `dirty = false`, `batches_valid = false`.
  - `ensure_batches()` compiles disjoint batches and sets `batches_valid = true`.
- **Risks**: Two boolean flags plus a vector synchronize cache validity, making nonsensical states like `dirty == true && batches_valid == true` representable.
- **Refactoring Suggestion**:
  ```rust
  enum BatchCache {
      Dirty,
      SortedOnly { entries: Vec<Sys> },
      Compiled { entries: Vec<Sys>, batches: Vec<BatchInfo>, built_for: Vec<bool> },
  }
  ```

---

### 1.4 Schedule Plugin Ownership Scoping
- **Location**: `crates/core/src/engine/schedule.rs:409, 423–431, 437`, `crates/core/src/engine/plugin.rs:12–23`
- **Implicit States**: `Root / Unowned` $\to$ `PluginScoped(name)` $\to$ `NestedPluginScoped`.
- **State Variables**: `Schedule.owner: Option<Arc<str>>`.
- **Transitions & Triggers**: `swap_owner(Some(name))` called before `p.build(self)`; restored after build.
- **Risks**: Ambient mutable state on `Schedule`. If `build()` panics or errors, the ambient owner remains corrupted for subsequent registrations.
- **Refactoring Suggestion**:
  Use an explicit RAII `PluginScopeGuard<'a>` with `Drop` implementation to restore ownership.

---

### 1.5 System Execution Ordering Pipeline
- **Location**: `crates/core/src/engine/order.rs:1–20`, `crates/core/src/engine/schedule.rs:225–239`
- **Implicit States**: Phased integer sequence:
  `INPUT (-1000)` $\to$ `EARLY (-500)` $\to$ `SCRIPTS (-100)` $\to$ `USER (0)` $\to$ `PHYSICS (500)` $\to$ `TRANSFORM (900)` $\to$ `LATE (1000)`.
- **State Variables**: `Sys.order: i32`, `Sys.seq: u64`.
- **Risks**: Integer priority values act as implicit pipeline phases without compile-time boundaries. Order collisions fall back to registration order (`seq`), creating nondeterminism when plugin load order changes.
- **Refactoring Suggestion**:
  Replace raw `i32` with an explicit phase order enum or dependency graph (`Before(System)` / `After(System)`).

---

### 1.6 Worker Pool Resizing & Kill Cancellation
- **Location**: `crates/core/src/engine/thread_pool.rs:125–154`, `crates/core/src/engine/thread_pool/native.rs:84–105`, `crates/core/src/engine/thread_pool/web.rs:76–109`
- **Implicit States**: `Active` $\to$ `Shrinking` $\to$ `ShrinkCancelled` $\to$ `Zombie (Web with target=0)`.
- **State Variables**: `KillGate` (`AtomicUsize`), `ThreadPool.target: AtomicUsize`, `Jobs::Kill` messages, `Worker.done: Arc<AtomicBool>`.
- **Transitions & Triggers**:
  - `change_worker_amount(new_size)`: If shrinking, queues `Jobs::Kill` and charges `KillGate`. If growing while shrinking is in flight, calls `KillGate::cancel()`.
  - Worker receiving `Jobs::Kill` checks `KillGate::try_take()`. If false (cancelled), ignores kill and resumes.
- **Risks**: Worker lifecycle is emergent from channel ordering and atomic subtraction rather than explicit state. On Web, if JS URL is absent, pool drops to 0 workers and deadlocks on `join()`.
- **Refactoring Suggestion**:
  Model worker states explicitly (`Active`, `Retiring`, `Terminated`) and manage capacity changes synchronously.

---

### 1.7 Dedicated Worker Lifecycle
- **Location**: `crates/core/src/engine/thread_pool.rs:35–52, 99–122`, `crates/core/src/engine/thread_pool/native.rs:111–119, 134–146`
- **Implicit States**: `Running` $\to$ `KilledByHandleDrop` $\to$ `KilledByPoolDrop` $\to$ `Consumed (WorkerHandleOnce)`.
- **State Variables**: `WorkerHandle.sender`, `ThreadPool.dedicated_workers: Mutex<Vec<(Worker, Sender<Jobs>)>>`.
- **Risks**: Split ownership. When `WorkerHandle` drops and its thread exits, its `JoinHandle` remains leaked inside `ThreadPool.dedicated_workers` until the entire pool is dropped.

---

### 1.8 Parallel Batch Execution Protocol
- **Location**: `crates/core/src/engine/parallel.rs:34–85, 115–184`
- **Implicit States**: `Idle` $\to$ `Dispatched` $\to$ `SpinWaiting` $\to$ `PanicRecording` $\to$ `Completed`.
- **State Variables**: `Shared.next: AtomicUsize`, `Shared.done: AtomicUsize`, `Shared.panic: PanicSlot`.
- **Risks**: Coordination of lifetime-erased task closures relies entirely on atomic counters and spinloops without a formal state machine barrier.

---

## 2. Windowing & OS Backends

### 2.1 Web Pointer Lock / Cursor Grab
- **Location**: `crates/core/src/engine/window/web_backend.rs:330–340, 638–648`
- **Implicit States**: `Ungrabbed` $\to$ `PendingUserActivation` $\to$ `Locked`.
- **State Variables**: `want_grab: Cell<bool>`, browser DOM lock state.
- **Transitions & Triggers**:
  - `set_cursor_grab(true)` requests pointer lock and sets `want_grab = true`.
  - On user pointer interaction, if `want_grab` is true but lock was denied (lack of gesture), re-requests lock.
- **Risks**: When the user presses ESC, the browser drops pointer lock, but `want_grab` remains `true`. The next click anywhere on the canvas unexpectedly re-locks the cursor without game intent.
- **Refactoring Suggestion**:
  ```rust
  pub enum PointerGrabState {
      Unlocked,
      AwaitingUserActivation,
      Locked,
  }
  ```

---

### 2.2 Web Window Focus Loss Purge Cascade
- **Location**: `crates/core/src/engine/window/web_backend.rs:112–132, 311–328`
- **Implicit States**: `Focused` $\leftrightarrow$ `Unfocused`.
- **State Variables**: `focused: Cell<bool>`, `held: RefCell<HashSet<KeyCode>>`, `buttons: Cell<u16>`, `touches`.
- **Transitions & Triggers**: DOM `blur` / `visibilitychange` transitions `focused` to `false`, executing a full state purge: drains `held` emitting `Released`, clears mouse buttons, cancels all active touches, and resets pointer coordinates.
- **Risks**: Focus is treated as a simple boolean flag, but drives a major teardown. Uncoordinated or re-entrant DOM focus events can desynchronize input state.

---

### 2.3 Web Canvas Attachment & DOM Rebinding
- **Location**: `crates/core/src/engine/window/web_backend.rs:343–409, 451–474`
- **Implicit States**: `Unattached` $\to$ `Attached` $\to$ `Detached` $\to$ `Rebinding`.
- **State Variables**: `canvas: Option<HtmlCanvasElement>`, `canvas_listeners: Vec<Listener>`, `is_connected()`.
- **Risks**: Canvas attachment lifecycle is inferred ad-hoc during size sync by testing `is_connected()`. Moving the canvas in the DOM triggers silent teardown and listener recreation.

---

### 2.4 SDL Startup Events & Fractional Scale Re-Validation
- **Location**: `crates/core/src/engine/window/sdl_backend.rs:146–161, 321–331`
- **Implicit States**: `CreatedUninitialized` $\to$ `Active`; scale tracking: `ScaleClean` $\leftrightarrow$ `ScaleDirty`.
- **State Variables**: `sent_initial_state: bool`, `last_scale: f64`, `scale_dirty: bool`.
- **Transitions & Triggers**:
  - First `poll_events` emits initial `Resized` and `ScaleFactorChanged`.
  - Wayland `PixelSizeChanged` sets `scale_dirty = true` because SDL3 Wayland omits fractional display scale change events. At the end of the batch, current scale is queried and emitted.
- **Risks**: Local boolean flag acting as an ad-hoc two-phase commit. Early exits or error paths silently drop scale updates.

---

### 2.5 Headless Window Initialization
- **Location**: `crates/core/src/engine/window/headless.rs:66–78`
- **Implicit States**: `PendingInitial` $\to$ `Active`.
- **State Variables**: `sent_initial: bool`.
- **Risks**: One-way boolean flag. Window state cannot be reset or replayed for headless tests.

---

## 3. Input Subsystem & Peripherals

### 3.1 Digital Key Transitions
- **Location**: `crates/core/src/engine/input/state.rs:14–17, 73–90, 150–155`
- **Implicit States**: `Up`, `JustPressed`, `Held`, `JustReleased`, `SameFramePressAndRelease`.
- **State Variables**:
  - `held: HashSet<KeyCode>`
  - `just_pressed: HashSet<KeyCode>`
  - `just_released: HashSet<KeyCode>`
- **Transitions & Triggers**:
  - `apply(Key::Pressed)`: Inserts into `held` and `just_pressed`.
  - `begin_frame()`: Clears `just_pressed` and `just_released`.
  - `apply(Key::Released)`: Removes from `held`, inserts into `just_released`.
  - Focus loss: Extends `just_released` with `held.drain()`.
- **Risks**:
  - Three independent boolean sets yield $2^3 = 8$ mathematical states, allowing impossible illegal states (e.g. key present in both `held` and `just_released`).
  - Missing or duplicate `begin_frame()` calls corrupts input history.
- **Refactoring Suggestion**:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ButtonState {
      Up,
      JustPressed,
      Held,
      JustReleased,
  }
  ```
  Store a single lookup from `KeyCode` to `ButtonState`.

---

### 3.2 Mouse Buttons & Pointer Position Tracking
- **Location**: `crates/core/src/engine/input/state.rs:18–21, 98–112, 156–160`
- **Implicit States**: Same 4-state lifecycle for mouse buttons (`mouse_held`, `mouse_just_pressed`, `mouse_just_released`); pointer: `Uninitialized (None)` $\to$ `Tracked (Some(x, y))`.
- **Risks**: Triple hash-set allocation overhead for 5 buttons. On native SDL, `mouse_pos` never reverts to `None` when cursor leaves the window.

---

### 3.3 Gamepad Edge Transitions & Connection Tracking
- **Location**: `crates/core/src/engine/input/gamepad.rs:117–120, 146–159, 186–202`
- **Implicit States**: Buttons: `Up`, `JustPressed`, `Held`, `JustReleased`; Pads: `Disconnected` $\leftrightarrow$ `Connected`.
- **State Variables**: `buttons: u32`, `prev_buttons: u32`, `pads: Vec<Gamepad>`.
- **Risks**: Edge transitions rely on bitwise XOR between frame bitmasks. Inputs targeting disconnected pad IDs are dropped silently without diagnostic events.

---

## 4. Rendering & GPU Subsystem

### 4.1 GPU Surface Configuration & Zero-Size Handling
- **Location**: `crates/core/src/engine/render/gpu.rs:11–108`, `crates/core/src/engine.rs:1090–1151`
- **Implicit States**: `Unconfigured` $\to$ `Configured` $\to$ `Suspended / ZeroSized` $\to$ `Outdated / Lost` $\to$ `Suboptimal`.
- **State Variables**: `GpuContext.config: SurfaceConfiguration`, `resize() -> bool`, `SurfaceError`.
- **Status**: resolved for the zero-size case. `GpuContext` has an explicit `suspended` flag set by a zero-sized `resize` and cleared by the next non-zero one (which always reconfigures). While suspended, `render_frame_to_surface` does not call `get_current_texture()` and `reconfigure()` is a no-op; texture changes are requeued.
- **Refactoring Suggestion**:
  ```rust
  pub enum SurfaceLifecycle {
      Unconfigured,
      Active { surface: Surface<'static>, config: SurfaceConfiguration },
      Suspended { surface: Surface<'static>, last_config: SurfaceConfiguration },
      NeedsReconfigure { surface: Surface<'static>, config: SurfaceConfiguration, reason: ReconfigureReason },
  }
  ```

---

### 4.2 Texture Store Transaction & Frame Rollback
- **Location**: `crates/core/src/engine/render/textures.rs:88–105, 182–218`, `crates/core/src/engine.rs:1146–1150`
- **Implicit States**: `Clean/Idle` $\to$ `PendingAdded/Updated/Removed` $\to$ `Drained` $\to$ `Committed` or `RolledBack/Requeued`.
- **State Variables**: `Textures.pending: Vec<Op>`, `requeue()`, `drawn: bool`.
- **Transitions & Triggers**:
  - Ops accumulated in `pending`.
  - `drain_changes()` consolidates first/last operations into `TextureChanges`.
  - If frame presentation fails (`drawn == false`), `requeue()` splices operations back to index 0 of `pending`.
- **Risks**: Splicing operations back to index 0 can invert causal order if other operations were queued during or after the render pass.
- **Refactoring Suggestion**:
  Implement a formal `TextureTransaction` token with explicit `commit()` and `rollback()` semantics.

---

### 4.3 Renderer Lazy Initialization Fallback
- **Location**: `crates/core/src/engine/render/sprite_renderer.rs:91–112, 351–360`
- **Implicit States**: `Uninitialized (None)` $\to$ `Initialized (Some(State))`.
- **State Variables**: `SpriteRenderer.state: Option<State>`.
- **Risks**: `render()` calls `get_or_insert_with`, silently compiling shaders and creating GPU buffers mid-frame if `init()` was bypassed, causing frame drops.

---

### 4.4 Dynamic Vertex Buffer Allocation
- **Location**: `crates/core/src/engine/render/sprite_renderer.rs:74–89, 302–314`
- **Implicit States**: `Adequate` $\to$ `Exceeded / Reallocating` $\to$ `Clamped / Truncating`.
- **State Variables**: `capacity: usize`, `staging: Vec<SpriteGpu>`, `max_sprites`.
- **Risks**: Growth is monotonic with no mechanism to shrink buffers when sprite counts decrease. When exceeding device limits, sprites are truncated silently without warning.

---

### 4.5 Frame Double Buffering & Extraction
- **Location**: `crates/core/src/engine/render/extract.rs:68–175`, `crates/core/src/engine.rs:1114–1150`
- **Implicit States**: `Simulating` $\to$ `Extracting` $\to$ `Extracted` $\to$ `Rendering` $\to$ `Presented / RolledBack`.
- **State Variables**: `cx.resources.remove::<RenderFrame>()` / `insert(frame)`.
- **Risks**: Double buffering is managed by taking and re-inserting the ECS resource. If `render_extract` panics, the `RenderFrame` resource is lost permanently.

---

### 4.6 Camera Active Selection & Warning Latch
- **Location**: `crates/core/src/engine/render/extract.rs:76–115`, `crates/core/src/engine/render/frame.rs:91–98`
- **Implicit States**: `NoActiveCamera` $\to$ `SingleCamera` $\to$ `MultipleActiveConflict`; latch: `Unwarned` $\to$ `LatchedWarned`.
- **State Variables**: `cam.active: u32`, `cam.kind: u32`, `warnings.multiple_cameras: bool`.
- **Risks**: Integer flags (`0` vs `1`) define projection modes; unknown integers default silently to orthographic. Multi-camera warning permanently latches true on first occurrence.

---

### 4.7 Sprite Dimension Fallback
- **Status**: resolved. `Sprite.auto_size` (u32): non-zero takes the texture's pixel size (1x1 without a texture); zero uses `size` literally, so 0 is a real zero size. `Sprite::new` / `Default` set `auto_size = 1`; `Sprite::with_size` sets `size` and `auto_size = 0`.

---

### 4.8 PNG Stream Decoder & Scanline Filter
- **Location**: `crates/core/src/engine/render/png.rs:117–140, 156–270`
- **Implicit States**: `AwaitingSignature` $\to$ `AwaitingHeader` $\to$ `ReadingChunks` $\to$ `AwaitingEnd` $\to$ `Inflating` $\to$ `Unfiltering`.
- **State Variables**: `header: Option<Header>`, `saw_idat: bool`, `ended: bool`, `prev_start: Option<usize>`.
- **Risks**: Chunk sequence validation occurs via booleans after reading. Accepting `PLTE` after `IDAT` or non-consecutive `IDAT` chunks violates PNG specification.

---

## 5. Scripting, Mod Management & VM ABI

### 5.1 `ModuleSlot` Lifecycle & Execution Lock
- **Location**: `crates/core/src/engine/scripting/host.rs:51–80, 417–468`, `crates/core/src/engine/scripting/mods.rs:469–481`
- **Implicit States**: `Unloaded` $\to$ `LoadedPendingInit` $\to$ `LoadedReady` $\to$ `Executing`.
- **State Variables**:
  - `slot.loaded: bool`
  - `slot.instance: Option<Instance<ScriptHost>>`
  - `slot.pending_init: bool`
- **Transitions & Triggers**:
  - `instantiate()`: Sets `loaded: true`, `instance: Some(...)`, `pending_init: true`.
  - `run_lane()`: Clears `pending_init`, calls module `init` export $\to$ `LoadedReady`.
  - `run_inner()`: Takes instance via `slot.instance.take()` $\to$ `Executing`. Restores instance on return.
  - `unload_now()`: Sets `loaded = false`, `instance = None`.
- **Risks**:
  - `instance == None` is overloaded: it represents both an unloaded module (`loaded == false`) and an actively executing module (`loaded == true`).
  - If execution traps or panics before the instance is restored, the slot remains locked in `Executing` forever, causing subsequent calls to fail with `RunError::Busy`.
- **Refactoring Suggestion**:
  ```rust
  pub(crate) enum ModuleLifecycle {
      Unloaded,
      PendingInit(Instance<ScriptHost>),
      Ready(Instance<ScriptHost>),
      Executing,
  }
  ```

---

### 5.2 Scripting Frame Execution & Lane Dispatch
- **Location**: `crates/core/src/engine/scripting/mod.rs:441–475`, `crates/core/src/engine/scripting/system.rs:248–313`
- **Implicit States**: `Standby` $\to$ `FrameEntered` $\to$ `LanesDispatched` $\to$ `LanesMerged` $\to$ `FrameLeft`.
- **State Variables**: `Scripting.frame: Option<FrameView>`, `Scripting.modules: Vec<ModuleSlot>`.
- **Transitions & Triggers**:
  - `enter()`: Moves `World` into `self.frame`.
  - `run_phase()`: Takes `self.frame`, moves `self.modules` into worker thread lanes.
  - Lane join: Merges writes and restores `self.modules`.
  - `leave()`: Moves `World` back to engine.
- **Risks**: During parallel execution, `self.modules` is emptied via `mem::take()`. Any query to `scripting.modules()` during this window incorrectly reports zero loaded modules. Calling `enter()` twice drops the prior `FrameView`, destroying the game `World`.

---

### 5.3 Mod Hot Reload & Migration Transaction
- **Location**: `crates/core/src/engine/scripting/reload.rs:171–323`, `crates/core/src/engine/scripting/components.rs:159–204`
- **Implicit States**: `DependencyAnalysis` $\to$ `PreBuilding` $\to$ `UnloadAndStash` $\to$ `StagedReinstall` $\to$ `Rollback / Commit`.
- **State Variables**: `Scripting.stash: BTreeMap<u32, Stashed>`, `Scripting.migrations: Vec<Migration>`.
- **Risks**: Stash and migrations reside directly on `Scripting`. If reload aborts unexpectedly, orphaned components remain stashed and may be adopted by unrelated modules later.

---

### 5.4 Mod Package Staging Pipeline
- **Location**: `crates/core/src/engine/scripting/packages.rs:526–563`, `crates/core/src/engine/scripting/mods.rs:118–267`
- **Implicit States**: `Discovered` $\to$ `Decoded` $\to$ `PreValidated` $\to$ `PolicyNegotiated` $\to$ `ComponentsRegistered` $\to$ `ActivePendingInit`.
- **State Variables**: `fresh: Vec<u32>`, manual cleanup calls (`undo_components`, `unregister_owner`).
- **Risks**: Rollback logic is manually distributed across 4 files without RAII guards; failure at any step risks leaking component IDs or host registrations.

---

### 5.5 Ambient Execution Context & Re-entrancy
- **Location**: `crates/core/src/engine/scripting/host.rs:441–445`, `crates/core/src/engine/scripting/api_engine.rs#L80–103`
- **Implicit States**: `Idle` $\to$ `ExecutingModule(ModuleId)` $\to$ `NestedProvideCall(ModuleId)`.
- **State Variables**: `ScriptHost.current: Option<ModuleId>`.
- **Risks**: Host functions rely on ambient `current`. If called outside a script frame, they fail silently (e.g. `spawn` returns 0). An uncaught panic leaves the wrong module ID attributed.

---

### 5.6 Dynamic Component Migration
- **Location**: `crates/core/src/engine/scripting/components.rs:62–75`, `crates/core/src/engine/types.rs:194–202`
- **Implicit States**: `Unregistered` $\to$ `RegisteredActive` $\to$ `Stashed` $\to$ `PendingMigration` $\to$ `Migrated` $\to$ `Orphaned`.
- **State Variables**: `Scripting.stash`, `Scripting.migrations`, `world.dynamic_info(id)`.
- **Risks**: Layout migration is deferred. If entity queries run between `register_components` and `finish_swap`, they read old memory layouts with new descriptors.

---

### 5.7 Cross-Module Provide Call ABI
- **Location**: `crates/core/src/engine/scripting/provides.rs:69–168`
- **Implicit States**: `Marshalling` $\to$ `InvokingProvider` $\to$ `Demarshalling` $\to$ `Aborted / Trapped`.
- **State Variables**: `scratch: (u64, u64)`, `copies: Vec<CopyIn>`, `back_to: Vec<u64>`.
- **Risks**: Mutated parameters write back only on `Exit::Done`. If the provider traps midway, caller memory receives zero updates, leaving guest memory partially mutated.

---

### 5.8 Disk Script Cache Resolution
- **Location**: `crates/core/src/engine/scripting/cache.rs:63–77, 311–363`
- **Implicit States**: `Disk::Unresolved` $\to$ `Disk::Resolved(Option<PathBuf>)`; entries: `PendingVerification` $\to$ `VerifiedFresh` $\to$ `Stale / Deleted`.
- **State Variables**: `ScriptCache.disk`, `.efvm`/`.deps` filesystem presence.
- **Risks**: Querying a stale cache entry via `lookup()` immediately issues `fs::remove_file` calls, mixing read queries with destructive mutations.

---

## 6. Core Systems, Events, Commands & Clocks

### 6.1 Simulation Clock & Fixed-Step Mode
- **Location**: `crates/core/src/engine/time.rs:19–28, 58–74`, `crates/core/src/engine.rs:1081–1085`
- **Implicit States**: `Unstarted (last_now == None)` $\to$ `VariableMode (in_fixed == false)` $\leftrightarrow$ `FixedMode (in_fixed == true)`.
- **State Variables**: `last_now: Option<f64>`, `in_fixed: bool`, `accumulator: f64`.
- **Risks**: The return value of `time.delta()` fundamentally changes based on the boolean flag `in_fixed`. If an early return fails to clear `in_fixed`, the engine remains locked in fixed-timestep delta mode.
- **Refactoring Suggestion**:
  Pass distinct update contexts or split time access (`FrameTime` vs `FixedTime`).

---

### 6.2 Double-Buffered Event Queue
- **Location**: `crates/core/src/engine/events.rs:12–83`
- **Implicit States**: `Pending / Staged (Frame N)` $\to$ `Readable / Active (Frame N+1)` $\to$ `Expired (Frame N+2)`; bypass: `ImmediateReadable`.
- **State Variables**: `pending: Vec<T>`, `readable: Vec<T>`.
- **Risks**: Dual delivery paths (`send` = staged, `extend_readable` = immediate) create ordering ambiguities across systems.

---

### 6.3 Command Buffer Draining Stages
- **Location**: `crates/core/src/engine/commands.rs:60–66, 192–237`
- **Implicit States**: `Recording` $\to$ `Merging` $\to$ `ApplyingWorldOps` $\to$ `ApplyingSchedOps` $\to$ `ApplyingEngineOps` $\to$ `Flushed`.
- **State Variables**: `ops: Vec<WorldOp>`, `list: Vec<EngineCommand>`, `sched: Vec<ScheduleOp>`.
- **Risks**: Queues are drained in independent passes. Invoking `apply()` drains only world ops while leaving engine control and schedule ops unexecuted.

---

### 6.4 `TypeRegistry` Generation Invalidation
- **Location**: `crates/core/src/engine/types.rs:511–518`, `crates/core/src/engine/scripting/mod.rs:450–452`
- **Implicit States**: `Synchronized` $\leftrightarrow$ `StaleCache`.
- **State Variables**: `generation: u64`, `Scripting.types: Arc<TypeRegistry>`.
- **Risks**: Relies on poll-based cache invalidation at frame boundaries. Mutating types mid-frame leaves executing script lanes holding desynchronized schemas.

---

## 7. Architectural Recommendations

1. **Unify Engine & Subsystem Lifecycles with Typestates / Enums**:
   Replace boolean triads (e.g. `started`, `shut_down`, `api_open`, `status`) with formal lifecycle enums (`EnginePhase::Bootstrapping`, `Running`, `Terminated`). Use builder types (`EngineBuilder`) to make post-startup API registration impossible at compile time.
2. **Replace Multi-HashSet Input with Typed Key States**:
   Replace the three `HashSet<KeyCode>` structures with a single map or array storing an explicit `enum KeyState { Up, JustPressed, Held, JustReleased }`, eliminating impossible state combinations.
3. **Encapsulate Transactions with RAII Guards**:
   Wrap Mod Hot-Reloading (`SwapTransaction`), Cross-Module Provide calls (`ProvideCallGuard`), and Plugin Scoping (`PluginScopeGuard`) in explicit RAII transaction types that guarantee automated rollback or cleanup on trap or panic.
4. **Decouple Time Contexts**:
   Replace the mutable `in_fixed: bool` toggle with distinct contexts (`FrameTime` vs `FixedTime`) passed directly to systems.
