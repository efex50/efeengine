# Design: input system and engine loop (roadmap 1.3–1.6, 1.9–1.13, 2.1–2.7)

Status: implemented, 2026-11-14 (gamepad 1.12 as built, see section 11). Crate: `efeengine-core` (`crates/core`). This is the
contract every implementer works against. Where the code on disk disagrees with
this file, this file wins unless a compile or API fact makes it impossible; then
the deviation is reported, not hidden.

Baseline (before the change, verified): native and wasm `cargo check` pass, `cargo test -p efeengine-core`
passes (9 unit tests + 1 doctest), sdl3 0.14.42, web-sys 0.3.83.

## 0. Rules for all of it

- Platform-neutral types contain no `sdl3` or `web_sys` types.
- Module gating in code uses `target_family = "wasm"`; Cargo tables use `target_arch = "wasm32"` (keep each file's existing style).
- The engine, window and input live on the main thread. Nothing here needs `Send`, except `Message` (global flume channel).
- No new `unwrap`/`expect` on fallible platform calls in new code; log and continue, or return `EngineError`.
- Every wave must leave these green:
  - `cargo check -p efeengine-core`
  - `cargo check -p efeengine-core --target wasm32-unknown-unknown -Z build-std=panic_abort,std`
  - `cargo test -p efeengine-core`
  - `cargo check --workspace --exclude xtask` is not required; `cargo check -p test_example` is.

## 1. Module layout

```text
engine/input.rs            pub mod keys, events, state; re-exports everything
engine/input/keys.rs       KeyCode, Key, NamedKey, Modifiers, ElementState, MouseButton
engine/input/events.rs     InputEvent, InputEventKind, WindowEvent, WindowId, WheelUnit, TouchPhase
engine/input/state.rs      InputState (+ unit tests)
engine/window/events.rs    DELETED (WindowEvents, KeyMod, PhysicalKey/LogicalKey aliases go away)
engine/time.rs             Time
engine/error.rs            EngineError
engine/config.rs           EngineConfig (next to StartupConfig)
engine/resources.rs        Resources (type map)
engine/events.rs           Events<T>, EventRegistry
engine/schedule.rs         Stage, Schedule, Ctx, EngineCommands
engine/window/headless.rs  HeadlessWindow
```

`prelude.rs` exports the input types, `Time`, `EngineError`, `EngineConfig`, `Stage`, `Ctx`, `EngineCommands`, `Events`, `Resources` (explicit re-exports, see section 11).

## 2. Event types (1.3)

All derive `Debug, Clone, PartialEq`; the small ones also `Copy, Eq, Hash`. No serde.

```rust
pub struct WindowId(pub u32);
impl WindowId { pub const MAIN: WindowId = WindowId(0); }
// Both backends report the main window as MAIN. Native: SDL id of main window -> MAIN,
// any other SDL id -> WindowId(sdl_id). Events without a window (touch) use MAIN.

pub struct InputEvent {
    pub timestamp_ns: u64,   // monotonic, ns; origin = platform start (SDL init / page time origin).
                             // Comparable only within one run. Never wall-clock.
    pub window: WindowId,
    pub kind: InputEventKind,
}
impl InputEvent { pub fn new(kind: InputEventKind) -> Self /* ts 0, MAIN; for tests */ }
impl From<InputEventKind> for InputEvent

pub enum InputEventKind {
    Key { physical: KeyCode, logical: Key, state: ElementState, repeat: bool, mods: Modifiers },
    Text(String),
    TextEditing { text: String, cursor: i32, len: i32 }, // IME preedit; raw only (events()), never in InputState::text()
    MouseMotion { pos: (f32, f32), delta: (f32, f32) },
    MouseButton { button: MouseButton, state: ElementState, pos: (f32, f32) },
    MouseWheel { delta: (f32, f32), unit: WheelUnit },
    Touch { id: u64, phase: TouchPhase, pos: (f32, f32) },
    Window(WindowEvent),
}
pub enum WindowEvent {
    Resized { width: u32, height: u32 },   // physical pixels
    ScaleFactorChanged { scale: f64 },
    Focused(bool),
    CloseRequested,
}
pub enum ElementState { Pressed, Released }
pub enum MouseButton { Left, Middle, Right, Back, Forward, Other(u8) }
pub enum WheelUnit { Line, Pixel, Page }
pub enum TouchPhase { Started, Moved, Ended, Cancelled }
```

The roadmap sketch's enum is `InputEventKind`; `InputEvent` wraps it so the timestamp
and window are not repeated in every variant. `poll_events(&mut self, out: &mut Vec<InputEvent>)`
and `apply(&InputEvent)` keep the roadmap signatures.

Conventions (identical on both platforms):
- Positions and deltas: physical pixels, origin top-left of the window/canvas, y down.
- Wheel: `+y` = scroll up / away from the user, `+x` = right. Natural-scrolling flips are undone by the backend.
- `repeat` is always `false` for `Released`.

### Keys

- `KeyCode` = **physical** key only (position, layout-independent). Existing variant names stay. Add:
  `IntlBackslash, IntlRo, IntlYen, PrintScreen, ContextMenu, Power, Sleep, WakeUp, Help,
  F13..F24, NumpadEqual, NumpadComma,
  MediaPlayPause, MediaStop, MediaTrackNext, MediaTrackPrevious, MediaSelect, Eject,
  AudioVolumeMute, AudioVolumeUp, AudioVolumeDown,
  BrowserBack, BrowserForward, BrowserRefresh, BrowserStop, BrowserSearch, BrowserFavorites, BrowserHome,
  Undo, Cut, Copy, Paste, Find, Again, Select, Lang1..Lang5, Fn`.
  `Unknown` stays and means "the backend could not identify the key". The event field is
  `KeyCode`, not `Option<KeyCode>`.
- `Key` = **logical** key (layout-aware):
  ```rust
  pub enum Key { Named(NamedKey), Character(String), Unidentified }
  ```
  - `Character` holds the string the key produces on the current layout, **lowercased**
    (`"a"`, `"ç"`, `"1"`). Shifted symbols may differ between platforms (web reports `"!"`,
    SDL may report `"1"`); typed text comes from `Text` events, never from `Key`.
  - `NamedKey` follows W3C `KeyboardEvent.key` names, with `Space` as a named key:
    `Alt, AltGraph, CapsLock, Control, Fn, Meta, NumLock, ScrollLock, Shift,
    Enter, Tab, Space, ArrowDown, ArrowLeft, ArrowRight, ArrowUp, End, Home, PageDown, PageUp,
    Backspace, Delete, Insert, Clear, Escape, Pause, PrintScreen, ContextMenu, Help,
    Copy, Cut, Paste, Undo, Redo, Find, Select,
    F1..F24,
    MediaPlayPause, MediaStop, MediaTrackNext, MediaTrackPrevious, Eject,
    AudioVolumeMute, AudioVolumeUp, AudioVolumeDown,
    BrowserBack, BrowserForward, BrowserRefresh, BrowserStop, BrowserSearch, BrowserFavorites, BrowserHome,
    Power, Sleep, WakeUp, Dead, Process`.
- `Modifiers(u16)`: own bit layout (not SDL's), hand-written flags (no `bitflags` dependency):
  `LSHIFT, RSHIFT, LCTRL, RCTRL, LALT, RALT, LMETA, RMETA, CAPS_LOCK, NUM_LOCK, ALT_GRAPH`,
  plus `NONE`. Methods: `shift()`, `ctrl()`, `alt()`, `meta()` (either side), `contains`,
  `is_empty`, `insert`, `remove`, `BitOr`, `BitOrAssign`, `BitAnd`, `Default` (= NONE).
  Web cannot tell the side; it sets the left bit unless the event key itself is the right one.

Removed: `WindowEvents`, `KeyMod`, `raw: u16`, `PhysicalKey`/`LogicalKey` aliases, `From<sdl Keycode> for KeyCode`.

## 3. `InputState` (1.4)

Lives at `EngineState.input`. No window needed; fully unit-tested.

```rust
impl InputState {
    pub fn new() -> Self;                       // focused = true
    pub fn begin_frame(&mut self);              // clears just_*, mouse_delta, wheel, text, events
    pub fn apply(&mut self, ev: &InputEvent);

    pub fn pressed(&self, k: KeyCode) -> bool;
    pub fn just_pressed(&self, k: KeyCode) -> bool;
    pub fn just_released(&self, k: KeyCode) -> bool;
    pub fn any_pressed(&self) -> bool;
    pub fn pressed_keys(&self) -> impl Iterator<Item = KeyCode> + '_;

    pub fn mouse_pressed(&self, b: MouseButton) -> bool;
    pub fn mouse_just_pressed(&self, b: MouseButton) -> bool;
    pub fn mouse_just_released(&self, b: MouseButton) -> bool;
    pub fn mouse_pos(&self) -> Option<(f32, f32)>;   // None until the first motion/button event
    pub fn mouse_delta(&self) -> (f32, f32);         // summed over the frame
    pub fn wheel(&self) -> (f32, f32);               // summed over the frame, in lines

    pub fn touches(&self) -> impl Iterator<Item = (u64, (f32, f32))> + '_;
    pub fn text(&self) -> &str;                      // all Text events of the frame, concatenated
    pub fn modifiers(&self) -> Modifiers;
    pub fn focused(&self) -> bool;
    pub fn events(&self) -> &[InputEvent];           // raw events applied this frame, in order
}
```

Semantics (each one has a test):
1. `Pressed`, not repeat, key not held: held + `just_pressed`.
2. `Pressed` with `repeat == true`: never `just_pressed`. If the key was not held, it becomes held silently.
3. `Pressed`, not repeat, key already held (duplicate): no `just_pressed`.
4. `Released`: removed from held; `just_released` only if it was held.
5. Press and release in the same frame: `just_pressed` and `just_released` both true, `pressed` false.
6. `KeyCode::Unknown` is never tracked (but the event is still in `events()`).
7. `mods` of every Key event replaces `modifiers()`.
8. `Focused(false)`: every held key and mouse button becomes `just_released` and is cleared;
   modifiers reset; touches cleared; `focused() == false`. A later `Released` for those keys is a no-op.
9. Wheel units to lines: `Line` ×1, `Pixel` ÷ 40.0, `Page` × 20.0 (named constants).
10. `MouseMotion` sets `mouse_pos` and adds to `mouse_delta`; `MouseButton` also sets `mouse_pos`.
11. Touch `Started`/`Moved` upsert, `Ended`/`Cancelled` remove.
12. `Window(Resized | ScaleFactorChanged | CloseRequested)` do not change `InputState` (the engine handles them).

UI capture (added later): `InputState::set_capture(InputCapture { pointer, keyboard })` /
`capture()` (settable through `&InputState`; persists until set again, a UI plugin sets it each
frame). Raw queries are unchanged; `InputState::game_input()` (also `Ctx::game_input()` /
`ParCtx::game_input()`) returns a `GameInput` view with the same key / mouse / wheel / text
queries that reports not pressed, no movement, `None` position and empty text for a captured
device. Code in `input/capture.rs`.

## 4. Engine-owned input path (1.5)

- `InnerWindow::poll_events(&mut self, out: &mut Vec<InputEvent>)`; same change on the
  `WindowManager` trait and `GameWindowManager`. The duplicated inherent
  `GameWindowManager::poll_events` is removed (keep the trait method only).
- `GameWindowManager` gets `inject(&mut self, ev: InputEvent)`: a small queue drained into `out`
  before the backend's own events. Used by tests and headless mode.
- `Message::WindowEvent` is removed. `Message` keeps control commands and the JS bridge.
  The `message_json_shape` test must still pass.
- `Engine` keeps a reusable `event_buf: Vec<InputEvent>`.
- Frame order (final form after Phase 2, see §9):
  `input.begin_frame()` → `poll_events(&mut buf)` → for each event: engine window handling
  (`Resized` → `state.window.size` + `resize_surface`; `ScaleFactorChanged`; `Focused`;
  `CloseRequested` → Kill) then `input.apply(ev)` → control messages → stages → render.
- Backends still emit the initial `Resized` + `ScaleFactorChanged` on the first poll.

## 5. SDL3 backend (1.6, 1.11, 1.12 native half)

`sdl_backend.rs` owns the event loop; `sdl_impls.rs` owns pure conversion functions
(no `From` impls on shared types needed; plain `pub(super) fn`s are fine).

- Store a `VideoSubsystem` in `SdlWindowManager` (needed for text input).
- Text input: `video.text_input().start(&window)` at creation. Trait method
  `InnerWindow::set_text_input(&mut self, enabled: bool) {}` (default no-op) toggles it.
  `set_text_input_area(x, y, w, h, cursor)` (logical px, default no-op; SDL `set_rect`) places the IME
  popup; systems call `cx.commands.set_text_input_area(x, y, w, h, cursor)`. SDL `TextEditing` events
  become `InputEventKind::TextEditing`.
- Cursor icon and clipboard: `cx.commands.set_cursor_icon(CursorIcon)` queues `SetCursorIcon`; `WindowManager::set_cursor_icon`
  (default no-op) is only called when the icon changes (`GameWindowManager::cursor_icon()` reads the last one). SDL uses cached
  system cursors (no grab cursors: `Grab` = hand, `Grabbing` = move); web sets the canvas `cursor` CSS. `window::clipboard_text()` /
  `set_clipboard_text(&str)` (also `efeengine_core::clipboard`) are main-thread functions: SDL clipboard when SDL video is up,
  else an in-process store (headless tests round-trip); on the web `None` / `false` (DOM inputs handle copy / paste).
- Input filter: `Engine::add_input_filter(impl InputFilter)`; each raw event is offered to every filter
  (add order) before `InputState::apply`; any `Filter::Captured` sets `InputEvent::captured`. Captured events
  still reach `InputState` (raw queries) but `game_input()` ignores them. A key / button belongs to whoever
  got its press (repeats do not change that) and the release follows the owner, so nothing sticks.
- Keys: `physical` from Scancode (exhaustive match, extended for the new `KeyCode`s:
  NonUsBackslash→IntlBackslash, International1→IntlRo, International3→IntlYen, Application/Menu→ContextMenu, …).
  `logical`: named keys from the SDL `Keycode`; printable keys → `Character` from the keycode's
  Unicode value (`keycode as i32`, no `SDLK_SCANCODE_MASK` bit), lowercased. When sdl3 returns
  `keycode: None` (non-ASCII layouts, e.g. Turkish `ş ğ ı`), fall back to
  `sdl3_sys` `SDL_GetKeyFromScancode(scancode, modstate, false)` and build the `Character`
  from the raw value; `Unidentified` only if that fails too.
- `repeat` passed through for KeyDown; `false` for KeyUp.
- Mouse: SDL coordinates are logical window units; multiply by `size_in_pixels / size`
  to get physical pixels. Buttons: Left/Middle/Right/X1→Back/X2→Forward. Wheel: `unit: Line`,
  negate both axes when `direction == Flipped`.
- Touch-synthesised mouse events must not duplicate touch: set the hint
  `SDL_TOUCH_MOUSE_EVENTS = "0"` (or skip events whose `which` is the touch mouse id).
- Touch: `FingerDown/Motion/Up` (and cancel if this sdl3 version has it); `id = finger_id`;
  `pos = normalised × size_in_pixels`.
- Close: `Quit` and `Window::CloseRequested` both map to `CloseRequested`, but at most **one**
  is emitted per poll.
- Timestamps: SDL's `timestamp` (ns) passed through; for the synthetic initial events use
  the SDL ns tick counter (or 0 if not exposed).
- 1.11: `scale_factor()`, `logical_size()`, `set_cursor_visible(bool)`, `set_cursor_grab(bool)`
  (relative mouse mode) on `InnerWindow` (default no-ops) and `WindowManager`.
- 1.12 gamepad: lowest priority of the whole batch (see section 11). Design: `InputEventKind` is **not**
  extended; a separate `GamepadState` in `input/gamepad.rs` polled once per frame
  (`buttons`, `axes` per pad id). May be left unimplemented if time runs out; say so.

## 6. Web backend (migration of 1.8, then 1.9–1.11)

- Migration (required for 1.3–1.5 to compile): `Shared.queue` holds `InputEvent`;
  `poll_events` does `sync_canvas_size()` then `out.extend(queue.take())` (keep the
  take-then-process pattern). `physical_key` gains the new codes and returns `KeyCode`
  (`Unknown` for unidentified). `logical_key(key, location) -> Key`: named table →
  `Named`; one grapheme → `Character(lowercased)`; `"Dead"`/`"Process"` → named; empty or
  `"Unidentified"` → `Unidentified`.
- Held-key tracking is keyed by physical key and skips `Unknown` (fixes the collision bug).
  On blur/hidden: synthetic `Released` events, then `Focused(false)` (current behaviour).
- 1.9: `pointerdown/move/up/cancel` on the canvas with `setPointerCapture`.
  `pointerType == "touch"` → `Touch` events; mouse and pen → `MouseMotion`/`MouseButton`.
  Position = `(clientX − rect.left) × dpr`. `wheel` (non-passive, `preventDefault`):
  `deltaMode` 0→Pixel, 1→Line, 2→Page, sign flipped to the engine convention.
  `contextmenu` suppressed on the canvas; `touch-action: none` in `templates/web/index.html`.
  New web-sys features: `PointerEvent`, `MouseEvent`, `WheelEvent`, `DomRect`,
  `AddEventListenerOptions`, `MediaQueryList` (verify each name).
- 1.10: DPR changes via a `matchMedia("(resolution: …dppx)")` listener, re-armed from the per-frame poll
  (the per-frame size poll also stays as the safety net). Text: on keydown, when `key` is a single
  grapheme and no Ctrl/Meta is held (unless AltGraph is active), emit `Text(key)` (repeats included); plus
  `compositionupdate` on `window` → `TextEditing { text: data, cursor: chars, len: 0 }`, `compositionend` → empty `TextEditing` then `Text(data)`. A hidden-input IME path is out of scope; note it.
- 1.11: `set_title` also sets `document.title`; cursor visibility via canvas CSS `cursor`;
  grab via `requestPointerLock` / `exitPointerLock` (while locked, `MouseMotion.delta` comes
  from `movementX/Y`).
- 1.12 web half: poll `navigator.getGamepads()` once per frame into `GamepadState`.

## 7. Input test example (1.13)

`examples/input_test` (same Cargo shape as `test_example`, added to the workspace): one
`Update` system that logs `InputState` changes (just pressed/released keys with physical and
logical key, mouse buttons, wheel, text, focus, size). Works with `cargo xtask`/`build.py`
for native and web. Update `structure.md` files.

## 8. Phase 2 contracts

### 2.1 Errors and config
```rust
#[derive(Debug, thiserror::Error)]
pub enum EngineError { Window(String), Graphics(String), Config(String), ThreadPool(String) }

pub struct EngineConfig {
    pub title: String,                 // "efeengine"
    pub size: (u32, u32),              // (800, 600), logical
    pub vsync: bool,                   // true (consumed by the renderer in Phase 4)
    pub fixed_dt: f64,                 // 1.0 / 60.0
    pub max_fps: Option<f32>,          // Some(60.0); None = uncapped
    pub threads: Option<usize>,        // None = auto
    pub canvas_id: Option<String>,     // None = "canvas"
    pub headless: bool,                // false
}
impl Engine {
    pub fn new(title: impl Into<String>) -> Result<Engine, EngineError>;
    pub fn with_config(cfg: EngineConfig) -> Result<Engine, EngineError>;
}
```
`StartupConfig` (from JS) fills `threads` and `canvas_id` when the `EngineConfig` field is `None`.
Window creation returns `Result` instead of unwrapping. The examples and the `Engine::new` doctest are updated.

### 2.2 Time and frame loop
```rust
pub struct Time { /* private */ }
impl Time {
    pub fn delta(&self) -> f32;  pub fn delta_f64(&self) -> f64;   // clamped to 0.25 s
    pub fn fixed_delta(&self) -> f32;
    pub fn elapsed(&self) -> f64;        // sum of clamped deltas
    pub fn frame(&self) -> u64;
    pub fn alpha(&self) -> f32;          // accumulator / fixed_dt, for interpolation
}
impl Engine { pub fn frame(&mut self, now_secs: f64); }   // the one shared frame function
```
- `frame(now)` computes delta from the previous `now` (first frame: delta 0), clamps it,
  adds to the accumulator, runs at most 8 fixed steps per frame (drops the rest).
- Native: `Instant` since engine creation; frame limiter sleeps to `max_fps`
  (`Message::SetFrameRate(f)` sets it; `<= 0` = uncapped). No fixed `sleep(10ms)`.
- Web: rAF timestamp (ms → s). No `Instant`/`sleep` on wasm.
- `tick()` stays as a thin wrapper that reads the platform clock and calls `frame`.

### 2.3 Resources and `Ctx`
```rust
pub struct Resources;   // TypeId -> Box<dyn Any>
impl Resources { insert<T: 'static>, get<T>, get_mut<T>, remove<T>, contains<T>, get_or_insert_with }

pub struct Ctx<'a> {
    pub world: &'a mut EngineWorld,      // becomes hecs::World in Phase 3
    pub resources: &'a mut Resources,
    pub time: &'a Time,
    pub input: &'a InputState,
    pub window: &'a WindowState,
    pub commands: &'a mut EngineCommands,
}
impl Ctx<'_> { fn events<T: 'static>(&self) -> &Events<T> /* empty if absent */; fn send<T: 'static>(&mut self, ev: T); }
```
`EngineCommands`: a buffer of control requests applied by the engine after each stage:
`quit()`, `set_title(String)`, `set_frame_rate(f32)`, `set_cursor_visible(bool)`, `set_cursor_grab(bool)`.
(World spawn/despawn commands are Phase 3.3.)

### 2.4 Schedule
```rust
pub enum Stage { Startup, PreUpdate, FixedUpdate, Update, PostUpdate, RenderExtract, Shutdown }
impl Engine { pub fn add_system(&mut self, stage: Stage, f: impl FnMut(&mut Ctx) + 'static) -> &mut Self; }
```
Systems of a stage run in insertion order (the `EngineCommands` of a stage are applied right after it). `Startup` runs once before the first frame,
`Shutdown` once when the loop ends (Kill). `FixedUpdate` runs N times from the accumulator
with `time.delta() == fixed_delta` inside it. Removes `GameSystem`, the old `add_system`
and `GameRunner` (`gamerunner.rs` deleted).

### 2.5 Typed events
```rust
pub struct Events<T> { /* readable, pending */ }
impl<T> Events<T> { send(&mut self, T); iter(&self) -> impl Iterator<Item=&T>; is_empty; len; update(&mut self) }
```
Double-buffered: `send` writes to `pending`; `update()` (called by the engine at the start
of each frame for every registered event type) makes `pending` the readable buffer and
drops the old one. So an event sent in frame N is readable during all of frame N+1, exactly once.
The engine drains flume at the start of the tick and puts those `Message`s **directly into the
readable buffer** of `Events<Message>` (same-frame), handles the control ones itself, and
systems can read them via `cx.events::<Message>()`. Event types are registered on first `send`
or with `engine.add_event::<T>()`.

### 2.6 Headless mode
- `HeadlessWindow` (`window/headless.rs`): implements `InnerWindow` with a fixed size,
  window/display handles returning `HandleError::Unavailable`, events only via `inject`.
  Compiled on all targets.
- `EngineConfig.headless = true`: uses `HeadlessWindow`, never initialises SDL, graphics or
  `env_logger` side effects that break tests, and skips rendering.
- Message queue becomes engine-owned: each `Engine` has its own flume pair,
  `engine.sender() -> flume::Sender<Message>`. The global `send_message` channel remains for
  JS and workers; only a non-headless engine drains it. Headless engines read only their own queue.
- `Engine::run_frames(n: usize, dt: f64)` drives `frame()` with a synthetic clock.
- Test: a headless engine runs 100 frames with a FixedUpdate counter and injected key events,
  asserting the fixed-step count and `just_pressed` behaviour. No SDL, no GPU.

### 2.7 Thread pool hardening
Check each bullet against the current code first (some may already be done by the recent
refactor), then: `static mut CORES` → `OnceCell`; no `todo!()` in `thread_pool/web.rs`;
shrinking never panics; dedicated workers get Kill on drop; no panic when `set_wasmjs_path`
was never called (return `EngineError::ThreadPool` or log and fall back to 1 thread; as built: log and run with zero workers, see section 11);
doc comment stating wgpu, SDL and DOM objects must stay off the pool.

## 9. Final frame order

```text
frame(now):
  time.advance(now)
  events.update()                      // all Events<T>
  input.begin_frame()
  windows.poll_events(&mut buf)        // injected first, then backend
  for ev in buf: engine window handling; input.apply(ev)
  drain messages -> Events<Message> (readable) + engine control handling
  [first frame only] Startup
  PreUpdate
  FixedUpdate × n
  Update
  PostUpdate
  RenderExtract
  apply EngineCommands (also after each stage)
  render (placeholder renderer, unchanged; skipped when headless or no surface)
  [on Kill] Shutdown, loop ends
```

Kill aborts the rest of the frame (remaining stages and render are skipped), then Shutdown runs once.

## 10. Out of scope

Renderer changes (Phase 4), hecs world (Phase 3), IME via hidden input, `cargo fmt` (0.6).

## 11. Deviations (as built)

Where the code differs from sections 1-10, and why.

- `Events::empty()` returns a `'static` empty queue through an inline-const (`const { &Events::<T>::new() }`), so readers need no allocation or stored default. An `EventRegistry` (in `events.rs`) holds every `Events<T>` type-erased in a `TypeId` map, and `Ctx` keeps the registry in a private field and exposes only `events::<T>()` / `send::<T>()`, so systems cannot touch the registry itself.
- `EngineCommands` also has `set_text_input(bool)`, because text input (SDL `text_input().start/stop`, the web `Text` switch) needed a system-side switch. `EngineState` holds `time` and `resources` next to world/window/input, so `Ctx` can borrow every field of the state disjointly.
- Native scale changes are also caught through SDL's `Display { ContentScaleChanged }` event next to the window `DisplayChanged` event, because a scale change without the window moving to another display only shows up as a display event. (The reported reason that sdl3 0.14 has no window-level `DisplayScaleChanged` could not be checked in the source; the code only shows that both events are handled.)
- Native touch never reports `TouchPhase::Cancelled`: sdl3 0.14.42 has no finger-cancel event variant (a cancel arrives as `Event::Unknown` without a finger id). The web backend does report it (`pointercancel`).
- Web: the DPR `matchMedia` listener is re-armed from the per-frame poll when the DPR differs, not from inside its own closure, to avoid a self-referencing closure.
- Thread pool: `CORES` stays an `AtomicUsize` (it is written more than once, so a `OnceCell` would be wrong). Without a wasm JS path the web pool runs with zero workers and logs a warning (jobs sent to it never run, so a `join` on them blocks) instead of falling back to 1 thread, because there is no worker script to start any thread from. Dedicated web workers are not created in that case and their jobs are discarded.
- The prelude uses explicit re-exports instead of the `engine::*` / `input::*` globs, because the globs clashed on the `events` module name (`engine::events` vs `engine::input::events`).
- Headless engines skip the global thread pool, the wasm panic hook and the `log` crate logger (global state a test should not set up), and log through a `LogCrateLogger`, because the native default logger calls SDL.
- Kill (close request, `Message::Kill`, `commands.quit()`) is checked after event polling, message draining, every stage and every FixedUpdate step: it aborts the rest of the frame (remaining stages and render are skipped), then Shutdown runs once; Startup is skipped if the kill came first. `Message::Stop` pauses (`Lifecycle::Paused`): the loop keeps running and handling window events and messages, but no stage runs, nothing renders and game time stands still; `Message::Start` resumes (the clock is re-based, no big delta); Kill while paused shuts down once.
- `run_frames(n, dt)`: the first frame has delta 0, so n frames give n-1 fixed steps at `dt == fixed_dt`; the synthetic clock continues across calls.
- 1.12 gamepad: implemented as designed (separate `GamepadState`, not in `InputEventKind`). The web ignores non-"standard" mappings. `Ctx` gained `pub gamepads: &GamepadState`.
- Injected events are drained before the backend's events, so on the very first poll an injected `Resized` is followed (and overwritten) by the backend's initial `Resized`.
