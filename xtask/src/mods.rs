//! Mod packages: `examples/<example>/mods/<name>/` folders (each with a `mod.toml`) packed into
//! `.efmod` files with `efescript::pack::pack_dir`.
//!
//! * `cargo xtask mods <example>` packs them into `target/mods/<example>/`.
//! * `build` and `dist` (web and native) pack them too and copy the packages into `mods/`
//!   next to the built game: `dist/<example>/mods/` and `<profile dir>/mods/`.
//!
//! The game index the mods are packed against is the `.efapi` the `api` step writes. A mod
//! that imports functions of another mod is packed after it: packing repeats in passes, each
//! mod packed so far adds its index for the next ones, until nothing more can be packed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::api;
use crate::project::{Example, Project};
use crate::util::{dry_run, root, step, warn};

/// `examples/<example>/mods`.
fn mods_dir(ex: &Example) -> PathBuf {
    root().join("examples").join(&ex.dir).join("mods")
}

/// Where `api` writes the game's index: `target/efapi/<example>.efapi`.
pub fn game_index_path(project: &Project, ex: &Example) -> PathBuf {
    project.target_dir.join("efapi").join(format!("{}.efapi", ex.dir))
}

/// The mod folders of `ex` (`mods/*/` that contain a `mod.toml`), sorted by name.
/// A missing `mods/` directory means no mods.
pub fn mod_folders(ex: &Example) -> Result<Vec<PathBuf>> {
    let dir = mods_dir(ex);
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("mod.toml").is_file())
        .collect();
    v.sort();
    Ok(v)
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Packs `mods` against the game index at `index` and the mods packed before them, writing the
/// packages into `out_dir` (emptied first). Returns the written package paths.
fn pack_all(index: &Path, mods: &[PathBuf], out_dir: &Path) -> Result<Vec<PathBuf>> {
    let game = efescript::pack::read_api_input(index).map_err(anyhow::Error::msg)?;
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir).with_context(|| format!("removing {}", out_dir.display()))?;
    }
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let mut indices = vec![game];
    let mut written = vec![];
    let mut last_err: BTreeMap<String, String> = BTreeMap::new();
    let mut todo: Vec<PathBuf> = mods.to_vec();
    loop {
        let mut progress = false;
        let mut still_todo = vec![];
        let mut new_indices = vec![];
        {
            let refs: Vec<_> = indices.iter().collect();
            for dir in todo {
                let name = folder_name(&dir);
                match efescript::pack::pack_dir(&dir, &refs) {
                    Ok(packed) => {
                        let path = out_dir.join(&packed.file_name);
                        std::fs::write(&path, &packed.bytes)
                            .with_context(|| format!("writing {}", path.display()))?;
                        new_indices.push(efescript::pack::read_api_input(&path).map_err(anyhow::Error::msg)?);
                        println!("  packed {name} -> {}", path.display());
                        written.push(path);
                        progress = true;
                    }
                    Err(e) => {
                        last_err.insert(name, e);
                        still_todo.push(dir);
                    }
                }
            }
        }
        indices.extend(new_indices);
        todo = still_todo;
        if todo.is_empty() || !progress {
            break;
        }
    }
    if !todo.is_empty() {
        let names: Vec<String> = todo.iter().map(|d| folder_name(d)).collect();
        for n in &names {
            warn(format!(
                "mod `{n}` not packed: {}",
                last_err.get(n).map_or("unknown error", String::as_str)
            ));
        }
        bail!("{} of {} mod(s) could not be packed: {}", names.len(), mods.len(), names.join(", "));
    }
    Ok(written)
}

/// Exports the game's index with `api` (builds and runs the native binary) and returns its path.
fn export_game_index(project: &Project, ex: &Example) -> Result<PathBuf> {
    let path = game_index_path(project, ex);
    api::api_export(project, ex, Some(path.clone()), false, false)?;
    Ok(path)
}

/// `cargo xtask mods <example>`: pack every mod into `target/mods/<example>/`.
pub fn mods_cmd(project: &Project, ex: &Example, preflight: bool) -> Result<()> {
    let mods = mod_folders(ex)?;
    if mods.is_empty() {
        println!("{}: no mods to pack (no examples/{}/mods/*/mod.toml)", ex.dir, ex.dir);
        return Ok(());
    }
    if dry_run() {
        println!("[dry-run] pack {} mod(s) of {} into target/mods/{}", mods.len(), ex.dir, ex.dir);
        return Ok(());
    }
    if preflight {
        crate::preflight_native(false)?;
    }
    step(format!("mods: exporting the API index of {}", ex.dir));
    let index = export_game_index(project, ex)?;
    step(format!("mods: packing {} mod(s) of {}", mods.len(), ex.dir));
    let out = project.target_dir.join("mods").join(&ex.dir);
    let written = pack_all(&index, &mods, &out)?;
    println!("{} package(s) in {}", written.len(), out.display());
    Ok(())
}

/// For `build` and `dist`: packs the mods of `ex` against the game index at `index` and copies
/// the packages into `dest`. Does nothing when the example has no mods.
pub fn pack_into(project: &Project, ex: &Example, index: &Path, dest: &Path) -> Result<()> {
    let mods = mod_folders(ex)?;
    if mods.is_empty() {
        return Ok(());
    }
    if dry_run() {
        println!("[dry-run] pack {} mod(s) of {} into {}", mods.len(), ex.dir, dest.display());
        return Ok(());
    }
    if !index.is_file() {
        bail!(
            "the mods of {} are packed against the game's API index, which is missing: {}",
            ex.dir,
            index.display()
        );
    }
    step(format!("packing {} mod(s) of {}", mods.len(), ex.dir));
    let out = project.target_dir.join("mods").join(&ex.dir);
    let written = pack_all(index, &mods, &out)?;
    std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    for p in written {
        let to = dest.join(p.file_name().unwrap_or_default());
        std::fs::copy(&p, &to).with_context(|| format!("copying to {}", to.display()))?;
    }
    println!("mod packages: {}", dest.display());
    Ok(())
}
