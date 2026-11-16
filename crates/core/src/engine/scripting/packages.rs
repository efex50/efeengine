//! Mod packages (`.efmod`) in the engine (e8-design D8 and sections 9.2 to 9.4, 13).
//!
//! A package is one unit: its one module, its manifest and its assets. What the game grants
//! covers all of it ([`Grant`], one per package; no per-file permission). The package only
//! requests (`manifest.permissions`), the game's [`ModPolicy`] grants; the default policy
//! ([`DefaultPolicy`]) grants no namespace at all, so a package that imports an engine or game
//! namespace fails with [`ModError::NotGranted`] until the game installs a policy
//! (`Engine::set_mod_policy`). A declared dependency's namespace is importable without a grant.
//!
//! Load sequence of one package (`Scripting::load_package_parsed`; `Engine::load_package`
//! reads the bytes first and runs `init` afterwards):
//!
//! 1. `Package::read` (done by the caller, `ReadLimits::default()`).
//! 2. `game` check, then the id clash check: the id may not equal or prefix a registered
//!    namespace (the game's own, `engine`, `std`, `efe` or another mod's); both before the
//!    policy is asked.
//! 3. Dependencies: each must be a loaded package whose version matches (a bare `load_mod`
//!    module has no version and never satisfies one).
//! 4. Policy: [`ModRequest`] to [`Grant`] (intersected with the requested namespaces), then
//!    the imports must lie within the grant or the dependencies' namespaces.
//! 5. `load_mod` with the grant's budget and memory pages; [`PackageInfo`] is stored in the slot.
//!
//! `Engine::load_packages` orders a set with `efemod::resolve` and loads in that order.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use efemod::{Manifest, Package, PackageError, ReadLimits, ResolveProblem, Version};

use super::mods::ModOptions;
use super::{DEFAULT_BUDGET, DEFAULT_MEMORY_PAGES, LogLevel, ModuleId, ScriptError, Scripting};

/// What the engine asks the game's policy about one package.
pub struct ModRequest<'a> {
    /// The package's manifest: id, version, requested permissions, dependencies.
    pub manifest: &'a Manifest,
    /// The namespaces the module really imports, sorted and unique.
    pub imports: &'a [String],
    /// The namespaces of the events the module refers to in its events section (section 14),
    /// sorted and unique, without the package's own namespace (its own events need no grant).
    /// They are checked like imports: the grant (plus dependency ids) must cover each one, else
    /// `NotGranted`. Kept apart from `imports` so a policy can tell calling from event use.
    pub events: &'a [String],
}

/// What a policy grants a whole package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// Namespace prefixes the module may import (`engine.log` allows `engine.log.x`, not
    /// `engine.logs`). Intersected with the manifest's requested namespaces.
    pub namespaces: Vec<String>,
    /// VM units per frame.
    pub budget: u64,
    /// Most memory pages (64 KiB) of the module's instance.
    pub memory_pages: u32,
}

impl Default for Grant {
    /// No namespace, the engine's default budget and memory.
    fn default() -> Self {
        Grant { namespaces: Vec::new(), budget: DEFAULT_BUDGET, memory_pages: DEFAULT_MEMORY_PAGES }
    }
}

/// The game's decision on a package: a [`Grant`] or a refusal (the text becomes
/// [`ModError::Denied`]). Any `FnMut(&ModRequest) -> Result<Grant, String>` is a policy.
pub trait ModPolicy {
    fn grant(&mut self, request: &ModRequest<'_>) -> Result<Grant, String>;
}

impl<F: FnMut(&ModRequest<'_>) -> Result<Grant, String>> ModPolicy for F {
    fn grant(&mut self, request: &ModRequest<'_>) -> Result<Grant, String> {
        self(request)
    }
}

/// The policy until the game sets one: grants no namespace, budget and memory at the engine
/// defaults. A package that only uses its dependencies' namespaces loads; anything else is
/// [`ModError::NotGranted`].
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultPolicy;

impl ModPolicy for DefaultPolicy {
    fn grant(&mut self, _request: &ModRequest<'_>) -> Result<Grant, String> {
        Ok(Grant::default())
    }
}

/// A loaded package: its manifest, the grant (after intersecting with the request) and the
/// assets (paths without the `assets/` prefix). Dropped when the module unloads.
#[derive(Clone, Debug)]
pub struct PackageInfo {
    pub manifest: Manifest,
    pub grant: Grant,
    pub assets: BTreeMap<String, Vec<u8>>,
}

/// The outcome of `Engine::load_packages` / `load_mods_dir`. Labels are what the caller passed
/// (the file name for a directory). `loaded` is in load order; `failed` lists read problems,
/// then resolution problems, then load failures, each in the order found.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<(String, ModuleId)>,
    pub failed: Vec<(String, ModError)>,
}

