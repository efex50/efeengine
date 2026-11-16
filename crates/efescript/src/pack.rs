//! `efescript pack`: a mod folder (`mod.toml`, sources, assets) to a `.efmod` package
//! (MOD_PACKAGE.md section 4, e8-design.md sections 8 and 10).
//!
//! The functions here are the library side of the `pack`, `inspect` and `api-export`
//! commands, so tests and build tools can pack without spawning the binary.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use efemod::manifest::Permissions;
use efemod::{Manifest, Package, ReadLimits, Version, VersionReq};
use efevm::api::ApiIndex;

use crate::asm::{assemble_program, AsmOptions, FsSource};

/// The `[build]` table of `mod.toml` (not copied into the package).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildToml {
    /// The entry file, relative to the mod folder.
    pub entry: String,
    /// Default `--api` inputs, relative to `mod.toml` (`.efapi` or `.efmod`).
    pub api: Vec<String>,
    /// The asset folder, relative to the mod folder.
    pub assets: String,
}

impl Default for BuildToml {
    fn default() -> Self {
        BuildToml { entry: "src/main.efa".into(), api: Vec::new(), assets: "assets".into() }
    }
}

/// A parsed `mod.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModToml {
    /// Everything that becomes `manifest.json` (not yet validated).
    pub manifest: Manifest,
    pub build: BuildToml,
}

/// The result of [`pack_dir`].
#[derive(Debug)]
pub struct Packed {
    pub manifest: Manifest,
    pub module: efevm::Module,
    /// The `.efmod` file.
    pub bytes: Vec<u8>,
    /// `<id>-<version>.efmod`.
    pub file_name: String,
}

type Table = toml::Table;

/// Fails on a key of `t` that is not in `known`, naming it.
fn no_unknown(t: &Table, known: &[&str], table: &str) -> Result<(), String> {
    for k in t.keys() {
        if !known.contains(&k.as_str()) {
            return Err(format!("mod.toml: unknown key `{k}` in [{table}] (known: {})", known.join(", ")));
        }
    }
    Ok(())
}

fn get_str(t: &Table, key: &str, table: &str) -> Result<Option<String>, String> {
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("mod.toml: `{key}` in [{table}] must be a string")),
    }
}

fn get_int(t: &Table, key: &str, table: &str) -> Result<Option<i64>, String> {
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::Integer(n)) => Ok(Some(*n)),
        Some(_) => Err(format!("mod.toml: `{key}` in [{table}] must be an integer")),
    }
}

fn get_strs(t: &Table, key: &str, table: &str) -> Result<Option<Vec<String>>, String> {
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|v| match v {
                toml::Value::String(s) => Ok(s.clone()),
                _ => Err(format!("mod.toml: `{key}` in [{table}] must be an array of strings")),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("mod.toml: `{key}` in [{table}] must be an array of strings")),
    }
}

fn get_table<'a>(root: &'a Table, key: &str) -> Result<Option<&'a Table>, String> {
    match root.get(key) {
        None => Ok(None),
        Some(toml::Value::Table(t)) => Ok(Some(t)),
        Some(_) => Err(format!("mod.toml: `{key}` must be a table")),
    }
}

fn to_u32(n: i64, key: &str, table: &str) -> Result<u32, String> {
    u32::try_from(n).map_err(|_| format!("mod.toml: `{key}` in [{table}] is out of range"))
}

