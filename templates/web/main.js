import init, { __engine_start, __init_thread_pool, set_wasmjs_path, worker_loop } from "./pkg/{{CRATE}}.js"

function showError(err) {
    console.error(err)
    const box = document.createElement("pre")
    box.style.cssText = "position:fixed;left:0;right:0;top:0;margin:0;padding:12px;" +
        "background:#400;color:#fdd;font:14px monospace;white-space:pre-wrap;z-index:10"
    box.textContent = "Failed to start: " + (err && err.stack ? err.stack : err)
    document.body.appendChild(box)
}

async function run() {
    if (!self.crossOriginIsolated) {
        console.warn("Page is not cross-origin isolated (SharedArrayBuffer unavailable). " +
            "Serve with 'Cross-Origin-Opener-Policy: same-origin' and " +
            "'Cross-Origin-Embedder-Policy: require-corp' (see scripts/serve.py or _headers).")
    }
    try {
        await init()
        //__init_thread_pool(4);

        // Relative path: works under any sub-path (e.g. GitHub Pages /repo/).
        set_wasmjs_path("./pkg/{{CRATE}}.js")
        __engine_start()
    } catch (err) {
        showError(err)
    }
}

run()
