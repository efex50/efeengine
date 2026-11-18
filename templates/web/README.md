# Web template

Page shell used by `scripts/web.sh` to publish an example as a wasm web app.

## Files

- `index.html`: the page. A full-window `#stage` holds `<canvas id="canvas">` (the id the engine
  looks up), a fullscreen button, a notice area and the loading / click-to-start / error panels.
  Dark background, no page scroll, `touch-action: none`, context menu blocked on the canvas.
  `<div id="ui">` follows the canvas: the layer `efeengine-ui` builds its widgets in (see below).
- `main.js`: drives start-up (a `CONFIG` object at the top holds the switches):
  1. environment checks (WebAssembly, WebGPU / WebGL2, cross-origin isolation),
  2. downloads `pkg/{{CRATE}}_bg.wasm` with a progress bar (Content-Length; indeterminate when
     it is missing or compressed) and instantiates it through the wasm-bindgen glue,
  3. optional "Click to start" overlay, then `__engine_start({ wasmJsPath, ... })`,
  4. error screens, fullscreen, pause on hidden tab.
- `coi-sw.js`: minimal own-written service worker that adds COOP/COEP (see below). No third-party code.
- `embed.html`: documented iframe snippet for portals (not needed by the game itself).
- `favicon.ico`
- `_headers`: COOP/COEP/CORP headers in Netlify / Cloudflare Pages format.

## Switches (top of `main.js`, or `data-*` on `<body>`)

| `CONFIG` key | default | `<body>` attribute | meaning |
|---|---|---|---|
| `clickToStart` | `true` | `data-click-to-start="off"` | wait for a click before `__engine_start`; the click focuses the canvas and grants the user activation audio, pointer lock and fullscreen need |
| `fullscreenButton` | `true` | `data-fullscreen-button="off"` | fullscreen button (always hidden if the browser has no Fullscreen API) |
| `fullscreenShortcut` | `"Alt+Enter"` | | `""` disables it |
| `pauseWhenHidden` | `true` | `data-pause-hidden="off"` | send `Message::Stop` / `Message::Start` on `visibilitychange` |
| `coiServiceWorker` | `false` | `data-coi-sw="on"` | register `coi-sw.js` (see COOP / COEP) |

The engine start-up config (`threads`, `canvasId`, `uiLayerId`, `graphicsBackend`, ...) is `ENGINE_CONFIG`.

## UI layer (`#ui`)

`<div id="ui">` (CSS in `index.html`: `position: absolute; inset: 0; pointer-events: none`) sits
above the canvas and below the loading / error panels. The `efeengine-ui` web backend puts one
element per widget in it, adds `<style id="efui-base">` and `<style id="efui-theme">` to `<head>`
and marks the element `data-efui-layer` (the engine ignores key events from inside it). The id
comes from `uiLayerId` in `ENGINE_CONFIG` (default `"ui"`); when the page has no such element the
backend creates it inside the canvas's parent. Games without UI can drop the element. To restyle
the widgets from the page, use `!important` (node styles are inline) and note that the engine's
`<style>` elements are appended to `<head>` at start-up, after your `<link>`s; see
`docs/ui/API.md` section 12. The canvas rule is `#canvas`, so image widgets (canvases of
their own) are not stretched.

## More windows

`Engine::create_window(WindowDesc::new("Map").web_element("#sidebar"))` mounts another engine window as a canvas inside the page
element matching the selector (`web_canvas(sel)` uses an existing `<canvas>`, the default puts a box in `<body>`). The element must
exist when the window is created, so a game that does this ships its own page: put it in `examples/<name>/web/index.html` (files
there override this template's) and add the host element, see `examples/multi_window/web/index.html` (`#sidebar` with `#map-box`).
The window's size follows the element's CSS size; the window's UI layer is created next to its canvas. `docs/MULTI_WINDOW.md`.

## Behaviour notes

- Not cross-origin isolated: the engine still runs, with a thread pool of size 1
  (`docs/THREADING.md`); the page shows a notice. If the shared-memory wasm cannot be
  instantiated at all, the error screen says so and points at the headers.
- No WebGPU: the engine falls back to WebGL2 (`graphicsBackend` `auto`/`webgpu`), shown as a
  notice. With neither WebGPU nor WebGL2 (or `webgl` requested without WebGL2) an error screen is shown.
- Errors: wasm fetch / compile failure, `window.onerror`, `unhandledrejection` and Rust panics
  (`console_error_panic_hook` writes "panicked at ..." to `console.error`, which is surfaced)
  all replace the page with an error screen. Errors inside pool workers only reach the console.
- Hidden tab: browsers stop `requestAnimationFrame`, and the engine clamps the first frame delta
  (`MAX_DELTA` 0.25 s) and releases held keys on blur. Sending Stop/Start through the existing
  exported `send_message` (no engine change) freezes game time cleanly. Start is only sent for a Stop
  this page sent.
- Canvas size: CSS 100% of `#stage`; the engine sets the drawing buffer to
  `clientWidth/Height * devicePixelRatio` itself. Do not set `width`/`height` attributes.

## Placeholders

Substituted when the site is assembled (text files only):

- `{{CRATE}}`: Rust lib name (package name with `-` replaced by `_`).
- `{{TITLE}}`: page title (the example name).

## Per-example overrides

Any file in `examples/<example>/web/` is copied over the substituted template
into `dist/<example>/` (overrides win; a `pkg` directory there is ignored).
Placeholders are not substituted in override files. If the directory does not
exist the template is used as is.

## Build, serve, publish

From the repo root (needs nightly with `rust-src` and the `wasm32-unknown-unknown`
target, plus `wasm-bindgen-cli` matching the version in `Cargo.lock`):

    ./build.py build <example> web [--release]   (python build.py on Windows)
    scripts/web.sh build <example> [--release]   # same thing, directly
    scripts/web.sh serve <example> [--release] [--port N]   # default 8080
    scripts/web.sh dist  <example>                          # release build, publish-ready

Output goes to `dist/<example>/` (gitignored). Set `WASM_BINDGEN` to use a
specific wasm-bindgen binary.

## COOP / COEP

The engine's thread pool needs `SharedArrayBuffer`, so every response should carry

    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp

- Local: `scripts/serve.py` (used by `scripts/web.sh serve`) does this.
- Netlify / Cloudflare Pages: `_headers` is picked up automatically.
- Hosts without custom headers (GitHub Pages): set `coiServiceWorker: true`. `main.js` registers
  `coi-sw.js`, which re-serves same-origin responses with the headers, and reloads the page once
  (needs https or localhost; first visit costs one reload). `coi-sw.js` is our own ~25 line
  implementation modelled on the same idea as
  [coi-serviceworker](https://github.com/gzuidhof/coi-serviceworker) (MIT); no code is vendored.
- Without any of this the game runs single-threaded.

## Embedding in an iframe

See `embed.html`: `allow="fullscreen; gamepad; cross-origin-isolated; autoplay"`. For threads the
parent page must be cross-origin isolated too, and a cross-origin game must send
`Cross-Origin-Resource-Policy: cross-origin` (change `_headers`). Avoid `sandbox`; if used, give it
`allow-scripts allow-same-origin allow-pointer-lock`, otherwise workers and SharedArrayBuffer break.
