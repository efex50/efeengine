//! Loading and unloading modules and mods, declared provides (section 13), dependents and hot
//! reload. The copy ABI of a call across mods is in `provides.rs`.

use std::path::Path;

use efevm::{LinkError, HostOk, HostRegistry, Instance, Interface, Limits, Module, RegistryError, StructDesc};

use super::{DEFAULT_BUDGET, DEFAULT_MEMORY_PAGES, LogLevel, ModuleId, ScriptError, Scripting};
use super::host::{ModuleSlot, ScriptHost};
use crate::engine::resources::Resources;

/// Options of [`Scripting::load_mod`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModOptions {
    /// The mod's name: also the namespace (and owner) of the functions it provides.
    pub name: String,
    /// The version of the namespace the mod provides.
    pub version: u32,
    /// VM units per frame (default 1 000 000).
    pub budget: u64,
    /// Import namespace prefixes the mod may use: `engine.log` allows `engine.log` and
    /// `engine.log.x`, not `engine.logs`. Anything else is refused at load.
    pub allowed: Vec<String>,
    /// The most memory pages (64 KiB each) the module's instance may have
    /// (`Limits::max_memory_pages`, default 1024); a module that needs more is refused at load.
    pub memory_pages: u32,
    /// The doc of the mod's namespace in the API index; default ``Functions provided by the
    /// mod `name`.``
    pub doc: Option<String>,
}

impl ModOptions {
    /// Version 1, the default budget, nothing allowed yet (add with [`allow`](Self::allow)).
    pub fn new(name: impl Into<String>) -> Self {
        ModOptions { name: name.into(), version: 1, budget: DEFAULT_BUDGET, allowed: Vec::new(), memory_pages: DEFAULT_MEMORY_PAGES, doc: None }
    }

    pub fn allow(mut self, prefix: impl Into<String>) -> Self {
        self.allowed.push(prefix.into());
        self
    }

    pub fn with_budget(mut self, units: u64) -> Self {
        self.budget = units;
        self
    }

    pub fn with_memory_pages(mut self, pages: u32) -> Self {
        self.memory_pages = pages;
        self
    }

    pub fn with_version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn with_doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = Some(doc.into());
        self
    }
}

/// `&Name (N bytes)` or `no payload`, as `LinkError::EventPayload` spells it.
fn payload_text(p: Option<&(String, u32)>) -> String {
    match p {
        Some((s, n)) => format!("&{s} ({n} bytes)"),
        None => "no payload".into(),
    }
}

fn link_error(name: &str, e: efevm::LoadError) -> ScriptError {
    match e {
        efevm::LoadError::Link(l) => ScriptError::Link { module: name.to_string(), errors: vec![l] },
        other => ScriptError::Load(other),
    }
}

fn is_allowed(allowed: &[String], ns: &str) -> bool {
    allowed.iter().any(|p| ns == p || ns.strip_prefix(p.as_str()).is_some_and(|r| r.starts_with('.')))
}

/// Same size, alignment and fields (names, types, counts, offsets); docs, owner and component
/// id are not compared.
pub(crate) fn same_layout(a: &StructDesc, b: &StructDesc) -> bool {
    a.size == b.size
        && a.align == b.align
        && a.fields.len() == b.fields.len()
        && a.fields.iter().zip(&b.fields).all(|(x, y)| x.name == y.name && x.ty == y.ty && x.count == y.count && x.offset == y.offset)
}

/// `size 8, align 4, x: f32 @0, y: f32[2] @4`
fn describe(d: &StructDesc) -> String {
    let mut s = format!("size {}, align {}", d.size, d.align);
    for f in &d.fields {
        let count = if f.count > 1 { format!("[{}]", f.count) } else { String::new() };
        s.push_str(&format!(", {}: {}{count} @{}", f.name, f.ty.name(), f.offset));
    }
    s
}

pub(crate) fn default_doc(name: &str) -> String {
    format!("Functions provided by the mod `{name}`.")
}

