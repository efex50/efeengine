//! Loading inputs, mod sources and asset handlers (`e10-design.md` section 4).
//!
//! [`LoadInput`] is what `Engine::load_input` takes, [`ModSource`] lists mods for
//! `Engine::load_mods` (and reports changes for hot reload), [`DirSource`] is the native
//! directory source, [`AssetHandler`] lets the game take over some assets of a package.

use std::fmt;

use efemod::Manifest;

use super::{ModError, ModuleId, ScriptError};
use crate::engine::resources::Resources;

/// What `Engine::load_input` loads.
pub enum LoadInput<'a> {
    /// A game script in `.efa` source: compiled through the cache against the full registry
    /// index.
    #[cfg(feature = "efa")]
    Efa { name: &'a str, files: &'a dyn efa::asm::FileSource, root: &'a str },
    /// A compiled game module.
    Efvm { name: &'a str, bytes: &'a [u8] },
    /// A `.efmod` package.
    Package { label: &'a str, bytes: &'a [u8] },
    /// A mod folder (`mod.toml`, sources, assets), `assets` relative to its assets folder.
    #[cfg(feature = "efa")]
    Folder { label: &'a str, files: &'a dyn efa::asm::FileSource, assets: &'a [String] },
}

/// What was loaded: a game module or a mod (package or folder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    Module(ModuleId),
    Package(ModuleId),
}

impl Loaded {
    pub fn id(self) -> ModuleId {
        match self {
            Loaded::Module(id) | Loaded::Package(id) => id,
        }
    }
}

