//! Loading and unloading modules, mods and their exports, hot reload.

use std::path::Path;

use efevm::{ApiType, HostFn, HostOk, Instance, Limits, Module, Trap};

use super::{DEFAULT_BUDGET, LogLevel, ModuleId, ScriptError, Scripting};
use super::host::ModuleSlot;

/// Options of [`Scripting::load_mod`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModOptions {
    /// The mod's name: also the namespace (and owner) of the functions it exports.
    pub name: String,
    /// The version of the namespace the mod exports.
    pub version: u32,
    /// VM units per frame (default 1 000 000).
    pub budget: u64,
    /// Import namespace prefixes the mod may use: `engine.log` allows `engine.log` and
    /// `engine.log.x`, not `engine.logs`. Anything else is refused at load.
    pub allowed: Vec<String>,
}

impl ModOptions {
    /// Version 1, the default budget, nothing allowed yet (add with [`allow`](Self::allow)).
    pub fn new(name: impl Into<String>) -> Self {
        ModOptions { name: name.into(), version: 1, budget: DEFAULT_BUDGET, allowed: Vec::new() }
    }

    pub fn allow(mut self, prefix: impl Into<String>) -> Self {
        self.allowed.push(prefix.into());
        self
    }

    pub fn with_budget(mut self, units: u64) -> Self {
        self.budget = units;
        self
    }

    pub fn with_version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }
}

fn is_allowed(allowed: &[String], ns: &str) -> bool {
    allowed.iter().any(|p| ns == p || ns.strip_prefix(p.as_str()).is_some_and(|r| r.starts_with('.')))
}

impl Scripting {
    /// Decode, check and instantiate. Nothing is changed on failure.
    fn link(&self, name: &str, module: &Module, allowed: Option<&[String]>) -> Result<Instance<super::ScriptHost>, ScriptError> {
        if let Some(allowed) = allowed {
            let mut bad: Vec<String> = Vec::new();
            for imp in &module.data().imports {
                if !is_allowed(allowed, &imp.namespace) && !bad.contains(&imp.namespace) {
                    bad.push(imp.namespace.clone());
                }
            }
            if !bad.is_empty() {
                return Err(ScriptError::Disallowed { module: name.to_string(), namespaces: bad });
            }
        }
        let imports = self.registry.imports();
        let errors = imports.check_module(module);
        if !errors.is_empty() {
            return Err(ScriptError::Link { module: name.to_string(), errors });
        }
        Instance::new(module, &imports, Limits::default()).map_err(|e| match e {
            efevm::LoadError::Link(l) => ScriptError::Link { module: name.to_string(), errors: vec![l] },
            other => ScriptError::Load(other),
        })
    }

    fn instantiate(
        &mut self,
        name: &str,
        bytes: &[u8],
        allowed: Option<Vec<String>>,
        version: u32,
        budget: u64,
    ) -> Result<ModuleId, ScriptError> {
        if !efevm::api::is_ident(name) {
            return Err(ScriptError::InvalidName(name.to_string()));
        }
        let taken = name == super::api_engine::OWNER
            || self.host.modules.iter().any(|m| m.loaded && m.name == name)
            || self
                .registry
                .namespaces()
                .any(|n| n.owner != name && (n.name == name || n.name.strip_prefix(name).is_some_and(|r| r.starts_with('.'))));
        if taken {
            return Err(ScriptError::NameTaken(name.to_string()));
        }
        let module = Module::decode(bytes).map_err(ScriptError::Load)?;
        let instance = self.link(name, &module, allowed.as_deref())?;
        let id = ModuleId(self.host.modules.len() as u32);
        self.host.modules.push(ModuleSlot {
            name: name.to_string(),
            module,
            instance: Some(instance),
            loaded: true,
            version,
            budget,
            remaining: budget,
            allowed,
            pending_init: true,
            exports: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            watch: None,
        });
        Ok(id)
    }

    /// Load a game script module: every registered namespace may be imported. `init` (if the
    /// module exports it) runs at the next script-system run; `Engine::load_module` runs it
    /// at once.
    pub fn load_module(&mut self, name: &str, bytes: &[u8]) -> Result<ModuleId, ScriptError> {
        self.instantiate(name, bytes, None, 1, DEFAULT_BUDGET)
    }

    /// Load a mod: only the namespaces in `opts.allowed` may be imported; a refused import
    /// names the namespace, and link problems are reported all at once.
    pub fn load_mod(&mut self, bytes: &[u8], opts: ModOptions) -> Result<ModuleId, ScriptError> {
        self.instantiate(&opts.name, bytes, Some(opts.allowed), opts.version, opts.budget)
    }

    /// Unload module `id`: its instance is dropped and everything it registered
    /// (`unregister_owner(name)`) leaves the index. Modules that still import its functions
    /// get a clean trap ("module X unloaded") when they call them. `Engine::unload_module` is
    /// the public path (it also drops component types owned by the module).
    pub(crate) fn unload(&mut self, id: ModuleId) -> Result<String, ScriptError> {
        let idx = id.0 as usize;
        let slot = self.host.modules.get(idx).filter(|m| m.loaded).ok_or(ScriptError::NoSuchModule(id))?;
        let name = slot.name.clone();
        self.registry.unregister_owner(&name)?;
        let slot = &mut self.host.modules[idx];
        slot.loaded = false;
        slot.instance = None;
        slot.exports.clear();
        #[cfg(not(target_family = "wasm"))]
        {
            slot.watch = None;
        }
        Ok(name)
    }

