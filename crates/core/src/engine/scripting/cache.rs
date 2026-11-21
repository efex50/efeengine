//! The engine-owned script cache (E10, `crates/efescript/docs/e10-design.md` section 3).
//!
//! - A memory cache over `Module::decode` and `Package::read` (key: blake3 of the bytes, a hit is
//!   confirmed by comparing the full bytes).
//! - A disk cache of compiled `.efa` programs (feature `efa`, not on wasm): `<base>/efa/v{N}/<xx>/<key>.efvm`
//!   next to `<key>.deps`, the files the compile read with their blake3 hashes.

use std::collections::HashMap;

use efemod::{Package, PackageError, ReadLimits};
use efevm::{LoadError, Module};

/// Version of the disk layout and key derivation. Older `efa/v*` directories are deleted.
pub const CACHE_FORMAT: u32 = 2;

/// Total bytes the memory cache holds before it is cleared.
const MEMORY_CAP: usize = 64 * 1024 * 1024;

/// Where compiled programs are cached on disk.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum CacheDir {
    /// The platform cache directory, `efeengine/<game>` under it (resolved on first use).
    #[default]
    Platform,
    /// No disk cache.
    Off,
    /// This directory is the base (`<dir>/efa/v{N}` is created in it).
    Path(std::path::PathBuf),
}

/// Configuration of the [`ScriptCache`] (`EngineConfig::script_cache`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheConfig {
    pub disk: CacheDir,
    /// Keep decoded modules / packages in memory.
    pub memory: bool,
    /// Size cap of the disk cache, enforced once when the directory is first resolved.
    pub max_disk_bytes: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig { disk: CacheDir::Platform, memory: true, max_disk_bytes: 64 * 1024 * 1024 }
    }
}

/// Counters of what the cache did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub mem_hits: u64,
    pub mem_misses: u64,
    /// `Module::decode` / `Package::read` calls actually made (a memory hit makes none).
    pub decodes: u64,
    pub disk_hits: u64,
    pub disk_misses: u64,
    pub disk_writes: u64,
    pub disk_evicted: u64,
    /// Compiles actually run (a disk hit runs none).
    pub compiles: u64,
}

#[cfg(not(target_family = "wasm"))]
enum Disk {
    Unresolved,
    /// Resolved: the base directory, or none (off / no platform directory).
    Resolved(Option<std::path::PathBuf>),
}

pub struct ScriptCache {
    config: CacheConfig,
    stats: CacheStats,
    modules: HashMap<[u8; 32], (Vec<u8>, Module)>,
    packages: HashMap<[u8; 32], (Vec<u8>, Package)>,
    mem_bytes: usize,
    #[cfg(not(target_family = "wasm"))]
    disk: Disk,
}

impl ScriptCache {
    pub fn new(config: CacheConfig) -> ScriptCache {
        ScriptCache {
            config,
            stats: CacheStats::default(),
            modules: HashMap::new(),
            packages: HashMap::new(),
            mem_bytes: 0,
            #[cfg(not(target_family = "wasm"))]
            disk: Disk::Unresolved,
        }
    }

    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    /// The resolved disk directory (`<dir>/efa/v{CACHE_FORMAT}` lives under it), if any yet.
    pub fn disk_dir(&self) -> Option<&std::path::Path> {
        #[cfg(not(target_family = "wasm"))]
        if let Disk::Resolved(Some(p)) = &self.disk {
            return Some(p.as_path());
        }
        None
    }

    pub fn clear_memory(&mut self) {
        self.modules.clear();
        self.packages.clear();
        self.mem_bytes = 0;
    }

