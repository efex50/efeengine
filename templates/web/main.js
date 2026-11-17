// Web page driver: loading screen, error screens, click-to-start, fullscreen, pause on hide.
// Plain JS, no dependencies. {{CRATE}} is replaced when the site is assembled.
import init, { __engine_start, send_message } from "./pkg/{{CRATE}}.js"

// ---- Switches. Each can also be overridden by a data-* attribute on <body> (value "on"/"off"). ----
const CONFIG = {
    clickToStart: true,       // wait for a click before starting (audio / pointer lock / fullscreen need it)
    fullscreenButton: true,   // show the fullscreen button (hidden anyway when unsupported)
    fullscreenShortcut: "Alt+Enter", // "" to disable; matched against Alt/Ctrl/Shift + KeyboardEvent.key
    pauseWhenHidden: true,    // send Message::Stop / Message::Start when the tab is hidden / shown
    coiServiceWorker: false,  // register coi-sw.js to get COOP/COEP on hosts without headers (GitHub Pages)
}
const attr = (name, key) => {
    const v = document.body.dataset[name]
    if (v === "on" || v === "off") CONFIG[key] = v === "on"
}
attr("clickToStart", "clickToStart"); attr("fullscreenButton", "fullscreenButton")
attr("pauseHidden", "pauseWhenHidden"); attr("coiSw", "coiServiceWorker")

// Startup config handed to __engine_start (see crates/core structure.md, StartupConfig).
const ENGINE_CONFIG = {
    // Relative path: works under any sub-path (e.g. GitHub Pages /repo/).
    wasmJsPath: "./pkg/{{CRATE}}.js",
    // threads: 4,          // optional; omitted = automatic
    // canvasId: "canvas",  // optional; default "canvas"
    // externalApi: false,  // reserved
    // graphicsBackend: "auto", // "auto" | "webgpu" | "webgl"; auto = WebGL on desktop Linux
}
const WASM_URL = "./pkg/{{CRATE}}_bg.wasm"

const $ = (id) => document.getElementById(id)
const stage = $("stage"), canvas = $("canvas")
let failed = false

// ---- Error screen ----
function showError(title, message, hint) {
    if (failed) return // keep the first (root) error
    failed = true
    for (const id of ["loading", "start"]) $(id).hidden = true
    const box = $("error")
    box.replaceChildren()
    const add = (tag, cls, text) => {
        const el = document.createElement(tag)
        if (cls) el.className = cls
        el.textContent = text
        box.appendChild(el)
    }
    add("h1", "", title)
    if (hint) add("p", "hint", hint)
    if (message) add("pre", "", message)
    box.hidden = false
}
const describe = (e) => (e && e.stack) ? e.stack : String((e && e.message) || e)

function notice(text) {
    const n = $("notice")
    n.textContent = text
    n.hidden = false
    setTimeout(() => { n.hidden = true }, 12000)
}

// Uncaught errors / rejections after start. Worker errors do not reach here; the engine logs those.
const BENIGN = /ResizeObserver loop/
addEventListener("error", (ev) => {
    if (BENIGN.test(ev.message || "")) return
    showError("The game crashed", describe(ev.error || ev.message),
        "An uncaught error stopped the game. Reload the page to try again.")
})
addEventListener("unhandledrejection", (ev) => {
    showError("The game crashed", describe(ev.reason),
        "An unhandled error stopped the game. Reload the page to try again.")
})
// console_error_panic_hook reports Rust panics through console.error ("panicked at ...").
const origConsoleError = console.error
console.error = (...args) => {
    origConsoleError.apply(console, args)
    const first = args[0]
    if (typeof first === "string" && first.includes("panicked at")) {
        showError("The game crashed (Rust panic)", first, "Reload the page to try again.")
    }
}

