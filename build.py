#!/usr/bin/env python3
"""efeengine build/run helper (stdlib only). Run ./build.py --help for usage."""
import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "scripts"))
import remote  # noqa: E402  (scripts/remote.py: build on a remote machine over SSH)
import web  # noqa: E402  (scripts/web.py)

IS_WIN = os.name == "nt"
LINUX_TARGET = "x86_64-unknown-linux-gnu"
WASM_TARGET = web.WASM_TARGET
COLOR = sys.stdout.isatty() and (not IS_WIN or os.environ.get("WT_SESSION") or os.environ.get("ANSICON"))

USAGE = """\
Usage: build.py [--dry-run] [--no-preflight] [--remote[=HOST]] [<command> ...]

  ./build.py                                  interactive: pick example, native/web, build or run
  ./build.py run   [<example>] [native|web] [--release] [--static] [--jit] [--port N]
  ./build.py build [<example>] [native|web] [--release] [--static] [--jit]
  ./build.py dist  [<example>]                web publish folder (scripts/web.py dist)
  ./build.py editor [<example>] [--release] [--static] [--jit]
                                              run the game with the editor link (EFE_EDITOR=1),
                                              then the editor window; they talk over a private local socket
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
  --jit            native only: compile the script JIT in (cargo feature efeengine-core/jit);
                   the debug menu's Scripts tab can then switch Interpreter / Auto / JIT
  --port N         web server port (default 8080)
  --dry-run        print commands instead of running them
  --no-preflight   skip the SDL3 / cmake / cc checks before native builds
  --remote[=HOST]  native build/run/editor/check/test: cargo runs on a remote machine over SSH
                   (the tree is rsynced there), the built executables are fetched into
                   target/<profile>/ and run on this PC. HOST is any ssh destination; bare
                   --remote uses $EFE_REMOTE_HOST, else EFE_REMOTE_HOST in remotes.env (git
                   ignored; see remotes.env.example). Directory: EFE_REMOTE_DIR (default
                   remote-builds/<repo>), jobs: EFE_REMOTE_JOBS (default: remote cores - 3).
                   Not for web/dist. Example: ./build.py --remote run coin_dash native
                   (see docs/REMOTE_BUILD.md, scripts/remote.py doctor)

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
    remote = None  # remote.Remote when --remote is given


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
    pkg = web.package_name(example)
    in_bin = False
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("["):
            in_bin = s == "[[bin]]"
        elif in_bin and (re.match(r'name\s*=\s*"native"', s) or re.match(rf'name\s*=\s*"{re.escape(pkg)}"', s)):
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


# ------------------------------------------------------------- remote ----
def sdl3_runtime_found():
    """libSDL3 loadable at run time (what a remotely built, dynamically linked binary needs)."""
    ldconfig = shutil.which("ldconfig") or "/sbin/ldconfig"
    try:
        out = subprocess.run([ldconfig, "-p"], capture_output=True, text=True).stdout
        if "libSDL3.so" in out:
            return True
    except OSError:
        pass
    return sdl3_found()


def need_local_only(what):
    if Ctx.remote:
        raise Fail(f"--remote supports native builds only (not for {what})")


def remote_cargo(sub, args, fetch_names=None):
    """Run cargo on the remote; with fetch_names, bring those executables into the local
    target/<profile>/ afterwards."""
    r = Ctx.remote
    try:
        if r.cargo(sub, args) != 0:
            raise Fail(f"remote cargo {sub} failed")
        if fetch_names:
            profile = remote.profile_dir(args)
            r.fetch(profile, dest=remote.local_profile_dir(profile), names=fetch_names)
    except remote.RemoteError as e:
        raise Fail(str(e))


def cargo_build(args, fetch_names=None):
    """cargo build ARGS locally, or on the remote with --remote."""
    if Ctx.remote:
        remote_cargo("build", args, fetch_names)
    else:
        run(["cargo", "build"] + args)


def remote_exe(name, release):
    return remote.local_profile_dir("release" if release else "debug") / name


def native_preflight_or_remote(static):
    if not Ctx.remote:
        preflight_native(static)
    elif not static and not IS_WIN and Ctx.preflight and not sdl3_runtime_found():
        warn("no libSDL3.so found on this PC: a remotely built binary needs SDL3 at run time "
             "(install it, e.g. dnf install SDL3 / apt install libsdl3-0, or pass --static)")


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
def native_features(static, jit):
    """`--features a,b` cargo arguments for the optional engine features."""
    feats = ((["efeengine-core/static"] if static else []) + (["efeengine-core/jit"] if jit else []))
    return ["--features", ",".join(feats)] if feats else []


def do_native(action, ex, release, static, jit=False):
    pkg = web.package_name(ex)
    if not Ctx.remote:
        need_cargo()
    native_preflight_or_remote(static)
    extra = (["--release"] if release else []) + native_features(static, jit)
    desc = ("release" if release else "debug") + (", static SDL3" if static else "") + (", jit" if jit else "")
    if has_native_bin(ex):
        step(f"native {action}: {pkg} ({desc})" + (f" on {Ctx.remote.host}" if Ctx.remote else ""))
        try:
            if Ctx.remote:
                # Build there, fetch the binary, run it here (never `cargo run`: that would rebuild locally).
                cargo_build(["-p", pkg, "--bin", pkg] + extra, [pkg])
                if action == "run":
                    exe = remote_exe(pkg, release)
                    step(f"run {exe}")
                    if Ctx.dry:
                        print(f"[dry-run] (cd {ROOT} && {exe})")
                    else:
                        code = subprocess.run([str(exe)], cwd=ROOT).returncode
                        if code != 0:
                            raise Fail(f"{pkg} exited with {code}", code)
            else:
                run(["cargo", action, "-p", pkg, "--bin", pkg] + extra)
        except Fail:
            if static:
                print("hint: building SDL3 from source also needs its platform dev headers "
                      "(look for 'Could NOT find' / 'CMake Error' above), e.g.\n"
                      "  Fedora: sudo dnf install wayland-devel libxkbcommon-devel libX11-devel "
                      "libXext-devel mesa-libGL-devel alsa-lib-devel pulseaudio-libs-devel\n"
                      "  Debian: sudo apt install libwayland-dev libxkbcommon-dev libx11-dev "
                      "libxext-dev libgl-dev libasound2-dev libpulse-dev\n"
                      "Or skip the source build: install SDL3 itself (dnf install SDL3-devel) "
                      "and drop --static.", file=sys.stderr)
            raise
    else:
        step(f"{pkg} has no binary target; building the lib")
        cargo_build(["-p", pkg, "--lib"] + extra)
        if action == "run":
            raise Fail(f"example '{ex}' has no binary named '{pkg}', nothing to run (library was built)")


def do_web(action, ex, release, port):
    need_local_only(f"web {action}")
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
    if tgt == "web" and getattr(a, "jit", False):
        warn("--jit is ignored for web (the browser always uses the interpreter)")
    if tgt == "native" and getattr(a, "port", None) is not None:
        warn("--port is ignored for native")
    if tgt == "native":
        do_native(action, ex, a.release, a.static, getattr(a, "jit", False))
    else:
        do_web(action, ex, a.release, getattr(a, "port", None) or 8080)


def cmd_dist(a):
    need_local_only("dist")
    do_web("dist", pick_example(a.example), True, 8080)


def cmd_list(_a):
    exs = web.list_examples()
    for e in exs:
        print(f"{e} (package {web.package_name(e)}, native bin: {'yes' if has_native_bin(e) else 'no'})")
    if not exs:
        print("no examples found")


def cmd_check(_a):
    pkgs = ["-p", "efeengine-core"]
    for e in web.list_examples():
        pkgs += ["-p", web.package_name(e)]
    if Ctx.remote:
        step(f"cargo check --workspace (native) on {Ctx.remote.host}")
        remote_cargo("check", ["--workspace"])
        try:
            have_wasm = Ctx.dry or Ctx.remote.has_target(WASM_TARGET)
        except remote.RemoteError as e:
            raise Fail(str(e))
        if not have_wasm:
            print(f"note: skipping the {WASM_TARGET} check: {Ctx.remote.host} has no {WASM_TARGET} target "
                  "(rustup +nightly target add it there, or run ./build.py check without --remote)")
            return
        step(f"cargo check for {WASM_TARGET} on {Ctx.remote.host}")
        remote_cargo("check", pkgs + ["--lib", "--target", WASM_TARGET, "-Z", "build-std=panic_abort,std"])
        return
    need_cargo()
    step("cargo check --workspace (native)")
    run(["cargo", "check", "--workspace"])
    step(f"cargo check for {WASM_TARGET} ({' '.join(pkgs)})")
    run(["cargo", "check", *pkgs, "--lib", "--target", WASM_TARGET, "-Z", "build-std=panic_abort,std"])


def cmd_test(_a):
    if Ctx.remote:
        step(f"cargo test -p calc -p efepages -p efescript on {Ctx.remote.host}")
        remote_cargo("test", ["-p", "calc", "-p", "efepages", "-p", "efescript"])
        return
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
def links_editor(ex):
    """True when the example's sources start the editor link (EditorLink::from_env)."""
    src_dir = ROOT / "examples" / ex / "src"
    if not src_dir.is_dir():
        return False
    return any("EditorLink" in f.read_text(errors="ignore") for f in src_dir.glob("*.rs"))


