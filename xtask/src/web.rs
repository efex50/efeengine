//! Web build: cargo (nightly, build-std for wasm threads) → wasm-bindgen → dist/<example>/.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::project::{Example, Project, WASM_TARGET};
use crate::toolchain;
use crate::util::{cargo_nightly, dry_run, root, run, step};

/// Build `ex` for the web and assemble `dist/<example>/`. Returns the output directory.
pub fn build(project: &Project, ex: &Example, release: bool) -> Result<PathBuf> {
    let out = root().join("dist").join(&ex.dir);
    let crate_name = ex.crate_name();
    let profile = if release { "release" } else { "debug" };

    let wasm_bindgen = if dry_run() {
        PathBuf::from(std::env::var_os("WASM_BINDGEN").unwrap_or_else(|| "wasm-bindgen".into()))
    } else {
        toolchain::require_web()?
    };

    step(format!("cargo build ({profile}) {} for {WASM_TARGET}", ex.package));
    let mut cmd = cargo_nightly();
    cmd.args(["build", "--lib", "-p", &ex.package, "--target", WASM_TARGET])
        .args(["-Z", "build-std=panic_abort,std"]);
    if release {
        cmd.arg("--release");
    }
    run(cmd)?;

    let wasm = project.target_dir.join(WASM_TARGET).join(profile).join(format!("{crate_name}.wasm"));
    if !dry_run() {
        if !wasm.is_file() {
            bail!("expected wasm not found: {}", wasm.display());
        }
        if out.exists() {
            fs::remove_dir_all(&out).with_context(|| format!("removing {}", out.display()))?;
        }
        fs::create_dir_all(out.join("pkg"))?;
    }

    step("wasm-bindgen --target web");
    let mut wb = Command::new(&wasm_bindgen);
    wb.args(["--target", "web", "--out-dir"])
        .arg(out.join("pkg"))
        .args(["--out-name", &crate_name])
        .arg(&wasm);
    run(wb)?;

    step(format!("assembling {}", out.display()));
    if dry_run() {
        println!("[dry-run] copy templates/web (+ examples/{}/web overrides) into {}", ex.dir, out.display());
        crate::api::copy_into_dist(project, ex, &out);
        return Ok(out);
    }
    copy_template(&root().join("templates/web"), &out, &crate_name, &ex.dir)?;
    copy_overrides(&root().join("examples").join(&ex.dir).join("web"), &out)?;
    crate::api::copy_into_dist(project, ex, &out);
    Ok(out)
}

/// Copy templates/web into `out`, replacing {{CRATE}} / {{TITLE}} in text files.
fn copy_template(tpl: &Path, out: &Path, crate_name: &str, title: &str) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(tpl)
        .with_context(|| format!("reading {}", tpl.display()))?
        .collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        if !e.file_type()?.is_file() || name == "README.md" {
            continue;
        }
        let data = fs::read(e.path())?;
        let dest = out.join(&name);
        match String::from_utf8(data) {
            Ok(text) => fs::write(&dest, text.replace("{{CRATE}}", crate_name).replace("{{TITLE}}", title))?,
            Err(bin) => fs::write(&dest, bin.into_bytes())?,
        }
    }
    Ok(())
}

/// Copy examples/<ex>/web/** over the template (except a pkg/ dir).
fn copy_overrides(dir: &Path, out: &Path) -> Result<()> {
    fn walk(base: &Path, dir: &Path, out: &Path) -> Result<()> {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            let rel = path.strip_prefix(base)?;
            if rel.starts_with("pkg") {
                continue;
            }
            if e.file_type()?.is_dir() {
                walk(base, &path, out)?;
            } else {
                let dest = out.join(rel);
                fs::create_dir_all(dest.parent().unwrap_or(out))?;
                fs::copy(&path, &dest)?;
                println!("    override: {}", rel.display());
            }
        }
        Ok(())
    }
    if dir.is_dir() { walk(dir, dir, out) } else { Ok(()) }
}