// ---- Loading UI ----
const setStatus = (t) => { $("status").textContent = t }
function setProgress(loaded, total) {
    const bar = $("bar")
    if (total > 0 && loaded <= total) {
        bar.classList.remove("indeterminate")
        bar.firstElementChild.style.width = (100 * loaded / total).toFixed(1) + "%"
        setStatus(`Downloading ${(loaded / 1048576).toFixed(1)} / ${(total / 1048576).toFixed(1)} MB`)
    } else {
        // No (usable) Content-Length, e.g. compressed transfer: show bytes only.
        bar.classList.add("indeterminate")
        setStatus(`Downloading ${(loaded / 1048576).toFixed(1)} MB`)
    }
}

/** Fetch the wasm with progress; returns a Response for init(). Falls back to plain fetch. */
async function fetchWasm() {
    const res = await fetch(WASM_URL)
    if (!res.ok) throw new Error(`GET ${WASM_URL} failed: ${res.status} ${res.statusText}`)
    if (!res.body || !res.body.getReader) return res
    const total = Number(res.headers.get("Content-Length")) || 0
    let loaded = 0
    const reader = res.body.getReader()
    setProgress(0, total)
    const stream = new ReadableStream({
        async pull(ctl) {
            const { done, value } = await reader.read()
            if (done) { ctl.close(); return }
            loaded += value.byteLength
            setProgress(loaded, total)
            ctl.enqueue(value)
        },
        cancel: (r) => reader.cancel(r),
    })
    // instantiateStreaming insists on this MIME type.
    return new Response(stream, { headers: { "Content-Type": "application/wasm" } })
}

// ---- Environment checks ----
function hasWebGL2() {
    try { return !!document.createElement("canvas").getContext("webgl2") } catch { return false }
}

/** Returns true if startup may continue. */
function checkEnvironment() {
    if (typeof WebAssembly !== "object") {
        showError("Browser not supported", "", "This browser has no WebAssembly support.")
        return false
    }
    const gpu = !!navigator.gpu, gl = hasWebGL2()
    const backend = ENGINE_CONFIG.graphicsBackend || "auto"
    if (!gpu && !gl) {
        showError("No graphics support",
            "WebGPU: unavailable\nWebGL2: unavailable",
            "This game needs WebGPU or WebGL2. Try a current Chrome, Edge, Firefox or Safari, " +
            "and check that hardware acceleration is enabled.")
        return false
    }
    if (backend === "webgpu" && !gpu) {
        // The engine still falls back to WebGL2, but this was explicitly requested.
        notice("WebGPU is not available in this browser; using WebGL2.")
    } else if (!gpu && backend !== "webgl") {
        notice("WebGPU is not available; using the WebGL2 fallback.")
    } else if (!gl && backend === "webgl") {
        showError("No WebGL2", "", "The game is configured for WebGL2, which this browser does not provide.")
        return false
    }
    if (!self.crossOriginIsolated) {
        // The engine does not require isolation: without SharedArrayBuffer its thread pool has size 1
        // and the game runs single-threaded (docs/THREADING.md). The wasm build itself uses shared
        // memory, so if instantiation fails below, the error screen explains the cause.
        console.warn("Not cross-origin isolated: running single-threaded. Serve with COOP same-origin + COEP " +
            "require-corp (scripts/serve.py, _headers) or enable CONFIG.coiServiceWorker.")
        notice("Not cross-origin isolated: running single-threaded.")
    }
    return true
}

// ---- Optional service worker that adds COOP/COEP (GitHub Pages etc.) ----
/** Returns true when the page is reloading and startup must stop. */
async function setupCoiServiceWorker() {
    if (!CONFIG.coiServiceWorker || self.crossOriginIsolated) return false
    if (!("serviceWorker" in navigator) || !self.isSecureContext) {
        console.warn("coi-sw: service workers need a secure context (https or localhost)")
        return false
    }
    try {
        await navigator.serviceWorker.register("./coi-sw.js")
        await navigator.serviceWorker.ready
        // Reload once so the page itself is served through the worker; guard against loops.
        if (!sessionStorage.getItem("coi-reloaded")) {
            sessionStorage.setItem("coi-reloaded", "1")
            location.reload()
            return true
        }
    } catch (e) {
        console.warn("coi-sw failed", e)
    }
    return false
}