    /// Delete every cached program on disk (the `efa` directory of the resolved base).
    pub fn clear_disk(&mut self) -> std::io::Result<()> {
        #[cfg(not(target_family = "wasm"))]
        {
            // A `Path` base needs no game name, so it can be resolved here.
            if matches!(self.disk, Disk::Unresolved) {
                self.resolve(None);
            }
            if let Some(base) = self.disk_dir() {
                match std::fs::remove_dir_all(base.join("efa")) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    fn charge(&mut self, n: usize) {
        if self.mem_bytes + n > MEMORY_CAP {
            self.clear_memory();
        }
        self.mem_bytes += n;
    }

    /// Memory cache over `Module::decode` (decode + verify).
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Module, LoadError> {
        if !self.config.memory {
            self.stats.decodes += 1;
            return Module::decode(bytes);
        }
        let key = *blake3::hash(bytes).as_bytes();
        if let Some((stored, m)) = self.modules.get(&key) {
            if stored.as_slice() == bytes {
                self.stats.mem_hits += 1;
                return Ok(m.clone());
            }
        }
        self.stats.mem_misses += 1;
        self.stats.decodes += 1;
        let m = Module::decode(bytes)?;
        self.charge(bytes.len());
        self.modules.insert(key, (bytes.to_vec(), m.clone()));
        Ok(m)
    }

    /// Memory cache over `Package::read(bytes, &ReadLimits::default())`.
    pub fn read_package(&mut self, bytes: &[u8]) -> Result<Package, PackageError> {
        if !self.config.memory {
            self.stats.decodes += 1;
            return Package::read(bytes, &ReadLimits::default());
        }
        let key = *blake3::hash(bytes).as_bytes();
        if let Some((stored, p)) = self.packages.get(&key) {
            if stored.as_slice() == bytes {
                self.stats.mem_hits += 1;
                return Ok(p.clone());
            }
        }
        self.stats.mem_misses += 1;
        self.stats.decodes += 1;
        let p = Package::read(bytes, &ReadLimits::default())?;
        self.charge(bytes.len());
        self.packages.insert(key, (bytes.to_vec(), p.clone()));
        Ok(p)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

#[cfg(not(target_family = "wasm"))]
mod disk {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn format_dir(base: &Path) -> PathBuf {
        base.join("efa").join(format!("v{CACHE_FORMAT}"))
    }

    /// Characters outside `[A-Za-z0-9_-]` become `_`.
    pub(super) fn sanitize(game: &str) -> String {
        let s: String = game.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
        if s.is_empty() { "_".to_string() } else { s }
    }

    fn env_dir(name: &str) -> Option<PathBuf> {
        std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
    }

    fn platform_base() -> Option<PathBuf> {
        if cfg!(windows) {
            env_dir("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            env_dir("HOME").map(|h| h.join("Library").join("Caches"))
        } else {
            env_dir("XDG_CACHE_HOME").or_else(|| env_dir("HOME").map(|h| h.join(".cache")))
        }
    }

    /// `(path, size, mtime)` of the files in the format directory, grouped by key stem.
    fn scan(dir: &Path) -> HashMap<String, (Vec<PathBuf>, u64, std::time::SystemTime)> {
        let mut pairs: HashMap<String, (Vec<PathBuf>, u64, std::time::SystemTime)> = HashMap::new();
        let Ok(subs) = fs::read_dir(dir) else { return pairs };
        for sub in subs.flatten() {
            let Ok(files) = fs::read_dir(sub.path()) else { continue };
            for f in files.flatten() {
                let path = f.path();
                let Ok(meta) = f.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                let e = pairs.entry(stem).or_insert_with(|| (Vec::new(), 0, std::time::UNIX_EPOCH));
                e.0.push(path.clone());
                e.1 += meta.len();
                if path.extension().is_some_and(|x| x == "efvm") || e.2 == std::time::UNIX_EPOCH {
                    e.2 = mtime;
                }
            }
        }
        pairs
    }

    impl ScriptCache {
        /// Resolve the directory (once): choose the base, delete old format directories and
        /// enforce the size cap.
        pub(super) fn resolve(&mut self, game: Option<&str>) {
            if !matches!(self.disk, Disk::Unresolved) {
                return;
            }
            let base = match &self.config.disk {
                CacheDir::Off => None,
                CacheDir::Path(p) => Some(p.clone()),
                CacheDir::Platform => match game {
                    Some(g) => platform_base().map(|b| b.join("efeengine").join(sanitize(g))),
                    // Without a game name the platform directory cannot be chosen yet.
                    None => return,
                },
            };
            if let Some(base) = &base {
                self.tidy(base);
            }
            self.disk = Disk::Resolved(base);
        }

        fn tidy(&mut self, base: &Path) {
            let efa = base.join("efa");
            let current = format!("v{CACHE_FORMAT}");
            if let Ok(rd) = fs::read_dir(&efa) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('v') && name != current {
                        let _ = fs::remove_dir_all(e.path());
                    }
                }
            }
            let dir = format_dir(base);
            let pairs = scan(&dir);
            let mut total: u64 = pairs.values().map(|p| p.1).sum();
            if total <= self.config.max_disk_bytes {
                return;
            }
            let mut order: Vec<_> = pairs.into_values().collect();
            order.sort_by_key(|p| p.2);
            for (files, size, _) in order {
                if total <= self.config.max_disk_bytes {
                    break;
                }
                for f in files {
                    let _ = fs::remove_file(f);
                }
                total = total.saturating_sub(size);
                self.stats.disk_evicted += 1;
            }
        }

        #[cfg(feature = "efa")]
        fn paths(&self, key: &str) -> Option<(PathBuf, PathBuf)> {
            let base = self.disk_dir()?;
            let dir = format_dir(base).join(&key[..2]);
            Some((dir.join(format!("{key}.efvm")), dir.join(format!("{key}.deps"))))
        }

        /// A verified hit: the module bytes, or `None` (a stale pair is deleted).
        #[cfg(feature = "efa")]
        fn lookup(
            &mut self,
            key: &str,
            root: &str,
            files: &dyn efa::asm::FileSource,
            api_hash: u64,
        ) -> Option<(Module, Vec<u8>)> {
            let (efvm, deps) = self.paths(key)?;
            let deps_text = fs::read_to_string(&deps).ok()?;
            let result = self.verify(&efvm, &deps_text, root, files, api_hash);
            if result.is_none() {
                let _ = fs::remove_file(&efvm);
                let _ = fs::remove_file(&deps);
            }
            result
        }

        #[cfg(feature = "efa")]
        fn verify(
            &mut self,
            efvm: &Path,
            deps_text: &str,
            root: &str,
            files: &dyn efa::asm::FileSource,
            api_hash: u64,
        ) -> Option<(Module, Vec<u8>)> {
            let mut lines = deps_text.lines();
            if lines.next()? != "efa-deps 1" {
                return None;
            }
            // The compile reads the root first: a list without it (truncated or forged) is no
            // proof that the files of the use graph are unchanged.
            let mut first = true;
            for line in lines {
                let (h, path) = line.split_once(' ')?;
                if std::mem::take(&mut first) && path != root {
                    return None;
                }
                let bytes = files.read_bytes(path).ok()?;
                if hex(blake3::hash(&bytes).as_bytes()) != h {
                    return None;
                }
            }
            if first {
                return None;
            }
            let bytes = fs::read(efvm).ok()?;
            let module = self.decode(&bytes).ok()?;
            if module.data().api.as_ref().map(|a| a.hash) != Some(api_hash) {
                return None;
            }
            Some((module, bytes))
        }

        #[cfg(feature = "efa")]
        fn store(&mut self, key: &str, bytes: &[u8], deps: &str) {
            let Some((efvm, depsp)) = self.paths(key) else { return };
            let write = |path: &Path, data: &[u8]| -> std::io::Result<()> {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                static TMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = TMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let tmp = path.with_file_name(format!("{name}.tmp-{}-{n}", std::process::id()));
                let r = fs::write(&tmp, data).and_then(|_| fs::rename(&tmp, path));
                if r.is_err() {
                    let _ = fs::remove_file(&tmp);
                }
                r
            };
            match write(&efvm, bytes).and_then(|_| write(&depsp, deps.as_bytes())) {
                Ok(()) => self.stats.disk_writes += 1,
                Err(e) => log::warn!("script cache: cannot write {}: {e}", efvm.display()),
            }
        }

        #[cfg(feature = "efa")]
        pub(super) fn compile_disk(
            &mut self,
            game: &str,
            key: Option<&str>,
            root: &str,
            files: &dyn efa::asm::FileSource,
            api_hash: u64,
        ) -> Option<(Module, Vec<u8>)> {
            self.resolve(Some(game));
            let key = key?;
            self.disk_dir()?;
            let hit = self.lookup(key, root, files, api_hash);
            match &hit {
                Some(_) => self.stats.disk_hits += 1,
                None => self.stats.disk_misses += 1,
            }
            hit
        }

        #[cfg(feature = "efa")]
        pub(super) fn compile_store(&mut self, key: &str, bytes: &[u8], deps: &str) {
            if self.disk_dir().is_some() {
                self.store(key, bytes, deps);
            }
        }
    }
}

#[cfg(feature = "efa")]
mod compile {
    use super::*;
    use efa::asm::{assemble_program, AsmOptions, Diagnostics, FileSource};
    use std::cell::RefCell;

    /// Records every successful `read` (path and blake3 of the contents).
    struct Recording<'a> {
        inner: &'a dyn FileSource,
        seen: RefCell<Vec<(String, String)>>,
    }

    impl FileSource for Recording<'_> {
        fn read(&self, path: &str) -> Result<String, String> {
            let text = self.inner.read(path)?;
            let mut seen = self.seen.borrow_mut();
            if !seen.iter().any(|(p, _)| p == path) {
                seen.push((path.to_string(), hex(blake3::hash(text.as_bytes()).as_bytes())));
            }
            Ok(text)
        }
        fn read_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
            self.inner.read_bytes(path)
        }
        fn display_name(&self, path: &str) -> String {
            self.inner.display_name(path)
        }
    }

    fn key(root: &str, files: &dyn FileSource, api: &efevm::api::ApiIndex) -> Option<String> {
        let text = files.read(root).ok()?;
        let mut h = blake3::Hasher::new();
        h.update(b"efa-cache\0");
        h.update(&CACHE_FORMAT.to_le_bytes());
        h.update(efa::VERSION.as_bytes());
        h.update(b"\0");
        h.update(files.display_name(root).as_bytes());
        h.update(b"\0");
        h.update(text.as_bytes());
        h.update(&api.hash().to_le_bytes());
        Some(hex(h.finalize().as_bytes()))
    }

    impl ScriptCache {
        /// Disk-cached assemble of `root` in `files` against `api`. Returns the module and its
        /// bytes. `game` names the default directory. A hit still has to go through link and
        /// policy (the caller does that).
        pub fn compile(
            &mut self,
            game: &str,
            root: &str,
            files: &dyn FileSource,
            api: &efevm::api::ApiIndex,
            opts: &AsmOptions,
        ) -> Result<(Module, Vec<u8>), Diagnostics> {
            let key = key(root, files, api);
            #[cfg(not(target_family = "wasm"))]
            if let Some(hit) = self.compile_disk(game, key.as_deref(), root, files, api.hash()) {
                return Ok(hit);
            }
            #[cfg(target_family = "wasm")]
            let _ = game;
            self.stats.compiles += 1;
            let rec = Recording { inner: files, seen: RefCell::new(Vec::new()) };
            let opts = AsmOptions { api: Some(api), ..*opts };
            let module = assemble_program(root, &rec, &opts)?;
            let bytes = module.encode();
            #[cfg(not(target_family = "wasm"))]
            if let Some(key) = &key {
                let mut deps = String::from("efa-deps 1\n");
                for (path, h) in rec.seen.borrow().iter() {
                    deps.push_str(&format!("{h} {path}\n"));
                }
                self.compile_store(key, &bytes, &deps);
            }
            Ok((module, bytes))
        }

        /// Disk-cached translation of a `.wasm` mod entry `root` in `files` against `api`
        /// (feature `wasm-mods`, native only). Returns the decoded module and its bytes.
        #[cfg(all(feature = "wasm-mods", not(target_family = "wasm")))]
        pub fn translate_wasm(
            &mut self,
            game: &str,
            root: &str,
            files: &dyn FileSource,
            api: &efevm::api::ApiIndex,
            opts: &efe_wasm::TranslateOptions,
        ) -> Result<(Module, Vec<u8>), String> {
            let wasm = files.read_bytes(root).map_err(|e| format!("{}: {e}", files.display_name(root)))?;
            let mut h = blake3::Hasher::new();
            h.update(b"efa-wasm-cache\0");
            h.update(&CACHE_FORMAT.to_le_bytes());
            h.update(&efe_wasm::TRANSLATOR_VERSION.to_le_bytes());
            h.update(files.display_name(root).as_bytes());
            h.update(b"\0");
            h.update(&wasm);
            h.update(&api.hash().to_le_bytes());
            h.update(format!("{opts:?}").as_bytes());
            let key = hex(h.finalize().as_bytes());
            if let Some(hit) = self.compile_disk(game, Some(&key), root, files, api.hash()) {
                return Ok(hit);
            }
            self.stats.compiles += 1;
            let module = efe_wasm::translate(&wasm, api, opts).map_err(|e| format!("{}: {e}", files.display_name(root)))?;
            let bytes = module.encode();
            let deps = format!("efa-deps 1\n{} {root}\n", hex(blake3::hash(&wasm).as_bytes()));
            self.compile_store(&key, &bytes, &deps);
            Ok((module, bytes))
        }
    }
}

#[cfg(all(test, feature = "efa", not(target_family = "wasm")))]
mod tests {
    use super::*;
    use efa::asm::{AsmOptions, MemSource};
    use efevm::api::{ApiIndex, NamespaceInfo};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("efe-cache-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn cache(dir: &std::path::Path) -> ScriptCache {
        ScriptCache::new(CacheConfig { disk: CacheDir::Path(dir.to_path_buf()), ..CacheConfig::default() })
    }

