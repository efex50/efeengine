#!/usr/bin/env python3
"""Build and test on a remote machine over SSH, run the binaries locally (stdlib only).

The working tree (committed or not, minus ignored files) is rsynced to the remote, cargo runs
there, and for `build`/`run` the produced executables are copied back to target/<profile>/ here
(the normal local profile dir; $CARGO_TARGET_DIR is respected), so everything after the build
works unchanged on this machine.

Also importable (build.py uses it for `--remote`): Remote(host, rdir, jobs) holds the settings,
and its methods sync(), cargo(), fetch() and run() do the work. Nothing exits at import time;
failures raise RemoteError.

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
# Never copied: build output, git data, agent worktrees, web output.
EXCLUDES = ["/target/", "/.git/", "/.claude/", "**/target/", "**/web/pkg/", "/dist/", "__pycache__/"]
# Reuse one ssh connection for the rsync and the cargo calls (%C: hash of host/port/user).
SSH_OPTS = ["-o", "ControlMaster=auto", "-o", "ControlPath=~/.ssh/cm-remotebuild-%C",
            "-o", "ControlPersist=10m", "-o", "ConnectTimeout=10"]


class RemoteError(Exception):
    pass


def git_config(key, root=ROOT):
    r = subprocess.run(["git", "config", "--get", key], cwd=root, capture_output=True, text=True)
    return r.stdout.strip() or None


def log(msg):
    print(f"==> {msg}", file=sys.stderr, flush=True)


def profile_dir(cargo_args):
    """The target/<dir> name cargo uses for these build arguments."""
    if "--release" in cargo_args or "-r" in cargo_args:
        return "release"
    for i, a in enumerate(cargo_args):
        if a == "--profile" and i + 1 < len(cargo_args):
            return {"dev": "debug"}.get(cargo_args[i + 1], cargo_args[i + 1])
        if a.startswith("--profile="):
            p = a.split("=", 1)[1]
            return {"dev": "debug"}.get(p, p)
    return "debug"


def local_profile_dir(profile):
    """Local target/<profile>, honouring $CARGO_TARGET_DIR."""
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    return target / profile


class Remote:
    """Settings for one remote machine. Missing pieces come from the environment and git config."""

    def __init__(self, host=None, rdir=None, jobs=None, root=ROOT, dry=False):
        self.root = Path(root)
        self.host = host or os.environ.get("EFE_REMOTE_HOST") or git_config("remotebuild.host", self.root)
        if not self.host:
            raise RemoteError("no remote host: pass --host HOST (or --remote=HOST), set EFE_REMOTE_HOST, "
                              "or run\n  git config remotebuild.host HOST")
        self.rdir = (rdir or os.environ.get("EFE_REMOTE_DIR") or git_config("remotebuild.dir", self.root)
                     or f"remote-builds/{self.root.name}")
        self.jobs = jobs or os.environ.get("EFE_REMOTE_JOBS") or git_config("remotebuild.jobs", self.root)
        self.dry = dry
        self.synced = False
        self.ssh_cmd = ["ssh", *SSH_OPTS]

    # -- low level ---------------------------------------------------------------------------
    def _show(self, args):
        print("[dry-run] " + shlex.join(str(a) for a in args), flush=True)

    def ssh(self, cmd, tty=False, capture=False):
        args = self.ssh_cmd + (["-t"] if tty else []) + [self.host, cmd]
        if self.dry:
            self._show(args)
            return subprocess.CompletedProcess(args, 0, "", "") if capture else 0
        r = subprocess.run(args, text=True, capture_output=capture)
        return r if capture else r.returncode

    def remote_cmd(self, cargo_args):
        jobs = shlex.quote(self.jobs) if self.jobs else '$(( $(nproc) > 4 ? $(nproc) - 3 : 1 ))'
        # Login shells pick up rustup's PATH from ~/.profile / ~/.cargo/env.
        inner = (f'cd {self.rdir} && J={jobs} && export CARGO_BUILD_JOBS=$J RUST_TEST_THREADS=$J && '
                 f'echo "==> jobs: $J" >&2 && cargo {shlex.join(cargo_args)}')
        return f"bash -lc {shlex.quote(inner)}"

    # -- operations --------------------------------------------------------------------------
    def sync(self, force=False):
        """Copy the working tree to the remote (once per Remote object unless forced)."""
        if self.synced and not force:
            return
        log(f"sync -> {self.host}:{self.rdir}")
        if self.ssh(f"mkdir -p {self.rdir}") != 0:
            raise RemoteError(f"cannot reach {self.host}; try: scripts/remote.py doctor")
        args = ["rsync", "-az", "--delete", "-e", shlex.join(self.ssh_cmd)]
        args += [f"--exclude={e}" for e in EXCLUDES]
        args += [f"{self.root}/", f"{self.host}:{self.rdir}/"]
        if self.dry:
            self._show(args)
        elif subprocess.run(args).returncode != 0:
            raise RemoteError("rsync failed")
        self.synced = True

    def cargo(self, sub, args=(), tty=False):
        """Sync, then run `cargo SUB ARGS` on the remote. Returns the exit code."""
        self.sync()
        args = list(args)
        log(f"{self.host}: cargo {sub} {shlex.join(args)}")
        return self.ssh(self.remote_cmd([sub, *args]), tty=tty)

    def fetch(self, profile, dest=None, names=None):
        """Copy executables of the remote target/<profile>/ into dest (default: the local
        target/<profile>). With names, exactly those files; otherwise every top-level executable
        (the bins, not deps/ or build/). Returns the destination directory."""
        dest = Path(dest) if dest else local_profile_dir(profile)
        src = f"{self.rdir}/target/{profile}"
        if not self.dry:
            dest.mkdir(parents=True, exist_ok=True)
        if names is None:
            listing = self.ssh(f"cd {src} 2>/dev/null && find . -maxdepth 1 -type f -perm -u+x -printf '%P\\n'",
                               capture=True)
            names = [n for n in listing.stdout.split() if n]
            if not names and not self.dry:
                log("no executables found")
                return dest
        names = list(names)
        shown = " ".join(names) if names else "executables"
        log(f"fetch {shown} from {self.host}:target/{profile} -> {dest}")
        args = ["rsync", "-az", "-e", shlex.join(self.ssh_cmd), "--files-from=-", f"{self.host}:{src}/", f"{dest}/"]
        if self.dry:
            self._show(args + [f"(files: {shown})"])
            return dest
        r = subprocess.run(args, input="\n".join(names) + "\n", text=True)
        if r.returncode != 0:
            raise RemoteError(f"fetching {shown} from {self.host}:{src} failed (was it built there?)")
        return dest

    def probe(self, cmd):
        """Run a shell command (login shell) on the remote; True when it exits 0 and printed text
        is returned as the second value."""
        r = self.ssh(f"bash -lc {shlex.quote(cmd)}", capture=True)
        return r.returncode == 0, r.stdout

    def has_target(self, triple):
        """True if the remote's nightly toolchain has this rustup target installed."""
        ok, out = self.probe("rustup +nightly target list --installed")
        return ok and triple in out.split()

    def doctor(self):
        ok = True
        r = self.ssh("uname -m; ldd --version | head -1; nproc; bash -lc 'cargo --version; rustc --version'",
                     capture=True)
        if r.returncode != 0:
            print(f"ssh to {self.host} failed:\n{r.stderr.strip()}")
            print("Is the machine on and reachable, and does `ssh HOST` work without a password prompt?")
            return 1
        lines = r.stdout.strip().splitlines()
        print(f"remote: {' | '.join(lines)}")
        print(f"tree:   {self.host}:{self.rdir} (relative paths are under the remote home)")
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
            if self.ssh(f"command -v {tool} >/dev/null", capture=True).returncode != 0:
                print(f"!! {tool} missing on the remote")
                ok = False
        print("ok" if ok else "problems found")
        return 0 if ok else 1

    def run(self, args):
        """`run -p PKG [--bin B] [--release] [-- program args]`: build remotely, fetch the one
        executable, start it here from the repo root."""
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
            raise RemoteError("run needs -p PKG or --bin NAME")
        if self.cargo("build", build_args) != 0:
            raise RemoteError("remote build failed")
        dest = self.fetch(profile_dir(build_args), names=[bin_name])
        exe = dest / bin_name
        log(f"run {exe} {shlex.join(prog_args)}")
        if self.dry:
            return 0
        # Run from the repo root so relative asset paths resolve like `cargo run`.
        return subprocess.run([str(exe), *prog_args], cwd=self.root).returncode


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
    try:
        r = Remote(host, rdir)
        sub, args = argv[0], argv[1:]
        if sub == "doctor":
            return r.doctor()
        if sub == "sync":
            r.sync()
            return 0
        if sub in ("test", "check", "clippy"):
            return r.cargo(sub, args)
        if sub == "build":
            code = r.cargo("build", args)
            if code == 0:
                r.fetch(profile_dir(args))
            return code
        if sub == "run":
            return r.run(args)
        if sub == "shell":
            r.sync()
            return r.ssh(f"cd {r.rdir} && exec $SHELL -l", tty=True)
        if sub == "cargo":
            return r.cargo(args[0], args[1:]) if args else 2
    except RemoteError as e:
        sys.exit(str(e))
    sys.exit(f"unknown command {sub!r}; see --help")


if __name__ == "__main__":
    sys.exit(main())