impl Scripting {
    /// Check and instantiate. Nothing is changed on failure.
    ///
    /// Permissions first: every imported namespace and every namespace of section 14 must be in
    /// `allowed` (when there is a list); the module's own namespace `name` always is.
    ///
    /// A module that refers to events of its own namespace (or lists it in section 12) cannot
    /// link against the registry yet: those events are registered from its interface only after
    /// this check. They are then checked against `own` (the interface and the version the
    /// namespace gets) and the rest is linked from a copy without them; the returned flag is
    /// `true` for that case, and the caller links the real module once the interface is
    /// registered ([`link_registered`](Self::link_registered)).
    fn link(
        &self,
        name: &str,
        module: &Module,
        allowed: Option<&[String]>,
        memory_pages: u32,
        version: u32,
    ) -> Result<(Instance<ScriptHost>, bool), ScriptError> {
        let data = module.data();
        if let Some(allowed) = allowed {
            let mut bad: Vec<String> = Vec::new();
            let used = data.imports.iter().map(|i| i.namespace.as_str()).chain(data.event_namespaces());
            for ns in used {
                if ns != name && !is_allowed(allowed, ns) && !bad.iter().any(|b| b == ns) {
                    bad.push(ns.to_string());
                }
            }
            if !bad.is_empty() {
                return Err(ScriptError::Disallowed { module: name.to_string(), namespaces: bad });
            }
        }
        let imports = self.registry.imports();
        let limits = Limits { max_memory_pages: memory_pages, ..Limits::default() };
        let fail = |errors| ScriptError::Link { module: name.to_string(), errors };
        let own_api = data.api.as_ref().is_some_and(|a| a.namespaces.iter().any(|(n, _)| n == name));
        let own_events = data.events.iter().any(|e| e.namespace == name);
        if !own_api && !own_events {
            let errors = imports.check_module(module);
            if !errors.is_empty() {
                return Err(fail(errors));
            }
            let inst = Instance::new(module, &imports, limits).map_err(|e| link_error(name, e))?;
            return Ok((inst, false));
        }
        let mut stripped = data.clone();
        stripped.events.retain(|e| e.namespace != name);
        if let Some(api) = &mut stripped.api {
            api.namespaces.retain(|(n, _)| n != name);
        }
        let stripped = Module::new(stripped).map_err(ScriptError::Load)?;
        let mut errors = imports.check_module(&stripped);
        if let Some(api) = &data.api {
            for (n, v) in &api.namespaces {
                if n == name && *v != version {
                    errors.push(LinkError::NamespaceVersion { namespace: n.clone(), module: *v, host: version });
                }
            }
        }
        let iface = data.interface.as_ref();
        for ev in data.events.iter().filter(|e| e.namespace == name) {
            match iface.and_then(|i| i.event(&ev.name)) {
                None => errors.push(LinkError::UnknownEvent { namespace: ev.namespace.clone(), name: ev.name.clone() }),
                Some(pe) => {
                    let have = pe.payload.as_ref().map(|s| (s.clone(), iface.and_then(|i| i.struct_desc(s)).map_or(0, |d| d.size)));
                    if have != ev.payload {
                        errors.push(LinkError::EventPayload {
                            namespace: ev.namespace.clone(),
                            name: ev.name.clone(),
                            module: payload_text(ev.payload.as_ref()),
                            host: payload_text(have.as_ref()),
                        });
                    }
                }
            }
        }
        if !errors.is_empty() {
            return Err(fail(errors));
        }
        let inst = Instance::new(&stripped, &imports, limits).map_err(|e| link_error(name, e))?;
        Ok((inst, true))
    }

    /// Link the real module after its interface (and events) are registered.
    fn link_registered(&self, name: &str, module: &Module, memory_pages: u32) -> Result<Instance<ScriptHost>, ScriptError> {
        let imports = self.registry.imports();
        let errors = imports.check_module(module);
        if !errors.is_empty() {
            return Err(ScriptError::Link { module: name.to_string(), errors });
        }
        Instance::new(module, &imports, Limits { max_memory_pages: memory_pages, ..Limits::default() }).map_err(|e| link_error(name, e))
    }