    fn index(extra: bool) -> ApiIndex {
        let mut i = ApiIndex::default();
        if extra {
            i.namespaces.push(NamespaceInfo { name: "game".into(), version: 1, owner: "t".into(), doc: String::new() });
        }
        i
    }

    const MAIN: &str = "use \"lib.efa\" as l\nexport fn main() -> i64 {\n    reg x\n    i64.add x, x, l.LIMIT\n    ret x\n}\n";

    fn files(limit: u32) -> MemSource {
        MemSource::new().with("main.efa", MAIN).with("lib.efa", &format!("pub const LIMIT = {limit}\n"))
    }

    fn compile(c: &mut ScriptCache, f: &MemSource, api: &ApiIndex) -> Result<(Module, Vec<u8>), efa::asm::Diagnostics> {
        c.compile("test game", "main.efa", f, api, &AsmOptions::default())
    }

    fn module_bytes() -> Vec<u8> {
        efa::asm::assemble("export fn main() -> i64 {\n    reg x\n    ret x\n}\n", "a.efa").unwrap().encode()
    }

    #[test]
    fn decode_memory_hit() {
        let mut c = ScriptCache::new(CacheConfig { disk: CacheDir::Off, ..CacheConfig::default() });
        let b = module_bytes();
        c.decode(&b).unwrap();
        c.decode(&b).unwrap();
        let s = c.stats();
        assert_eq!((s.decodes, s.mem_hits, s.mem_misses), (1, 1, 1));
        c.clear_memory();
        c.decode(&b).unwrap();
        assert_eq!(c.stats().decodes, 2);
    }

