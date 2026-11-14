#!/usr/bin/env python3
"""efeengine build/run helper (stdlib only). Run ./build.py --help for usage."""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "scripts"))
import web  # noqa: E402  (scripts/web.py)

IS_WIN = os.name == "nt"
LINUX_TARGET = "x86_64-unknown-linux-gnu"
WASM_TARGET = web.WASM_TARGET
COLOR = sys.stdout.isatty() and (not IS_WIN or os.environ.get("WT_SESSION") or os.environ.get("ANSICON"))

USAGE = """\
Usage: build.py [--dry-run] [--no-preflight] [<command> ...]

  ./build.py                                  interactive: pick example, native/web, build or run
  ./build.py run   [<example>] [native|web] [--release] [--static] [--port N]
  ./build.py build [<example>] [native|web] [--release] [--static]
  ./build.py dist  [<example>]                web publish folder (scripts/web.py dist)
  ./build.py list                             list examples
  ./build.py check                            cargo check --workspace + wasm check of core/examples
  ./build.py test                             cargo test for calc, efepages, efescript
  ./build.py doctor                           report toolchain/tool status with fix hints
  ./build.py dev <linux|web|windows|triple>   set rust-analyzer target in .vscode/settings.json
  ./build.py clean [<example>]                cargo clean / remove dist/<example>
  -h, --help                                  this help

Options:
  --release        optimised build
  --static         build SDL3 from source and link it statically (needs cmake + a C compiler)
  --port N         web server port (default 8080)
  --dry-run        print commands instead of running them
  --no-preflight   skip the SDL3 / cmake / cc checks before native builds

On Windows use: python build.py ...
Environment: CARGO_TARGET_DIR is respected; WASM_BINDGEN selects a wasm-bindgen binary.
Prompts only appear when stdin is a terminal and an argument is missing.
"""


class Fail(Exception):
    def __init__(self, msg, code=1, usage=False):
        super().__init__(msg)
        self.code, self.usage = code, usage


def paint(code, s):
    return f"\033[{code}m{s}\033[0m" if COLOR else s


def step(msg):
    print(f"{paint('1;36', '==>')} {msg}", flush=True)


def warn(msg):
    print(f"{paint('1;33', 'warning:')} {msg}", file=sys.stderr)


def need_args(msg):
    return Fail(msg, 2, usage=True)


class Ctx:
    dry = False
    preflight = True


def run(cmd, **kw):
    if Ctx.dry:
        print("[dry-run] " + subprocess.list2cmdline([str(c) for c in cmd]), flush=True)
        return
    try:
        r = subprocess.run([str(c) for c in cmd], cwd=kw.pop("cwd", ROOT), **kw)
    except FileNotFoundError:
        raise Fail(f"command not found: {cmd[0]}")
    except KeyboardInterrupt:
        raise Fail("interrupted", 130)
    if r.returncode != 0:
        raise Fail(f"command failed ({r.returncode}): {subprocess.list2cmdline([str(c) for c in cmd])}", r.returncode)


def need_cargo():
    if not shutil.which("cargo"):
        raise Fail("cargo not found (install rustup: https://rustup.rs)")


# ------------------------------------------------------------ examples ----
def has_native_bin(example):
    text = (ROOT / "examples" / example / "Cargo.toml").read_text(encoding="utf-8")
    in_bin = False
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("["):
            in_bin = s == "[[bin]]"
        elif in_bin and re.match(r'name\s*=\s*"native"', s):
            return True
    return False


def need_example(ex):
    exs = web.list_examples()
    if ex not in exs:
        raise Fail(f"unknown example '{ex}' (available: {' '.join(exs) or 'none'})")


def prompt_choice(title, options, default=1):
    while True:
        print(title)
        for i, o in enumerate(options, 1):
            print(f"  {i}) {o}{' (default)' if i == default else ''}")
        try:
            ans = input("> ").strip()
        except EOFError:
            raise Fail("input closed")
        ans = ans or str(default)
        if ans.isdigit() and 1 <= int(ans) <= len(options):
            return options[int(ans) - 1]
        if ans in options:
            return ans
        print(f"Invalid choice '{ans}'.")