    fn instantiate(
        &mut self,
        name: &str,
        module: Module,
        allowed: Option<Vec<String>>,
        version: u32,
        budget: u64,
        memory_pages: u32,
        doc: Option<String>,
    ) -> Result<ModuleId, ScriptError> {
        if !efevm::api::is_ident(name) {
            return Err(ScriptError::InvalidName(name.to_string()));
        }
        let taken = name == super::api_engine::OWNER
            || self.modules.iter().any(|m| m.loaded && m.name == name)
            || self
                .registry
                .namespaces()
                .any(|n| n.owner != name && (n.name == name || n.name.strip_prefix(name).is_some_and(|r| r.starts_with('.'))));
        if taken {
            return Err(ScriptError::NameTaken(name.to_string()));
        }
        // Only mods provide: a game module with an interface section is refused.
        if module.data().interface.is_some() && allowed.is_none() {
            return Err(ScriptError::NotAMod(name.to_string()));
        }
        let (mut instance, relink) = self.link(name, &module, allowed.as_deref(), memory_pages, version)?;
        let doc = doc.unwrap_or_else(|| default_doc(name));
        // The id the slot will get: trampolines capture it. The slot is pushed only after the
        // registration succeeded, so a failed load consumes no id and keeps nothing.
        let id = ModuleId(self.modules.len() as u32);
        let owned_before = self.owns_anything(name);
        if let Some(iface) = &module.data().interface {
            self.register_interface(id, name, version, &doc, iface)?;
        }
        if relink {
            // The mod's own events are registered now: link the real module.
            match self.link_registered(name, &module, memory_pages) {
                Ok(i) => instance = i,
                Err(e) => {
                    if !owned_before {
                        let _ = self.registry.unregister_owner(name);
                    }
                    return Err(e);
                }
            }
        }
        self.modules.push(ModuleSlot {
            name: name.to_string(),
            module,
            instance: Some(instance),
            loaded: true,
            version,
            budget,
            memory_pages,
            remaining: budget,
            allowed,
            pending_init: true,
            rng: super::host::seed_for(name),
            spawn_pool: Default::default(),
            pool_size: super::host::MIN_POOL,
            doc,
            package: None,
            #[cfg(not(target_family = "wasm"))]
            watch: None,
        });
        Ok(id)
    }

    /// Load a game script module: every registered namespace may be imported. A module with a
    /// section 13 interface is refused ([`ScriptError::NotAMod`]): only mods provide. `init`
    /// (if the module exports it) runs at the next script-system run; `Engine::load_module`
    /// runs it at once.
    pub fn load_module(&mut self, name: &str, bytes: &[u8]) -> Result<ModuleId, ScriptError> {
        let module = self.cache.decode(bytes).map_err(ScriptError::Load)?;
        self.load_module_decoded(name, module)
    }

    /// [`load_module`](Self::load_module) for a module that is already decoded.
    pub(crate) fn load_module_decoded(&mut self, name: &str, module: Module) -> Result<ModuleId, ScriptError> {
        self.instantiate(name, module, None, 1, DEFAULT_BUDGET, DEFAULT_MEMORY_PAGES, None)
    }

    /// Load a mod: only the namespaces in `opts.allowed` may be imported; a refused import
    /// names the namespace, and link problems are reported all at once. If the module has a
    /// section 13 interface, right after linking and before `init` the namespace `opts.name`
    /// (version `opts.version`, owner = the mod) is defined, its own structs are registered,
    /// its copied structs are checked against the registry
    /// ([`ScriptError::StructMismatch`]), one trampoline per provide is registered and the
    /// provided events are registered too (owner = the mod, before the mod's own events section
    /// is linked, so it may refer to them). Any
    /// failure registers nothing and keeps no module slot (no id is used).
    pub fn load_mod(&mut self, bytes: &[u8], opts: ModOptions) -> Result<ModuleId, ScriptError> {
        let module = self.cache.decode(bytes).map_err(ScriptError::Load)?;
        self.load_mod_decoded(module, opts)
    }

    /// [`load_mod`](Self::load_mod) for a module that is already decoded.
    pub(crate) fn load_mod_decoded(&mut self, module: Module, opts: ModOptions) -> Result<ModuleId, ScriptError> {
        self.instantiate(&opts.name, module, Some(opts.allowed), opts.version, opts.budget, opts.memory_pages, opts.doc)
    }

