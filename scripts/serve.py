#!/usr/bin/env python3
"""Static file server with COOP/COEP headers (stdlib only).

Usage: python3 -I scripts/serve.py <dir> [--port N]
"""
import argparse
import functools
import http.server
import os
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
    }

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("dir")
    p.add_argument("--port", type=int, default=8080)
    a = p.parse_args()
    if not os.path.isdir(a.dir):
        sys.exit(f"serve.py: not a directory: {a.dir}")
    handler = functools.partial(Handler, directory=a.dir)
    with http.server.ThreadingHTTPServer(("127.0.0.1", a.port), handler) as srv:
        print(f"Serving {a.dir} at http://localhost:{a.port}/ (COOP/COEP enabled)", flush=True)
        try:
            srv.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