/// Parses `mod.toml`. Unknown keys and tables are errors naming the key. The manifest part
/// is not validated here (`pack_dir` does that); only types and versions are checked.
pub fn read_mod_toml(text: &str) -> Result<ModToml, String> {
    let root: Table = text.parse().map_err(|e| format!("mod.toml: {e}"))?;
    for k in root.keys() {
        if !["mod", "dependencies", "permissions", "build"].contains(&k.as_str()) {
            return Err(format!("mod.toml: unknown key or table `{k}` (known tables: mod, dependencies, permissions, build)"));
        }
    }
    let m = get_table(&root, "mod")?.ok_or("mod.toml: the [mod] table is missing")?;
    no_unknown(m, &["id", "version", "name", "description", "authors", "license", "game", "api_version"], "mod")?;
    let id = get_str(m, "id", "mod")?.ok_or("mod.toml: [mod] needs `id`")?;
    let version = get_str(m, "version", "mod")?.ok_or("mod.toml: [mod] needs `version`")?;
    let version: Version = version.parse().map_err(|e| format!("mod.toml: [mod] version: {e}"))?;
    let mut manifest = Manifest::new(id, version);
    manifest.name = get_str(m, "name", "mod")?;
    manifest.description = get_str(m, "description", "mod")?;
    manifest.authors = get_strs(m, "authors", "mod")?.unwrap_or_default();
    manifest.license = get_str(m, "license", "mod")?;
    manifest.game = get_str(m, "game", "mod")?;
    if let Some(n) = get_int(m, "api_version", "mod")? {
        manifest.api_version = to_u32(n, "api_version", "mod")?;
    }

    if let Some(d) = get_table(&root, "dependencies")? {
        // toml tables iterate in key order, which is the sorted-by-id order.
        for (id, v) in d {
            let toml::Value::String(req) = v else {
                return Err(format!("mod.toml: dependency `{id}` must be a range string"));
            };
            let req: VersionReq = req.parse().map_err(|e| format!("mod.toml: dependency `{id}`: {e}"))?;
            manifest.dependencies.push(efemod::Dependency { id: id.clone(), req });
        }
    }

    if let Some(p) = get_table(&root, "permissions")? {
        no_unknown(p, &["namespaces", "budget", "memory_pages"], "permissions")?;
        let mut perms = Permissions::default();
        perms.namespaces = get_strs(p, "namespaces", "permissions")?.unwrap_or_default();
        if let Some(n) = get_int(p, "budget", "permissions")? {
            perms.budget =
                Some(u64::try_from(n).map_err(|_| "mod.toml: `budget` in [permissions] is out of range".to_string())?);
        }
        if let Some(n) = get_int(p, "memory_pages", "permissions")? {
            perms.memory_pages = Some(to_u32(n, "memory_pages", "permissions")?);
        }
        manifest.permissions = perms;
    }

    let mut build = BuildToml::default();
    if let Some(b) = get_table(&root, "build")? {
        no_unknown(b, &["entry", "api", "assets"], "build")?;
        if let Some(e) = get_str(b, "entry", "build")? {
            build.entry = e;
        }
        if let Some(a) = get_strs(b, "api", "build")? {
            build.api = a;
        }
        if let Some(a) = get_str(b, "assets", "build")? {
            build.assets = a;
        }
    }
    Ok(ModToml { manifest, build })
}

/// A `[build]` path: relative, no `..`, no root or prefix. Returns its segments.
fn inside(path: &str, what: &str) -> Result<Vec<String>, String> {
    let mut segs = Vec::new();
    for c in Path::new(path).components() {
        match c {
            Component::Normal(s) => {
                segs.push(s.to_str().ok_or_else(|| format!("[build] {what}: `{path}` is not valid UTF-8"))?.to_string())
            }
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "[build] {what}: `{path}` must stay inside the mod folder (no absolute path, no `..`)"
                ))
            }
        }
    }
    if segs.is_empty() {
        return Err(format!("[build] {what}: `{path}` is empty"));
    }
    Ok(segs)
}

/// Every regular file below `dir`, as `(path relative with '/', full path)`, sorted.
/// Symbolic links are not followed (they are skipped).
fn collect_files(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name =
            e.file_name().into_string().map_err(|n| format!("{}: file name {n:?} is not valid UTF-8", dir.display()))?;
        let ty = e.file_type().map_err(|e2| format!("{}: {e2}", e.path().display()))?;
        let rel_name = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if ty.is_symlink() {
            continue;
        } else if ty.is_dir() {
            collect_files(&e.path(), &rel_name, out)?;
        } else if ty.is_file() {
            out.push((rel_name, e.path()));
        }
    }
    Ok(())
}

