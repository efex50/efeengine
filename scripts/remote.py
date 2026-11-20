#!/usr/bin/env python3
"""Build and test on a remote machine over SSH, run the binaries locally (stdlib only).

The working tree (committed or not, minus ignored files) is rsynced to the remote, cargo runs
there, and for `build`/`run` the produced executables are copied back to
target/remote/<profile>/ here.

Usage:
  scripts/remote.py [--host HOST] [--dir DIR] COMMAND [args]

  scripts/remote.py doctor                       check ssh, toolchain, arch and glibc
  scripts/remote.py sync                         only copy the tree to the remote
  scripts/remote.py test  [cargo test args]      e.g. test --workspace --no-fail-fast
  scripts/remote.py check [cargo check args]
  scripts/remote.py build [cargo build args]     e.g. build --release -p coin_dash
  scripts/remote.py run -p PKG [--bin B] [--release] [-- program args]
  scripts/remote.py shell                        interactive shell in the remote tree
  scripts/remote.py cargo [any cargo args]       anything else

The host is any ssh destination (a ~/.ssh/config alias, user@host, an IP). It comes from, in
order: --host, $EFE_REMOTE_HOST, or `git config remotebuild.host` (per clone, never committed):
  git config remotebuild.host myserver
There is no default; nothing is hardcoded.

The remote directory comes from --dir, $EFE_REMOTE_DIR or `git config remotebuild.dir`, default
~/remote-builds/<repo folder name> (relative paths are under the remote user's home).
$EFE_REMOTE_JOBS (or `git config remotebuild.jobs`) sets the parallelism on the remote, used for
both CARGO_BUILD_JOBS and RUST_TEST_THREADS. Default: the remote's core count minus 3 (at least 1),
so the server stays usable.

Binaries built remotely link against the remote's glibc; `doctor` checks that it is not newer
than the local one and that both sides have the same CPU architecture.
"""
import os
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Set by configure() from flags, environment and git config.
HOST = RDIR = JOBS = None
SSH = []
# Never copied: build output, git data, agent worktrees, web output.
EXCLUDES = ["/target/", "/.git/", "/.claude/", "**/target/", "**/web/pkg/", "/dist/", "__pycache__/"]