def pick_example(given):
    exs = web.list_examples()
    if not exs:
        raise Fail("no examples found in examples/")
    if given:
        need_example(given)
        return given
    if len(exs) == 1:
        step(f"only one example: {exs[0]}")
        return exs[0]
    if sys.stdin.isatty():
        return prompt_choice("Which example?", exs)
    raise need_args(f"missing <example> (available: {' '.join(exs)})")


# ----------------------------------------------------------- preflight ----
def sdl3_found():
    if shutil.which("pkg-config") and subprocess.run(["pkg-config", "--exists", "sdl3"]).returncode == 0:
        return True
    dirs = ["/usr/lib", "/usr/lib64", "/usr/local/lib", "/usr/local/lib64", "/lib", "/lib64",
            "/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/opt/homebrew/lib"]
    dirs += [d for d in os.environ.get("LIBRARY_PATH", "").split(os.pathsep) if d]
    return any((Path(d) / n).exists() for d in dirs for n in ("libSDL3.so", "libSDL3.dylib"))


def preflight_native(static):
    if not Ctx.preflight:
        return
    if static:
        missing = [t for t, alts in (("cmake", ["cmake"]), ("cc", ["cc", "gcc", "clang", "cl"]))
                   if not any(shutil.which(a) for a in alts)]
        if missing:
            raise Fail(
                f"--static builds SDL3 from source and needs: {', '.join(missing)} (not found).\n"
                "Install them, e.g.:\n"
                "  Fedora: sudo dnf install gcc gcc-c++ cmake\n"
                "  Debian: sudo apt install build-essential cmake\n"
                "  Arch:   sudo pacman -S base-devel cmake\n"
                "  Windows: install CMake and Visual Studio Build Tools\n"
                "or drop --static and use a system SDL3.")
        return
    if IS_WIN:
        if not os.environ.get("VCPKG_ROOT"):
            print("note: on Windows SDL3 comes from vcpkg (set VCPKG_ROOT) or use --static; "
                  "skipping the SDL3 check.", file=sys.stderr)
        return
    if not sdl3_found():
        raise Fail(
            "SDL3 development files were not found (pkg-config sdl3, libSDL3.so in a\n"
            "standard lib dir or in LIBRARY_PATH). Install them, e.g.:\n"
            "  Fedora: sudo dnf install SDL3-devel\n"
            "  Debian: sudo apt install libsdl3-dev\n"
            "  Arch:   sudo pacman -S sdl3\n"
            "or build SDL3 from source with: ./build.py <build|run> <example> native --static\n"
            "(needs cmake and a C compiler). Use --no-preflight to skip this check.")


# ------------------------------------------------------ build/run actions --
def do_native(action, ex, release, static):
    pkg = web.package_name(ex)
    need_cargo()
    preflight_native(static)
    extra = (["--release"] if release else []) + (["--features", "efeengine-core/static"] if static else [])
    desc = ("release" if release else "debug") + (", static SDL3" if static else "")
    if has_native_bin(ex):
        step(f"native {action}: {pkg} ({desc})")
        run(["cargo", action, "-p", pkg, "--bin", "native"] + extra)
    else:
        step(f"{pkg} has no 'native' bin; building the lib")
        run(["cargo", "build", "-p", pkg, "--lib"] + extra)
        if action == "run":
            raise Fail(f"example '{ex}' has no [[bin]] named 'native', nothing to run (library was built)")


def do_web(action, ex, release, port):
    step(f"web {action}: {ex}")
    try:
        out = web.build(ex, release or action == "dist", Ctx.dry)
        if action == "build":
            print(f"Built: {out}")
        elif action == "dist":
            print(f"Publish-ready folder: {out}")
        else:
            web.serve(out, port, Ctx.dry)
    except web.WebError as e:
        raise Fail(str(e))


