#!/usr/bin/env python3
"""Build both wasm mods of examples/coin_dash_wasm (dash_c and dash_rust).

Standard library only. Writes `dash_c.wasm` and `dash_rust.wasm` into
`target/coin_dash_wasm/wasm/` (or `$CARGO_TARGET_DIR/coin_dash_wasm/wasm/`, or `--out DIR`).
Skips cleanly with a clear message if a toolchain (clang / wasm-ld / wasm32-unknown-unknown)
is missing.

Each `.wasm` is also copied next to its `mod.toml` (`mods/<name>/<name>.wasm`, git-ignored), which is
where `[build] wasm` points: `cargo xtask` / `build.py` pack the mod folders and need them there.

`--remote[=HOST]` builds on the remote machine (see docs/REMOTE_BUILD.md; this PC may lack clang)
and copies the two `.wasm` files back.
"""
import argparse
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

EXAMPLE_DIR = Path(__file__).resolve().parent
REPO_ROOT = EXAMPLE_DIR.parent.parent


def default_out_dir() -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO_ROOT / "target"))
    if not target.is_absolute():
        target = REPO_ROOT / target
    return target / "coin_dash_wasm" / "wasm"


def sysroots() -> list[Path]:
    found: list[Path] = []
    rustc = os.environ.get("RUSTC", "rustc")
    try:
        r = subprocess.run([rustc, "--print", "sysroot"], capture_output=True, text=True, check=False)
        if r.returncode == 0 and r.stdout.strip():
            found.append(Path(r.stdout.strip()))
    except OSError:
        pass
    rustup_home = Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup"))
    toolchains = rustup_home / "toolchains"
    if toolchains.is_dir():
        for entry in sorted(toolchains.iterdir()):
            if entry not in found:
                found.append(entry)
    return found


def find_wasm32_toolchain() -> Path | None:
    for sr in sysroots():
        if (sr / "lib/rustlib/wasm32-unknown-unknown").is_dir() and (sr / "bin/cargo").is_file():
            return sr
    return None


def find_wasm_ld() -> str | None:
    if shutil.which("wasm-ld"):
        return None  # on PATH
    arch = platform.machine()
    hosts = [f"{arch}-unknown-linux-gnu", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]
    for sr in sysroots():
        for h in hosts:
            cand = sr / "lib/rustlib" / h / "bin/gcc-ld/wasm-ld"
            if cand.is_file():
                return str(cand)
    return "MISSING"


def build_c(out_dir: Path) -> bool:
    clang = shutil.which("clang")
    if not clang:
        print("SKIPPED dash_c: clang is not installed")
        return False
    ld = find_wasm_ld()
    if ld == "MISSING":
        print("SKIPPED dash_c: no wasm-ld found (neither on PATH nor in a rustup toolchain)")
        return False

    c_dir = EXAMPLE_DIR / "mods/dash_c"
    inc_guest = REPO_ROOT / "crates/efe-guest/include"
    out_wasm = out_dir / "dash_c.wasm"
    cmd = [
        clang,
        "--target=wasm32",
        "-nostdlib",
        "-O2",
        "-mbulk-memory",
        "-Wl,--no-entry",
        "-Wl,--export=efe_scratch",
    ]
    if ld is not None:
        cmd.append(f"-fuse-ld={ld}")
    cmd += [
        "-I", str(inc_guest),
        "-I", str(c_dir),
        str(c_dir / "mod.c"),
        "-o", str(out_wasm),
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if r.returncode != 0:
        if "unknown target" in r.stderr or "No available targets" in r.stderr:
            print(f"SKIPPED dash_c: clang cannot target wasm32:\n{r.stderr.strip()}")
            return False
        print(f"ERROR building dash_c:\n{r.stderr}", file=sys.stderr)
        sys.exit(r.returncode)
    print(f"built {out_wasm}")
    return True


def build_rust(out_dir: Path) -> bool:
    tc = find_wasm32_toolchain()
    if tc is None:
        print("SKIPPED dash_rust: no Rust toolchain has target wasm32-unknown-unknown "
              "(install with: rustup target add wasm32-unknown-unknown)")
        return False

    cargo = str(tc / "bin/cargo")
    rustc = str(tc / "bin/rustc")
    rust_dir = EXAMPLE_DIR / "mods/dash_rust"
    build_target = out_dir.parent / "rust_build"

    env = os.environ.copy()
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    # Override the repo .cargo/config.toml atomics/shared-memory flags for web builds.
    env["RUSTFLAGS"] = "-Cdebuginfo=0"
    env["RUSTC"] = rustc
    env["CARGO_TARGET_DIR"] = str(build_target)
    env.setdefault("CARGO_BUILD_JOBS", "4")

    cmd = [
        cargo,
        "build",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "--manifest-path",
        str(rust_dir / "Cargo.toml"),
    ]
    r = subprocess.run(cmd, env=env, capture_output=True, text=True, check=False)
    if r.returncode != 0:
        print(f"ERROR building dash_rust:\n{r.stderr}", file=sys.stderr)
        sys.exit(r.returncode)

    built = build_target / "wasm32-unknown-unknown/release/dash_rust.wasm"
    out_wasm = out_dir / "dash_rust.wasm"
    shutil.copy2(built, out_wasm)
    print(f"built {out_wasm}")
    return True


def install_into_mod_folders(out_dir: Path) -> None:
    for name in ("dash_c", "dash_rust"):
        src = out_dir / f"{name}.wasm"
        if src.is_file():
            shutil.copy2(src, EXAMPLE_DIR / "mods" / name / f"{name}.wasm")
            print(f"installed mods/{name}/{name}.wasm")


def build_remote(host: str | None) -> int:
    sys.path.insert(0, str(REPO_ROOT / "scripts"))
    import remote  # scripts/remote.py

    try:
        r = remote.Remote(host=host)
        r.sync()
        rel = EXAMPLE_DIR.relative_to(REPO_ROOT)
        if r.ssh(f"bash -lc 'cd {r.rdir} && python3 {rel}/build_mods.py'") != 0:
            print("error: the remote build failed")
            return 1
        rsh = " ".join(r.ssh_cmd)
        for name in ("dash_c", "dash_rust"):
            dest = EXAMPLE_DIR / "mods" / name
            src = f"{r.host}:{r.rdir}/{rel}/mods/{name}/{name}.wasm"
            if subprocess.run(["rsync", "-az", "-e", rsh, src, str(dest) + "/"]).returncode != 0:
                print(f"error: could not fetch {name}.wasm (was it skipped on the remote?)")
                return 1
            out = default_out_dir()
            out.mkdir(parents=True, exist_ok=True)
            shutil.copy2(dest / f"{name}.wasm", out / f"{name}.wasm")
            print(f"fetched {name}.wasm")
    except remote.RemoteError as e:
        print(f"error: {e}")
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Build coin_dash_wasm Rust & C wasm mods.")
    parser.add_argument("--out", type=Path, default=default_out_dir(), help="Output directory for .wasm files")
    parser.add_argument("--remote", nargs="?", const="", metavar="HOST",
                        help="build on the remote machine (HOST, or EFE_REMOTE_HOST / remotes.env) and fetch the result")
    args = parser.parse_args()
    if args.remote is not None:
        return build_remote(args.remote or None)
    args.out.mkdir(parents=True, exist_ok=True)

    c_ok = build_c(args.out)
    r_ok = build_rust(args.out)
    install_into_mod_folders(args.out)
    if not (c_ok and r_ok):
        print("NOTE: one or both wasm mods were skipped due to missing toolchains.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
