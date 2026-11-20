//! Mod packages (`.efmod`) in the engine (e8-design D8 and sections 9.2 to 9.4, 13).
//!
//! A package is one unit: its one module, its manifest and its assets. What the game grants
//! covers all of it ([`Grant`], one per package; no per-file permission). The package only
//! requests (`manifest.permissions`), the game's [`ModPolicy`] grants; the default policy
//! ([`DefaultPolicy`]) grants no namespace at all, so a package that imports an engine or game
//! namespace fails with [`ModError::NotGranted`] until the game installs a policy
//! (`Engine::set_mod_policy`). A declared dependency's namespace is importable without a grant.
//!
//! Load sequence of one package (`Scripting::load_package_item`; `Engine::load_input` reads
//! the bytes first and runs `init` afterwards):
//!
//! 1. `Package::read` (done by the caller, `ReadLimits::default()`).
//! 2. `game` check, then the id clash check: the id may not equal or prefix a registered
//!    namespace (the game's own, `engine`, `std`, `efe` or another mod's); both before the
//!    policy is asked.
//! 3. Dependencies: each must be a loaded package whose version matches (a bare `load_mod`
//!    module has no version and never satisfies one).
//! 4. Policy: [`ModRequest`] to [`Grant`] (intersected with the requested namespaces), then
//!    the imports must lie within the grant or the dependencies' namespaces.
//! 5. Asset check by the game's `AssetHandler`s, then `load_mod` with the grant's budget and
//!    memory pages, then `commit` of the handled assets; [`PackageInfo`] (the unhandled assets)
//!    is stored in the slot.
//!
//! `Engine::load_mods` orders the items of the mod sources with `efemod::resolve` and loads in
//! that order. Hot reload of source items is `Scripting::check_sources`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use efemod::{Manifest, Package, PackageError, ResolveProblem, Version};

use super::load::{AssetHandler, FolderKept, ModBody, ModSource};
use super::components::{ModComponent, parse_components, same_declarations};
use super::mods::ModOptions;
use super::{DEFAULT_BUDGET, DEFAULT_MEMORY_PAGES, LogLevel, ModuleId, ScriptError, Scripting};
use crate::engine::resources::Resources;
use crate::engine::types::TypeRegistry;
use hecs::World;

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
    /// The components the mod declares in `meta.component` (names, sizes, layouts), in
    /// declaration order. The policy may refuse by what it sees here; the grant's
    /// `max_component_types` and `max_component_size` are checked after it answers.
    pub components: &'a [ModComponent],
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
    /// Most component types the mod may declare (default 16).
    pub max_component_types: u32,
    /// Largest size in bytes of one declared component (default 256).
    pub max_component_size: u32,
    /// Whether the mod's code may run on the JIT (when the engine's `ScriptBackend` and the
    /// `jit` feature allow it). Default `false`: mods are untrusted input, and a lowering bug
    /// in the JIT is a host memory-safety bug where an interpreter bug is only a wrong result.
    pub jit: bool,
}

impl Default for Grant {
    /// No namespace, the engine's default budget and memory, 16 component types of at most
    /// 256 bytes.
    fn default() -> Self {
        Grant {
            namespaces: Vec::new(),
            budget: DEFAULT_BUDGET,
            memory_pages: DEFAULT_MEMORY_PAGES,
            max_component_types: DEFAULT_MAX_COMPONENT_TYPES,
            max_component_size: DEFAULT_MAX_COMPONENT_SIZE,
            jit: false,
        }
    }
}

/// `Grant::default().max_component_types`.
pub const DEFAULT_MAX_COMPONENT_TYPES: u32 = 16;
/// `Grant::default().max_component_size`.
pub const DEFAULT_MAX_COMPONENT_SIZE: u32 = 256;

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
    /// The components the mod declared (registered while it is loaded).
    pub components: Vec<ModComponent>,
    /// blake3 of the content, as 64 hex characters: the `.efmod` bytes of a package, or a
    /// canonical walk of the files of a mod folder (sorted paths, each with its bytes). A
    /// folder whose source cannot list its files gets a hash that is never equal to another.
    pub hash: String,
    /// The label the mod was loaded under (file or folder name, or what the source gave).
    pub source: String,
    /// What a reload needs to put this version back: the module and the assets a handler
    /// took (the others are in `assets`).
    pub(crate) retained: Package,
}

