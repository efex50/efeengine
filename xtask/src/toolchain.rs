//! Checks for the tools the build needs, with install hints.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use crate::project::{WASM_TARGET, locked_version};
use crate::util::{output, which};

/// A missing tool and the command that installs it.
pub struct Problem {
    pub what: String,
    pub fix: String,
}

/// Problems with nightly / rust-src / the wasm target (empty = all good).
pub fn rust_problems() -> Vec<Problem> {
    let p = |what: &str, fix: String| Problem { what: what.into(), fix };
    if which("rustup").is_none() {
        return vec![p("rustup", "install rustup: https://rustup.rs".into())];
    }
    let Ok(sysroot) = output(Command::new("rustc").args(["+nightly", "--print", "sysroot"])) else {
        return vec![p(
            "nightly toolchain",
            format!("rustup toolchain install nightly --component rust-src --target {WASM_TARGET}"),
        )];
    };
    let sysroot = PathBuf::from(sysroot);
    let mut out = vec![];
    if !sysroot.join("lib/rustlib/src/rust/library/Cargo.lock").is_file() {
        out.push(p("rust-src (nightly)", "rustup component add rust-src --toolchain nightly".into()));
    }
    if !sysroot.join("lib/rustlib").join(WASM_TARGET).is_dir() {
        out.push(p(
            &format!("{WASM_TARGET} target (nightly)"),
            format!("rustup target add {WASM_TARGET} --toolchain nightly"),
        ));
    }
    out
}

pub struct WasmBindgen {
    pub path: Option<PathBuf>,
    pub have: Option<String>,
    pub want: String,
}

impl WasmBindgen {
    pub fn hint(&self) -> String {
        format!("cargo install wasm-bindgen-cli --version {} --locked", self.want)
    }
}

/// The wasm-bindgen CLI named by $WASM_BINDGEN (default `wasm-bindgen`) and its version.
pub fn wasm_bindgen() -> Result<WasmBindgen> {
    let want = locked_version("wasm-bindgen")?;
    let name = std::env::var_os("WASM_BINDGEN").unwrap_or_else(|| "wasm-bindgen".into());
    let path = which(&name);
    let have = path.as_ref().and_then(|p| {
        output(Command::new(p).arg("--version"))
            .ok()
            .and_then(|s| s.split_whitespace().nth(1).map(str::to_string))
    });
    Ok(WasmBindgen { path, have, want })
}

/// Everything a web build needs; returns the wasm-bindgen path.
pub fn require_web() -> Result<PathBuf> {
    if let Some(p) = rust_problems().into_iter().next() {
        bail!("{} missing. Install with: {}", p.what, p.fix);
    }
    let wb = wasm_bindgen()?;
    match (&wb.path, &wb.have) {
        (None, _) => bail!("wasm-bindgen CLI not found. Install with:\n  {}", wb.hint()),
        (Some(_), have) if have.as_deref() != Some(wb.want.as_str()) => bail!(
            "wasm-bindgen CLI version '{}' != Cargo.lock version '{}'. Install with:\n  {}",
            have.clone().unwrap_or_default(),
            wb.want,
            wb.hint()
        ),
        (Some(p), _) => Ok(p.clone()),
    }
}

pub fn has_c_compiler() -> bool {
    ["cc", "gcc", "clang", "cl"].iter().any(|c| which(c).is_some())
}

/// SDL3 development files: pkg-config, or libSDL3.so/.dylib in a usual lib dir or LIBRARY_PATH.
pub fn sdl3_found() -> bool {
    if which("pkg-config").is_some()
        && Command::new("pkg-config").args(["--exists", "sdl3"]).status().is_ok_and(|s| s.success())
    {
        return true;
    }
    let mut dirs: Vec<PathBuf> = [
        "/usr/lib",
        "/usr/lib64",
        "/usr/local/lib",
        "/usr/local/lib64",
        "/lib",
        "/lib64",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/opt/homebrew/lib",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(lp) = std::env::var_os("LIBRARY_PATH") {
        dirs.extend(std::env::split_paths(&lp));
    }
    dirs.iter().any(|d| ["libSDL3.so", "libSDL3.dylib"].iter().any(|n| Path::new(d).join(n).exists()))
}
