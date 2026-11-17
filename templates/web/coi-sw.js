// Minimal cross-origin-isolation service worker (own implementation, no third-party code).
// Static hosts that cannot set COOP/COEP headers (GitHub Pages) can use it: it re-serves every
// same-origin response with the headers that enable SharedArrayBuffer. Enabled by main.js when
// CONFIG.coiServiceWorker is true; main.js registers it and reloads the page once.
self.addEventListener("install", () => self.skipWaiting())
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()))

self.addEventListener("fetch", (e) => {
    const req = e.request
    // Chrome bug: only-if-cached requests with a non-same-origin mode throw.
    if (req.cache === "only-if-cached" && req.mode !== "same-origin") return
    e.respondWith((async () => {
        const res = await fetch(req)
        // Opaque / error responses (status 0) cannot be rewrapped.
        if (res.status === 0) return res
        const headers = new Headers(res.headers)
        headers.set("Cross-Origin-Opener-Policy", "same-origin")
        headers.set("Cross-Origin-Embedder-Policy", "require-corp")
        headers.set("Cross-Origin-Resource-Policy", "same-origin")
        return new Response(res.body, { status: res.status, statusText: res.statusText, headers })
    })().catch((err) => { console.error("coi-sw fetch failed", err); return Response.error() }))
})