    /// The copies of external structs in `iface` must equal what the registry holds.
    fn check_copied(&self, iface: &Interface) -> Result<(), ScriptError> {
        for s in iface.copied_structs() {
            match self.registry.struct_type(&s.name) {
                Some(r) if same_layout(r, s) => {}
                Some(r) => {
                    return Err(ScriptError::StructMismatch { name: s.name.clone(), expected: describe(r), found: describe(s) });
                }
                None => {
                    return Err(ScriptError::StructMismatch {
                        name: s.name.clone(),
                        expected: "not registered".to_string(),
                        found: describe(s),
                    });
                }
            }
        }
        Ok(())
    }

    /// A dry run of [`commit_interface`](Self::commit_interface) on a scratch registry seeded
    /// with the real one (minus what `old`, the previous version of the same mod, registered),
    /// so the real registry is only touched when every rule holds.
    fn precheck_interface(&self, name: &str, version: u32, doc: &str, new: &Interface, old: Option<&Interface>) -> Result<(), ScriptError> {
        let mut scratch = HostRegistry::<ScriptHost>::new();
        for n in self.registry.namespaces().filter(|n| n.owner != name) {
            scratch.define_namespace(&n.name, n.version, &n.owner, &n.doc)?;
        }
        let old_own = |t: &str| old.is_some_and(|o| o.own_structs().any(|s| s.name == t));
        for t in self.registry.types().filter(|t| !(t.owner == name && old_own(&t.name))) {
            scratch.register_type(t.clone())?;
        }
        scratch.define_namespace(name, version, name, doc)?;
        for s in new.own_structs() {
            let mut d = s.clone();
            d.owner = name.to_string();
            scratch.register_type(d)?;
        }
        for hf in new.host_fns(name) {
            let ours = old.is_some_and(|o| o.provide(&hf.name).is_some());
            if !ours && self.registry.function(&hf.namespace, &hf.name).is_some() {
                return Err(RegistryError::DuplicateFunction { function: hf.qualified() }.into());
            }
            scratch.register(hf, |_c, _a, _r| Ok(HostOk::Continue))?;
        }
        for ed in new.event_descs(name) {
            scratch.register_event(ed)?;
        }
        Ok(())
    }

