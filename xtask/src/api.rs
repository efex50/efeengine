//! API index (`.efapi`) export and script assembly.
//!
//! * `cargo xtask api-export <example> [-o FILE]` builds the example natively and runs its
//!   `native` binary with `EFE_API_EXPORT=<file>`: the engine then runs headless, the game
//!   registers its functions, and `Engine::game_loop` writes the index and returns.
//! * `build`/`run` (native) write `<example>.efapi` next to the binary and assemble
//!   `examples/<example>/scripts/*.efa` against it into `<profile dir>/scripts/NAME.efvm`
//!   with the efescript CLI. A script whose leading comment block has a line
//!   `; needs-mod: NAME` is skipped: it imports functions a mod registers at run time, so it
//!   can only be assembled against an index exported after that mod is loaded.
//! * `dist` copies the `.efapi` into `dist/<example>/`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::project::{Example, Project};
use crate::util::{cargo, dry_run, root, run, step, warn};

/// How long the game may take to write the index before it is considered stuck.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn profile_dir(project: &Project, release: bool) -> PathBuf {
    project.target_dir.join(if release { "release" } else { "debug" })
}

fn native_bin(project: &Project, release: bool) -> PathBuf {
    profile_dir(project, release).join(format!("native{}", std::env::consts::EXE_SUFFIX))
}

/// Extra cargo arguments for a native build.
pub fn build_flags(release: bool, static_sdl: bool) -> Vec<&'static str> {
    let mut v = vec![];
    if release {
        v.push("--release");
    }
    if static_sdl {
        v.extend(["--features", "efeengine-core/static"]);
    }
    v
}

fn build_native(ex: &Example, release: bool, static_sdl: bool) -> Result<()> {
    let mut c = cargo();
    c.args(["build", "-p", &ex.package, "--bin", "native"]).args(build_flags(release, static_sdl));
    run(c)
}

/// Run `bin` with `EFE_API_EXPORT=out` and check that the file was written.
fn run_export(bin: &Path, out: &Path) -> Result<()> {
    if dry_run() {
        println!("[dry-run] EFE_API_EXPORT={} {}", out.display(), bin.display());
        return Ok(());
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let _ = std::fs::remove_file(out);
    let mut child = Command::new(bin)
        .env("EFE_API_EXPORT", out)
        .current_dir(root())
        .spawn()
        .with_context(|| format!("could not start {}", bin.display()))?;
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if start.elapsed() > EXPORT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{} did not exit within {}s with EFE_API_EXPORT set; the game must call \
                 Engine::game_loop() right after registering its functions",
                bin.display(),
                EXPORT_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        bail!("{} failed with {status} while exporting the API index", bin.display());
    }
    match std::fs::metadata(out) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => bail!(
            "{} exited normally but did not write {}: does the game create its Engine with \
             Engine::new/with_config and call game_loop()? (EFE_API_EXPORT is handled there)",
            bin.display(),
            out.display()
        ),
    }
}

/// `cargo xtask api-export`: build natively, export, print the path.
pub fn api_export(project: &Project, ex: &Example, output: Option<PathBuf>, release: bool, static_sdl: bool) -> Result<()> {
    if !ex.native_bin {
        bail!("example '{}' has no [[bin]] named 'native'; the index is exported by running it", ex.dir);
    }
    let out = match output {
        Some(p) if p.is_absolute() => p,
        Some(p) => std::env::current_dir()?.join(p),
        None => project.target_dir.join("efapi").join(format!("{}.efapi", ex.dir)),
    };
    step(format!("api-export: building {}", ex.package));
    build_native(ex, release, static_sdl)?;
    step(format!("api-export: running {}", native_bin(project, release).display()));
    run_export(&native_bin(project, release), &out)?;
    println!("API index: {}", out.display());
    Ok(())
}

/// After a native build: write `<example>.efapi` next to the binary and assemble the scripts.
/// Returns the directory with the assembled scripts when there are any.
pub fn after_native_build(project: &Project, ex: &Example, release: bool) -> Result<Option<PathBuf>> {
    let src_dir = root().join("examples").join(&ex.dir).join("scripts");
    let sources = script_sources(&src_dir)?;
    let efapi = profile_dir(project, release).join(format!("{}.efapi", ex.dir));
    step(format!("writing {}", efapi.display()));
    if let Err(e) = run_export(&native_bin(project, release), &efapi) {
        if sources.is_empty() {
            warn(format!("no .efapi written: {e:#}"));
            return Ok(None);
        }
        return Err(e.context("scripts need the API index"));
    }
    if sources.is_empty() {
        return Ok(None);
    }
    let out_dir = profile_dir(project, release).join("scripts");
    if !dry_run() {
        std::fs::create_dir_all(&out_dir)?;
    }
    for src in &sources {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("script");
        if let Some(m) = needs_mod(&std::fs::read_to_string(src).unwrap_or_default()) {
            step(format!("skipping {stem}.efa (needs mod `{m}`; assemble it against an index exported with the mod loaded)"));
            continue;
        }
        let out = out_dir.join(format!("{stem}.efvm"));
        step(format!("assembling {} -> {}", src.display(), out.display()));
        let mut c = cargo();
        c.args(["run", "-q", "-p", "efescript", "--", "build"]).arg(src).arg("--api").arg(&efapi).arg("-o").arg(&out);
        run(c)?;
    }
    Ok(Some(out_dir))
}

/// `dist` (web): build the native binary, export the index and copy it into `dist/<example>/`.
/// Failures are warnings: the web build itself does not need the index.
pub fn copy_into_dist(project: &Project, ex: &Example, dist: &Path) {
    let go = || -> Result<()> {
        if !ex.native_bin {
            bail!("example has no 'native' bin");
        }
        let efapi = project.target_dir.join("efapi").join(format!("{}.efapi", ex.dir));
        step(format!("exporting the API index into {}", dist.display()));
        build_native(ex, false, false)?;
        run_export(&native_bin(project, false), &efapi)?;
        if !dry_run() {
            let dest = dist.join(format!("{}.efapi", ex.dir));
            std::fs::copy(&efapi, &dest).with_context(|| format!("copying to {}", dest.display()))?;
            println!("API index: {}", dest.display());
        }
        Ok(())
    };
    if let Err(e) = go() {
        warn(format!("{}.efapi not written to dist: {e:#}", ex.dir));
    }
}

/// `examples/<ex>/scripts/*.efa`, sorted. A missing directory is no scripts.
fn script_sources(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "efa"))
        .collect();
    v.sort();
    Ok(v)
}

/// The mod named by a `; needs-mod: NAME` line in the leading comment block of a script.
pub fn needs_mod(src: &str) -> Option<String> {
    src.lines()
        .map(str::trim)
        .take_while(|l| l.is_empty() || l.starts_with(';'))
        .find_map(|l| {
            let body = l.trim_start_matches(';').trim();
            body.strip_prefix("needs-mod:").map(|m| m.trim().to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_mod_header() {
        assert_eq!(needs_mod("; needs-mod: adder\n; more\nfn x"), Some("adder".into()));
        assert_eq!(needs_mod("; title\n\n;needs-mod:  foo \nimport a.b"), Some("foo".into()));
        // only in the leading comment block
        assert_eq!(needs_mod("import a.b\n; needs-mod: adder"), None);
        assert_eq!(needs_mod("; nothing here\n"), None);
    }
}