def do_editor(ex, release, static, jit=False):
    """Run the game with the editor link on (EFE_EDITOR=1), then the editor in front of it."""
    pkg = web.package_name(ex)
    if not Ctx.remote:
        need_cargo()
    native_preflight_or_remote(static)
    if not has_native_bin(ex):
        raise Fail(f"example '{ex}' has no binary target, so there is no game to attach the editor to")
    if not links_editor(ex):
        raise Fail(f"example '{ex}' does not start the editor link: add EditorLink::from_env() "
                   "to its engine setup (examples/script_demo does this)")
    # The editor is started by this process, so the session name uses this process id.
    sock = f"efe-editor-{os.getpid()}"
    extra = (["--release"] if release else []) + native_features(static, jit)
    profile = "release" if release else "debug"
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    game = target / profile / (pkg + (".exe" if os.name == "nt" else ""))
    if Ctx.remote:
        # Both binaries are built there and fetched; the editor is started directly, not via cargo run.
        editor_cmd = [str(remote_exe("efe-editor", release)), "--socket", sock]
    else:
        editor_cmd = ["cargo", "run", "-p", "efeengine-editor", "--bin", "efe-editor", "--", "--socket", sock]
    step(f"editor: {pkg} ({'release' if release else 'debug'}) on socket {sock}"
         + (f", built on {Ctx.remote.host}" if Ctx.remote else ""))
    cargo_build(["-p", pkg, "--bin", pkg] + extra, [pkg])
    cargo_build(["-p", "efeengine-editor", "--bin", "efe-editor"] + (["--release"] if release else []),
                ["efe-editor"])
    if Ctx.dry:
        print(f"[dry-run] EFE_EDITOR=1 EFE_EDITOR_SOCKET={sock} {game} &")
        if Ctx.remote:
            print("[dry-run] " + subprocess.list2cmdline(editor_cmd))
        else:
            run(editor_cmd)
        return
    env = dict(os.environ, EFE_EDITOR="1", EFE_EDITOR_SOCKET=sock)
    proc = subprocess.Popen([str(game)], cwd=str(ROOT), env=env)
    # SIGTERM (e.g. `timeout`) should stop the game too: raise the same interrupt as Ctrl+C,
    # so the editor's child process and the game are cleaned up below.
    def _interrupt(*_):
        raise KeyboardInterrupt
    previous = signal.signal(signal.SIGTERM, _interrupt)
    try:
        run(editor_cmd)
    finally:
        signal.signal(signal.SIGTERM, previous)
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