    /// Define the namespace, register the own structs that are not there yet, and register
    /// (or replace) one trampoline per provide.
    fn commit_interface(&mut self, id: ModuleId, name: &str, version: u32, doc: &str, new: &Interface) -> Result<(), RegistryError> {
        self.registry.define_namespace(name, version, name, doc)?;
        for s in new.own_structs() {
            if self.registry.struct_type(&s.name).is_none() {
                let mut d = s.clone();
                d.owner = name.to_string();
                self.registry.register_type(d)?;
            }
        }
        for hf in new.host_fns(name) {
            let (ns, pname) = (name.to_string(), hf.name.clone());
            let call = move |c: &mut efevm::Caller<'_, ScriptHost>, a: &efevm::Args<'_>, r: &mut efevm::Results<'_>| {
                super::provides::call(c, id, &ns, &pname, a, r)
            };
            if self.registry.function(&hf.namespace, &hf.name).is_some() {
                self.registry.replace(hf, call)?;
            } else {
                self.registry.register(hf, call)?;
            }
        }
        for ed in new.event_descs(name) {
            if self.registry.event(&ed.namespace, &ed.name).is_some() {
                self.registry.replace_event(ed)?;
            } else {
                self.registry.register_event(ed)?;
            }
        }
        Ok(())
    }

    /// Registration at load (e8-design section 6).
    fn register_interface(&mut self, id: ModuleId, name: &str, version: u32, doc: &str, iface: &Interface) -> Result<(), ScriptError> {
        self.check_copied(iface)?;
        self.precheck_interface(name, version, doc, iface, None)?;
        let owned_before = self.owns_anything(name);
        if let Err(e) = self.commit_interface(id, name, version, doc, iface) {
            // Not reachable after the dry run; undo what this load registered. When something
            // of the owner was registered before (Rust-side structs), it cannot be told apart
            // from this load's entries (the registry removes by owner only), so it stays.
            if !owned_before {
                let _ = self.registry.unregister_owner(name);
            }
            return Err(e.into());
        }
        Ok(())
    }

    fn owns_anything(&self, owner: &str) -> bool {
        self.registry.namespaces().any(|n| n.owner == owner) || self.registry.types().any(|t| t.owner == owner)
    }

    /// Indices of the loaded modules (other than `idx`) that depend on module `idx`: they
    /// import from a namespace the module owns, or copy a struct it owns.
    pub(crate) fn dependents_of(&self, idx: usize) -> Vec<usize> {
        let Some(owner) = self.modules.get(idx).map(|m| m.name.as_str()) else { return Vec::new() };
        self.modules
            .iter()
            .enumerate()
            .filter(|(j, m)| *j != idx && m.loaded && self.depends_on(m, owner))
            .map(|(j, _)| j)
            .collect()
    }

    fn depends_on(&self, m: &ModuleSlot, owner: &str) -> bool {
        let d = m.module.data();
        d.imports.iter().any(|i| self.registry.namespace(&i.namespace).is_some_and(|n| n.owner == owner))
            || d.events.iter().any(|e| self.registry.namespace(&e.namespace).is_some_and(|n| n.owner == owner))
            || d.interface.as_ref().is_some_and(|i| i.copied_structs().any(|s| s.owner == owner))
    }

    fn names_of(&self, idxs: &[usize]) -> Vec<String> {
        idxs.iter().map(|&j| self.modules[j].name.clone()).collect()
    }

    /// Unload module `id`: its instance is dropped and everything it registered
    /// (`unregister_owner(name)`: namespace, functions, own structs) leaves the index. Refused
    /// with [`ScriptError::Needed`] while another loaded module depends on it
    /// ([`unload_cascade`](Self::unload_cascade) removes those first). `Engine::unload_module`
    /// is the public path (it also drops component types owned by the module).
    pub(crate) fn unload(&mut self, id: ModuleId) -> Result<String, ScriptError> {
        let idx = id.0 as usize;
        let slot = self.modules.get(idx).filter(|m| m.loaded).ok_or(ScriptError::NoSuchModule(id))?;
        let by = self.dependents_of(idx);
        if !by.is_empty() {
            return Err(ScriptError::Needed { module: slot.name.clone(), by: self.names_of(&by) });
        }
        self.unload_now(idx)
    }

    /// Unload `id` and, first, every loaded module that depends on it (transitively), the
    /// latest loaded first. Returns the removed modules in removal order (`id` last) with their
    /// names. If one removal fails, the ones before it stay unloaded.
    pub(crate) fn unload_cascade(&mut self, id: ModuleId) -> Result<Vec<(ModuleId, String)>, ScriptError> {
        let root = id.0 as usize;
        self.modules.get(root).filter(|m| m.loaded).ok_or(ScriptError::NoSuchModule(id))?;
        let mut set = vec![root];
        let mut i = 0;
        while i < set.len() {
            for d in self.dependents_of(set[i]) {
                if !set.contains(&d) {
                    set.push(d);
                }
            }
            i += 1;
        }
        set.sort_unstable_by(|a, b| b.cmp(a));
        let mut out = Vec::new();
        for idx in set {
            let name = self.unload_now(idx)?;
            out.push((ModuleId(idx as u32), name));
        }
        Ok(out)
    }

    pub(crate) fn unload_now(&mut self, idx: usize) -> Result<String, ScriptError> {
        let name = self.modules[idx].name.clone();
        self.registry.unregister_owner(&name)?;
        let slot = &mut self.modules[idx];
        slot.loaded = false;
        slot.instance = None;
        slot.package = None;
        #[cfg(not(target_family = "wasm"))]
        {
            slot.watch = None;
        }
        Ok(name)
    }

    /// Replace the code of module `idx` (hot reload; e8-design section 6). `version` is the
    /// namespace version the new code is loaded with (a file reload keeps the old one; a
    /// package reload may change it). The new module is linked first, then, for a mod with an
    /// interface, refused with [`ScriptError::ReloadRefused`] if a loaded dependent imports a
    /// function the new version drops or whose lowered signature changes, uses a struct
    /// whose layout changes, or the version changes while dependents are loaded. If accepted,
    /// provides that disappeared leave the registry, new ones are registered and changed ones
    /// replaced (trampolines look the provide up by name on every call, so dependents need no
    /// relinking), the instance is swapped and `init` runs again.
    ///
    /// The registry can only remove by owner, so dropping a provide or changing an own struct
    /// rebuilds everything the mod owns (the namespace, its structs, its provides). That is
    /// refused when the mod owns other namespaces, or when another owner's function takes one of
    /// its structs.
    pub(crate) fn reload_module(&mut self, idx: usize, module: Module, version: u32) -> Result<(), ScriptError> {
        let (name, allowed, old_module, old_version, doc) = {
            let s = &self.modules[idx];
            (s.name.clone(), s.allowed.clone(), s.module.clone(), s.version, s.doc.clone())
        };
        let (old_if, new_if) = (old_module.data().interface.as_ref(), module.data().interface.as_ref());
        if new_if.is_some() && allowed.is_none() {
            return Err(ScriptError::NotAMod(name));
        }
        let memory_pages = self.modules[idx].memory_pages;
        let (mut instance, relink) = self.link(&name, &module, allowed.as_deref(), memory_pages, version)?;
        if old_if.is_some() || new_if.is_some() {
            let refuse = |reasons: Vec<String>| ScriptError::ReloadRefused { module: name.clone(), reasons };
            let deps = self.dependents_of(idx);
            let mut problems = Vec::new();
            if !deps.is_empty() && version != old_version {
                problems.push(format!(
                    "the namespace version changes from {old_version} to {version} while {} depend{} on it",
                    self.names_of(&deps).join(", "),
                    if deps.len() == 1 { "s" } else { "" }
                ));
            }
            for &d in &deps {
                let dm = &self.modules[d];
                let dd = dm.module.data();
                for imp in dd.imports.iter().filter(|i| i.namespace == name) {
                    let want = dd.types.get(imp.type_idx as usize);
                    match new_if.and_then(|n| n.provide(&imp.name)) {
                        None => problems.push(format!("`{}` imports `{name}.{}`, which the new version does not provide", dm.name, imp.name)),
                        Some(p) if Some(&p.func_type()) != want => problems.push(format!(
                            "`{}` imports `{name}.{}` as {}, the new version takes {}",
                            dm.name,
                            imp.name,
                            want.map_or("?".to_string(), |t| t.to_string()),
                            p.func_type()
                        )),
                        Some(_) => {}
                    }
                }
                for ev in dd.events.iter().filter(|e| e.namespace == name) {
                    match new_if.and_then(|n| n.event(&ev.name)) {
                        None => problems.push(format!("`{}` uses the event `{name}.{}`, which the new version does not provide", dm.name, ev.name)),
                        Some(pe) => {
                            let have = pe.payload.as_ref().map(|s| (s.clone(), new_if.and_then(|n| n.struct_desc(s)).map_or(0, |d| d.size)));
                            if have != ev.payload {
                                problems.push(format!(
                                    "`{}` uses the event `{name}.{}` with {}, the new version has {}",
                                    dm.name,
                                    ev.name,
                                    payload_text(ev.payload.as_ref()),
                                    payload_text(have.as_ref())
                                ));
                            }
                        }
                    }
                }
                for s in dd.interface.iter().flat_map(|i| i.copied_structs()).filter(|s| s.owner == name) {
                    match new_if.and_then(|n| n.own_structs().find(|o| o.name == s.name)) {
                        Some(n) if same_layout(n, s) => {}
                        _ => problems.push(format!("`{}` uses struct `{}`, whose layout the new version changes or removes", dm.name, s.name)),
                    }
                }
            }
            if !problems.is_empty() {
                return Err(refuse(problems));
            }
            if let Some(n) = new_if {
                self.check_copied(n)?;
                self.precheck_interface(&name, version, &doc, n, old_if)?;
            }
            let rebuild = match (old_if, new_if) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(o), Some(n)) => {
                    o.provides.iter().any(|p| n.provide(&p.name).is_none())
                        || o.events.iter().any(|e| n.event(&e.name).is_none())
                        || o.own_structs().any(|s| n.own_structs().find(|x| x.name == s.name).is_none_or(|x| !same_layout(x, s)))
                }
            };
            if rebuild {
                if let Some(other) = self.registry.namespaces().find(|n| n.owner == name && n.name != name) {
                    return Err(refuse(vec![format!(
                        "provides disappear, which needs the registry to rebuild the mod's entries, but the mod also owns the namespace `{}`",
                        other.name
                    )]));
                }
                let old_own = |t: &str| old_if.is_some_and(|o| o.own_structs().any(|s| s.name == t));
                let keep: Vec<StructDesc> = self.registry.types().filter(|t| t.owner == name && !old_own(&t.name)).cloned().collect();
                self.registry.unregister_owner(&name).map_err(|e| refuse(vec![e.to_string()]))?;
                for t in keep {
                    let _ = self.registry.register_type(t);
                }
            }
            if let Some(n) = new_if
                && let Err(e) = self.commit_interface(ModuleId(idx as u32), &name, version, &doc, n)
            {
                // Not reachable after the dry run.
                self.log(LogLevel::Error, format!("[{name}] registering the new interface failed half way: {e}"));
            }
        }
        if relink {
            // The mod's own events are registered now: link the real module (the checks of
            // `link` passed, so this cannot fail in practice).
            match self.link_registered(&name, &module, memory_pages) {
                Ok(i) => instance = i,
                Err(e) => {
                    self.log(LogLevel::Error, format!("[{name}] linking the new version after registering its events failed: {e}"));
                    return Err(e);
                }
            }
        }
        let slot = &mut self.modules[idx];
        slot.module = module;
        slot.instance = Some(instance);
        slot.version = version;
        slot.pending_init = true;
        Ok(())
    }
}