    #[test]
    fn memory_off_decodes_every_time() {
        let mut c = ScriptCache::new(CacheConfig { disk: CacheDir::Off, memory: false, ..CacheConfig::default() });
        let b = module_bytes();
        c.decode(&b).unwrap();
        c.decode(&b).unwrap();
        assert_eq!(c.stats().decodes, 2);
        assert_eq!(c.stats().mem_hits, 0);
    }

    #[test]
    fn decode_error_is_not_cached() {
        let mut c = ScriptCache::new(CacheConfig { disk: CacheDir::Off, ..CacheConfig::default() });
        assert!(c.decode(b"nonsense").is_err());
        assert!(c.decode(b"nonsense").is_err());
        assert_eq!(c.stats().decodes, 2);
    }

    #[test]
    fn disk_miss_write_then_hit_in_a_second_cache() {
        let dir = temp();
        let api = index(false);
        let f = files(100);
        let mut a = cache(&dir);
        let (m1, b1) = compile(&mut a, &f, &api).unwrap();
        let s = a.stats();
        assert_eq!((s.compiles, s.disk_misses, s.disk_writes, s.disk_hits), (1, 1, 1, 0));
        assert_eq!(a.disk_dir(), Some(dir.as_path()));
        let mut b = cache(&dir);
        let (m2, b2) = compile(&mut b, &f, &api).unwrap();
        let s = b.stats();
        assert_eq!((s.compiles, s.disk_hits, s.disk_misses), (0, 1, 0));
        assert_eq!(b1, b2);
        assert_eq!(m1.encode(), m2.encode());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editing_a_used_file_misses() {
        let dir = temp();
        let api = index(false);
        let mut a = cache(&dir);
        compile(&mut a, &files(100), &api).unwrap();
        let mut b = cache(&dir);
        let (_, bytes) = compile(&mut b, &files(200), &api).unwrap();
        assert_eq!((b.stats().compiles, b.stats().disk_hits), (1, 0));
        // the edited program is what the cache now holds
        let mut c = cache(&dir);
        let (_, again) = compile(&mut c, &files(200), &api).unwrap();
        assert_eq!(c.stats().compiles, 0);
        assert_eq!(bytes, again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_api_index_misses() {
        let dir = temp();
        let f = files(100);
        let mut a = cache(&dir);
        compile(&mut a, &f, &index(false)).unwrap();
        let mut b = cache(&dir);
        compile(&mut b, &f, &index(true)).unwrap();
        assert_eq!((b.stats().compiles, b.stats().disk_hits), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn find_ext(dir: &std::path::Path, ext: &str) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for sub in std::fs::read_dir(dir.join("efa").join(format!("v{CACHE_FORMAT}"))).unwrap().flatten() {
            for f in std::fs::read_dir(sub.path()).unwrap().flatten() {
                if f.path().extension().is_some_and(|e| e == ext) {
                    out.push(f.path());
                }
            }
        }
        out
    }

    #[test]
    fn corrupt_efvm_recompiles() {
        let dir = temp();
        let api = index(false);
        let f = files(100);
        let mut a = cache(&dir);
        compile(&mut a, &f, &api).unwrap();
        for p in find_ext(&dir, "efvm") {
            std::fs::write(p, b"garbage").unwrap();
        }
        let mut b = cache(&dir);
        compile(&mut b, &f, &api).unwrap();
        assert_eq!((b.stats().compiles, b.stats().disk_hits, b.stats().disk_misses), (1, 0, 1));
        let mut c = cache(&dir);
        compile(&mut c, &f, &api).unwrap();
        assert_eq!(c.stats().compiles, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncated_deps_miss() {
        let dir = temp();
        let api = index(false);
        let f = files(100);
        let mut a = cache(&dir);
        compile(&mut a, &f, &api).unwrap();
        for p in find_ext(&dir, "deps") {
            std::fs::write(p, "efa-deps 1\n").unwrap();
        }
        // lib.efa changed, but the forged list names nothing: it must not be a hit
        let mut b = cache(&dir);
        compile(&mut b, &files(200), &api).unwrap();
        assert_eq!(b.stats().disk_hits, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_compile_is_not_cached() {
        let dir = temp();
        let mut a = cache(&dir);
        let f = MemSource::new().with("main.efa", "this is not efa\n");
        assert!(a.compile("g", "main.efa", &f, &index(false), &AsmOptions::default()).is_err());
        assert_eq!(a.stats().disk_writes, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_cap_evicts_oldest_pairs() {
        let dir = temp();
        let api = index(false);
        let multi = |n: u32| MemSource::new().with(&format!("m{n}.efa"), &MAIN.replace("lib.efa", "lib.efa")).with("lib.efa", "pub const LIMIT = 5\n");
        let run = |c: &mut ScriptCache, n: u32| c.compile("g", &format!("m{n}.efa"), &multi(n), &api, &AsmOptions::default()).map(|_| ());
        let mut a = cache(&dir);
        for n in [1, 2, 3] {
            run(&mut a, n).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert_eq!(find_ext(&dir, "efvm").len(), 3);
        let total: u64 = ["efvm", "deps"].iter().flat_map(|e| find_ext(&dir, e)).map(|p| std::fs::metadata(p).unwrap().len()).sum();
        let mut b = ScriptCache::new(CacheConfig {
            disk: CacheDir::Path(dir.clone()),
            max_disk_bytes: total * 2 / 3 + 1,
            ..CacheConfig::default()
        });
        run(&mut b, 3).unwrap();
        assert!(b.stats().disk_evicted >= 1);
        assert_eq!(b.stats().compiles, 0, "the newest pair survived");
        assert!(find_ext(&dir, "efvm").len() < 3);
        assert_eq!(find_ext(&dir, "efvm").len(), find_ext(&dir, "deps").len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_format_dirs_are_removed() {
        let dir = temp();
        let old = dir.join("efa").join("v0").join("ab");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("x.efvm"), b"x").unwrap();
        let mut a = cache(&dir);
        compile(&mut a, &files(1), &index(false)).unwrap();
        assert!(!dir.join("efa").join("v0").exists());
        assert!(dir.join("efa").join(format!("v{CACHE_FORMAT}")).exists());
        a.clear_disk().unwrap();
        assert!(!dir.join("efa").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn off_has_no_disk() {
        let mut a = ScriptCache::new(CacheConfig { disk: CacheDir::Off, ..CacheConfig::default() });
        compile(&mut a, &files(1), &index(false)).unwrap();
        assert_eq!((a.stats().compiles, a.stats().disk_writes), (1, 0));
        assert!(a.disk_dir().is_none());
    }

    #[test]
    fn game_names_are_sanitised() {
        assert_eq!(disk::sanitize("My Game/1"), "My_Game_1");
    }
}