/// Why `Engine::load` / `load_input` failed.
#[derive(Debug)]
pub enum LoadError {
    Io(String),
    #[cfg(feature = "efa")]
    Compile(efa::asm::Diagnostics),
    Script(ScriptError),
    Mod(ModError),
    /// The path needs the compiler (`.efa`, mod folder) and the `efa` feature is off.
    CompilerDisabled,
    /// The path is not a `.efa`, `.efvm`, `.efmod` or mod folder.
    UnknownKind(String),
    /// `reload_mod` was given a game module (`.efa`, `.efvm`); only mods can be reloaded.
    NotAMod(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => f.write_str(e),
            #[cfg(feature = "efa")]
            LoadError::Compile(d) => write!(f, "{d}"),
            LoadError::Script(e) => write!(f, "{e}"),
            LoadError::Mod(e) => write!(f, "{e}"),
            LoadError::CompilerDisabled => f.write_str("the .efa compiler is not part of this build (feature `efa`)"),
            LoadError::UnknownKind(p) => write!(f, "`{p}` is not a .efa, .efvm or .efmod file or a mod folder"),
            LoadError::NotAMod(n) => write!(f, "`{n}` is a game module, not a mod: reload_mod only reloads .efmod files and mod folders"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<ScriptError> for LoadError {
    fn from(e: ScriptError) -> Self {
        LoadError::Script(e)
    }
}

impl From<ModError> for LoadError {
    fn from(e: ModError) -> Self {
        LoadError::Mod(e)
    }
}

/// A place mods come from (a directory, an archive, a download, ...). `Engine::load_mods`
/// lists every registered source.
pub trait ModSource {
    fn list(&mut self) -> Vec<ModItem>;
    /// Labels whose content changed since the last `list` / `changed`. Default: none.
    fn changed(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// One mod of a source.
pub struct ModItem {
    pub label: String,
    pub body: ModBody,
}

pub enum ModBody {
    /// The bytes of a `.efmod`.
    Package(Vec<u8>),
    /// A mod folder: its files and its assets (relative to the assets folder).
    #[cfg(feature = "efa")]
    Folder { files: Box<dyn efa::asm::FileSource>, assets: Vec<String> },
}

/// Takes over assets of packages and mod folders: the first handler whose `matches` is true
/// for an asset path owns it. The engine calls `check` for every owned asset before anything
/// loads (an `Err` refuses the whole mod), `commit` after the load, and `unload` when the
/// package unloads (and before the `commit` of a hot reload). Owned assets are not kept in
/// `PackageInfo.assets`.
pub trait AssetHandler {
    fn matches(&self, path: &str) -> bool;
    fn check(&mut self, m: &Manifest, path: &str, bytes: &[u8]) -> Result<(), String>;
    fn commit(&mut self, id: ModuleId, m: &Manifest, path: &str, bytes: &[u8], res: &mut Resources);
    fn unload(&mut self, id: ModuleId, res: &mut Resources);
}

/// What a source kept of a loaded folder (for rebuilding dependents on hot reload).
#[cfg(feature = "efa")]
pub(crate) type FolderKept = Option<(Box<dyn efa::asm::FileSource>, Vec<String>)>;
#[cfg(not(feature = "efa"))]
pub(crate) type FolderKept = Option<std::convert::Infallible>;

#[cfg(not(target_family = "wasm"))]
pub use dir::DirSource;

#[cfg(not(target_family = "wasm"))]
mod dir {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    use super::{ModBody, ModItem, ModSource};

    type Fp = Vec<(String, u64, Option<SystemTime>)>;

    /// A directory of mods: `*.efmod` files (label = file name) and sub-folders that contain
    /// a `mod.toml` (label = folder name), sorted by name. Unreadable entries are skipped
    /// with a warning; a missing directory is empty. `changed` compares fingerprints (length
    /// and modification time; folders: every file below, symlinks skipped).
    pub struct DirSource {
        dir: PathBuf,
        seen: BTreeMap<String, Fp>,
    }

    impl DirSource {
        pub fn new(dir: impl Into<PathBuf>) -> DirSource {
            DirSource { dir: dir.into(), seen: BTreeMap::new() }
        }

        /// `(label, path, is_folder)` sorted by label.
        fn scan(&self) -> Vec<(String, PathBuf, bool)> {
            let rd = match std::fs::read_dir(&self.dir) {
                Ok(rd) => rd,
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        log::warn!("mods: cannot read {}: {e}", self.dir.display());
                    }
                    return Vec::new();
                }
            };
            let mut out = Vec::new();
            for e in rd {
                let Ok(e) = e else { continue };
                let path = e.path();
                let label = e.file_name().to_string_lossy().into_owned();
                if path.is_file() {
                    if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("efmod")) {
                        out.push((label, path, false));
                    }
                } else if path.is_dir() && path.join("mod.toml").is_file() {
                    out.push((label, path, true));
                }
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        }
    }

    fn walk(dir: &Path, rel: &str, out: &mut Fp) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
            let name = format!("{rel}{}", e.file_name().to_string_lossy());
            if md.file_type().is_symlink() {
                continue;
            }
            if md.is_dir() {
                walk(&e.path(), &format!("{name}/"), out);
            } else if md.is_file() {
                out.push((name, md.len(), md.modified().ok()));
            }
        }
    }

    fn fingerprint(path: &Path, folder: bool) -> Fp {
        let mut fp = Vec::new();
        if folder {
            walk(path, "", &mut fp);
            fp.sort();
        } else if let Ok(md) = std::fs::metadata(path) {
            fp.push((String::new(), md.len(), md.modified().ok()));
        }
        fp
    }

    impl ModSource for DirSource {
        fn list(&mut self) -> Vec<ModItem> {
            let mut items = Vec::new();
            let mut seen = BTreeMap::new();
            for (label, path, folder) in self.scan() {
                seen.insert(label.clone(), fingerprint(&path, folder));
                if folder {
                    #[cfg(feature = "efa")]
                    {
                        match efa::mod_folder::list_assets(&path) {
                            Ok(assets) => {
                                let files = efa::asm::FsSource::with_prefix(&path, path.display().to_string());
                                items.push(ModItem { label, body: ModBody::Folder { files: Box::new(files), assets } });
                            }
                            Err(e) => log::warn!("mods: skipping folder {}: {e}", path.display()),
                        }
                    }
                    #[cfg(not(feature = "efa"))]
                    log::warn!("mods: skipping folder {}: the compiler is not part of this build", path.display());
                } else {
                    match std::fs::read(&path) {
                        Ok(bytes) => items.push(ModItem { label, body: ModBody::Package(bytes) }),
                        Err(e) => log::warn!("mods: skipping {}: {e}", path.display()),
                    }
                }
            }
            self.seen = seen;
            items
        }

        fn changed(&mut self) -> Vec<String> {
            let mut out = Vec::new();
            let mut seen = BTreeMap::new();
            for (label, path, folder) in self.scan() {
                let fp = fingerprint(&path, folder);
                if self.seen.get(&label) != Some(&fp) {
                    out.push(label.clone());
                }
                seen.insert(label, fp);
            }
            self.seen = seen;
            out
        }
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("efe-load-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn load_error_display() {
        assert!(LoadError::Io("x".into()).to_string() == "x");
        assert!(LoadError::UnknownKind("a.txt".into()).to_string().contains("a.txt"));
        assert!(LoadError::CompilerDisabled.to_string().contains("efa"));
        let e: LoadError = ModError::Build("bad".into()).into();
        assert_eq!(e.to_string(), "bad");
        let e = ModError::Asset { path: "a.bin".into(), reason: "nope".into() };
        assert!(e.to_string().contains("a.bin") && e.to_string().contains("nope"));
        assert_eq!(Loaded::Package(ModuleId(3)).id(), ModuleId(3));
    }

    #[test]
    fn dir_source_lists_and_reports_changes() {
        let d = temp("dir");
        assert!(DirSource::new(d.join("missing")).list().is_empty());
        std::fs::write(d.join("b.efmod"), b"not a package").unwrap();
        std::fs::write(d.join("note.txt"), b"x").unwrap();
        std::fs::create_dir_all(d.join("a_mod/src")).unwrap();
        std::fs::write(d.join("a_mod/mod.toml"), "[mod]\nid = \"a\"\nversion = \"1.0.0\"\n").unwrap();
        std::fs::write(d.join("a_mod/src/main.efa"), "export fn main() {\n    ret\n}\n").unwrap();
        std::fs::create_dir_all(d.join("plain")).unwrap();
        let mut s = DirSource::new(&d);
        let items = s.list();
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, if cfg!(feature = "efa") { vec!["a_mod", "b.efmod"] } else { vec!["b.efmod"] });
        assert!(matches!(items.last().unwrap().body, ModBody::Package(_)));
        assert!(s.changed().is_empty());
        std::fs::write(d.join("b.efmod"), b"changed contents").unwrap();
        assert_eq!(s.changed(), vec!["b.efmod".to_string()]);
        assert!(s.changed().is_empty());
        if cfg!(feature = "efa") {
            std::fs::write(d.join("a_mod/src/main.efa"), "export fn main() {\n    ret\n}\n\n").unwrap();
            assert_eq!(s.changed(), vec!["a_mod".to_string()]);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(feature = "efa")]
    #[test]
    fn folder_mods_load_and_hot_reload_through_a_dir_source() {
        use crate::engine::config::EngineConfig;
        use crate::engine::scripting::{CacheDir, LogLevel};
        let d = temp("hot");
        std::fs::create_dir_all(d.join("a/src")).unwrap();
        std::fs::write(d.join("a/mod.toml"), "[mod]\nid = \"a\"\nversion = \"1.0.0\"\n").unwrap();
        let main = d.join("a/src/main.efa");
        std::fs::write(&main, "export fn main() {\n    ret\n}\n").unwrap();
        let mut cfg = EngineConfig::headless();
        cfg.script_cache.disk = CacheDir::Off;
        let mut e = crate::Engine::with_config(cfg).unwrap();
        e.add_mod_source(DirSource::new(&d));
        let report = e.load_mods();
        assert_eq!(report.loaded.len(), 1, "{:?}", report.failed);
        let id = report.loaded[0].1;
        assert_eq!(e.scripting().package(id).unwrap().manifest.id, "a");
        std::fs::write(&main, "export fn main() {\n    nop\n    ret\n}\n").unwrap();
        e.run_frames(2, 0.3);
        let logs: Vec<_> = e.scripting().log_lines().map(|l| (l.level, l.text.clone())).collect();
        assert!(logs.iter().any(|(l, t)| *l == LogLevel::Info && t.starts_with("[a] reloaded")), "{logs:?}");
        std::fs::write(&main, "export fn main() {\n    oops oops\n}\n").unwrap();
        e.run_frames(2, 0.3);
        assert!(e.scripting().log_lines().any(|l| l.text.contains("reload failed, keeping the old version")));
        assert!(e.scripting().is_loaded(id));
        let _ = std::fs::remove_dir_all(&d);
    }
}
