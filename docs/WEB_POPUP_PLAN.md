# Plan: multiple windows on the web as popups

Status: **TODO. Windowing on the web is Not Supported.** Every `create_window` on wasm fails with
"Not Supported: secondary windows are not available on the web yet (TODO)" (`engine.rs` `open_window`).
`WebTarget::Popup` and `WindowDesc::web_popup()` exist but are unreachable on the web. Native is unchanged.
Builds on `docs/MULTI_WINDOW.md` (canvas-based design, also Not Supported on the web for now).

## 1. Goal and non-goals

**Goal.** `engine.create_window(WindowDesc::new("Map").web_popup())` opens a real browser popup with its own
canvas, showing what a canvas window shows: its cameras' frame, its title, cursor and fullscreen. Input from the
popup reaches the engine with the popup's `WindowId`. Closing the popup or the game closes the window (`Closed`).

**Supported backend.** WebGPU only. Popups are **Not Supported on WebGL2**. Canvas windows in the page are not
affected by this rule (they keep working on WebGL2 as documented in `MULTI_WINDOW.md`).

**Non-goals (first version).**
- No second wasm instance and no second world.
- No readback to the popup on WebGL2 (decided against, see §4).
- No window placement beyond the `window.open` features string.
- No UI layers in popups in phases 1–4 (§8).

## 2. Constraints

1. **A popup is another document.** A canvas cannot move into it, and a GPU device cannot be shared across
   documents. The popup owns its canvas; the engine gets an OffscreenCanvas that presents into that canvas.
2. **`window.open` needs a user gesture.** The engine's `create_window` runs in the frame loop. The popup must be
   opened while the browser still counts the page as user-activated (`navigator.userActivation.isActive`).
3. **Structured clone cannot carry DOM events.** Input from the popup is sent as plain data.
4. **The page is cross-origin isolated** (`templates/web/_headers`: COOP `same-origin`, COEP `require-corp`).
   The popup must be same-origin so it keeps `window.opener` and can `postMessage` with a fixed `targetOrigin`.
   **Not verified in a browser yet.**
5. **The backend is decided at startup and cannot change.** `web_instance()` in `render/gpu.rs` chooses WebGPU or
   GL before any context exists. Desktop Linux uses WebGL2 by default because Chrome's WebGPU there fails to present
   (`config.rs`, `GraphicsBackend::Auto`: "Dawn `ImportMemory` errors every frame").

## 3. Public API

```rust
pub enum WebTarget {
    Body, Element(String), Canvas(String),
    /// A separate browser popup with its own canvas. WebGPU only; "Not Supported" on WebGL2.
    Popup,
}
impl WindowDesc { pub fn web_popup(self) -> Self }   // implemented
```

- Natively `WebTarget` is ignored and `web_popup()` opens a normal window (as with the other web targets).
- The popup's size is `WindowDesc::size` (the window's logical size), as for other windows.
- Errors (all `EngineError::Window`):
  - "popup windows need graphics up: open them after the game has started" (before the GPU exists; implemented)
  - "Not Supported: popup windows need WebGPU; this page runs on WebGL2 (graphicsBackend)" (implemented)
  - "popup windows are not implemented yet" (placeholder until phase 3)
  - "popup blocked; open windows from a click or key press" (phase 3)

## 4. Frame transport: OffscreenCanvas (decided)

The popup calls `transferControlToOffscreen()` on its own `<canvas>` and sends the resulting `OffscreenCanvas`
to the opener with `postMessage` (OffscreenCanvas is transferable). The engine creates a surface for it on the
shared WebGPU device with `SurfaceTarget::OffscreenCanvas` (in wgpu 30.0.2, `wgpu` `src/api/surface.rs`).
The GPU presents straight into the popup's canvas. No pixels cross the boundary.

**Why not the readback path** (the earlier plan):
- It would cost one GPU→CPU readback and one copy per frame per popup (about 500 MB/s at 1080p and 60 fps).
- It does not solve the WebGL2 problem anyway. On WebGL2 the device lives in the main canvas's GL context, and an
  OffscreenCanvas gets its own GL context. GL textures do not cross contexts, so presenting shared-device output into
  a popup on WebGL2 would also need the readback. The decision is to not build that path: popups are Not Supported
  on WebGL2.

**Consequences:**
- The popup feature works only on WebGPU. Desktop Linux defaults to WebGL2, so popups are Not Supported there
  unless the game sets `graphicsBackend: "webgpu"`, and that combination hits the Dawn present error above. This is
  the main open risk (§13).
