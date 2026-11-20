//! `--remote[=HOST]`: build on another machine over SSH, run on this one.
//!
//! The working tree is rsynced to `HOST:DIR`, cargo runs there through `bash -lc` with
//! CARGO_BUILD_JOBS / RUST_TEST_THREADS set to the remote's cores minus 3 (unless overridden),
//! and the needed executables are copied back into the local `target/<profile>/`. Same behaviour
//! as `scripts/remote.py`, which this mirrors. Settings (never hardcoded):
//!
//! * host: `--remote=HOST`, else `$EFE_REMOTE_HOST`, else `EFE_REMOTE_HOST` in `remotes.env`
//! * directory: `$EFE_REMOTE_DIR` / `remotes.env`, default `remote-builds/<repo folder>`
//! * jobs: `$EFE_REMOTE_JOBS` / `remotes.env`
//!
//! `remotes.env` (repo root, git ignored; see `remotes.env.example`) holds `KEY=VALUE` lines.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

use crate::util::{dry_run, root, show, step};

/// Never copied: build output, git data, agent worktrees, web output.
const EXCLUDES: [&str; 8] =
    ["/target/", "/.git/", "/.claude/", "**/target/", "**/web/pkg/", "/dist/", "__pycache__/", "/remotes.env"];

/// Reuse one ssh connection for the rsync and the cargo calls (%C: hash of host/port/user).
const SSH_OPTS: [&str; 8] = [
    "-o", "ControlMaster=auto",
    "-o", "ControlPath=~/.ssh/cm-remotebuild-%C",
    "-o", "ControlPersist=10m",
    "-o", "ConnectTimeout=10",
];

pub struct Remote {
    pub host: String,
    pub dir: String,
    pub jobs: Option<String>,
    synced: AtomicBool,
}

static REMOTE: OnceLock<Remote> = OnceLock::new();

/// The remote selected with `--remote`, if any.
pub fn get() -> Option<&'static Remote> {
    REMOTE.get()
}

/// Turn `--remote` on. `host` is the `=HOST` part, if given.
pub fn enable(host: Option<String>) -> Result<()> {
    let r = Remote::resolve(host)?;
    let _ = REMOTE.set(r);
    Ok(())
}

const ENV_FILE: &str = "remotes.env";

/// The value of `key` in `KEY=VALUE` text (`#` comments, optional `export ` and quotes).
fn env_file_value(text: &str, key: &str) -> Option<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).find_map(|l| {
        let (k, v) = l.split_once('=')?;
        let k = k.trim();
        let k = k.strip_prefix("export ").unwrap_or(k).trim();
        let v = v.split(" #").next().unwrap_or("").trim().trim_matches(|c| c == '"' || c == '\'');
        (k == key && !v.is_empty()).then(|| v.to_string())
    })
}

/// `$key` from the environment, else from `remotes.env` in the repo root.
fn setting(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty()).or_else(|| {
        let text = std::fs::read_to_string(root().join(ENV_FILE)).ok()?;
        env_file_value(&text, key)
    })
}

/// The directory cargo puts a profile in: `release` or `debug`.
pub fn profile_name(release: bool) -> &'static str {
    if release { "release" } else { "debug" }
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