def cmd_editor(a):
    ex = pick_example(a.example)
    do_editor(ex, a.release, a.static, a.jit)


def build_parser():
    p = argparse.ArgumentParser(prog="build.py", add_help=False)
    p.add_argument("-h", "--help", action="store_true")
    sub = p.add_subparsers(dest="cmd")

    def common(sp, run_opts):
        sp.add_argument("example", nargs="?")
        sp.add_argument("target", nargs="?", choices=["native", "web"])
        sp.add_argument("--release", action="store_true")
        sp.add_argument("--static", action="store_true")
        sp.add_argument("--jit", action="store_true")
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
    ed = sub.add_parser("editor", add_help=False)
    ed.add_argument("example", nargs="?")
    ed.add_argument("--release", action="store_true")
    ed.add_argument("--static", action="store_true")
    ed.add_argument("--jit", action="store_true")
    c = sub.add_parser("clean", add_help=False)
    c.add_argument("example", nargs="?")
    return p


def main(argv):
    argv = list(argv)
    Ctx.dry = "--dry-run" in argv
    Ctx.preflight = "--no-preflight" not in argv
    remote_host = None
    use_remote = False
    for x in argv:
        if x == "--remote" or x.startswith("--remote="):
            use_remote = True
            remote_host = x.partition("=")[2] or remote_host
    argv = [x for x in argv if x not in ("--dry-run", "--no-preflight") and not (x == "--remote" or x.startswith("--remote="))]
    if use_remote:
        try:
            Ctx.remote = remote.Remote(remote_host, dry=Ctx.dry)
        except remote.RemoteError as e:
            raise Fail(str(e))
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
        a = argparse.Namespace(example=None, target=None, release=False, static=False, jit=False, port=None)
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