impl Scripting {
    /// Every 0.25 s of engine time: reload the watched files (native) and the mods whose
    /// source reports a change. Returns the reloaded modules.
    pub(crate) fn check_reload(&mut self, now: f64, res: &mut Resources) -> Vec<ModuleId> {
        if now < self.next_watch_check {
            return Vec::new();
        }
        self.next_watch_check = now + 0.25;
        let mut reloaded = Vec::new();
        #[cfg(not(target_family = "wasm"))]
        self.check_watches(&mut reloaded, res);
        self.check_sources(&mut reloaded, res);
        reloaded
    }
}

#[cfg(not(target_family = "wasm"))]
impl Scripting {
    /// Hot reload: whenever the modification time of `path` changes, the script system
    /// (checking at most every 0.25 s of engine time) decodes and links the new file, swaps it
    /// in (the old module keeps running if that fails; the failure is logged), re-runs
    /// `init`, updates the module's provides and resets the `state` of its `Script`s to 0. The
    /// reload is refused (the old module keeps running, the reason is logged) when it would
    /// break loaded dependents; see [`reload_module`](Self::reload_module) for the rules.
    pub fn watch(&mut self, id: ModuleId, path: impl AsRef<Path>) -> Result<(), ScriptError> {
        let path = path.as_ref().to_path_buf();
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map_err(|e| ScriptError::Io(format!("{}: {e}", path.display())))?;
        let slot = self.modules.get_mut(id.0 as usize).filter(|m| m.loaded).ok_or(ScriptError::NoSuchModule(id))?;
        slot.watch = Some(super::host::Watch { package: false, source: false, path, mtime: Some(mtime) });
        Ok(())
    }