    /// Register the exports requested during `init` of module `id` (functions in the
    /// namespace named like the module, owner = the module). Existing entries are replaced
    /// (hot reload).
    pub(crate) fn register_exports(&mut self, id: ModuleId) {
        let idx = id.0 as usize;
        let Some(slot) = self.host.modules.get_mut(idx) else { return };
        let exports = std::mem::take(&mut slot.exports);
        if exports.is_empty() {
            return;
        }
        let (name, version, module) = (slot.name.clone(), slot.version, slot.module.clone());
        let ns_doc = format!("Functions exported by `{name}`.");
        if let Err(e) = self.registry.define_namespace(&name, version, &name, &ns_doc) {
            self.log(LogLevel::Error, format!("[{name}] cannot define its namespace: {e}"));
            return;
        }
        for (func, doc) in exports {
            let Some(fidx) = module.export_func(&func) else { continue };
            let Some(ty) = module.func_type(fidx) else { continue };
            let names = &module.data().names;
            let mut used: Vec<String> = Vec::new();
            let mut desc = HostFn::new(name.as_str(), func.as_str()).doc(doc);
            for (i, vt) in ty.params.iter().enumerate() {
                let given = names.regs.iter().find(|(f, r, _)| *f == fidx && *r == i as u32).map(|(_, _, n)| n.clone());
                let pname = given.filter(|n| efevm::api::is_ident(n) && !used.contains(n)).unwrap_or_else(|| format!("p{i}"));
                used.push(pname.clone());
                desc = desc.param(pname, ApiType::from_val(*vt));
            }
            let many = ty.results.len() > 1;
            for (i, vt) in ty.results.iter().enumerate() {
                let mut rname = if many { format!("ret{i}") } else { "ret".to_string() };
                while used.contains(&rname) {
                    rname.push('_');
                }
                used.push(rname.clone());
                desc = desc.result(rname, ApiType::from_val(*vt));
            }
            let f = func.clone();
            let call = move |c: &mut efevm::Caller<'_, super::ScriptHost>, a: &efevm::Args<'_>, r: &mut efevm::Results<'_>| {
                match c.host().run_export(id, &f, a.raw()) {
                    Ok(vals) => {
                        for (slot, v) in r.raw().iter_mut().zip(vals) {
                            *slot = v;
                        }
                        Ok(HostOk::Continue)
                    }
                    Err(e) => Err(Trap::Host(e.to_string())),
                }
            };
            let result = match self.registry.register(desc.clone(), call.clone()) {
                Err(efevm::RegistryError::DuplicateFunction { .. }) => self.registry.replace(desc, call),
                other => other,
            };
            if let Err(e) = result {
                self.log(LogLevel::Error, format!("[{name}] cannot export `{func}`: {e}"));
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl Scripting {
    /// Read and load a game module from a file; the module is named after the file stem.
    pub fn load_module_file(&mut self, path: impl AsRef<Path>) -> Result<ModuleId, ScriptError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| ScriptError::Io(format!("{}: {e}", path.display())))?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        self.load_module(name, &bytes)
    }

    /// Hot reload: whenever the modification time of `path` changes, the script system
    /// (checking at most every 0.25 s of engine time) decodes and links the new file, swaps it
    /// in (the old module keeps running if that fails; the failure is logged), re-runs
    /// `init`, re-registers the module's exports and resets the `state` of its `Script`s to 0.
    pub fn watch(&mut self, id: ModuleId, path: impl AsRef<Path>) -> Result<(), ScriptError> {
        let path = path.as_ref().to_path_buf();
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map_err(|e| ScriptError::Io(format!("{}: {e}", path.display())))?;
        let slot = self.host.modules.get_mut(id.0 as usize).filter(|m| m.loaded).ok_or(ScriptError::NoSuchModule(id))?;
        slot.watch = Some(super::host::Watch { path, mtime: Some(mtime) });
        Ok(())
    }

    /// Check the watched files if 0.25 s of engine time passed; returns the reloaded modules.
    pub(crate) fn check_reload(&mut self, now: f64) -> Vec<ModuleId> {
        if now < self.next_watch_check {
            return Vec::new();
        }
        self.next_watch_check = now + 0.25;
        let mut reloaded = Vec::new();
        for idx in 0..self.host.modules.len() {
            let Some(w) = self.host.modules[idx].watch.as_ref().filter(|_| self.host.modules[idx].loaded) else { continue };
            let path = w.path.clone();
            let seen = w.mtime;
            let Ok(m) = std::fs::metadata(&path).and_then(|m| m.modified()) else { continue };
            if Some(m) == seen {
                continue;
            }
            if let Some(w) = self.host.modules[idx].watch.as_mut() {
                w.mtime = Some(m);
            }
            let name = self.host.modules[idx].name.clone();
            match self.reload(idx, &path) {
                Ok(()) => {
                    self.log(LogLevel::Info, format!("[{name}] reloaded from {}", path.display()));
                    reloaded.push(ModuleId(idx as u32));
                }
                Err(e) => self.log(LogLevel::Error, format!("[{name}] reload failed, keeping the old version: {e}")),
            }
        }
        reloaded
    }

    fn reload(&mut self, idx: usize, path: &Path) -> Result<(), ScriptError> {
        let bytes = std::fs::read(path).map_err(|e| ScriptError::Io(format!("{}: {e}", path.display())))?;
        let module = Module::decode(&bytes).map_err(ScriptError::Load)?;
        let (name, allowed) = {
            let s = &self.host.modules[idx];
            (s.name.clone(), s.allowed.clone())
        };
        let instance = self.link(&name, &module, allowed.as_deref())?;
        // Drop the old index entries unless something else (types) is registered under the
        // same owner; `register_exports` replaces what stays.
        if !self.registry.types().any(|t| t.owner == name) {
            let _ = self.registry.unregister_owner(&name);
        }
        let slot = &mut self.host.modules[idx];
        slot.module = module;
        slot.instance = Some(instance);
        slot.pending_init = true;
        slot.exports.clear();
        Ok(())
    }
}
