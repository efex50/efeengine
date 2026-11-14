#!/usr/bin/env bash
# Build / serve / publish an example as a wasm web app.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<USAGE
Usage:
  scripts/web.sh build <example> [--release]
  scripts/web.sh serve <example> [--release] [--port N]   (default port 8080)
  scripts/web.sh dist  <example>                          (release build, publish-ready)

Output: dist/<example>/ . Set WASM_BINDGEN to use a specific wasm-bindgen binary.
USAGE
}

die() { echo "web.sh: $*" >&2; exit 1; }

cmd="${1:-}"
[ -n "$cmd" ] || { usage; exit 1; }
shift || true
case "$cmd" in
    -h|--help|help) usage; exit 0 ;;
    build|serve|dist) ;;
    *) usage >&2; die "unknown command '$cmd'" ;;
esac

example="${1:-}"
[ -n "$example" ] || { usage >&2; die "missing <example>"; }
shift || true

release=0
port=8080
[ "$cmd" = dist ] && release=1
while [ $# -gt 0 ]; do
    case "$1" in
        --release) release=1 ;;
        --port) [ $# -ge 2 ] || die "--port needs a value"; port="$2"; shift ;;
        --port=*) port="${1#--port=}" ;;
        *) die "unknown option '$1'" ;;
    esac
    shift
done

EX_DIR="$ROOT/examples/$example"
if [ ! -f "$EX_DIR/Cargo.toml" ]; then
    avail="$(cd "$ROOT/examples" 2>/dev/null && for d in */; do [ -f "$d/Cargo.toml" ] && echo -n "${d%/} "; done || true)"
    die "unknown example '$example' (available: ${avail:-none})"
fi

# Package name from the [package] table, crate name = name with - -> _
pkg_name="$(awk '/^\[/{p=($0=="[package]")} p && /^name[ \t]*=/{gsub(/^[^"]*"|".*$/,""); print; exit}' "$EX_DIR/Cargo.toml")"
[ -n "$pkg_name" ] || die "could not read package name from $EX_DIR/Cargo.toml"
crate="${pkg_name//-/_}"
title="$example"

# --- toolchain checks -------------------------------------------------------
rust_hint="rustup toolchain install nightly --component rust-src --target wasm32-unknown-unknown"
command -v cargo >/dev/null || die "cargo not found"
if ! cargo +nightly --version >/dev/null 2>&1; then
    die "nightly toolchain missing. Install with: $rust_hint"
fi
sysroot="$(rustc +nightly --print sysroot)"
[ -f "$sysroot/lib/rustlib/src/rust/library/Cargo.lock" ] || \
    die "rust-src missing for nightly. Install with: rustup component add rust-src --toolchain nightly"
[ -d "$sysroot/lib/rustlib/wasm32-unknown-unknown" ] || \
    die "wasm32 target missing for nightly. Install with: rustup target add wasm32-unknown-unknown --toolchain nightly"

want_ver="$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/version = |"/,""); print; exit}' "$ROOT/Cargo.lock")"
[ -n "$want_ver" ] || die "wasm-bindgen not found in Cargo.lock"
wb="${WASM_BINDGEN:-wasm-bindgen}"
install_hint="cargo install wasm-bindgen-cli --version $want_ver --locked"
if ! command -v "$wb" >/dev/null 2>&1; then
    echo "web.sh: wasm-bindgen CLI not found ($wb). Install with:" >&2
    echo "  $install_hint" >&2
    exit 1
fi
have_ver="$("$wb" --version 2>/dev/null | awk '{print $2}')"
if [ "$have_ver" != "$want_ver" ]; then
    echo "web.sh: wasm-bindgen CLI version '$have_ver' != Cargo.lock version '$want_ver'. Install with:" >&2
    echo "  $install_hint" >&2
    exit 1
fi

# --- build ------------------------------------------------------------------
profile=debug
cargo_args=()
if [ "$release" = 1 ]; then profile=release; cargo_args+=(--release); fi

cd "$ROOT"
echo "==> cargo build ($profile) $pkg_name for wasm32-unknown-unknown"
cargo +nightly build --lib -p "$pkg_name" --target wasm32-unknown-unknown \
    -Z build-std=panic_abort,std ${cargo_args[@]+"${cargo_args[@]}"}

target_dir="$(cargo +nightly metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
wasm="$target_dir/wasm32-unknown-unknown/$profile/$crate.wasm"
[ -f "$wasm" ] || die "expected wasm not found: $wasm"

OUT="$ROOT/dist/$example"
rm -rf "$OUT"
mkdir -p "$OUT/pkg"
echo "==> wasm-bindgen --target web"
"$wb" --target web --out-dir "$OUT/pkg" --out-name "$crate" "$wasm"

# --- assemble site ----------------------------------------------------------
echo "==> assembling $OUT"
tpl="$ROOT/templates/web"
for f in "$tpl"/* "$tpl"/.[!.]*; do
    [ -f "$f" ] || continue
    name="$(basename "$f")"
    [ "$name" = README.md ] && continue
    if grep -Iq . "$f"; then
        CRATE="$crate" TITLE="$title" python3 -I -c '
import os,sys
s=open(sys.argv[1],encoding="utf-8").read()
s=s.replace("{{CRATE}}",os.environ["CRATE"]).replace("{{TITLE}}",os.environ["TITLE"])
open(sys.argv[2],"w",encoding="utf-8").write(s)' "$f" "$OUT/$name"
    else
        cp "$f" "$OUT/$name"
    fi
done

ov="$EX_DIR/web"
if [ -d "$ov" ]; then
    (cd "$ov" && find . -path ./pkg -prune -o -type f -print) | while read -r f; do
        mkdir -p "$OUT/$(dirname "$f")"
        cp "$ov/$f" "$OUT/$f"
        echo "    override: $f"
    done
fi

case "$cmd" in
    build) echo "Built: $OUT" ;;
    dist)  echo "Publish-ready folder: $OUT" ;;
    serve)
        echo "==> serving on http://localhost:$port/"
        exec python3 -I "$ROOT/scripts/serve.py" "$OUT" --port "$port"
        ;;
esac