def git_config(key):
    r = subprocess.run(["git", "config", "--get", key], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() or None


def configure(host, rdir):
    global HOST, RDIR, JOBS, SSH
    HOST = host or os.environ.get("EFE_REMOTE_HOST") or git_config("remotebuild.host")
    if not HOST:
        sys.exit("no remote host: pass --host HOST, set EFE_REMOTE_HOST, or run\n"
                 "  git config remotebuild.host HOST")
    RDIR = rdir or os.environ.get("EFE_REMOTE_DIR") or git_config("remotebuild.dir") or f"remote-builds/{ROOT.name}"
    JOBS = os.environ.get("EFE_REMOTE_JOBS") or git_config("remotebuild.jobs")
    # Reuse one ssh connection for the rsync and the cargo calls (%C: hash of host/port/user).
    SSH = ["ssh", "-o", "ControlMaster=auto", "-o", "ControlPath=~/.ssh/cm-remotebuild-%C",
           "-o", "ControlPersist=10m", "-o", "ConnectTimeout=10"]


def log(msg):
    print(f"==> {msg}", file=sys.stderr, flush=True)


def ssh(cmd, tty=False, capture=False):
    args = SSH + (["-t"] if tty else []) + [HOST, cmd]
    r = subprocess.run(args, text=True, capture_output=capture)
    return r if capture else r.returncode


def remote_cmd(cargo_args):
    jobs = shlex.quote(JOBS) if JOBS else '$(( $(nproc) > 4 ? $(nproc) - 3 : 1 ))'
    # Login shells pick up rustup's PATH from ~/.profile / ~/.cargo/env.
    inner = (f'cd {RDIR} && J={jobs} && export CARGO_BUILD_JOBS=$J RUST_TEST_THREADS=$J && '
             f'echo "==> jobs: $J" >&2 && cargo {shlex.join(cargo_args)}')
    return f"bash -lc {shlex.quote(inner)}"


def sync():
    log(f"sync -> {HOST}:{RDIR}")
    if ssh(f"mkdir -p {RDIR}") != 0:
        sys.exit(f"cannot reach {HOST}; try: scripts/remote.py doctor")
    args = ["rsync", "-az", "--delete", "-e", shlex.join(SSH)]
    args += [f"--exclude={e}" for e in EXCLUDES]
    args += [f"{ROOT}/", f"{HOST}:{RDIR}/"]
    if subprocess.run(args).returncode != 0:
        sys.exit("rsync failed")


def profile_dir(cargo_args):
    if "--release" in cargo_args or "-r" in cargo_args:
        return "release"
    for i, a in enumerate(cargo_args):
        if a == "--profile" and i + 1 < len(cargo_args):
            return {"dev": "debug"}.get(cargo_args[i + 1], cargo_args[i + 1])
        if a.startswith("--profile="):
            p = a.split("=", 1)[1]
            return {"dev": "debug"}.get(p, p)
    return "debug"


def fetch(profile):
    """Copy the executables (and nothing else) of target/<profile>/ back."""
    dest = ROOT / "target" / "remote" / profile
    dest.mkdir(parents=True, exist_ok=True)
    log(f"fetch target/{profile} executables -> {dest.relative_to(ROOT)}")
    # Top-level files of target/<profile> that are executable: the bins, not deps/ or build/.
    listing = ssh(f"cd {RDIR}/target/{profile} 2>/dev/null && find . -maxdepth 1 -type f -perm -u+x -printf '%P\\n'",
                  capture=True)
    names = [n for n in listing.stdout.split() if n]
    if not names:
        log("no executables found")
        return dest
    files = "\n".join(names) + "\n"
    args = ["rsync", "-az", "-e", shlex.join(SSH), "--files-from=-",
            f"{HOST}:{RDIR}/target/{profile}/", f"{dest}/"]
    if subprocess.run(args, input=files, text=True).returncode != 0:
        sys.exit("rsync back failed")
    return dest


def cargo(sub, args, tty=False):
    sync()
    log(f"{HOST}: cargo {sub} {shlex.join(args)}")
    return ssh(remote_cmd([sub, *args]), tty=tty)


def doctor():
    ok = True
    r = ssh("uname -m; ldd --version | head -1; nproc; bash -lc 'cargo --version; rustc --version'", capture=True)
    if r.returncode != 0:
        print(f"ssh to {HOST} failed:\n{r.stderr.strip()}")
        print("Is the machine on and reachable, and does `ssh HOST` work without a password prompt?")
        return 1
    lines = r.stdout.strip().splitlines()
    print(f"remote: {' | '.join(lines)}")
    print(f"tree:   {HOST}:{RDIR} (relative paths are under the remote home)")
    rarch, rlibc = lines[0], lines[1].split()[-1]
    larch = os.uname().machine
    llibc = subprocess.run(["ldd", "--version"], capture_output=True, text=True).stdout.splitlines()[0].split()[-1]
    print(f"local:  {larch} | glibc {llibc}")
    if rarch != larch:
        print(f"!! architecture differs ({rarch} vs {larch}): remote binaries will not run here")
        ok = False
    if tuple(map(int, rlibc.split("."))) > tuple(map(int, llibc.split("."))):
        print(f"!! remote glibc {rlibc} is newer than local {llibc}: binaries may fail to start here")
        ok = False
    if not any("cargo" in l for l in lines):
        print("!! cargo not found on the remote (install rustup; the repo's rust-toolchain pins nightly)")
        ok = False
    for tool in ["rsync"]:
        if ssh(f"command -v {tool} >/dev/null", capture=True).returncode != 0:
            print(f"!! {tool} missing on the remote")
            ok = False
    print("ok" if ok else "problems found")
    return 0 if ok else 1


def run(args):
    if "--" in args:
        i = args.index("--")
        build_args, prog_args = args[:i], args[i + 1:]
    else:
        build_args, prog_args = args, []
    def flag(*names):
        for i, a in enumerate(build_args):
            if a in names and i + 1 < len(build_args):
                return build_args[i + 1]
        return None

    # The executable is named after --bin, else after the package.
    bin_name = flag("--bin") or flag("-p", "--package")
    if not bin_name:
        sys.exit("run needs -p PKG or --bin NAME")
    if cargo("build", build_args) != 0:
        sys.exit("remote build failed")
    dest = fetch(profile_dir(build_args))
    exe = dest / bin_name
    if not exe.exists():
        exe = dest / bin_name.replace("-", "_")
    if not exe.exists():
        sys.exit(f"{bin_name} not among the fetched executables in {dest}")
    log(f"run {exe.relative_to(ROOT)} {shlex.join(prog_args)}")
    # Run from the repo root so relative asset paths resolve like `cargo run`.
    return subprocess.run([str(exe), *prog_args], cwd=ROOT).returncode


def main():
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help", "help"):
        print(__doc__)
        return 0
    argv = sys.argv[1:]
    host = rdir = None
    while argv and argv[0] in ("--host", "--dir") or (argv and argv[0].startswith(("--host=", "--dir="))):
        a = argv.pop(0)
        if "=" in a:
            name, val = a.split("=", 1)
        else:
            if not argv:
                sys.exit(f"{a} needs a value")
            name, val = a, argv.pop(0)
        if name == "--host":
            host = val
        else:
            rdir = val
    if not argv:
        print(__doc__)
        return 2
    configure(host, rdir)
    sub, args = argv[0], argv[1:]
    if sub == "doctor":
        return doctor()
    if sub == "sync":
        sync()
        return 0
    if sub in ("test", "check", "clippy"):
        return cargo(sub, args)
    if sub == "build":
        code = cargo("build", args)
        if code == 0:
            fetch(profile_dir(args))
        return code
    if sub == "run":
        return run(args)
    if sub == "shell":
        sync()
        return ssh(f"cd {RDIR} && exec $SHELL -l", tty=True)
    if sub == "cargo":
        return cargo(args[0], args[1:]) if args else 2
    sys.exit(f"unknown command {sub!r}; see --help")


if __name__ == "__main__":
    sys.exit(main())
