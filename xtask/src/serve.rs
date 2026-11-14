//! Minimal static file server for web builds.
//!
//! Every response carries the cross-origin isolation headers (COOP/COEP/CORP)
//! that SharedArrayBuffer, and therefore wasm threads, need.

use std::fs::File;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow};
use tiny_http::{Header, Method, Response, Server};

use crate::util::{dry_run, step};

const HEADERS: &[(&str, &str)] = &[
    ("Cross-Origin-Opener-Policy", "same-origin"),
    ("Cross-Origin-Embedder-Policy", "require-corp"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    ("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0"),
];

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "json" | "map" => "application/json",
        "css" => "text/css; charset=utf-8",
        "ts" | "txt" | "md" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ttf" => "font/ttf",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid ASCII")
}

/// Map a request URL to a file under `dir`, refusing anything that escapes it.
fn resolve(dir: &Path, url: &str) -> Option<PathBuf> {
    let path = url.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_decode(path)?;
    let mut out = dir.to_path_buf();
    for comp in Path::new(decoded.trim_start_matches('/')).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.is_dir() {
        out.push("index.html");
    }
    out.is_file().then_some(out)
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Serve `dir` on `host:port` until the process is interrupted.
pub fn serve(dir: &Path, host: &str, port: u16) -> Result<()> {
    let shown_host = if host == "127.0.0.1" || host == "0.0.0.0" { "localhost" } else { host };
    step(format!("serving {} on http://{shown_host}:{port}/ (COOP/COEP enabled, Ctrl+C to stop)", dir.display()));
    if dry_run() {
        println!("[dry-run] http server on {host}:{port} for {}", dir.display());
        return Ok(());
    }
    if !dir.is_dir() {
        return Err(anyhow!("not a directory: {} (build it first)", dir.display()));
    }
    let server = Server::http((host, port)).map_err(|e| anyhow!("cannot listen on {host}:{port}: {e}"))?;
    for req in server.incoming_requests() {
        let method = req.method().clone();
        let url = req.url().to_string();
        let result = match (&method, resolve(dir, &url)) {
            (Method::Get | Method::Head, Some(path)) => match File::open(&path) {
                Ok(f) => {
                    let mut resp = Response::from_file(f).with_header(header("Content-Type", content_type(&path)));
                    for (k, v) in HEADERS {
                        resp.add_header(header(k, v));
                    }
                    println!("{} {url} 200", method);
                    req.respond(resp)
                }
                Err(e) => {
                    println!("{} {url} 500 ({e})", method);
                    req.respond(Response::from_string("internal error").with_status_code(500))
                }
            },
            (Method::Get | Method::Head, None) => {
                println!("{} {url} 404", method);
                let mut resp = Response::from_string("not found").with_status_code(404);
                for (k, v) in HEADERS {
                    resp.add_header(header(k, v));
                }
                req.respond(resp)
            }
            _ => req.respond(Response::from_string("method not allowed").with_status_code(405)),
        };
        if let Err(e) = result {
            eprintln!("response error for {url}: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_path_traversal() {
        let dir = std::env::temp_dir();
        assert!(resolve(&dir, "/../etc/passwd").is_none());
        assert!(resolve(&dir, "/%2e%2e/etc/passwd").is_none());
    }

    #[test]
    fn decodes_percent() {
        assert_eq!(percent_decode("/a%20b.js").as_deref(), Some("/a b.js"));
        assert!(percent_decode("/bad%zz").is_none());
    }

    #[test]
    fn wasm_type() {
        assert_eq!(content_type(Path::new("x/test_bg.wasm")), "application/wasm");
    }
}