    /// Hot reload of a game script from source: whenever a `.efa` file under the directory of
    /// `path` (the root file and the files it `use`s live there) changes, the script system
    /// (checking at most every 0.25 s of engine time) recompiles `path` through the cache and
    /// swaps the module in place, like [`watch`](Self::watch): the module id stays, `init` runs
    /// again and the `state` of its `Script`s resets to 0. A compile or link error is logged and
    /// the old version keeps running.
    #[cfg(feature = "efa")]
    pub fn watch_source(&mut self, id: ModuleId, path: impl AsRef<Path>) -> Result<(), ScriptError> {
        let path = path.as_ref().to_path_buf();
        let mtime = newest_efa_mtime(&path).ok_or_else(|| ScriptError::Io(format!("{}: not readable", path.display())))?;
        let slot = self.modules.get_mut(id.0 as usize).filter(|m| m.loaded).ok_or(ScriptError::NoSuchModule(id))?;
        slot.watch = Some(super::host::Watch { package: false, source: true, path, mtime: Some(mtime) });
        Ok(())
    }

    /// Check the watched files.
    fn check_watches(&mut self, reloaded: &mut Vec<ModuleId>, res: &mut Resources) {
        for idx in 0..self.modules.len() {
            let Some(w) = self.modules[idx].watch.as_ref().filter(|_| self.modules[idx].loaded) else { continue };
            let path = w.path.clone();
            let seen = w.mtime;
            let is_source = w.source;
            let now = if is_source {
                newest_efa_mtime(&path)
            } else {
                std::fs::metadata(&path).and_then(|m| m.modified()).ok()
            };
            let Some(m) = now else { continue };
            if Some(m) == seen {
                continue;
            }
            if let Some(w) = self.modules[idx].watch.as_mut() {
                w.mtime = Some(m);
            }
            let name = self.modules[idx].name.clone();
            let ok = if is_source {
                match self.reload_source(idx, &path) {
                    Ok(()) => {
                        self.log(LogLevel::Info, format!("[{name}] recompiled and reloaded from {}", path.display()));
                        true
                    }
                    Err(e) => {
                        self.log(LogLevel::Error, format!("[{name}] reload failed, keeping the old version: {e}"));
                        false
                    }
                }
            } else if self.modules[idx].watch.as_ref().is_some_and(|w| w.package) {
                let r = self.reload_package(idx, &path, res);
                self.log_package_reload(&name, &path, r)
            } else {
                match self.reload(idx, &path) {
                    Ok(()) => {
                        self.log(LogLevel::Info, format!("[{name}] reloaded from {}", path.display()));
                        true
                    }
                    Err(e) => {
                        self.log(LogLevel::Error, format!("[{name}] reload failed, keeping the old version: {e}"));
                        false
                    }
                }
            };
            if ok {
                reloaded.push(ModuleId(idx as u32));
            }
        }
    }

