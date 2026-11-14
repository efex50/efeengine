//! Small shared helpers: repo root, step output, running commands, PATH lookup.

use std::ffi::OsStr;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

static DRY_RUN: AtomicBool = AtomicBool::new(false);

pub fn set_dry_run(v: bool) {
    DRY_RUN.store(v, Ordering::Relaxed);
}

pub fn dry_run() -> bool {
    DRY_RUN.load(Ordering::Relaxed)
}

/// Repository root (the workspace this xtask lives in).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

pub fn color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn paint(code: &str, s: &str) -> String {
    if color() { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
}

pub fn step(msg: impl AsRef<str>) {
    println!("{} {}", paint("1;36", "==>"), msg.as_ref());
}

pub fn warn(msg: impl AsRef<str>) {
    eprintln!("{} {}", paint("1;33", "warning:"), msg.as_ref());
}

pub fn ok_line(msg: impl AsRef<str>) {
    println!("  {}    {}", paint("1;32", "ok"), msg.as_ref());
}

pub fn miss_line(msg: impl AsRef<str>, fix: impl AsRef<str>) {
    println!("  {}  {}\n        fix: {}", paint("1;31", "MISS"), msg.as_ref(), fix.as_ref());
}

pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// Render a command for display (quoting arguments that contain spaces).
pub fn show(cmd: &Command) -> String {
    let quote = |s: &OsStr| {
        let s = s.to_string_lossy();
        if s.contains(' ') { format!("\"{s}\"") } else { s.into_owned() }
    };
    std::iter::once(quote(cmd.get_program()))
        .chain(cmd.get_args().map(quote))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run a command (inheriting stdio) from the repo root unless a dir was set.
/// In dry-run mode the command is only printed.
pub fn run(mut cmd: Command) -> Result<()> {
    if dry_run() {
        println!("[dry-run] {}", show(&cmd));
        return Ok(());
    }
    if cmd.get_current_dir().is_none() {
        cmd.current_dir(root());
    }
    let shown = show(&cmd);
    let status = cmd
        .status()
        .with_context(|| format!("could not start: {shown}"))?;
    if !status.success() {
        bail!("command failed ({}): {shown}", status.code().map_or("signal".into(), |c| c.to_string()));
    }
    Ok(())
}

/// Run a command and capture stdout (trimmed). Errors if it can't start or fails.
pub fn output(cmd: &mut Command) -> Result<String> {
    let shown = show(cmd);
    let out = cmd.output().with_context(|| format!("could not start: {shown}"))?;
    if !out.status.success() {
        bail!("command failed: {shown}\n{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `cargo` from the environment (set by cargo when running `cargo xtask`).
pub fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// `cargo +nightly` through the rustup proxy. ($CARGO points at the toolchain's
/// real cargo binary, which doesn't understand `+toolchain`.) Without rustup,
/// fall back to plain cargo and hope it is nightly.
pub fn cargo_nightly() -> Command {
    match (which("rustup"), which("cargo")) {
        (Some(_), Some(proxy)) => {
            let mut c = Command::new(proxy);
            c.arg("+nightly");
            c
        }
        _ => cargo(),
    }
}

/// Find an executable on PATH (or accept an existing path).
pub fn which(name: impl AsRef<OsStr>) -> Option<PathBuf> {
    let name = name.as_ref();
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        exts.iter().find_map(|ext| {
            let mut cand = dir.join(name);
            if !ext.is_empty() {
                cand.set_extension(ext.trim_start_matches('.'));
            }
            cand.is_file().then_some(cand)
        })
    })
}
