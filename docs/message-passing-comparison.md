# Message passing: efeengine compared with termrender

_Written 2026-11-15. Sources: two read-only code analyses (Sonnet agents, appended below),
a comparison reviewed with a second model, and a Haiku check of the claims the
recommendations rest on. Nobody ran code for this report. Claims marked **(verified)** were
re-read in the source by a second agent; the rest come from one reading. Line numbers in
`crates/core/src/engine/scripting` may drift while E8 is being implemented._

## Summary

efeengine's message passing is the stronger design: typed events and an exhaustive control
enum, safe double buffering, deterministic order, a thread-safe queue for workers and JS, and
deferred world commands. termrender (`/home/efe/projeler/game/termrender`) is single-threaded,
stringly typed and has aliasing UB, but it has two ideas efeengine lacks: **addressing a
message to one target** and **core events delivered on the same bus as game events**.

The main finding: efeengine's *script* side repeats termrender's main weakness. A
`ScriptEvent` is a string name plus untyped bytes with no schema, so script events are the
one script-visible thing that is not in the API index.

## Side by side

| | termrender | efeengine |
|---|---|---|
| Threads | one; the `Send` bound on messages is unused | main thread (engine, window, input), thread pool, logger thread; flume across threads |
| Message types | `Message { hint: String, target: String, msg: Box<dyn Any> }` | `Message` enum (control), `Events<T>` per Rust type, `Commands`, `ScriptEvent { name, data }` |
| Typing in Rust | string names, downcast with `unwrap` (panics on mismatch) | typed `Events<T>`, exhaustive `match` on `Message` |
| Typing for scripts | n/a | string name + bytes, no schema |
| Delivery | push: handler called per system, by target name or to subscribers | pull: systems read `Events<T>`; broadcast only |
| Latency | next frame | events next frame; `Message` same frame via `extend_readable` **(verified)**; `Commands` after every stage |
| Sent while delivering | dropped by `clear()`, or UB if the vec reallocates | safe (double-buffered) |
| Order | `HashMap` iteration, differs per run | insertion order, deterministic |
| Back-pressure | none (unbounded `Vec`) | none (all flume queues unbounded); script events capped at 4096 per frame, extras silently dropped **(verified)** |
| Errors | one handler error abandons the rest of the queue | dead-entity commands logged; script traps logged and the module skipped for that frame |
| Re-entrancy | n/a | a call back into a running module is a clean trap (`RunError::Busy`, test `reentering_a_running_module_is_a_clean_trap`) **(verified)** |
| Per-message cost | ~4 `String` allocations + a box | `Message` carries Strings; script events and `InputState` cloned per frame |

## Recommended improvements for efeengine (priority order)

1. **Typed script events in the API index.** Register each script event with a name and a
   struct layout (the type registry already describes structs), so events appear in `.efapi`
   and the assembler and language server can check `engine.events.send`/`read` against them.
   This closes the last gap in "everything script-visible is in the index". It overlaps E8's
   copy ABI (E8.5, task T10), which copies structs by `ApiType`, so it is cheapest to decide
   now, while that is being built.