def cmd_build_run(a, action, interactive=False):
    tgt = a.target
    if a.example in ("native", "web") and a.example not in web.list_examples():
        if tgt:
            raise need_args(f"unexpected argument '{a.target}'")
        a.example, tgt = None, a.example  # "run native": example omitted
    ex = pick_example(a.example)
    if not tgt:
        if not sys.stdin.isatty():
            raise need_args("missing target (native|web)")
        tgt = prompt_choice(f"Run {ex} how?", ["native", "web"])
    if interactive:
        action = "run" if prompt_choice("What to do?", ["run", "build only"]) == "run" else "build"
    if tgt == "web" and a.static:
        warn("--static is ignored for web")
    if tgt == "native" and getattr(a, "port", None) is not None:
        warn("--port is ignored for native")
    if tgt == "native":
        do_native(action, ex, a.release, a.static)
    else:
        do_web(action, ex, a.release, getattr(a, "port", None) or 8080)


def cmd_dist(a):
    do_web("dist", pick_example(a.example), True, 8080)


def cmd_list(_a):
    exs = web.list_examples()
    for e in exs:
        print(f"{e} (package {web.package_name(e)}, native bin: {'yes' if has_native_bin(e) else 'no'})")
    if not exs:
        print("no examples found")


def cmd_check(_a):
    need_cargo()
    step("cargo check --workspace (native)")
    run(["cargo", "check", "--workspace"])
    pkgs = ["-p", "efeengine-core"]
    for e in web.list_examples():
        pkgs += ["-p", web.package_name(e)]
    step(f"cargo check for {WASM_TARGET} ({' '.join(pkgs)})")
    run(["cargo", "check", *pkgs, "--lib", "--target", WASM_TARGET, "-Z", "build-std=panic_abort,std"])


def cmd_test(_a):
    need_cargo()
    step("cargo test -p calc -p efepages -p efescript")
    run(["cargo", "test", "-p", "calc", "-p", "efepages", "-p", "efescript"])


def cmd_clean(a):
    if a.example:
        need_example(a.example)
        d = ROOT / "dist" / a.example
        step(f"removing {d}")
        if Ctx.dry:
            print(f"[dry-run] rmtree {d}")
        else:
            shutil.rmtree(d, ignore_errors=True)
    else:
        need_cargo()
        step("cargo clean")
        run(["cargo", "clean"])


def cmd_dev(a):
    t = a.target
    triples = {"linux": LINUX_TARGET, "web": WASM_TARGET, "windows": "x86_64-pc-windows-gnu"}
    triple = triples.get(t, t)
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", triple):
        raise Fail(f"invalid target '{t}'")
    f = ROOT / ".vscode" / "settings.json"
    step(f"rust-analyzer target -> {triple} ({f})")
    settings = {
        "rust-analyzer.check.command": "check",
        "rust-analyzer.cargo.buildScripts.enable": True,
        "rust-analyzer.cargo.allTargets": False,
        "rust-analyzer.cargo.target": triple,
        "rust-analyzer.server.extraEnv": {"RUSTUP_TOOLCHAIN": "nightly"},
    }
    if Ctx.dry:
        print(f"[dry-run] write {f}: rust-analyzer.cargo.target={triple}")
        return
    f.parent.mkdir(exist_ok=True)
    f.write_text(json.dumps(settings, indent=4) + "\n", encoding="utf-8")