- Two engine paths stay: canvas windows (any backend) and popup windows (WebGPU). Both use the same shared device.

## 5. Components and changes

### 5.1 Engine (Rust)

- **`engine.rs` `open_window`**: the gate (implemented, §3). Popups open only after graphics is up and only on WebGPU.
- **`render/gpu.rs`**: `GpuContext::backend()` (implemented). A new `add_window` branch for popup windows: create the
  surface with `SurfaceTarget::OffscreenCanvas(offscreen)` on the shared instance. The OffscreenCanvas is sent
  from the bridge page and stored with the window.
- **`render/extra_surface.rs`**: `ExtraSurface::for_offscreen(gpu, offscreen)` next to `for_canvas`. Same surface
  format and alpha rules (`pick_format`, `pick_alpha`). The copy path is not used for popups.
- **Sizing**: the engine sets `offscreen.width/height` to the physical size (`css * devicePixelRatio`), the same rule as
  canvases today. The placeholder's CSS size comes from the popup page. The per-frame DPR check stays.
- **`window/web_backend.rs`**: `CanvasWin` gets a popup source: the `Window` handle, the `OffscreenCanvas`, the last
  reported size and DPR. `set_title`, `set_cursor_*`, `set_fullscreen` and `close_window` post messages to the popup.
  `canvas_of(popup)` returns `None` (popups have no canvas in this document).
- **`window/mod.rs`**: `WebTarget::Popup` (done).

### 5.2 Bridge page (`templates/web/popup.html` + `popup.js`)

- Plain JS, no wasm. One full-window `<canvas>`, the same CSS reset as `main.js`, served from the same origin and
  under the same headers.
- On load it calls `canvas.transferControlToOffscreen()` and posts `{type:"surface", canvas}` (with the OffscreenCanvas
  in the transfer list) to the opener. It sends `{type:"resized", css:[w,h], dpr}` on `resize`, and `{type:"closed"}` on
  `pagehide` (best effort).
- It receives `title`, `cursor`, `fullscreen` and `close` from the opener, checking `event.source` and `event.origin`.
- It forwards input as plain data: `pointer`, `wheel`, `key`, `text` (phase 4), `focus`.
- The page never calls `getContext` on the canvas. After the transfer the canvas belongs to the engine.

### 5.3 Input bridge

- **Refactor** `handle_pointer(shared, window, e: &PointerEvent, phase)` to take a `PointerData` struct, built from either
  a `PointerEvent` (canvas) or a popup message. Same for keyboard. This is phase 1 and must not change behaviour.
- Popup messages are accepted only from the stored `Window` handle and the page origin.

## 6. Opening rules

- Open from the frame loop only when `navigator.userActivation.isActive` is true (where the API exists). Otherwise the
  request is retried for a short time and then fails with `Closed` and an error log.
- A key press counts as activation, so `M` (reopen) works. The first popup should be opened after the click-to-start
  gesture.
- Each `window.open` gets a unique name (`efe-window-<id>`), so a reopen after close never reuses a closed handle.

## 7. Lifecycle, focus, fullscreen

- **Opening**: `surface` marks the window live. Then `Created`, `Resized`, `ScaleFactorChanged`, as for canvases.
- **Closed by the user**: `pagehide` posts `closed`. The engine emits `CloseRequested`, then `Closed`, and drops the
  surface first. A fallback poll of `popup.closed` every ~500 ms covers a missed `pagehide`.
- **Closed by the game**: `close_window` drops the surface, posts `close`, then calls `popup.close()`.
- **Opener gone** (game tab closed or reloaded): the popup closes itself when `window.opener` is null.
- **Focus**: the popup's focus and blur drive `Focused` and `key_window` for its window id.
- **Fullscreen**: `requestFullscreen` needs activation in the popup itself. A request from the engine is forwarded
  to the popup; if it fails, the popup shows a "click to go fullscreen" hint.
- **Pause when hidden**: the opener's visibility decides, as now. A popup can keep its last frame while the opener is in
  the background. Documented, not fixed in v1.

## 8. UI in popups (phase 5)

The UI layer of a popup window is built in the popup document by the same mirror approach as before: the web UI backend
for that window creates its DOM in the popup's `document`, and input comes through §5.3. Phases 1–4 ship popups with no
UI layers. `ui.layer_in(popup, ...)` logs one warning and draws nothing.

## 9. Phases

