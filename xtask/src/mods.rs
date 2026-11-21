//! Mod packages: `examples/<example>/mods/<name>/` folders (each with a `mod.toml`) packed into
//! `.efmod` files with `efescript::pack::pack_dir`.
//!
//! * `cargo xtask mods <example>` packs them into `target/mods/<example>/`.
//! * `build` (native) packs them as a check and copies the mod *folders* into `<profile dir>/mods/`
//!   (the game compiles them at run time). `dist` (web) copies the packed `.efmod` files into
//!   `dist/<example>/mods/`.
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

/// Packs `mods` in passes against `game` and the fragments of the mods packed before them.
/// Returns the packed mods (folder, result) and the last error of each mod that could not be
/// packed (by folder name).
fn pack_loop(
    game: efescript::pack::ApiIndex,
    mods: &[PathBuf],
) -> (Vec<(PathBuf, efescript::pack::Packed)>, BTreeMap<String, String>) {
    let mut indices = vec![game];
    let mut done: Vec<(PathBuf, efescript::pack::Packed)> = vec![];
    let mut last_err: BTreeMap<String, String> = BTreeMap::new();
    let mut todo: Vec<PathBuf> = mods.to_vec();
    loop {
        let mut progress = false;
        let mut still_todo = vec![];
        let mut new_indices = vec![];
        {
            let refs: Vec<_> = indices.iter().collect();
            for dir in todo {
                match efescript::pack::pack_dir(&dir, &refs) {
                    Ok(packed) => {
                        new_indices.push(packed.api.clone());
                        done.push((dir, packed));
                        progress = true;
                    }
                    Err(e) => {
                        last_err.insert(folder_name(&dir), e);
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
    last_err.retain(|n, _| todo.iter().any(|d| folder_name(d) == *n));
    (done, last_err)
}

/// Packs `mods` against the game index at `index` and the mods packed before them, writing the
/// packages (and each mod's `.efapi` fragment) into `out_dir` (emptied first). Returns the
/// written package paths.
fn pack_all(index: &Path, mods: &[PathBuf], out_dir: &Path) -> Result<Vec<PathBuf>> {
    let game = efescript::pack::read_api_input(index).map_err(anyhow::Error::msg)?;
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir).with_context(|| format!("removing {}", out_dir.display()))?;
    }
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let (done, last_err) = pack_loop(game, mods);
    let mut written = vec![];
    for (dir, packed) in &done {
        let path = out_dir.join(&packed.file_name);
        std::fs::write(&path, &packed.bytes).with_context(|| format!("writing {}", path.display()))?;
        let api = path.with_extension("efapi");
        std::fs::write(&api, packed.api.to_json()).with_context(|| format!("writing {}", api.display()))?;
        println!("  packed {} -> {}", folder_name(dir), path.display());
        written.push(path);
    }
    if done.len() < mods.len() {
        let names: Vec<String> = mods.iter().map(|d| folder_name(d)).filter(|n| last_err.contains_key(n)).collect();
        for n in &names {
            warn(format!("mod `{n}` not packed: {}", last_err.get(n).map_or("unknown error", String::as_str)));
        }
        bail!("{} of {} mod(s) could not be packed: {}", names.len(), mods.len(), names.join(", "));
    }
    Ok(written)
}

/// The API fragments of mod `name` (a folder name or a mod id) and of the mods of `ex` it
/// depends on (transitively), built from the declarations against the game index at `index`.
/// The mod's own fragment comes first.
pub fn fragments_for(ex: &Example, index: &Path, name: &str) -> Result<Vec<efescript::pack::ApiIndex>> {
    let mods = mod_folders(ex)?;
    let game = efescript::pack::read_api_input(index).map_err(anyhow::Error::msg)?;
    let (done, errs) = pack_loop(game, &mods);
    let want = done.iter().find(|(d, p)| folder_name(d) == name || p.manifest.id == name);
    let Some((dir, target)) = want else {
        if let Some((n, e)) = errs.iter().find(|(n, _)| n.as_str() == name) {
            bail!("mod `{n}` of {} could not be built: {e}", ex.dir);
        }
        let known: Vec<String> = mods.iter().map(|d| folder_name(d)).collect();
        bail!(
            "mod `{name}` not found in examples/{}/mods/ (folders: {})",
            ex.dir,
            if known.is_empty() { "none".to_string() } else { known.join(", ") }
        );
    };
    let _ = dir;
    let mut out = vec![target.api.clone()];
    let mut seen = vec![target.manifest.id.clone()];
    let mut queue: Vec<String> = target.manifest.dependencies.iter().map(|d| d.id.clone()).collect();
    while let Some(id) = queue.pop() {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        if let Some((_, p)) = done.iter().find(|(_, p)| p.manifest.id == id) {
            out.push(p.api.clone());
            queue.extend(p.manifest.dependencies.iter().map(|d| d.id.clone()));
        }
    }
    Ok(out)
}

/// `cargo xtask mod-api <mod dir> [-o FILE]`: write the API index fragment of a mod folder
/// (its namespace, provides, events, structs and `[[meta.component]]` components) without
/// running the game. A mod inside `examples/<example>/mods/` is built against that example's
/// exported game index (and the sibling mods it depends on); elsewhere `--api GAME.efapi` is needed.
pub fn mod_api_cmd(project: &Project, dir: &Path, output: Option<PathBuf>, api: Option<PathBuf>) -> Result<()> {
    let dir = std::fs::canonicalize(dir).with_context(|| format!("mod folder {}", dir.display()))?;
    if !dir.join("mod.toml").is_file() {
        bail!("{} has no mod.toml", dir.display());
    }
    let ex_root = root().join("examples");
    let example = dir
        .strip_prefix(std::fs::canonicalize(&ex_root).unwrap_or(ex_root))
        .ok()
        .and_then(|rel| rel.components().next())
        .and_then(|c| project.get(&c.as_os_str().to_string_lossy()).ok());
    let name = folder_name(&dir);
    let frag = match (example, api) {
        (Some(ex), api) => {
            let index = match api {
                Some(p) => p,
                None => export_game_index(project, ex)?,
            };
            fragments_for(ex, &index, &name)?.remove(0)
        }
        (None, Some(api)) => {
            let game = efescript::pack::read_api_input(&api).map_err(anyhow::Error::msg)?;
            efescript::pack::pack_dir(&dir, &[&game]).map_err(anyhow::Error::msg)?.api
        }
        (None, None) => bail!("{} is not inside examples/<example>/mods/: pass --api GAME.efapi", dir.display()),
    };
    let out = output.unwrap_or_else(|| PathBuf::from(format!("{name}.efapi")));
    std::fs::write(&out, frag.to_json()).with_context(|| format!("writing {}", out.display()))?;
    println!("mod API fragment: {}", out.display());
    Ok(())
}

/// Exports the game's index with `api` (builds and runs the native binary) and returns its path.
fn export_game_index(project: &Project, ex: &Example) -> Result<PathBuf> {
    let path = game_index_path(project, ex);
    api::api_export(project, ex, Some(path.clone()), false, false, false)?;
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

/// For native `build`: packs the mods of `ex` against the game index at `index` (a check, into
/// `target/mods/<example>/`), removes every `*.efmod` in `dest` and copies each mod folder into
/// `dest/<name>/`, replacing an existing one. The game compiles the folders at run time.
pub fn install_native(project: &Project, ex: &Example, index: &Path, dest: &Path) -> Result<()> {
    let mods = mod_folders(ex)?;
    if dry_run() {
        println!("[dry-run] copy {} mod folder(s) of {} into {}", mods.len(), ex.dir, dest.display());
        return Ok(());
    }
    if !mods.is_empty() {
        if !index.is_file() {
            bail!(
                "the mods of {} are packed against the game's API index, which is missing: {}",
                ex.dir,
                index.display()
            );
        }
        step(format!("checking {} mod(s) of {}", mods.len(), ex.dir));
        pack_all(index, &mods, &project.target_dir.join("mods").join(&ex.dir))?;
    }
    std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    for e in std::fs::read_dir(dest).with_context(|| format!("reading {}", dest.display()))? {
        let path = e?.path();
        if path.is_file() && path.extension().is_some_and(|x| x == "efmod") {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    for dir in &mods {
        let to = dest.join(folder_name(dir));
        if to.exists() {
            std::fs::remove_dir_all(&to).with_context(|| format!("removing {}", to.display()))?;
        }
        copy_dir(dir, &to)?;
    }
    if !mods.is_empty() {
        println!("mod folders: {}", dest.display());
    }
    Ok(())
}

/// Recursively copies the folder `from` to `to` (which must not exist yet).
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for e in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), &dest).with_context(|| format!("copying to {}", dest.display()))?;
        }
    }
    Ok(())
}

/// For `dist` (web): packs the mods of `ex` against the game index at `index` and copies
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