def cmd_doctor(_a):
    bad = []

    def ok(m):
        print(f"  {paint('1;32', 'ok')}    {m}")

    def miss(m, hint):
        print(f"  {paint('1;31', 'MISS')}  {m}\n        fix: {hint}")
        bad.append(m)

    step("doctor")
    if shutil.which("cargo"):
        ok("cargo: " + subprocess.run(["cargo", "--version"], capture_output=True, text=True).stdout.strip())
    else:
        miss("cargo", "install rustup: https://rustup.rs")
    if shutil.which("rustup"):
        tcs = subprocess.run(["rustup", "toolchain", "list"], capture_output=True, text=True).stdout
        if any(l.startswith("nightly") for l in tcs.splitlines()):
            ok("rustup nightly toolchain")
        else:
            miss("nightly toolchain", "rustup toolchain install nightly")
    else:
        miss("rustup", "https://rustup.rs")
    if shutil.which("cargo") and subprocess.run(["cargo", "+nightly", "--version"], capture_output=True).returncode == 0:
        probs = {p: h for p, h in web.rust_status()}
        for label, key in (("rust-src (nightly)", "rust-src"), (f"{WASM_TARGET} target (nightly)", WASM_TARGET)):
            hit = [(p, h) for p, h in probs.items() if key in p]
            if hit:
                miss(label, hit[0][1])
            else:
                ok(label)
    try:
        path, have, want, hint = web.wasm_bindgen_status()
        if path is None:
            miss(f"wasm-bindgen CLI ({os.environ.get('WASM_BINDGEN', 'wasm-bindgen')})", hint)
        elif have != want:
            miss(f"wasm-bindgen {have} != Cargo.lock {want}", hint)
        else:
            ok(f"wasm-bindgen {have} (matches Cargo.lock)")
    except web.WebError as e:
        warn(str(e))
    cc = any(shutil.which(c) for c in ("cc", "gcc", "clang", "cl"))
    ok("cmake (for --static)") if shutil.which("cmake") else miss("cmake (only needed for --static)", "dnf install cmake / apt install cmake")
    ok("C compiler (for --static)") if cc else miss("C compiler (only needed for --static)", "dnf install gcc gcc-c++ / apt install build-essential")
    if IS_WIN:
        ok("VCPKG_ROOT set") if os.environ.get("VCPKG_ROOT") else miss("VCPKG_ROOT (SDL3 via vcpkg)", "set VCPKG_ROOT or use --static")
    elif sdl3_found():
        ok("SDL3 (system)")
    else:
        miss("SDL3 dev files (not needed with --static)", "dnf install SDL3-devel / apt install libsdl3-dev / pacman -S sdl3")
    ok(f"python {sys.version.split()[0]}")
    print("All good." if not bad else "Some items are missing (see fixes above).")


# ---------------------------------------------------------------- main ----
def build_parser():
    p = argparse.ArgumentParser(prog="build.py", add_help=False)
    p.add_argument("-h", "--help", action="store_true")
    sub = p.add_subparsers(dest="cmd")

    def common(sp, run_opts):
        sp.add_argument("example", nargs="?")
        sp.add_argument("target", nargs="?", choices=["native", "web"])
        sp.add_argument("--release", action="store_true")
        sp.add_argument("--static", action="store_true")
        if run_opts:
            sp.add_argument("--port", type=int)

    common(sub.add_parser("run", add_help=False), True)
    common(sub.add_parser("build", add_help=False), False)
    d = sub.add_parser("dist", add_help=False)
    d.add_argument("example", nargs="?")
    for name in ("list", "check", "test", "doctor"):
        sub.add_parser(name, add_help=False)
    dv = sub.add_parser("dev", add_help=False)
    dv.add_argument("target")
    c = sub.add_parser("clean", add_help=False)
    c.add_argument("example", nargs="?")
    return p


def main(argv):
    argv = list(argv)
    Ctx.dry = "--dry-run" in argv
    Ctx.preflight = "--no-preflight" not in argv
    argv = [x for x in argv if x not in ("--dry-run", "--no-preflight")]
    p = build_parser()
    try:
        a, rest = p.parse_known_args([x for x in argv])
    except SystemExit as e:
        raise Fail("invalid arguments", 2, usage=True) from e
    if rest:
        raise need_args(f"unknown argument(s): {' '.join(rest)}")
    if a.help or a.cmd == "help":
        print(USAGE)
        return 0
    if a.cmd is None:
        if not sys.stdin.isatty():
            raise need_args("no command given and stdin is not a terminal")
        a = argparse.Namespace(example=None, target=None, release=False, static=False, port=None)
        cmd_build_run(a, "run", interactive=True)
        return 0
    if a.cmd == "run" or a.cmd == "build":
        if getattr(a, "port", None) is not None and not (0 < a.port < 65536):
            raise Fail(f"invalid port {a.port}")
        cmd_build_run(a, a.cmd)
    else:
        globals()["cmd_" + a.cmd](a)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Fail as f:
        if f.usage:
            print(USAGE, file=sys.stderr)
        print(f"{paint('1;31', 'error:')} {f}", file=sys.stderr)
        sys.exit(f.code)
    except KeyboardInterrupt:
        sys.exit(130)