/// Why a package was not loaded.
#[derive(Debug)]
pub enum ModError {
    /// Container, manifest, module decode or verify, undeclared import.
    Package(PackageError),
    /// A dependency is missing or has the wrong version, a duplicate id, a cycle, or a
    /// dependency that failed to load.
    Resolve(ResolveProblem),
    /// The manifest names another game.
    GameMismatch { id: String, wanted: String, game: String },
    /// The id equals or prefixes a registered namespace (`namespace`, owned by `owner`).
    IdClash { id: String, namespace: String, owner: String },
    /// The policy refused.
    Denied { id: String, reason: String },
    /// The module imports namespaces, or refers to events of namespaces, outside the grant and
    /// its dependencies.
    NotGranted { id: String, namespaces: Vec<String> },
    /// Name taken, link errors, struct mismatch, reload refused, ...
    Script(ScriptError),
}

impl fmt::Display for ModError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModError::Package(e) => write!(f, "{e}"),
            ModError::Resolve(e) => write!(f, "{e}"),
            ModError::GameMismatch { id, wanted, game } => {
                write!(f, "mod `{id}` is for the game `{wanted}`, this is `{game}`")
            }
            ModError::IdClash { id, namespace, owner } => {
                write!(f, "mod id `{id}` clashes with the namespace `{namespace}` (owner `{owner}`)")
            }
            ModError::Denied { id, reason } => write!(f, "mod `{id}` was refused: {reason}"),
            ModError::NotGranted { id, namespaces } => write!(
                f,
                "mod `{id}` uses namespace{} {} (imports or events) which the game did not grant",
                if namespaces.len() == 1 { "" } else { "s" },
                namespaces.join(", ")
            ),
            ModError::Script(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ModError {}

impl From<PackageError> for ModError {
    fn from(e: PackageError) -> Self {
        ModError::Package(e)
    }
}

impl From<ScriptError> for ModError {
    fn from(e: ScriptError) -> Self {
        ModError::Script(e)
    }
}

fn covers(prefix: &str, ns: &str) -> bool {
    ns == prefix || ns.strip_prefix(prefix).is_some_and(|r| r.starts_with('.'))
}

/// The grant's namespaces that the request covers, plus the requested ones a broader grant
/// covers: what both sides allow.
fn intersect(grant: &[String], requested: &[String]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for g in grant {
        for p in requested {
            if covers(p, g) {
                out.insert(g.clone());
            } else if covers(g, p) {
                out.insert(p.clone());
            }
        }
    }
    out.into_iter().collect()
}

fn imports_of(module: &efevm::Module) -> Vec<String> {
    module.data().imports.iter().map(|i| i.namespace.clone()).collect::<BTreeSet<_>>().into_iter().collect()
}

/// The result of the policy step: the intersected grant and the import allow-list (grant
/// and dependency ids).
struct Negotiated {
    grant: Grant,
    allowed: Vec<String>,
}

impl Scripting {
    /// Install the game's policy for packages (default: [`DefaultPolicy`]).
    pub fn set_mod_policy(&mut self, policy: impl ModPolicy + 'static) {
        self.policy = Box::new(policy);
    }

    /// The package behind module `id`: `None` for game modules, bare mods and unloaded ones.
    pub fn package(&self, id: ModuleId) -> Option<&PackageInfo> {
        self.host.modules.get(id.0 as usize).filter(|m| m.loaded).and_then(|m| m.package.as_ref())
    }

    /// An asset of the package behind module `id` (path without `assets/`).
    pub fn package_asset(&self, id: ModuleId, path: &str) -> Option<&[u8]> {
        self.package(id)?.assets.get(path).map(Vec::as_slice)
    }

    /// `(id, version)` of every loaded package.
    pub(crate) fn loaded_packages(&self) -> Vec<(String, Version)> {
        self.host
            .modules
            .iter()
            .filter(|m| m.loaded)
            .filter_map(|m| m.package.as_ref().map(|p| (m.name.clone(), p.manifest.version.clone())))
            .collect()
    }

    fn check_game(&self, m: &Manifest) -> Result<(), ModError> {
        match &m.game {
            Some(w) if w != self.registry.game() => {
                Err(ModError::GameMismatch { id: m.id.clone(), wanted: w.clone(), game: self.registry.game().to_string() })
            }
            _ => Ok(()),
        }
    }

    /// The id must not equal or prefix any registered namespace, nor be reserved.
    fn check_id_clash(&self, id: &str) -> Result<(), ModError> {
        if self.host.modules.iter().any(|m| m.loaded && m.name == id) {
            return Err(ScriptError::NameTaken(id.to_string()).into());
        }
        if matches!(id, "engine" | "std" | "efe") {
            return Err(ModError::IdClash { id: id.to_string(), namespace: id.to_string(), owner: "(reserved)".to_string() });
        }
        if let Some(n) = self.registry.namespaces().find(|n| covers(id, &n.name)) {
            return Err(ModError::IdClash { id: id.to_string(), namespace: n.name.clone(), owner: n.owner.clone() });
        }
        Ok(())
    }

    /// Every dependency must be a loaded package within its version range.
    fn check_deps(&self, m: &Manifest) -> Result<(), ModError> {
        let loaded = self.loaded_packages();
        for d in &m.dependencies {
            match loaded.iter().find(|(id, _)| *id == d.id) {
                None => return Err(ModError::Resolve(ResolveProblem::Missing { dep: d.id.clone(), req: d.req.to_string() })),
                Some((_, v)) if !d.req.matches(v) => {
                    return Err(ModError::Resolve(ResolveProblem::Version {
                        dep: d.id.clone(),
                        req: d.req.to_string(),
                        found: v.to_string(),
                    }));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// Ask the policy, intersect, and check the imports.
    fn negotiate(&mut self, manifest: &Manifest, module: &efevm::Module) -> Result<Negotiated, ModError> {
        let imports = imports_of(module);
        let events: Vec<String> =
            module.data().event_namespaces().into_iter().filter(|ns| *ns != manifest.id).map(str::to_string).collect();
        let req = ModRequest { manifest, imports: &imports, events: &events };
        let mut grant = self.policy.grant(&req).map_err(|reason| ModError::Denied { id: manifest.id.clone(), reason })?;
        grant.namespaces = intersect(&grant.namespaces, &manifest.permissions.namespaces);
        let mut allowed = grant.namespaces.clone();
        allowed.extend(manifest.dependencies.iter().map(|d| d.id.clone()));
        let mut bad: Vec<String> = Vec::new();
        for ns in imports.iter().chain(&events) {
            if !allowed.iter().any(|p| covers(p, ns)) && !bad.contains(ns) {
                bad.push(ns.clone());
            }
        }
        if !bad.is_empty() {
            return Err(ModError::NotGranted { id: manifest.id.clone(), namespaces: bad });
        }
        Ok(Negotiated { grant, allowed })
    }

    /// Steps 2 to 5 of the load sequence for a package that was read. `init` has not run
    /// (it is pending; `Engine::load_package` runs it at once).
    pub(crate) fn load_package_parsed(&mut self, pkg: Package) -> Result<ModuleId, ModError> {
        let Package { manifest, module, module_bytes, assets, .. } = pkg;
        self.check_game(&manifest)?;
        self.check_id_clash(&manifest.id)?;
        self.check_deps(&manifest)?;
        let Negotiated { grant, allowed } = self.negotiate(&manifest, &module)?;
        let opts = ModOptions {
            name: manifest.id.clone(),
            version: manifest.api_version,
            budget: grant.budget,
            allowed,
            memory_pages: grant.memory_pages,
            doc: Some(manifest.namespace_doc()),
        };
        let id = self.load_mod(&module_bytes, opts)?;
        self.host.modules[id.0 as usize].package = Some(PackageInfo { manifest, grant, assets });
        Ok(id)
    }
}

#[cfg(not(target_family = "wasm"))]
impl Scripting {
    /// Hot reload of a package: whenever the modification time of `path` changes, the script
    /// system re-reads the package (the id must be unchanged, the dependencies must still
    /// resolve against loaded packages, the policy is asked again, and every loaded dependent
    /// must still accept the version), then reloads like [`watch`](Scripting::watch) does
    /// (the rules of `reload_module`) and replaces the [`PackageInfo`]. A failure is logged
    /// and the old version keeps running.
    pub fn watch_package(&mut self, id: ModuleId, path: impl AsRef<std::path::Path>) -> Result<(), ScriptError> {
        if self.package(id).is_none() {
            return if self.is_loaded(id) {
                Err(ScriptError::Io(format!("module {} is not a package", id.0)))
            } else {
                Err(ScriptError::NoSuchModule(id))
            };
        }
        self.watch(id, path)?;
        if let Some(w) = self.host.modules[id.0 as usize].watch.as_mut() {
            w.package = true;
        }
        Ok(())
    }

    pub(crate) fn reload_package(&mut self, idx: usize, path: &std::path::Path) -> Result<(), ModError> {
        let bytes = std::fs::read(path).map_err(|e| ScriptError::Io(format!("{}: {e}", path.display())))?;
        let pkg = Package::read(&bytes, &ReadLimits::default())?;
        let Package { manifest, module, assets, .. } = pkg;
        let name = self.host.modules[idx].name.clone();
        if manifest.id != name {
            return Err(PackageError::Manifest(format!("id: the package is now `{}`, it was loaded as `{name}`", manifest.id)).into());
        }
        self.check_game(&manifest)?;
        self.check_deps(&manifest)?;
        // Loaded dependents must accept the new version.
        for d in self.dependents_of(idx) {
            let Some(p) = self.host.modules[d].package.as_ref() else { continue };
            if let Some(dep) = p.manifest.dependencies.iter().find(|x| x.id == name)
                && !dep.req.matches(&manifest.version)
            {
                return Err(ModError::Resolve(ResolveProblem::Version {
                    dep: name,
                    req: dep.req.to_string(),
                    found: manifest.version.to_string(),
                }));
            }
        }
        let Negotiated { grant, allowed } = self.negotiate(&manifest, &module)?;
        let doc = manifest.namespace_doc();
        let version = manifest.api_version;
        let slot = &mut self.host.modules[idx];
        let old = (
            std::mem::replace(&mut slot.allowed, Some(allowed)),
            std::mem::replace(&mut slot.budget, grant.budget),
            std::mem::replace(&mut slot.memory_pages, grant.memory_pages),
            std::mem::replace(&mut slot.doc, doc),
        );
        match self.reload_module(idx, module, version) {
            Ok(()) => {
                let slot = &mut self.host.modules[idx];
                slot.remaining = slot.budget;
                slot.package = Some(PackageInfo { manifest, grant, assets });
                Ok(())
            }
            Err(e) => {
                let slot = &mut self.host.modules[idx];
                (slot.allowed, slot.budget, slot.memory_pages, slot.doc) = old;
                Err(e.into())
            }
        }
    }

    /// Log helper for the watcher.
    pub(crate) fn log_package_reload(&mut self, name: &str, path: &std::path::Path, r: Result<(), ModError>) -> bool {
        match r {
            Ok(()) => {
                self.log(LogLevel::Info, format!("[{name}] reloaded from {}", path.display()));
                true
            }
            Err(e) => {
                self.log(LogLevel::Error, format!("[{name}] reload failed, keeping the old version: {e}"));
                false
            }
        }
    }
}
