# Building on a remote machine

When this PC is too slow for a full build, compile on another machine over SSH and run the
result here. Works with `./build.py` and `cargo xtask`; `scripts/remote.py` is the standalone
tool both are built on.

```
./build.py --remote run coin_dash native          # host from $EFE_REMOTE_HOST / remotes.env
./build.py --remote=myserver build coin_dash native --release
cargo xtask --remote=myserver run coin_dash native
cargo xtask --remote editor script_demo
./build.py --remote check                         # cargo check on the remote
cargo xtask --remote test
```

`--remote` is global (before or after the subcommand). Bare `--remote` resolves the host from
`$EFE_REMOTE_HOST`, then `EFE_REMOTE_HOST` in `remotes.env`; `--remote=HOST` overrides both. HOST is
any ssh destination (a `~/.ssh/config` alias, `user@host`, an IP). There is no default host.

`remotes.env` sits in the repo root and is git ignored, so machine names never reach git. Start from
the committed template: `cp remotes.env.example remotes.env`. It is `KEY=VALUE` lines (`#` comments);
the environment wins over the file.

| setting | key (environment or `remotes.env`) | default |
| --- | --- | --- |
| host | `EFE_REMOTE_HOST` | none (required) |
| directory on the remote | `EFE_REMOTE_DIR` | `~/remote-builds/<repo folder>` |
| parallelism (`CARGO_BUILD_JOBS`, `RUST_TEST_THREADS`) | `EFE_REMOTE_JOBS` | remote cores minus 3 (min 1) |

## What happens

1. The working tree (committed or not; not `target/`, `.git/`, `.claude/`, `dist/`) is rsynced to
   `HOST:DIR`. One ssh connection is reused (ControlMaster, 10 minutes).
2. `cargo` runs there through `bash -lc`.
3. Only the needed executables (the example's binary, plus `efe-editor` for `editor`) are copied
   into the local `target/<profile>/` ( `$CARGO_TARGET_DIR` is respected).
4. Everything after the build is unchanged and local: xtask runs the binary to write the `.efapi`,
   copies scripts, installs mods, and `run` / `editor` start the fetched binaries directly (from the
   repo root, with `EFE_SCRIPT_DIR` as usual) instead of `cargo run`, which would rebuild here.

`check` and `test` run entirely on the remote. The wasm part of `check` runs there only if the
remote's nightly has the `wasm32-unknown-unknown` target; otherwise it is skipped with a note.
`--dry-run` prints the ssh/rsync commands.

## Limits

- Native builds only: `web`, `dist` and `serve` fail with "--remote supports native builds only".
- Same CPU architecture, and the remote glibc must not be newer than the local one
  (`python3 scripts/remote.py doctor` checks both, plus rsync and cargo on the remote).
- The remote needs rustup (the repo's toolchain), rsync and the SDL3 dev files (or use `--static`
  with cmake and a C compiler there). This PC does not need dev files, only the `libSDL3.so` runtime
  library; a warning is printed when it is missing and `--static` is not given. The local SDL3 and
  cmake preflight is skipped.
- The first build on the remote compiles everything; later ones reuse its `target/`.