impl PackageInfo {
    /// The package as it was loaded, for loading it again.
    pub(crate) fn rebuilt(&self) -> Built {
        let mut pkg = self.retained.clone();
        pkg.assets.extend(self.assets.iter().map(|(k, v)| (k.clone(), v.clone())));
        Built { pkg, label: self.source.clone(), hash: self.hash.clone() }
    }
}

/// A package that is read or built and ready to load, with where it came from.
pub(crate) struct Built {
    pub pkg: Package,
    pub label: String,
    pub hash: String,
}

/// The outcome of `Engine::load_mods`. Labels are the ones the sources gave (file or folder
/// name for a directory). `loaded` is in load order; `failed` lists read problems,
/// then resolution problems, then load failures, each in the order found.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<(String, ModuleId)>,
    pub failed: Vec<(String, ModError)>,
    /// Mods that were already loaded with the same contents (label and the loaded module).
    pub skipped: Vec<(String, ModuleId)>,
    /// `reload_mods` only: mods that were replaced by a changed version (label and the new
    /// module), including loaded dependents that were reloaded with them.
    pub reloaded: Vec<(String, ModuleId)>,
    /// `reload_mods` only: ids of mods that were unloaded because their source no longer
    /// lists them (dependents included).
    pub unloaded: Vec<String>,
}

/// What `Engine::unload_mods` did.
#[derive(Debug, Default)]
pub struct UnloadReport {
    /// Ids of the mods that were unloaded, in unload order (dependents first).
    pub unloaded: Vec<String>,
    /// Mods that could not be unloaded: a game module still depends on them.
    pub failed: Vec<(String, ModError)>,
}

/// What `Engine::reload_mod` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reloaded {
    /// The loaded mod has the same content hash: nothing happened.
    Unchanged(ModuleId),
    /// The mod was replaced (the new module id) and `dependents` (mod ids) were reloaded
    /// against it. Their module ids changed too.
    Replaced { id: ModuleId, dependents: Vec<String> },
    /// No mod with that id was loaded: it was loaded like `load_input` does.
    Loaded(ModuleId),
}

