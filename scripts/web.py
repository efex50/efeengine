#!/usr/bin/env python3
"""Build / serve / publish an example as a wasm web app (stdlib only).

Usage:
  scripts/web.py build <example> [--release]
  scripts/web.py serve <example> [--release] [--port N]   (default port 8080)
  scripts/web.py dist  <example>                          (release build, publish-ready)

Output: dist/<example>/ . Set WASM_BINDGEN to use a specific wasm-bindgen binary.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WASM_TARGET = "wasm32-unknown-unknown"


class WebError(Exception):
    pass


def log(msg):
    print(f"==> {msg}", flush=True)


def list_examples():
    d = ROOT / "examples"
    return sorted(p.name for p in d.iterdir() if (p / "Cargo.toml").is_file()) if d.is_dir() else []


def package_name(example):
    """Name from the [package] table of examples/<example>/Cargo.toml."""
    section = None
    for line in (ROOT / "examples" / example / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s.startswith("["):
            section = s
            continue
        m = re.match(r'name\s*=\s*"([^"]+)"', s)
        if section == "[package]" and m:
            return m.group(1)
    raise WebError(f"could not read package name from examples/{example}/Cargo.toml")


def wanted_wasm_bindgen():
    lines = (ROOT / "Cargo.lock").read_text(encoding="utf-8").splitlines()
    for i, line in enumerate(lines):
        if line.strip() == 'name = "wasm-bindgen"' and i + 1 < len(lines):
            m = re.match(r'version\s*=\s*"([^"]+)"', lines[i + 1].strip())
            if m:
                return m.group(1)
    raise WebError("wasm-bindgen not found in Cargo.lock")


def wasm_bindgen_status():
    """Return (path_or_None, have_version_or_None, want_version, install_hint)."""
    want = wanted_wasm_bindgen()
    hint = f"cargo install wasm-bindgen-cli --version {want} --locked"
    wb = os.environ.get("WASM_BINDGEN", "wasm-bindgen")
    path = shutil.which(wb)
    if not path:
        return None, None, want, hint
    try:
        out = subprocess.run([path, "--version"], capture_output=True, text=True).stdout.split()
        have = out[1] if len(out) > 1 else ""
    except OSError:
        have = ""
    return path, have, want, hint


def rust_status():
    """List of (problem, hint) for the nightly toolchain / rust-src / wasm target."""
    problems = []
    if not shutil.which("cargo"):
        return [("cargo not found", "install rustup: https://rustup.rs")]
    r = subprocess.run(["cargo", "+nightly", "--version"], capture_output=True, text=True)
    if r.returncode != 0:
        return [("nightly toolchain missing",
                 f"rustup toolchain install nightly --component rust-src --target {WASM_TARGET}")]
    sysroot = Path(subprocess.run(["rustc", "+nightly", "--print", "sysroot"],
                                  capture_output=True, text=True).stdout.strip())
    if not (sysroot / "lib/rustlib/src/rust/library/Cargo.lock").is_file():
        problems.append(("rust-src missing for nightly", "rustup component add rust-src --toolchain nightly"))
    if not (sysroot / "lib/rustlib" / WASM_TARGET).is_dir():
        problems.append((f"{WASM_TARGET} target missing for nightly",
                         f"rustup target add {WASM_TARGET} --toolchain nightly"))
    return problems


def check_toolchain():
    for problem, hint in rust_status():
        raise WebError(f"{problem}. Install with: {hint}")
    path, have, want, hint = wasm_bindgen_status()
    if path is None:
        raise WebError(f"wasm-bindgen CLI not found ({os.environ.get('WASM_BINDGEN', 'wasm-bindgen')}). Install with:\n  {hint}")
    if have != want:
        raise WebError(f"wasm-bindgen CLI version '{have}' != Cargo.lock version '{want}'. Install with:\n  {hint}")
    return path


def run(cmd, dry=False, **kw):
    if dry:
        print("[dry-run] " + " ".join(subprocess.list2cmdline([c]) if " " in c else c for c in cmd), flush=True)
        return
    r = subprocess.run(cmd, **kw)
    if r.returncode != 0:
        raise WebError(f"command failed ({r.returncode}): {' '.join(cmd)}")


def target_dir():
    env = os.environ.get("CARGO_TARGET_DIR")
    if env:
        return Path(env)
    r = subprocess.run(["cargo", "+nightly", "metadata", "--format-version", "1", "--no-deps"],
                       capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        raise WebError("cargo metadata failed:\n" + r.stderr)
    return Path(json.loads(r.stdout)["target_directory"])


def build(example, release=False, dry=False):
    """Build the example for wasm and assemble dist/<example>. Returns the output dir."""
    if example not in list_examples():
        raise WebError(f"unknown example '{example}' (available: {' '.join(list_examples()) or 'none'})")
    pkg = package_name(example)
    crate = pkg.replace("-", "_")
    out = ROOT / "dist" / example
    profile = "release" if release else "debug"

    if dry:
        wb = os.environ.get("WASM_BINDGEN", "wasm-bindgen")
    else:
        wb = check_toolchain()

    log(f"cargo build ({profile}) {pkg} for {WASM_TARGET}")
    cmd = ["cargo", "+nightly", "build", "--lib", "-p", pkg, "--target", WASM_TARGET,
           "-Z", "build-std=panic_abort,std", "--no-default-features"] + (["--release"] if release else [])
    run(cmd, dry, cwd=ROOT)

    if dry:
        log("wasm-bindgen --target web")
        run([wb, "--target", "web", "--out-dir", str(out / "pkg"), "--out-name", crate,
             f"<target-dir>/{WASM_TARGET}/{profile}/{crate}.wasm"], dry)
        print(f"[dry-run] assemble {out} from templates/web")
        return out

    wasm = target_dir() / WASM_TARGET / profile / f"{crate}.wasm"
    if not wasm.is_file():
        raise WebError(f"expected wasm not found: {wasm}")

    if out.exists():
        shutil.rmtree(out)
    (out / "pkg").mkdir(parents=True)
    log("wasm-bindgen --target web")
    run([wb, "--target", "web", "--out-dir", str(out / "pkg"), "--out-name", crate, str(wasm)])

    log(f"assembling {out}")
    tpl = ROOT / "templates" / "web"
    for f in sorted(tpl.iterdir()):
        if not f.is_file() or f.name == "README.md":
            continue
        data = f.read_bytes()
        try:
            text = data.decode("utf-8")
            text = text.replace("{{CRATE}}", crate).replace("{{TITLE}}", example)
            (out / f.name).write_text(text, encoding="utf-8", newline="")
        except UnicodeDecodeError:
            (out / f.name).write_bytes(data)

    ov = ROOT / "examples" / example / "web"
    if ov.is_dir():
        for f in sorted(ov.rglob("*")):
            rel = f.relative_to(ov)
            if f.is_file() and rel.parts[0] != "pkg":
                (out / rel).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(f, out / rel)
                print(f"    override: {rel.as_posix()}")
    return out


def serve(out, port, dry=False):
    log(f"serving on http://localhost:{port}/")
    cmd = [sys.executable, "-I", str(ROOT / "scripts" / "serve.py"), str(out), "--port", str(port)]
    if dry:
        print("[dry-run] " + " ".join(cmd))
        return
    try:
        subprocess.run(cmd)
    except KeyboardInterrupt:
        pass


def main(argv=None):
    p = argparse.ArgumentParser(prog="web.py", description="Build / serve / publish an example as a wasm web app.")
    p.add_argument("command", choices=["build", "serve", "dist"])
    p.add_argument("example")
    p.add_argument("--release", action="store_true")
    p.add_argument("--port", type=int, default=8080)
    p.add_argument("--dry-run", action="store_true")
    a = p.parse_args(argv)
    try:
        release = a.release or a.command == "dist"
        out = build(a.example, release, a.dry_run)
        if a.command == "build":
            print(f"Built: {out}")
        elif a.command == "dist":
            print(f"Publish-ready folder: {out}")
        else:
            serve(out, a.port, a.dry_run)
    except WebError as e:
        print(f"web.py: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
