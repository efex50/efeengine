# Web template

Page shell used by `scripts/web.sh` to publish an example as a wasm web app.

## Files

- `index.html`: full-window `<canvas id="canvas">`, loads `main.js`.
- `main.js`: initialises the wasm module from `./pkg/{{CRATE}}.js`, sets the
  worker script path (relative, so it works under any sub-path), starts the
  engine, shows an error box if start-up fails.
- `favicon.ico`
- `_headers`: COOP/COEP headers in Netlify / Cloudflare Pages format.

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

The engine's thread pool needs `SharedArrayBuffer`, so every response must carry

    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp

`scripts/serve.py` does this for local use. Netlify and Cloudflare Pages pick up
`_headers` automatically. GitHub Pages cannot set custom headers; the usual
workaround is a service worker such as
[coi-serviceworker](https://github.com/gzuidhof/coi-serviceworker), copied into
the site and loaded with a `<script>` tag before `main.js` (not vendored here).