2. **Targeted delivery** (from termrender's name + target). Let an event go to one entity, one
   script or one mod, as well as broadcast. Scripts would gain `send_to(entity, event)`.
3. **Same-frame events.** Give readers cursors (an event sent in PreUpdate readable in Update
   of the same frame), as `Message` already has with `extend_readable`. Today system-to-system
   events always lag one frame **(verified)**.
4. **Core events on the bus** (from termrender's built-in `RESIZED`). Publish
   `Events<InputEvent>` and window events (resized, focus, close) as well as the folded
   `InputState`, so systems and scripts can react to raw events.
5. **No silent drops.** When the 4096 script-event cap is hit, log once per frame and count
   the dropped events (`host.rs:186-189` has no else branch) **(verified)**.
6. **One queue per engine.** Route the JS bridge to the engine's own sender instead of the
   global `MESSAGE_SYSTEM`, so two engines cannot race for one queue, and move logging out of
   `Message`. A reply channel from Rust to JS is later work.
7. **Cheap performance fixes.**
   - `Events::update` does `mem::take`, freeing the old buffer every frame; swap the two
     buffers and `clear()` instead (the doc comment already says "Swaps") **(verified)**.
   - Borrow script events and `InputState` in the script system instead of cloning them every frame.
   - Resolve `Script.entry` to an export index once, at load, instead of cloning the name per entity per frame.
   - Reuse the `Vec` that `drain_messages` allocates every frame.
8. **Robustness.**
   - `JobHandle::join` does `recv().unwrap()` (`thread_pool.rs:229`) and panics when the job panicked; return a `Result` **(verified)**.
   - The web `worker_loop` has no `catch_unwind` (`thread_pool/web.rs:267`), so a panicking job kills the worker **(verified)**.
   - With zero web workers, `join` on the global pool blocks forever; on a dedicated worker with no URL it panics instead. Both should be errors **(verified)**.
   - The global logger is a `static` (`log.rs:8`), so its Shutdown-on-drop never runs and queued lines may be lost at exit; flush it explicitly at engine shutdown. Dropping any clone of `LoggerSender` would also shut it down **(verified)**.
   - `Stop` ends the loop and `Start` does not resume it (unverified).

## For termrender (report only; that repository was not changed)

- Remove the `&self` → `&mut` raw-pointer casts (`subgamegetters.rs`, `Systems::clone_self`, `World::self_p`): this is undefined behaviour.
- Double-buffer the signal queue, so signals sent from a handler go to the next frame instead of being lost.
- Typed events (a `Vec<T>` per type, or an enum) instead of `Box<dyn Any>` plus `unwrap` downcasts.
- Keep systems in an ordered collection, so run and delivery order is deterministic.
- Intern signal names to ids, to drop the ~4 string allocations per signal.
- A handler error should not abandon the rest of the queue.

Worth keeping from termrender: the deferred queue drained at a fixed point, name + target
addressing, per-system subscriptions, `pressed`/`just_pressed` input, and the panic hook that
replays panics after the terminal is restored.

## Gaps found along the way

- Runtime call cycles between mods (A calls B, B calls A) end in a clean nested trap, because a
  running module is marked busy. The E8 design mentions this only in the copy-ABI section; it
  should state it as a general rule (a re-entrant call between mods is refused) and keep the
  existing test when the trampoline is rewritten.

---

The two source reports follow unchanged.

## Appendix A: termrender analysis

### termrender message passing analysis

Paths relative to /home/efe/projeler/game/termrender/. Crate is ~3.7k LOC, deps: crossterm, once_cell, paste, rand, strip-ansi-escapes, uuid. No channel, async, or threading crate.

## Summary
Single-threaded. No channels, no spawned threads, no async. "Messaging" is a deferred in-process signal queue (`Vec<Message>`) on `Systems`, plus polled input state, polled flags, and a global mutex-guarded logger. Grep for mpsc/crossbeam/flume/thread::spawn/tokio finds nothing; the only `Arc<Mutex>` uses are the logger (src/game/logger.rs:129-140) and the panic-hook log buffer (src/game/mod.rs:122,138-148).

## 1. Message types
- `Message { hint: String, target: String, msg: Box<dyn Any + Send> }` (src/game/systems.rs:17-21). Only message struct. `hint` = signal name; `target` = system name (or anything that is not a system name, to mean "broadcast").
- Signal names are plain `&str` constants. Core one: `RESIZED = "game::resized"` (src/game/signal_types.rs:5), payload is a `Vec2` (screen size).
  - Producer of RESIZED: `Game::tick_enter` (src/game/mod.rs:213-219) with `GAME_NAMESPACE` ("game", mod.rs:15) as target.
  - Examples define their own: pong `SCORE_P1/SCORE_P2` sent from a physics loop (examples/pong.rs:319-333, payload `Box::new(())`), consumed by ScoreSystem `_on_signal_recieved` (pong.rs:81, registered 439-441). Snake `SCORE_INC` (snake.rs:479 sent, handled 169-198, payload `i32` via `downcast_ref`, :185).
- `Flags` enum (Debug, Rerender, Resized, ForceRerender, Custom(String)) in a `HashSet` on `Game` (mod.rs:112-119). Frame-local state flags, set by input/tick and read by the render step. Not messages to other systems.
- `Input { pressed_keys, just_pressed_keys: HashSet<Keys> }` (src/game/input.rs:1-8), `Keys` enum (:23-39). Input is polled state, not events: a producer (crossterm) feeds it, systems read it.

## 2. Transport
- Signals: unbounded `Vec<Message>` called `pending_signals` (systems.rs:25), push at :58, drained at :75-90. Synchronous, in-thread, single consumer per delivery, multicast via registration.
- Input: `crossterm::event::poll(1ms)` + `read()` in `Input::poll_keys` (input.rs:14-26), called at the top of every loop iteration (mod.rs:352).
- Shared state: nearly everything is accessed via `&mut Game` handed to every callback. `Game` getters (src/game/subgamegetters.rs:1-68) deliberately cast `&self` to `&mut` with unsafe raw pointers, so aliasing is UB-adjacent. `Systems::clone_self` (systems.rs:117-121) and `World::self_p` (mod.rs:105) do the same.
- Logger: global `Lazy<Arc<Mutex<Logger>>>` (logger.rs:129), `log()` locks and pushes a formatted `String` onto `Vec<String>` (logger.rs:~122); dumped on `Drop` (mod.rs:392-444).

## 3. Threading model
One thread. `Game::main_loop` (mod.rs:349-359): `poll_keys` -> `organize_flags` -> `tick`. In `tick` (mod.rs:231-286): Esc check, `tick_enter` (init, resize detection, deliver signals), then physics tick (if `should_physics`), process tick (if `should_process`), render (if `should_render`), clear `just_pressed_keys`. Each stage has its own fps in `Timing` (timing.rs). The loop is a busy loop: only wait is the 1ms input poll. `Message: Send` bound exists but is never exploited (nothing crosses threads).

## 4. Ordering and timing
- Signals are queued by `send_signal` and delivered only in `tick_enter` of the next loop iteration, before physics/process. Latency: sent during physics/process of frame N, handled at start of frame N+1 (one frame later). A signal sent inside a handler during delivery is pushed onto `pending_signals` while it is being iterated and then dropped by `clear()` at systems.rs:88 (never delivered; also iteration is over an aliased `&mut` of the same Vec, see weaknesses).
- FIFO order, but across broadcast targets the order follows `HashMap` iteration order (`get_system_with_registered` systems.rs:91-99, and `sys: HashMap<String,System>` for physics/process loops, mod.rs:245,260), so system execution and delivery order are nondeterministic between runs.
- No back-pressure: queue grows without bound within a frame; no capacity. No "closed" concept.
- Unhandled: a Message whose `target` is not a system and has no registered listeners is silently dropped. Targeted messages ignore the registration set entirely.
- Input: `poll_keys` drains all pending OS events each loop (`while poll(1ms)`), so a steady stream can stall the loop. `just_pressed_keys` lives exactly one `tick`. Release events require the kitty keyboard-enhancement flags (screen.rs imports them).

## 5. Typing and dispatch
- Stringly-typed, dynamically typed payload: `Box<dyn Any + Send>`; receivers must `downcast_ref::<T>().unwrap()` (snake.rs:185) which panics on mismatch. Dispatch inside a handler is a manual `match hint.as_str()` (snake.rs:176-198).
- Handlers: one trait method `GameSystem::_on_signal_recieved(&mut self, sys_name, game, &mut Message, from)` (systems.rs:217) on the `GameSystem` trait (:212-224, default no-op methods `_setup/_physics_loop/_process_loop/_kill_system`). Registration: `Game::register_signal(sig, sys)` / `System::register_signal` (internals.rs:70, systems.rs:187), stored as `HashSet<String>` per system (:157). Adding a new message kind = pick a name constant, register listeners, add a match arm, agree on payload type; no compile-time checking.
- Systems themselves live in `HashMap<String, System>` with `Box<dyn GameSystem>`, `as_any` downcast access via `impl_sys!` macro (systems.rs:5-16, 105-115).

## 6. Error handling and shutdown
- Handlers return `RetTick` (`Result<bool, io::Error>`); `?` in `deliver_signals` aborts delivery for all remaining messages and propagates out of `main_loop`, leaving `pending_signals` uncleared (clear at :88 is skipped). `Ok(false)` from a system's loop stops the game (mod.rs:253,268), but note `res` is only overwritten if a system is active.
- Missing system on register gives `Error::from_raw_os_error(31)` (systems.rs:62,68), an obscure errno rather than a message.
- Shutdown: Esc pressed (mod.rs:232) or `Ok(false)`; `Drop for Game` (mod.rs:392) restores the terminal, prints panic logs captured by the hook, writes log file with numbered suffix, strips ANSI. `Drop` calls `exit().unwrap()` which can double-panic. No thread joins, no dropped-sender semantics.

## 7. Allocation and performance
- Each `send_signal` allocates: `hint.into()` and `to.into()` (two `String`s in internals.rs:62), then `to_string()` again for both inside `Systems::send_signal` (systems.rs:53-57), plus the `Box` for the payload (even for `()` it is zero-sized so free, but `i32`/`Vec2` allocate). So about 4 string allocations per signal.
- Delivery: `msg.hint.clone()` per handler call (systems.rs:78,83), `get_system_with_registered` scans all systems and clones names into a `Vec<String>` per message (O(systems) per broadcast). Name-keyed lookups hash strings.
- Per-frame also: `Flags::Custom(String)` hashed/allocated in render path; `LOG!` calls lock the global logger mutex (Debug level is skipped when `feature = "debug"` is off, logger.rs).
- No batching, no pooling, no generational ids.

## 8. Strengths, weaknesses, clever bits
Strengths
- Simple, deterministic-in-intent frame model: signals deferred to a known point (start of tick) avoids re-entrancy during system loops; decouples producer and consumer.
- Two addressing modes in one API: direct-to-system and publish/subscribe by signal name via per-system registered set (systems.rs:75-90).
- Built-in `RESIZED` signal gives systems a uniform way to learn about core events; `just_pressed` vs `pressed` split is a good input pattern; panic hook that saves messages and replays them after terminal restore (mod.rs:138-148, 396-409) is a nice touch for TUIs.
- `Message` carries a `hint` plus `target`, handy for debugging/logging.

Weaknesses
- Pervasive UB: `&self` -> `&mut` casts (subgamegetters.rs, systems.rs:117, mod.rs:105), `deliver_signals` iterates `pending_signals` through an aliased `&mut` while handlers can push to it (Vec reallocation under iteration = use-after-free potential).
- Messages sent from handlers during delivery are lost (cleared) or unsafe.
- Stringly-typed names and `Any` payloads, unwrap-downcast panics; typos silently drop messages.
- HashMap ordering makes system/handler order nondeterministic; no priority/ordering control.
- No threads/channels means no off-main-thread work (asset loading, input thread); `Send` bound is vestigial. Busy loop wastes CPU.
- Error in one handler aborts the rest of the queue.

What to copy: the deferred-queue-delivered-at-frame-start idea with name+target addressing, per-system subscription sets, and `just_pressed` input semantics. What to change: use typed events (per-type `Vec<T>` queues or an enum), double-buffer the queue (swap before delivery so handler-sent messages go to the next frame), take `&mut self` borrowing from split fields instead of raw-pointer casts, use ordered `Vec`/`IndexMap` for systems, and intern names to ids to avoid string allocations.

## Appendix B: efeengine analysis

### efeengine message passing (read-only analysis; paths under crates/core/src)

## 1. Types, producers, consumers
- `Message` enum (engine/messages.rs:34-49): SetFrameRate, SetThreads, Start, Stop, Kill, Log(LogMsg), Say, ChangeTitle, SetCanvasId (wasm only), Null. Serde externally tagged; JS shape pinned by `message_json_shape` (lib.rs:64). Producers: JS (`#[wasm_bindgen] send_message`, messages.rs:58), workers/any thread (`message_sender()`), Rust via `Engine::sender()` (engine.rs:434). Sole consumer: `Engine::drain_messages` (engine.rs:517). Also exposed to systems as `Events<Message>`.
- `Events<T>` (events.rs:12): generic typed queue, `EventRegistry` TypeId map of `Box<dyn AnyEvents>` (events.rs:124). Producers/consumers: any system via `Ctx::send` / `Ctx::events::<T>()` (schedule.rs:75-88).
- `InputEvent`/`InputEventKind`/`WindowEvent` (input/events.rs:22-85): produced by SDL/DOM backends and `Engine::inject` (engine.rs:439); consumed only by the engine (`handle_window_event`, `InputState::apply`, engine.rs:593, 631). They are NOT put into `Events<T>`; systems read the folded `Ctx::input` state, so there is no per-event input stream (no text/key event iteration besides InputState).
- `Commands` (commands.rs:53): `EngineCommand` control list (Quit, SetTitle, SetFrameRate, cursor/text input) plus `WorldOp` deferred ops (Spawn/Despawn/Insert/Remove/InsertRaw/RemoveRaw). Producers: systems and scripts (host `commands`, appended in `Scripting::leave`, scripting/mod.rs:244). Consumer: `Engine::apply_commands` (engine.rs:531).
- `ScriptEvent {name, data: Vec<u8>}` (scripting/mod.rs:43): sent by scripts via `engine.events.send` (api_engine.rs:384) or Rust `Ctx::send`; read by scripts via `engine.events.count/read` and Rust systems.
- `Jobs {Job(Box<dyn FnOnce+Send>), Kill}` (thread_pool.rs:30): pool job queue. `LogCommand` (log.rs:45): worker logger queue.
- Mod trampolines (scripting/mods.rs:152-200): a mod's declared `provide` function is registered as a host fn (E8; this replaced `engine.mods.export`, see `crates/efescript/docs/e8-design.md` D6) that calls `run_export(id, f, a.raw())` and copies the raw u64 results back. Data crossing is raw u64 args only; no message passing, synchronous nested call.

## 2. Transport
- Global `MESSAGE_SYSTEM`: `Lazy<flume::unbounded>` static (messages.rs:14), MPMC technically, drained only by the main thread. Per-engine `msg_tx/msg_rx` also `flume::unbounded` (engine.rs:185). Headless engines read only their own queue (engine.rs:519).
- Events: two Vecs per type (`readable`, `pending`), `update()` swaps via `mem::take` (events.rs:62). Unbounded, single-threaded (no Send needed).
- Commands: two plain Vecs, unbounded, single-threaded, `WorldOp` holds `EntityBuilder` (heap alloc per op).
- Window: `GameWindowManager.injected: VecDeque` (window/mod.rs:74); web: `Rc<RefCell<VecDeque<InputEvent>>>` filled by DOM closures, taken wholesale in poll (web_backend.rs:26, 625); SDL: SDL event pump polled directly (sdl_backend.rs:143).
- Thread pool: one shared `flume::unbounded<Jobs>` MPMC channel, N workers `rx.recv()` (thread_pool/native.rs:23-75); dedicated workers each get their own unbounded channel. Result return via `oneshot` (`JobHandle`, thread_pool.rs:220). Resize uses Kill jobs + `KillGate` atomic counter (thread_pool.rs:135) so stale kills are ignored.
- Logger: global `GLOBAL_LOGGER` (log.rs:8) = dedicated worker thread with `flume::unbounded<LogCommand>`; `LogCommand::Change` swaps backend; `LoggerSender::drop` sends Shutdown.
- JS bridge: wasm-bindgen `send_message(JsValue)` -> serde_wasm_bindgen -> global queue. Reverse direction (Rust -> JS) only through the console logger and `Message::Log` handling; no JS-callback channel.
- Script host: events_in `Vec<ScriptEvent>`, events_out capped at `MAX_EVENTS = 4096` (host.rs:30, 187, excess silently dropped); logs capped (MAX_LOG_LINES).

## 3. Threading
- Native: engine, SDL window and event pump on main thread (`InnerWindow` is deliberately !Send, window/mod.rs:20). Workers: N from `worker_count(cores)` leaving 1-4 cores reserve, each `catch_unwind` around jobs (native.rs: Worker::new). Only `Message` crosses threads, plus `Jobs`/`LogCommand`.
- Wasm: engine runs on the main thread in a rAF loop (engine.rs:756), DOM listeners feed a local queue. Web Workers share wasm memory (SharedArrayBuffer, needs COOP/COEP); worker script is a blob that imports the wasm JS glue and calls `worker_loop(ptr)` (thread_pool/web.rs, worker.js). Pool has zero workers if `wasm_js_path` unset (jobs queue forever, `join` blocks). Workers cannot touch DOM/world; they send `Message`s.
- Script VM and world are single-threaded on the main thread; world is `mem::take`n into the host during script runs (scripting/mod.rs:233).

## 4. Ordering and timing (engine.rs:620-690, docs sec. 9)
frame: time.advance -> `events.update_all()` -> input.begin_frame -> poll window (injected first) -> window handling + `input.apply` -> poll gamepads -> `drain_messages` -> Startup(once) -> PreUpdate -> FixedUpdate x n -> Update (script system first) -> PostUpdate -> RenderExtract -> render. `apply_commands` runs after every stage.
- Messages: sent any time, handled at the start of next frame's drain (<= 1 frame latency), made readable the same frame via `extend_readable` (events.rs:69), after the swap. Not double-buffered: Message events live one frame only.
- `Events<T>`: send during frame N readable during all of N+1; note sends during a stage are not visible to later stages of the same frame, and a reader that skips a frame misses nothing but reads only once per swap.
- Commands: structural changes visible to the next stage (spawn reserves entity immediately). Kill (Message/commands.quit()/CloseRequested) is checked after poll, drain, each stage and fixed step; skips remaining stages and render, runs Shutdown once (engine.rs:462).
- Back-pressure: none. All queues unbounded; the drain takes everything queued at that moment (`flume::Drain`, messages.rs:28, snapshot semantics). Script events are the only capped queue (4096/frame, silent drop). `Stop` ends the loop on both platforms; `Start` after Stop does not resume (docs sec. 11).
- Channels full/closed: unbounded so never full; sends use `let _ =`; statics keep receivers alive so sends cannot fail (messages.rs:23). Closed pool channels are ignored (`let _ = send`).

## 5. Typing and dispatch
- New control message: add variant to `Message` (messages.rs), handle in `handle_message` (engine.rs:479, exhaustive match so compiler enforces), JS gets it automatically through serde; `ts_rs::TS` import exists but derive is not applied (messages.rs:3, unused).
- New typed event: any `'static` type; `engine.add_event::<T>()` optional (registers early), `ctx.send(ev)` registers on first send; readers call `ctx.events::<T>().iter()`. No `EventReader` cursors, no per-system "already read" state; ordering across types is undefined.
- Systems are `Box<dyn FnMut(&mut Ctx)>` per stage, insertion order, no dependencies/parallelism (schedule.rs:92-122).
- New command: add `EngineCommand` variant + `Commands` method + arm in `apply_commands`.
- Scripts subscribe by string name; payload is bytes (no schema, no typed decode).

## 6. Errors and shutdown
- Dead-entity commands logged at debug, never panic (commands.rs:160-188); raw component errors logged warn.
- Script errors: traps/out-of-budget logged, module skipped that frame; `run_export` taken-instance prevents reentrancy (`Busy`). Mod exports: errors -> `Trap::Host(String)`.
- Native worker panics caught (catch_unwind). Web `worker_loop` has no catch (JS try/catch in worker.js only logs), a panicking job ends the worker and never sets `done`, so the pool count drifts.
- Dropping `WorkerHandle` sends Kill after queued jobs; pool Drop kills and joins (native.rs:99-130). `JobHandle::join` uses `.recv().unwrap()` and panics if the job panicked (sender dropped) (thread_pool.rs:229).
- Logger Shutdown is sent on `LoggerSender` drop, but the global is a static, so it is never dropped; pending log lines can be lost at process exit.
- `Message::Kill`/CloseRequested -> Shutdown stage once -> loop returns; wasm drops the engine and closure via microtask (engine.rs:765-815). Messages still in the queue after Kill are discarded.

## 7. Allocation and performance
- Every `Message` is an enum with Strings (`Say`, `Log`, `ChangeTitle`), allocated per send; `drain_messages` collects into a fresh Vec each frame (engine.rs:518) and again moves into events. Handled by reference then moved (no clone).
- Script path clones: `ScriptEvent` list cloned from `Events` every frame (system.rs:57; engine.rs:336) even when empty/no scripts (system returns early only if no module loaded), `InputState` cloned via `clone_from` and `TypeRegistry` cloned on generation change (scripting/mod.rs:233-240); `events.read` clones payload per call; `read_bytes`/`read_str` allocate per host call; `Script.entry` String cloned per entity per frame (system.rs:132); `jobs` Vec rebuilt per frame; `check_sig` per call.
- `Events::update` replaces the Vec with `mem::take`, so the old readable buffer's capacity is freed every frame (reallocation churn); a swap would reuse capacity.
- `EventRegistry::update_all` iterates a HashMap of boxed trait objects every frame; `get_or_empty` is a hash lookup + downcast per read.
- `Commands`: one heap `EntityBuilder` per spawn/insert; `apply_commands` collects control cmds into a new Vec (engine.rs:533).
- Job submission: Box per job + oneshot per `spawn`; logger formats and allocates a String per call (`to_string`), `debug_info` in the worker loop calls format! per job even though gated by cfg!(feature) inside the callee (`format!` evaluated regardless, thread_pool/native.rs).
- Global statics: MESSAGE_SYSTEM, GLOBAL_POOL, GLOBAL_LOGGER, CORES, wasm js path. Multiple engines in one process share pool/logger/global queue; only one non-headless engine should drain the global queue.

## 8. Strengths / weaknesses / gaps
Strengths: simple, deterministic frame order; headless-testable (own queue + `inject` + `run_frames`); lock-free hot paths; well-defined 1-frame event latency; resilient pool resize; deferred commands avoid iterator invalidation; script execution budgeted and sandboxed by namespace allowlist.
Weaknesses/gaps:
- No back-pressure, bounds or drop policy (except script events); a flooding worker can grow memory unbounded and stall a frame on drain.
- InputEvent is not a typed `Events` stream; no way for systems to see raw key/text/touch order or timestamps; no reply channel from engine to JS/workers.
- `Message` conflates control and logging; no request/response; `Message::Log`/`Say` go through the engine logger, itself backed by another queue.
- Events: no readers/cursors, no same-frame delivery for system-to-system events, no ordering between types, events dropped if not read within a frame; no `Drain`/clear API for consumers; stage-level event latency forces one-frame delay even for FixedUpdate multi-step (all steps see the same readable set).
- Message handling happens before Startup but only via a single global receiver; multiple non-headless engines race for it.
- Web: worker panic handling, zero-worker pools silently hang `join`, `join` blocks main thread (experimental warning); logger on 1-core web still uses a worker (ROADMAP "Logging" soon).
- ts_rs types unused; Stop cannot resume (documented deviation).
- Script events: string-named, bytes payload, cloned per frame, dropped silently at 4096.