| Phase | What | Done when | Size |
|---|---|---|---|
| **0. Decide** | §10 answered. | Answers recorded here. | S |
| **1. Refactor** | `PointerData` / keyboard data structs. No behaviour change. | `examples/multi_window` and `crates/core/tests/windows.rs` pass unchanged. | S–M |
| **2. Bridge page** | `popup.html` / `popup.js`: transfer, resize, close, input forwarding. Standalone test page. | Chrome with `graphicsBackend: "webgpu"` shows a frame from the test page's own OffscreenCanvas surface and forwards a click. | M |
| **3. Engine popup window** | `add_window` branch for OffscreenCanvas, `for_offscreen`, sizing, popup branches in `web_backend.rs`, activation rule (§6). | `examples/multi_window` with `web_popup()` on the Map window, on WebGPU: frames in the popup, `C` closes it, `M` reopens it. On WebGL2 it fails with "Not Supported". | L |
| **4. Input and lifecycle** | Input into `Shared` with the popup's window id; focus; closed-by-user and opener-gone; fullscreen; `text` input. | Clicks in the popup move the Map camera; closing emits `Closed`; no leaked listeners after 20 open/close cycles. | M |
| **5. UI in popups** | Mirror UI layers into the popup document (§8). | The Map label appears inside the popup. | L |
| **6. Main window on OffscreenCanvas (optional)** | Separate decision, see §15. | Only if §15's experiment shows a gain. | M |
| **7. Docs** | `MULTI_WINDOW.md` "Popups" section; `templates/web/README.md`; `ROADMAP.md`. | Docs match the code; §11 checklist recorded. | S |

Phases 1–4 are the first useful version. Phase 5 can wait.

## 10. Decisions

Decided:
- Transport: OffscreenCanvas (§4). Readback: dropped.
- WebGL2: popups "Not Supported" (§3, implemented).

Still open:
1. Scope of v1: popups without UI (phases 1–4), or with UI? Recommendation: 1–4 first.
2. Opening trigger in the example: a key or button, or after click-to-start? Recommendation: a key, with `M` to reopen.
3. Window placement: browser placement, or `left`/`top` fields on `WindowDesc`? Recommendation: browser placement.
4. Popup page location: shared `templates/web/popup.html` (recommended) or per example.
5. Whether to run the §15 experiment at all.

## 11. Test plan

**Automated (Rust):**
- Unit tests for the `PointerData` conversion from popup messages to `InputEventKind`.
- The gate: `open_window` returns "Not Supported" on a GL adapter and "graphics up" before graphics exists. The gate is
  inside `#[cfg(target_arch = "wasm32")]`, so it is compile-checked (`cargo check --target wasm32-unknown-unknown`, done) but
  not run by native tests. Behaviour needs the browser test below.
- `crates/core/tests/windows.rs` keeps passing (headless behaviour unchanged).

**Browser (Chrome, WebGPU):** popups need WebGPU, so headless Firefox (no WebGPU, per `MULTI_WINDOW.md`) cannot test them.
- Open the Map popup, check the popup canvas is non-blank with the right size, the main canvas still draws, a synthetic
  click in the popup reaches the engine, closing the popup emits `[multi_window] map window closed`.
- Check the WebGL2 default: the same page with the default backend shows "Not Supported" in the log.
- Screenshots come from the browser (DevTools or the driver). `canvas.toDataURL()` does not work on a transferred canvas,
  so the canvas-based checks in existing drivers cannot be reused for popups.

**Manual checklist (record in §12):** Chrome on desktop Linux with `graphicsBackend: "webgpu"` (the Dawn case), and
Chrome on another OS with WebGPU if available. Firefox and Safari WebGPU status to be recorded.
- Open from a key press and from a click; a blocked popup shows the error.
- Drag to a second monitor; the frame keeps updating; resize; zoom (DPR).
- Mouse, wheel, keyboard and IME text reach the right window.
- Close with the popup's X; close the game tab; reload the game tab (no orphan popups, no console errors).
- Fullscreen in the popup; Esc leaves it and the engine gets `FullscreenChanged(false)`.
- 20 open/close cycles; check listener and memory counts in DevTools.

## 12. Results

- 2026-11-18: web windowing set to Not Supported (TODO) in `open_window`; earlier gate replaced.
- Gate, compile check: `cargo check` passes for `x86_64-unknown-linux-gnu` and `wasm32-unknown-unknown` in `crates/core`
  (done, after the change).
- Browser runs: not yet.

## 13. Risks