impl Remote {
    fn resolve(host: Option<String>) -> Result<Self> {
        let host = host
            .filter(|h| !h.is_empty())
            .or_else(|| setting("EFE_REMOTE_HOST"))
            .context(
                "no remote host: pass --remote=HOST, set EFE_REMOTE_HOST, or put EFE_REMOTE_HOST=HOST in \
                 remotes.env at the repo root (see remotes.env.example)",
            )?;
        let dir = setting("EFE_REMOTE_DIR").unwrap_or_else(|| {
            format!("remote-builds/{}", root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        });
        let jobs = setting("EFE_REMOTE_JOBS");
        Ok(Self { host, dir, jobs, synced: AtomicBool::new(false) })
    }

    fn ssh_base(&self) -> Command {
        let mut c = Command::new("ssh");
        c.args(SSH_OPTS);
        c
    }

    /// `ssh -o ... ` as one string, for rsync's `-e`.
    fn ssh_e(&self) -> String {
        format!("ssh {}", SSH_OPTS.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "))
    }

    /// Run a shell command on the remote (stdio inherited). Prints it only in dry-run mode.
    fn ssh_run(&self, cmd: &str) -> Result<bool> {
        let mut c = self.ssh_base();
        c.arg(&self.host).arg(cmd);
        if dry_run() {
            println!("[dry-run] {}", show(&c));
            return Ok(true);
        }
        Ok(c.status().with_context(|| format!("could not start ssh to {}", self.host))?.success())
    }

    /// Run a shell command on the remote and capture stdout. `None` if it failed.
    fn ssh_out(&self, cmd: &str) -> Result<Option<String>> {
        let mut c = self.ssh_base();
        c.arg(&self.host).arg(cmd);
        if dry_run() {
            println!("[dry-run] {}", show(&c));
            return Ok(None);
        }
        let out = c.output().with_context(|| format!("could not start ssh to {}", self.host))?;
        Ok(out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
    }

    /// Copy the working tree to the remote (once per process).
    pub fn sync(&self) -> Result<()> {
        if self.synced.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        step(format!("sync -> {}:{}", self.host, self.dir));
        if !self.ssh_run(&format!("mkdir -p {}", self.dir))? {
            bail!("cannot reach {}; try: python3 scripts/remote.py doctor", self.host);
        }
        let mut c = Command::new("rsync");
        c.args(["-az", "--delete", "-e", &self.ssh_e()]);
        c.args(EXCLUDES.iter().map(|e| format!("--exclude={e}")));
        c.arg(format!("{}/", root().display())).arg(format!("{}:{}/", self.host, self.dir));
        if dry_run() {
            println!("[dry-run] {}", show(&c));
            return Ok(());
        }
        if !c.status().context("could not start rsync")?.success() {
            bail!("rsync failed");
        }
        Ok(())
    }

    fn remote_cmd(&self, cargo_args: &[String]) -> String {
        let jobs = match &self.jobs {
            Some(j) => shell_quote(j),
            None => "$(( $(nproc) > 4 ? $(nproc) - 3 : 1 ))".to_string(),
        };
        let args = cargo_args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
        // Login shells pick up rustup's PATH from ~/.profile / ~/.cargo/env.
        let inner = format!(
            "cd {} && J={jobs} && export CARGO_BUILD_JOBS=$J RUST_TEST_THREADS=$J && echo \"==> jobs: $J\" >&2 && cargo {args}",
            self.dir
        );
        format!("bash -lc {}", shell_quote(&inner))
    }

    /// Sync, then run `cargo <args>` on the remote.
    pub fn cargo(&self, args: &[String]) -> Result<()> {
        self.sync()?;
        step(format!("{}: cargo {}", self.host, args.join(" ")));
        if !self.ssh_run(&self.remote_cmd(args))? {
            bail!("remote cargo {} failed on {}", args.first().map(String::as_str).unwrap_or(""), self.host);
        }
        Ok(())
    }

    /// Copy the named executables of the remote `target/<profile>/` into `dest`.
    pub fn fetch(&self, profile: &str, dest: &Path, names: &[&str]) -> Result<()> {
        step(format!("fetch {} from {}:target/{profile} -> {}", names.join(" "), self.host, dest.display()));
        let mut c = Command::new("rsync");
        c.args(["-az", "-e", &self.ssh_e(), "--files-from=-"]);
        c.arg(format!("{}:{}/target/{profile}/", self.host, self.dir)).arg(format!("{}/", dest.display()));
        if dry_run() {
            println!("[dry-run] {} (files: {})", show(&c), names.join(" "));
            return Ok(());
        }
        std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
        let mut child = c.stdin(Stdio::piped()).spawn().context("could not start rsync")?;
        let mut stdin = child.stdin.take().context("rsync stdin")?;
        stdin.write_all(format!("{}\n", names.join("\n")).as_bytes())?;
        drop(stdin);
        if !child.wait()?.success() {
            bail!("fetching {} from {}:{}/target/{profile} failed (was it built there?)", names.join(" "), self.host, self.dir);
        }
        Ok(())
    }

    /// True if the remote's nightly toolchain has this rustup target installed.
    pub fn has_target(&self, triple: &str) -> Result<bool> {
        if dry_run() {
            // Print the probe, assume the target is there so the dry run shows every command.
            self.ssh_out("bash -lc 'rustup +nightly target list --installed'")?;
            return Ok(true);
        }
        let out = self.ssh_out("bash -lc 'rustup +nightly target list --installed'")?;
        Ok(out.is_some_and(|o| o.split_whitespace().any(|t| t == triple)))
    }
}

/// `cargo build <args>` on the remote, then fetch `names` (executables of the profile) into the
/// local `<target dir>/<profile>/`.
pub fn build_and_fetch(r: &Remote, local_target: &Path, args: &[String], release: bool, names: &[&str]) -> Result<()> {
    let mut a = vec!["build".to_string()];
    a.extend(args.iter().cloned());
    r.cargo(&a)?;
    if !names.is_empty() {
        let profile = profile_name(release);
        r.fetch(profile, &local_target.join(profile), names)?;
    }
    Ok(())
}

/// `libSDL3.so` loadable here (a remotely built, dynamically linked binary needs it at run time).
pub fn warn_if_no_local_sdl3(static_sdl: bool) {
    if static_sdl || cfg!(windows) {
        return;
    }
    let ldconfig = Command::new("ldconfig").arg("-p").output().or_else(|_| Command::new("/sbin/ldconfig").arg("-p").output());
    let in_cache = ldconfig.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("libSDL3.so"));
    if !in_cache && !crate::toolchain::sdl3_found() {
        crate::util::warn(
            "no libSDL3.so found on this PC: a remotely built binary needs SDL3 at run time \
             (install it, e.g. dnf install SDL3 / apt install libsdl3-0, or pass --static)",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_values() {
        let t = "# comment\nEFE_REMOTE_HOST=box  # inline\nexport EFE_REMOTE_DIR=\"builds/x\"\nEFE_REMOTE_JOBS=\n";
        assert_eq!(env_file_value(t, "EFE_REMOTE_HOST").as_deref(), Some("box"));
        assert_eq!(env_file_value(t, "EFE_REMOTE_DIR").as_deref(), Some("builds/x"));
        assert_eq!(env_file_value(t, "EFE_REMOTE_JOBS"), None);
        assert_eq!(env_file_value(t, "OTHER"), None);
    }

    fn remote(jobs: Option<&str>) -> Remote {
        Remote { host: "h".into(), dir: "remote-builds/x".into(), jobs: jobs.map(Into::into), synced: AtomicBool::new(false) }
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("--bin"), "--bin");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn remote_command_uses_login_shell_and_jobs() {
        let c = remote(None).remote_cmd(&["build".into(), "-p".into(), "coin_dash".into()]);
        assert!(c.starts_with("bash -lc "));
        assert!(c.contains("nproc") && c.contains("CARGO_BUILD_JOBS=$J RUST_TEST_THREADS=$J"));
        assert!(c.contains("cargo build -p coin_dash"));
        assert!(remote(Some("6")).remote_cmd(&["check".into()]).contains("J=6 "));
    }

    #[test]
    fn profile_names() {
        assert_eq!(profile_name(true), "release");
        assert_eq!(profile_name(false), "debug");
    }
}