// ---- Fullscreen (on #stage so the overlays and canvas go together) ----
function setupFullscreen() {
    const doc = document
    const supported = !!(doc.fullscreenEnabled || doc.webkitFullscreenEnabled)
    const isFs = () => !!(doc.fullscreenElement || doc.webkitFullscreenElement)
    const toggle = () => {
        const p = isFs()
            ? (doc.exitFullscreen || doc.webkitExitFullscreen).call(doc)
            : (stage.requestFullscreen || stage.webkitRequestFullscreen).call(stage)
        if (p && p.catch) p.catch((e) => console.warn("fullscreen:", e.message || e))
        canvas.focus({ preventScroll: true })
    }
    const btn = $("fs")
    if (!supported) return
    if (CONFIG.fullscreenButton) {
        btn.hidden = false
        btn.addEventListener("click", toggle)
        // Reflect exit via Esc as well; the canvas resizes by itself (engine syncs the size).
        const sync = () => { btn.title = isFs() ? "Exit fullscreen" : "Fullscreen" }
        doc.addEventListener("fullscreenchange", sync)
        doc.addEventListener("webkitfullscreenchange", sync)
    }
    const sc = CONFIG.fullscreenShortcut
    if (sc) {
        const parts = sc.split("+"), key = parts.pop().toLowerCase()
        addEventListener("keydown", (e) => {
            if (e.key.toLowerCase() === key && e.altKey === parts.includes("Alt") &&
                e.ctrlKey === parts.includes("Ctrl") && e.shiftKey === parts.includes("Shift")) {
                e.preventDefault()
                toggle()
            }
        })
    }
}

// ---- Pause when hidden ----
// Browsers already stop requestAnimationFrame in hidden tabs, and the engine clamps the time gap
// (MAX_DELTA = 0.25 s) and releases keys on blur. Stop/Start additionally freezes game time and
// systems cleanly (and any game logic reading Message events) instead of a clamped catch-up frame.
function setupPauseWhenHidden() {
    if (!CONFIG.pauseWhenHidden) return
    let sentStop = false // only resume a pause that we caused
    document.addEventListener("visibilitychange", () => {
        try {
            if (document.hidden) { send_message("Stop"); sentStop = true }
            else if (sentStop) { send_message("Start"); sentStop = false }
        } catch (e) { console.warn("pause hook failed", e) }
    })
}

// ---- Main ----
function waitForStartClick() {
    return new Promise((resolve) => {
        $("loading").hidden = true
        const panel = $("start")
        panel.hidden = false
        const go = () => { panel.hidden = true; canvas.focus({ preventScroll: true }); resolve() }
        panel.addEventListener("click", go, { once: true })
        $("play").focus()
    })
}

async function run() {
    canvas.addEventListener("contextmenu", (e) => e.preventDefault())
    // Touch scrolling / pinch on the canvas is blocked by `touch-action: none` in index.html.
    setupFullscreen()

    if (await setupCoiServiceWorker()) return
    if (!checkEnvironment()) return

    try {
        setStatus("Downloading...")
        const response = await fetchWasm()
        setStatus("Compiling...")
        await init({ module_or_path: response })
    } catch (err) {
        const isolated = self.crossOriginIsolated
        showError("Could not load the game", describe(err),
            isolated
                ? "The game files failed to download or compile. Check your connection and reload."
                : "The game files failed to download or compile. If this mentions shared memory or " +
                  "SharedArrayBuffer, the server must send COOP/COEP headers (see README) or enable the " +
                  "coi service worker. Otherwise check your connection and reload.")
        return
    }
    if (failed) return

    if (CONFIG.clickToStart) await waitForStartClick()
    $("loading").hidden = true
    setupPauseWhenHidden()
    try {
        setStatus("Starting...")
        __engine_start(ENGINE_CONFIG)
        canvas.focus({ preventScroll: true })
    } catch (err) {
        showError("The game failed to start", describe(err))
    }
}

run()
