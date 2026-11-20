//! API index (`.efapi`) export and script checks.
//!
//! * `cargo xtask api-export <example> [-o FILE]` builds the example natively and runs its
//!   `native` binary with `EFE_API_EXPORT=<file>`: the engine then runs headless, the game
//!   registers its functions, and `Engine::game_loop` writes the index and returns.
//! * `build`/`run` (native) write `<example>.efapi` next to the binary (for the language
//!   server) and copy `examples/<example>/scripts/*.efa` into `<profile dir>/scripts/`. The
//!   game compiles them at run time through its cache; nothing is assembled here.
//! * `check-scripts <example>` exports the index and assembles every script against it in
//!   process, failing with all diagnostics. A script whose leading comment block has a line
//!   `; needs-mod: NAME` is skipped: it imports functions a mod registers at run time, so it
//!   can only be checked against an index exported after that mod is loaded.
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

fn native_bin(project: &Project, ex: &Example, release: bool) -> PathBuf {
    profile_dir(project, release).join(format!("{}{}", ex.package, std::env::consts::EXE_SUFFIX))
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

/// Build the example's binary: with cargo here, or on the remote (`--remote`) followed by
/// fetching the binary into this machine's `target/<profile>/`.
pub fn build_native(project: &Project, ex: &Example, release: bool, static_sdl: bool) -> Result<()> {
    if let Some(r) = crate::remote::get() {
        let mut args: Vec<String> = ["-p", &ex.package, "--bin", &ex.package].map(String::from).to_vec();
        args.extend(build_flags(release, static_sdl).into_iter().map(String::from));
        return crate::remote::build_and_fetch(r, &project.target_dir, &args, release, &[&ex.package]);
    }
    let mut c = cargo();
    c.args(["build", "-p", &ex.package, "--bin", &ex.package]).args(build_flags(release, static_sdl));
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
        bail!("example '{}' has no binary target; the index is exported by running it", ex.dir);
    }
    let out = match output {
        Some(p) if p.is_absolute() => p,
        Some(p) => std::env::current_dir()?.join(p),
        None => project.target_dir.join("efapi").join(format!("{}.efapi", ex.dir)),
    };
    step(format!("api-export: building {}", ex.package));
    build_native(project, ex, release, static_sdl)?;
    step(format!("api-export: running {}", native_bin(project, ex, release).display()));
    run_export(&native_bin(project, ex, release), &out)?;
    println!("API index: {}", out.display());
    Ok(())
}

/// After a native build: write `<example>.efapi` next to the binary and copy the script sources
/// into `<profile dir>/scripts/`. Returns that directory when there are scripts. A missing index
/// is a warning: the game does not need it to run.
pub fn after_native_build(project: &Project, ex: &Example, release: bool) -> Result<Option<PathBuf>> {
    let sources = script_sources(&root().join("examples").join(&ex.dir).join("scripts"))?;
    let efapi = profile_dir(project, release).join(format!("{}.efapi", ex.dir));
    step(format!("writing {}", efapi.display()));
    if let Err(e) = run_export(&native_bin(project, ex, release), &efapi) {
        warn(format!("no .efapi written: {e:#}"));
    }
    if sources.is_empty() {
        return Ok(None);
    }
    let out_dir = profile_dir(project, release).join("scripts");
    step(format!("copying {} script(s) to {}", sources.len(), out_dir.display()));
    if dry_run() {
        return Ok(Some(out_dir));
    }
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    // assembled modules from before scripts were copied as source
    for e in std::fs::read_dir(&out_dir).with_context(|| format!("reading {}", out_dir.display()))? {
        let path = e?.path();
        if path.extension().is_some_and(|x| x == "efvm") {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    for src in &sources {
        let dest = out_dir.join(src.file_name().unwrap_or_default());
        std::fs::copy(src, &dest).with_context(|| format!("copying to {}", dest.display()))?;
    }
    Ok(Some(out_dir))
}

/// `cargo xtask check-scripts <example>`: export the game index, then assemble every
/// `examples/<example>/scripts/*.efa` against it. A script with a `; needs-mod: NAME` header is
/// assembled against the game index merged with the API fragment of the mod folder
/// `examples/<example>/mods/NAME` (and the mods it depends on), built from its declaration. Fails
/// listing the diagnostics of every script that does not assemble, and with a clear error when a
/// needed mod folder cannot be found or built.
pub fn check_scripts(project: &Project, ex: &Example) -> Result<()> {
    let sources = script_sources(&root().join("examples").join(&ex.dir).join("scripts"))?;
    if sources.is_empty() {
        println!("{}: no scripts to check", ex.dir);
        return Ok(());
    }
    let index_path = project.target_dir.join("efapi").join(format!("{}.efapi", ex.dir));
    api_export(project, ex, Some(index_path.clone()), false, false)?;
    let index = efescript::pack::read_api_input(&index_path).map_err(anyhow::Error::msg)?;
    let opts = efescript::asm::AsmOptions { api: Some(&index), ..Default::default() };
    let mut merged: std::collections::BTreeMap<String, efescript::pack::ApiIndex> = Default::default();

    let mut failures = vec![];
    let mut checked = 0;
    for src in &sources {
        let text = std::fs::read_to_string(src).with_context(|| format!("reading {}", src.display()))?;
        let mut script_opts = opts;
        if let Some(m) = needs_mod(&text) {
            if !merged.contains_key(&m) {
                let frags = crate::mods::fragments_for(ex, &index_path, &m)
                    .with_context(|| format!("{} needs mod `{m}`", src.display()))?;
                let mut parts: Vec<&efescript::pack::ApiIndex> = vec![&index];
                parts.extend(frags.iter());
                let ix = efescript::pack::ApiIndex::merge(&parts)
                    .map_err(|e| anyhow::anyhow!("{}: merging the game index with mod `{m}`: {e}", src.display()))?;
                merged.insert(m.clone(), ix);
            }
            step(format!("{}: using the API fragment of mod `{m}`", src.display()));
            script_opts.api = merged.get(&m);
        }
        checked += 1;
        let (files, root_name) = efescript::asm::FsSource::for_file(&src.display().to_string());
        match efescript::asm::assemble_program(&root_name, &files, &script_opts) {
            Ok(_) => println!("  ok: {}", src.display()),
            Err(d) => failures.push(format!("{}:\n{d}", src.display())),
        }
    }
    if !failures.is_empty() {
        bail!("{} of {checked} script(s) do not assemble:\n{}", failures.len(), failures.join("\n"));
    }
    println!("{checked} script(s) assembled against {}", index_path.display());
    Ok(())
}

/// `dist` (web): build the native binary, export the index and copy it into `dist/<example>/`.
/// Failures are warnings: the web build itself does not need the index.
pub fn copy_into_dist(project: &Project, ex: &Example, dist: &Path) {
    let go = || -> Result<()> {
        if !ex.native_bin {
            bail!("example has no binary target");
        }
        let efapi = project.target_dir.join("efapi").join(format!("{}.efapi", ex.dir));
        step(format!("exporting the API index into {}", dist.display()));
        build_native(project, ex, false, false)?;
        run_export(&native_bin(project, ex, false), &efapi)?;
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