- **Desktop Linux**: the default backend is WebGL2, so popups are Not Supported there by default. With
  `graphicsBackend: "webgpu"`, Chrome's WebGPU present error may still break frames. Unknown until the §15 experiment or
  phase 2 runs on this machine.
- **Cross-document OffscreenCanvas transfer**: the transfer to a popup window is unverified in Chrome and in Firefox
  and Safari. If it fails, the popup feature has no fallback on WebGL2.
- **Browser WebGPU coverage**: Firefox and Safari WebGPU support depends on version and platform. Popups are limited to
  those browsers.
- **Irreversible transfer**: `transferControlToOffscreen` can run only once per canvas. A failed popup must be dropped
  with its element, not reused.
- **Popup blockers**: the activation rule (§6) can still be too late in a long frame. Keep the retry short and show the
  error.
- **COOP/COEP and `window.opener`**: unverified under the real headers (phase 2).
- **Tests**: popups cannot be tested in headless Firefox. Only Chrome with WebGPU exercises them.
- **Popup frames when the opener is hidden**: the popup freezes on its last frame. Documented.

## 14. Related

- `docs/MULTI_WINDOW.md`: canvas-based multi-window design.
- `crates/core/src/engine/render/gpu.rs`: `web_instance`, `create_surface`, `add_window`.
- `crates/core/src/engine/render/extra_surface.rs`: `ExtraSurface::from_surface` / `for_canvas`.
- `crates/core/src/engine/window/web_backend.rs`: `make_canvas`, `setup_canvas`, `Shared`, `handle_pointer`.
- `crates/core/src/engine/config.rs`: `GraphicsBackend::Auto` and the Dawn note.
- `templates/web/main.js`, `templates/web/_headers`, `templates/web/coi-sw.js`.

## 15. Option: main window on OffscreenCanvas too (cost analysis, not decided)

Today the main window on WebGPU uses `SurfaceTarget::Canvas`, a canvas that keeps its own context. Switching it to
OffscreenCanvas would mean transferring the page's `<canvas id="canvas">` once at startup and presenting through the
OffscreenCanvas.

**What it would gain:**
- One surface path for all WebGPU windows (main, popups, canvas windows that run on WebGPU).
- Possibly a different present path. Canvas presentation on WebGPU is already zero-copy, so a speed gain is not expected;
  it needs measuring, not assuming.
- The option to move rendering into a Worker later. That is a separate and much larger change (see below).

**What it would cost:**
1. **Irreversible per element.** After the transfer the main canvas has no 2D or WebGL context. The backend is fixed at
   startup, so this is fine on the main path, but any fallback to the canvas path needs a fresh element (replace the node,
   then retry). Otherwise a failed WebGPU surface leaves the page with no usable canvas.
2. **Engine code that treats the main canvas as a canvas.** `sync_canvas_size` sets `canvas.width/height`; it would set
   the OffscreenCanvas's size instead. `canvas_of(MAIN)` stays valid for input listeners, cursor and fullscreen (the
   placeholder is still in the DOM), but `getContext`, `toDataURL` and `drawImage` on it would break.
3. **Tests and tools.** Screenshot checks that read the canvas (`toDataURL`, the existing canvas-based drivers) stop
   working for the main window on WebGPU. Screenshots must come from the browser.
4. **Browser coverage.** Main-window WebGPU would depend on OffscreenCanvas WebGPU support too, which is less
   established than canvas WebGPU in Firefox and Safari. Those browsers would need the fallback in point 1.
5. **Desktop Linux.** Default is WebGL2, so this only matters with `graphicsBackend: "webgpu"`. That is the case where
   Chrome's present error happens. Whether OffscreenCanvas avoids it is unknown and is the one experiment worth running.
6. **Resize.** The placeholder's CSS size and the OffscreenCanvas buffer size must match. A mismatch shows as a stretched or
   blurry frame. The existing per-frame DPR check covers the buffer, but this is a new failure mode to test.
7. **Context and device loss.** Handled as now, but it is untested for offscreen surfaces.
8. **Worker rendering (not part of this switch).** Moving the engine to a Worker would also need the wasm instance, input
   forwarding, and the DOM UI backend moved or split off the main thread (UI builds DOM on the main thread). That is a large
   redesign and not justified by the switch alone.

**Recommendation:** do not switch the main window now. Build popups first (phases 1–4), where OffscreenCanvas is needed
anyway. Then run one experiment on desktop Linux with `graphicsBackend: "webgpu"`: if the popup's OffscreenCanvas
presents correctly where the main canvas does not, that is a reason to revisit the main window. Otherwise, keep the
canvas path for the main window.