/// Packs the mod folder `dir`: reads `dir/mod.toml`, assembles `[build] entry` against the
/// merge of `extra_apis` and the `[build] api` files, checks the manifest against the module,
/// collects the assets and writes the canonical package.
pub fn pack_dir(dir: &Path, extra_apis: &[&ApiIndex]) -> Result<Packed, String> {
    let toml_path = dir.join("mod.toml");
    let text = std::fs::read_to_string(&toml_path).map_err(|e| format!("{}: {e}", toml_path.display()))?;
    let mt = read_mod_toml(&text)?;
    let manifest = mt.manifest;
    manifest.validate().map_err(|e| format!("mod.toml: {e}"))?;

    // Inputs: the given indices, then [build] api (relative to mod.toml).
    let mut owned: Vec<ApiIndex> = Vec::new();
    for a in &mt.build.api {
        owned.push(read_api_input(&dir.join(a))?);
    }
    let mut refs: Vec<&ApiIndex> = extra_apis.to_vec();
    refs.extend(owned.iter());
    let merged = if refs.is_empty() { None } else { Some(ApiIndex::merge(&refs).map_err(|e| e.to_string())?) };

    let entry = inside(&mt.build.entry, "entry")?;
    let entry_name = entry.last().cloned().unwrap_or_default();
    let mut entry_dir = dir.to_path_buf();
    entry_dir.extend(&entry[..entry.len() - 1]);
    let src = FsSource::new(&entry_dir);
    let module =
        assemble_program(&entry_name, &src, &AsmOptions { api: merged.as_ref() }).map_err(|d| d.to_string())?;
    efemod::check_module(&manifest, &module).map_err(|e| e.to_string())?;

    let mut assets = BTreeMap::new();
    let seg = inside(&mt.build.assets, "assets")?;
    let mut asset_dir = dir.to_path_buf();
    asset_dir.extend(&seg);
    match std::fs::symlink_metadata(&asset_dir) {
        Ok(md) if md.is_dir() => {
            let mut files = Vec::new();
            collect_files(&asset_dir, "", &mut files)?;
            let limits = ReadLimits::default();
            for (rel, full) in files {
                efemod::container::validate_entry_name(&format!("assets/{rel}"), &limits)
                    .map_err(|e| format!("asset `{rel}`: {e}"))?;
                let data = std::fs::read(&full).map_err(|e| format!("{}: {e}", full.display()))?;
                assets.insert(rel, data);
            }
        }
        Ok(_) => return Err(format!("[build] assets: `{}` is not a directory", mt.build.assets)),
        // The default folder may be absent; a named one that is missing is an error.
        Err(_) if mt.build.assets == "assets" => {}
        Err(e) => return Err(format!("[build] assets: {}: {e}", asset_dir.display())),
    }

    let module_bytes = module.encode();
    let bytes = Package::write(&manifest, &module_bytes, &assets).map_err(|e| e.to_string())?;
    let file_name = format!("{}-{}.efmod", manifest.id, manifest.version);
    Ok(Packed { manifest, module, bytes, file_name })
}

/// Reads an API input: `.efapi` (JSON index) or `.efmod` (the package's own index). Other
/// extensions are told apart by content (the 7z signature, or JSON).
pub fn read_api_input(path: &Path) -> Result<ApiIndex, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_pkg = match ext {
        "efmod" => true,
        "efapi" => false,
        _ if bytes.starts_with(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]) => true,
        _ if bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{') => false,
        _ => return Err(format!("{}: not an .efapi or .efmod file", path.display())),
    };
    if is_pkg {
        let pkg = Package::read(&bytes, &ReadLimits::default()).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(pkg.index())
    } else {
        let text = std::str::from_utf8(&bytes).map_err(|_| format!("{}: not valid UTF-8", path.display()))?;
        ApiIndex::from_json(text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The API index of a bare module (no manifest): its section 13 under namespace `id`.
pub fn mod_index(module: &efevm::Module, id: &str, version: u32, doc: &str) -> ApiIndex {
    match &module.data().interface {
        Some(i) => i.index(id, version, doc),
        None => efevm::Interface::default().index(id, version, doc),
    }
}