impl Reloaded {
    pub fn id(&self) -> ModuleId {
        match self {
            Reloaded::Unchanged(id) | Reloaded::Loaded(id) | Reloaded::Replaced { id, .. } => *id,
        }
    }
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
    /// A mod folder could not be built (mod.toml, limits, compile diagnostics as text).
    Build(String),
    /// An asset was refused (reserved name, or an `AssetHandler::check` failed).
    Asset { path: String, reason: String },
    /// A mod with this id is already loaded (as `module`). `same_contents` tells whether the
    /// content hash is the same; if not, `Engine::reload_mod` replaces it.
    AlreadyLoaded { id: String, module: ModuleId, same_contents: bool },
    /// `reload_mod` failed with `cause` (its text). `restored` is true when the previous
    /// version (and its dependents) is loaded again, false when the restore failed too and
    /// the mod is left unloaded (the log has the details).
    ReloadFailed { id: String, cause: String, restored: bool },
    /// A declared component is invalid (bad type, duplicate field, empty, name clash, too many
    /// or too large for the grant), or a hot reload in place would change the declarations.
    /// `component` is empty when the whole `meta.component` entry is at fault.
    Component { id: String, component: String, reason: String },
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
            ModError::Build(e) => f.write_str(e),
            ModError::Asset { path, reason } => write!(f, "asset `{path}`: {reason}"),
            ModError::AlreadyLoaded { id, same_contents, .. } => {
                write!(f, "mod `{id}` is already loaded")?;
                if !same_contents {
                    f.write_str(" (contents changed; use reload_mod)")?;
                }
                Ok(())
            }
            ModError::Component { id, component, reason } => {
                if component.is_empty() {
                    write!(f, "mod `{id}`: components: {reason}")
                } else {
                    write!(f, "mod `{id}`: component `{component}`: {reason}")
                }
            }
            ModError::ReloadFailed { id, cause, restored } => write!(
                f,
                "reloading mod `{id}` failed: {cause}; {}",
                if *restored { "the previous version is loaded again" } else { "the previous version could not be restored, the mod is unloaded" }
            ),
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
        self.modules.get(id.0 as usize).filter(|m| m.loaded).and_then(|m| m.package.as_ref())
    }

    /// An asset of the package behind module `id` (path without `assets/`).
    pub fn package_asset(&self, id: ModuleId, path: &str) -> Option<&[u8]> {
        self.package(id)?.assets.get(path).map(Vec::as_slice)
    }

    /// `(id, version)` of every loaded package.
    pub(crate) fn loaded_packages(&self) -> Vec<(String, Version)> {
        self.modules
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
        if self.modules.iter().any(|m| m.loaded && m.name == id) {
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
    fn negotiate(&mut self, manifest: &Manifest, module: &efevm::Module, components: &[ModComponent]) -> Result<Negotiated, ModError> {
        let imports = imports_of(module);
        let events: Vec<String> =
            module.data().event_namespaces().into_iter().filter(|ns| *ns != manifest.id).map(str::to_string).collect();
        let req = ModRequest { manifest, imports: &imports, events: &events, components };
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
        if components.len() as u64 > grant.max_component_types as u64 {
            return Err(ModError::Component {
                id: manifest.id.clone(),
                component: String::new(),
                reason: format!("{} component types declared, the grant allows {}", components.len(), grant.max_component_types),
            });
        }
        if let Some(c) = components.iter().find(|c| c.size() > grant.max_component_size) {
            return Err(ModError::Component {
                id: manifest.id.clone(),
                component: c.name.clone(),
                reason: format!("{} bytes, the grant allows {}", c.size(), grant.max_component_size),
            });
        }
        Ok(Negotiated { grant, allowed })
    }


    /// Route every asset to the first handler that matches (reserved names are refused first)
    /// and run its `check`. Nothing is committed.
    fn route_assets(&mut self, m: &Manifest, assets: &BTreeMap<String, Vec<u8>>) -> Result<Vec<(String, usize)>, ModError> {
        let mut routes = Vec::new();
        for (path, bytes) in assets {
            if path == "manifest.json" || path == "module.efvm" || path == "engine" || path.starts_with("engine/") {
                return Err(ModError::Asset { path: path.clone(), reason: "the name is reserved".to_string() });
            }
            if let Some(h) = self.handlers.iter().position(|h| h.matches(path)) {
                self.handlers[h]
                    .check(m, path, bytes)
                    .map_err(|reason| ModError::Asset { path: path.clone(), reason })?;
                routes.push((path.clone(), h));
            }
        }
        Ok(routes)
    }

    /// Commit the routed assets and return the ones no handler took.
    fn commit_assets(
        &mut self,
        id: ModuleId,
        m: &Manifest,
        mut assets: BTreeMap<String, Vec<u8>>,
        routes: Vec<(String, usize)>,
        res: &mut Resources,
    ) -> BTreeMap<String, Vec<u8>> {
        for (path, h) in routes {
            let Some(bytes) = assets.remove(&path) else { continue };
            self.handlers[h].commit(id, m, &path, &bytes, res);
            if !self.committed.contains(&(id, h)) {
                self.committed.push((id, h));
            }
        }
        assets
    }

    /// `unload` on every handler that committed for `id`.
    pub(crate) fn release_assets(&mut self, id: ModuleId, res: &mut Resources) {
        let hs: Vec<usize> = self.committed.iter().filter(|c| c.0 == id).map(|c| c.1).collect();
        self.committed.retain(|c| c.0 != id);
        for h in hs {
            self.handlers[h].unload(id, res);
        }
    }

    /// Forget a module that unloaded: its handlers' `unload` and its source origin.
    pub(crate) fn release_module(&mut self, id: ModuleId, res: &mut Resources) {
        self.release_assets(id, res);
        self.origins.retain(|o| o.id != id);
    }

    pub(crate) fn add_asset_handler(&mut self, h: impl AssetHandler + 'static) {
        self.handlers.push(Box::new(h));
    }

    pub(crate) fn add_mod_source(&mut self, s: impl ModSource + 'static) {
        self.sources.push(Box::new(s));
    }

    /// The loaded mod (a package, not a game module) with manifest id `id`.
    pub(crate) fn loaded_mod_idx(&self, id: &str) -> Option<usize> {
        self.modules.iter().position(|m| m.loaded && m.name == id && m.package.is_some())
    }

    /// `AlreadyLoaded` when a mod with this id is loaded.
    pub(crate) fn check_already(&self, id: &str, hash: &str) -> Result<(), ModError> {
        match self.loaded_mod_idx(id) {
            None => Ok(()),
            Some(idx) => {
                let same = self.modules[idx].package.as_ref().is_some_and(|p| p.hash == hash);
                Err(ModError::AlreadyLoaded { id: id.to_string(), module: ModuleId(idx as u32), same_contents: same })
            }
        }
    }

    /// Read the `.efmod` bytes (through the cache) and hash them.
    pub(crate) fn read_built(&mut self, label: &str, bytes: &[u8]) -> Result<Built, ModError> {
        let pkg = self.cache.read_package(bytes)?;
        Ok(Built { pkg, label: label.to_string(), hash: blake3::hash(bytes).to_hex().to_string() })
    }

    /// What a reload keeps of the version it loads.
    fn retain(
        manifest: &Manifest,
        module: &efevm::Module,
        module_bytes: Vec<u8>,
        assets: &BTreeMap<String, Vec<u8>>,
        routes: &[(String, usize)],
        ignored: Vec<String>,
    ) -> Package {
        let handled = routes.iter().filter_map(|(p, _)| assets.get(p).map(|b| (p.clone(), b.clone()))).collect();
        Package { manifest: manifest.clone(), module: module.clone(), module_bytes, assets: handled, ignored }
    }

    /// Steps 2 to 5 of the load sequence for a package that was read (or built from a folder).
    /// `init` has not run (it is pending; the engine runs it right after). A mod with the same
    /// id that is loaded is `AlreadyLoaded`.
    pub(crate) fn load_package_item(
        &mut self,
        b: Built,
        res: &mut Resources,
        world: &mut World,
        types: &mut TypeRegistry,
    ) -> Result<ModuleId, ModError> {
        let Built { pkg, label, hash } = b;
        let Package { manifest, module, module_bytes, assets, ignored } = pkg;
        self.check_already(&manifest.id, &hash)?;
        self.check_game(&manifest)?;
        self.check_id_clash(&manifest.id)?;
        self.check_deps(&manifest)?;
        let components = parse_components(&manifest.id, manifest.meta.as_ref())?;
        let Negotiated { grant, allowed } = self.negotiate(&manifest, &module, &components)?;
        let routes = self.route_assets(&manifest, &assets)?;
        let retained = Self::retain(&manifest, &module, module_bytes, &assets, &routes, ignored);
        // Components first: the mod's `init` (run by the engine after this) may use them.
        self.register_components(&manifest.id, &components, world, types)?;
        let opts = ModOptions {
            name: manifest.id.clone(),
            version: manifest.api_version,
            budget: grant.budget,
            allowed,
            memory_pages: grant.memory_pages,
            doc: Some(manifest.namespace_doc()),
            jit: grant.jit,
        };
        let id = match self.load_mod_decoded(module, opts) {
            Ok(id) => id,
            Err(e) => {
                self.undo_components(&manifest.id, world, types);
                return Err(e.into());
            }
        };
        let assets = self.commit_assets(id, &manifest, assets, routes, res);
        self.modules[id.0 as usize].package = Some(PackageInfo { manifest, grant, assets, components, hash, source: label, retained });
        Ok(id)
    }

    /// Hot reload of module `idx` with a freshly read package: the id must be unchanged, the
    /// dependencies must still resolve, the policy is asked again, every loaded dependent
    /// must still accept the version, handled assets must pass their `check`. On success
    /// the handlers get `unload` then `commit` and the [`PackageInfo`] is replaced; on failure
    /// the old version keeps running.
    pub(crate) fn reload_package_parsed(&mut self, idx: usize, b: Built, res: &mut Resources) -> Result<(), ModError> {
        let Built { pkg, label, hash } = b;
        let Package { manifest, module, module_bytes, assets, ignored } = pkg;
        let name = self.modules[idx].name.clone();
        if manifest.id != name {
            return Err(PackageError::Manifest(format!("id: the package is now `{}`, it was loaded as `{name}`", manifest.id)).into());
        }
        self.check_game(&manifest)?;
        self.check_deps(&manifest)?;
        // Loaded dependents must accept the new version.
        for d in self.dependents_of(idx) {
            let Some(p) = self.modules[d].package.as_ref() else { continue };
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
        let components = parse_components(&manifest.id, manifest.meta.as_ref())?;
        if !same_declarations(&components, &self.declared_components(&name)) {
            return Err(ModError::Component {
                id: name,
                component: String::new(),
                reason: "the declared components changed; a hot reload in place cannot migrate them (use Engine::reload_mod, or the engine's reload_mods)".to_string(),
            });
        }
        let Negotiated { grant, allowed } = self.negotiate(&manifest, &module, &components)?;
        let routes = self.route_assets(&manifest, &assets)?;
        let retained = Self::retain(&manifest, &module, module_bytes, &assets, &routes, ignored);
        let doc = manifest.namespace_doc();
        let version = manifest.api_version;
        let slot = &mut self.modules[idx];
        let old = (
            std::mem::replace(&mut slot.allowed, Some(allowed)),
            std::mem::replace(&mut slot.budget, grant.budget),
            std::mem::replace(&mut slot.memory_pages, grant.memory_pages),
            std::mem::replace(&mut slot.doc, doc),
            std::mem::replace(&mut slot.jit, grant.jit),
        );
        match self.reload_module(idx, module, version) {
            Ok(()) => {
                let slot = &mut self.modules[idx];
                slot.remaining = slot.budget;
                let id = ModuleId(idx as u32);
                self.release_assets(id, res);
                let assets = self.commit_assets(id, &manifest, assets, routes, res);
                self.modules[idx].package = Some(PackageInfo { manifest, grant, assets, components, hash, source: label, retained });
                Ok(())
            }
            Err(e) => {
                let slot = &mut self.modules[idx];
                (slot.allowed, slot.budget, slot.memory_pages, slot.doc, slot.jit) = old;
                Err(e.into())
            }
        }
    }

    /// The game index (the registry's without the namespaces and types of loaded modules)
    /// merged with the index of each declared dependency, which `m` is compiled against.
    #[cfg(feature = "efa")]
    fn folder_index(&self, m: &Manifest) -> Result<efevm::api::ApiIndex, ModError> {
        use efevm::api::ApiIndex;
        let full = self.registry.index();
        let owners: BTreeSet<&str> = self.modules.iter().filter(|x| x.loaded).map(|x| x.name.as_str()).collect();
        let mut game = ApiIndex { game: full.game.clone(), ..ApiIndex::default() };
        game.namespaces = full.namespaces.iter().filter(|n| !owners.contains(n.owner.as_str())).cloned().collect();
        let keep: BTreeSet<&str> = game.namespaces.iter().map(|n| n.name.as_str()).collect();
        game.types = full.types.iter().filter(|t| !owners.contains(t.owner.as_str())).cloned().collect();
        game.functions = full.functions.iter().filter(|f| keep.contains(f.namespace.as_str())).cloned().collect();
        game.events = full.events.iter().filter(|e| keep.contains(e.namespace.as_str())).cloned().collect();
        let deps: Vec<ApiIndex> = m
            .dependencies
            .iter()
            .filter_map(|d| self.modules.iter().find(|x| x.loaded && x.name == d.id))
            .map(|x| efa::mod_folder::mod_index(&x.module, &x.name, x.version, &x.doc))
            .collect();
        let mut parts: Vec<&ApiIndex> = vec![&game];
        parts.extend(deps.iter());
        ApiIndex::merge(&parts).map_err(|e| ModError::Build(e.to_string()))
    }

    /// Read just the manifest of a mod folder and hash its files, so the caller can compare
    /// before compiling anything.
    #[cfg(feature = "efa")]
    pub(crate) fn peek_folder(files: &dyn efa::asm::FileSource) -> Result<(Manifest, String), ModError> {
        use efa::mod_folder::{BuildLimits, LimitedSource, prepare_mod};
        let limits = BuildLimits::default();
        let lim = LimitedSource::new(files, &limits);
        let prep = prepare_mod(&lim, &limits).map_err(ModError::Build)?;
        Ok((prep.manifest, Self::folder_hash(files)?))
    }

    /// The content hash of a mod folder: blake3 over every file, sorted by path, each as
    /// its path and its bytes (both length-prefixed). A source that cannot list its files
    /// gets a hash that is unique to this call, so it always counts as changed.
    #[cfg(feature = "efa")]
    fn folder_hash(files: &dyn efa::asm::FileSource) -> Result<String, ModError> {
        use efa::asm::FileSource;
        use efa::mod_folder::{BuildLimits, LimitedSource};
        use std::sync::atomic::{AtomicU64, Ordering};
        static UNLISTED: AtomicU64 = AtomicU64::new(0);
        let mut h = blake3::Hasher::new();
        let Some(mut list) = files.list() else {
            h.update(b"efe-mod-unlisted\0");
            h.update(&UNLISTED.fetch_add(1, Ordering::Relaxed).to_le_bytes());
            return Ok(h.finalize().to_hex().to_string());
        };
        list.sort();
        list.dedup();
        let limits = BuildLimits::default();
        let lim = LimitedSource::new(files, &limits);
        h.update(b"efe-mod-folder\0");
        for path in list {
            let bytes = lim.read_bytes(&path).map_err(|e| ModError::Build(format!("{path}: {e}")))?;
            h.update(&(path.len() as u64).to_le_bytes());
            h.update(path.as_bytes());
            h.update(&(bytes.len() as u64).to_le_bytes());
            h.update(&bytes);
        }
        Ok(h.finalize().to_hex().to_string())
    }

    /// Build a mod folder into a package: `prepare_mod`, compile through the cache, `finish_mod`.
    /// `hash` is the folder's hash when the caller has it already.
    #[cfg(feature = "efa")]
    pub(crate) fn build_folder(
        &mut self,
        label: &str,
        files: &dyn efa::asm::FileSource,
        assets: &[String],
        hash: Option<String>,
    ) -> Result<Built, ModError> {
        use efa::mod_folder::{BuildLimits, LimitedSource, asm_options, finish_mod, prepare_mod};
        let hash = match hash {
            Some(h) => h,
            None => Self::folder_hash(files)?,
        };
        let limits = BuildLimits::default();
        let src = LimitedSource::new(files, &limits);
        let prep = prepare_mod(&src, &limits).map_err(ModError::Build)?;
        self.check_game(&prep.manifest)?;
        self.check_deps(&prep.manifest)?;
        let api = self.folder_index(&prep.manifest)?;
        let game = self.registry.game().to_string();
        let (module, bytes) = self
            .cache
            .compile(&game, &prep.entry, &src, &api, &asm_options(None, &limits))
            .map_err(|d| ModError::Build(d.to_string()))?;
        let built = finish_mod(prep, module, &src, assets, &limits).map_err(ModError::Build)?;
        let pkg = Package { manifest: built.manifest, module: built.module, module_bytes: bytes, assets: built.assets, ignored: Vec::new() };
        Ok(Built { pkg, label: label.to_string(), hash })
    }

    /// Reload module `idx` from a source item. Returns what to keep of a folder.
    fn reload_body(&mut self, idx: usize, label: &str, body: ModBody, res: &mut Resources) -> Result<FolderKept, ModError> {
        match body {
            ModBody::Package(bytes) => {
                let b = self.read_built(label, &bytes)?;
                self.reload_package_parsed(idx, b, res)?;
                Ok(None)
            }
            #[cfg(feature = "efa")]
            ModBody::Folder { files, assets } => {
                let b = self.build_folder(label, &*files, &assets, None)?;
                self.reload_package_parsed(idx, b, res)?;
                Ok(Some((files, assets)))
            }
        }
    }

    fn log_reload(&mut self, name: &str, what: &str, r: &Result<(), ModError>) {
        match r {
            Ok(()) => self.log(LogLevel::Info, format!("[{name}] reloaded from {what}")),
            Err(e) => self.log(LogLevel::Error, format!("[{name}] reload failed, keeping the old version: {e}")),
        }
    }

    /// Hot reload of mods that came from sources: ask each source what changed, reload the
    /// changed ones that are loaded in place, then rebuild the loaded folder mods that depend
    /// on a reloaded one (when their module bytes change). Reloaded modules are appended to
    /// `reloaded`.
    pub(crate) fn check_sources(&mut self, reloaded: &mut Vec<ModuleId>, res: &mut Resources) {
        if self.sources.is_empty() {
            return;
        }
        let mut sources = std::mem::take(&mut self.sources);
        for (si, src) in sources.iter_mut().enumerate() {
            let changed = src.changed();
            if changed.is_empty() {
                continue;
            }
            let targets: Vec<(ModuleId, String)> = self
                .origins
                .iter()
                .filter(|o| o.source == si && changed.contains(&o.label) && self.is_loaded(o.id))
                .map(|o| (o.id, o.label.clone()))
                .collect();
            if targets.is_empty() {
                continue;
            }
            let mut items = src.list();
            for (id, label) in targets {
                let Some(pos) = items.iter().position(|i| i.label == label) else { continue };
                let item = items.swap_remove(pos);
                let name = self.modules[id.0 as usize].name.clone();
                match self.reload_body(id.0 as usize, &label, item.body, res) {
                    Ok(kept) => {
                        if let Some(o) = self.origins.iter_mut().find(|o| o.id == id) {
                            o.folder = kept;
                        }
                        self.log_reload(&name, &label, &Ok(()));
                        reloaded.push(id);
                        self.rebuild_dependents(id, reloaded, res);
                    }
                    Err(e) => {
                        // Changed component declarations cannot be hot reloaded in place: ask
                        // the engine for a full reload (the mod is replaced, with migration).
                        if matches!(e, ModError::Component { .. }) {
                            self.pending.reload_mods();
                        }
                        self.log_reload(&name, &label, &Err(e));
                    }
                }
            }
        }
        sources.append(&mut self.sources);
        self.sources = sources;
    }

    /// After `root` reloaded: recompile each loaded folder mod that depends on it (transitively,
    /// in load order) and reload it when its module bytes changed.
    #[cfg(feature = "efa")]
    fn rebuild_dependents(&mut self, root: ModuleId, reloaded: &mut Vec<ModuleId>, res: &mut Resources) {
        let mut changed_ids = vec![self.modules[root.0 as usize].name.clone()];
        for idx in 0..self.modules.len() {
            let slot = &self.modules[idx];
            let Some(p) = slot.package.as_ref().filter(|_| slot.loaded) else { continue };
            if !p.manifest.dependencies.iter().any(|d| changed_ids.contains(&d.id)) {
                continue;
            }
            let id = ModuleId(idx as u32);
            let name = slot.name.clone();
            let Some(oi) = self.origins.iter().position(|o| o.id == id && o.folder.is_some()) else { continue };
            let Some((files, assets)) = self.origins[oi].folder.take() else { continue };
            let label = self.origins[oi].label.clone();
            let built = self.build_folder(&label, &*files, &assets, None);
            self.origins[oi].folder = Some((files, assets));
            match built {
                Ok(b) => {
                    if b.pkg.module.encode() == self.modules[idx].module.encode() {
                        continue;
                    }
                    let r = self.reload_package_parsed(idx, b, res);
                    self.log_reload(&name, &format!("{label} (a dependency changed)"), &r);
                    if r.is_ok() {
                        changed_ids.push(name);
                        reloaded.push(id);
                    }
                }
                Err(e) => self.log_reload(&name, "its folder", &Err(e)),
            }
        }
    }

    #[cfg(not(feature = "efa"))]
    fn rebuild_dependents(&mut self, _root: ModuleId, _reloaded: &mut Vec<ModuleId>, _res: &mut Resources) {}
}

/// Where a loaded mod came from (a source registered with `add_mod_source`).
pub(crate) struct Origin {
    pub id: ModuleId,
    pub source: usize,
    pub label: String,
    /// The files and asset list of a folder mod, to rebuild it when a dependency reloads.
    pub folder: FolderKept,
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
        if let Some(w) = self.modules[id.0 as usize].watch.as_mut() {
            w.package = true;
        }
        Ok(())
    }

    pub(crate) fn reload_package(&mut self, idx: usize, path: &std::path::Path, res: &mut Resources) -> Result<(), ModError> {
        let bytes = std::fs::read(path).map_err(|e| ScriptError::Io(format!("{}: {e}", path.display())))?;
        let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let b = self.read_built(&label, &bytes)?;
        self.reload_package_parsed(idx, b, res)
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