    #[cfg(feature = "efa")]
    fn reload_source(&mut self, idx: usize, path: &Path) -> Result<(), ScriptError> {
        let display = path.display().to_string();
        let (files, root) = efa::asm::FsSource::for_file(&display);
        let index = self.registry().index();
        let game = self.registry().game().to_string();
        let (module, _) = self
            .cache
            .compile(&game, &root, &files, &index, &efa::asm::AsmOptions::default())
            .map_err(|d| ScriptError::Io(format!("{display}: {d}")))?;
        let version = self.modules[idx].version;
        self.reload_module(idx, module, version)
    }

    #[cfg(not(feature = "efa"))]
    fn reload_source(&mut self, _idx: usize, _path: &Path) -> Result<(), ScriptError> {
        Err(ScriptError::Io("the efa feature is off".into()))
    }

    fn reload(&mut self, idx: usize, path: &Path) -> Result<(), ScriptError> {
        let bytes = std::fs::read(path).map_err(|e| ScriptError::Io(format!("{}: {e}", path.display())))?;
        let module = self.cache.decode(&bytes).map_err(ScriptError::Load)?;
        let version = self.modules[idx].version;
        self.reload_module(idx, module, version)
    }
}

/// The newest modification time of the `.efa` files under the directory of `path`.
#[cfg(not(target_family = "wasm"))]
fn newest_efa_mtime(path: &Path) -> Option<std::time::SystemTime> {
    fn walk(dir: &Path, best: &mut Option<std::time::SystemTime>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, best);
            } else if p.extension().is_some_and(|x| x == "efa") {
                if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                    if best.is_none_or(|b| m > b) {
                        *best = Some(m);
                    }
                }
            }
        }
    }
    let mut best = None;
    walk(path.parent()?, &mut best);
    best
}
