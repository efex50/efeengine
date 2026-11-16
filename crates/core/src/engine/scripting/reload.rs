//! Syncing, reloading and unloading mods (the "mod update" half of E10).
//!
//! `load_mods` / `reload_mods` read every source, classify each item by the content hash of
//! the loaded mod with the same id (new, unchanged, changed), then do one dependency resolve.
//! A changed mod is *swapped* ([`Scripting::swap_mods`]): it and every loaded dependent are
//! unloaded (asset handlers get `unload`), the new version is loaded, then each dependent is
//! loaded again (a folder dependent is recompiled against the new version, a package
//! dependent is loaded from the retained module). If anything fails, everything that was
//! loaded in the attempt is unloaded and the previous versions are loaded again from what the
//! engine retained. Module ids change on a swap or a restore (an id is never reused), and
//! `init` runs again for every module that was loaded again. When even the restore fails the
//! mod stays unloaded and the error says so (`ModError::ReloadFailed { restored: false }`).
//!
//! A game module (`load_module`, `.efa`) that depends on a swapped mod cannot be put back
//! (the engine keeps no source for it), so such a reload is refused before anything changes.

use std::collections::{BTreeMap, BTreeSet};

use efemod::{Manifest, ResolveProblem};

use super::load::{FolderKept, ModBody, ModItem};
use super::packages::{Built, LoadReport, ModError, Origin, Reloaded, UnloadReport};
use super::{LogLevel, ModuleId, ScriptError, Scripting};
use crate::engine::resources::Resources;
use crate::engine::types::TypeRegistry;

/// Builds the new version of a mod that is being replaced (compiles a folder, or just hands
/// over the read package) and what a source keeps of a folder.
type Build<'a> = Box<dyn FnOnce(&mut Scripting) -> Result<(Built, FolderKept), ModError> + 'a>;

/// One mod to replace in `swap_mods`.
pub(crate) struct Target<'a> {
    /// The loaded module being replaced.
    pub idx: usize,
    pub label: String,
    /// Ids of the dependencies of the new version.
    pub deps: Vec<String>,
    /// `Some(source index)`: the new version comes from that source and gets an origin;
    /// `None`: keep the origin the mod had (without its folder).
    pub source: Option<usize>,
    pub build: Build<'a>,
}

enum Kind {
    Pkg(Built),
    #[cfg(feature = "efa")]
    Folder { files: Box<dyn efa::asm::FileSource>, assets: Vec<String>, hash: String },
}

struct Cand {
    source: usize,
    label: String,
    manifest: Manifest,
    hash: String,
    kind: Kind,
}

/// A loaded mod as it was before a swap: to load it again, and its origin.
struct Saved {
    idx: usize,
    name: String,
    built: Built,
    origin: Option<Origin>,
}

impl Scripting {
    /// Loaded modules that depend on `idx`: they import a namespace it owns (or copy a struct
    /// or use an event of it), or declare it in their manifest.
    fn direct_dependents(&self, idx: usize) -> Vec<usize> {
        let mut v = self.dependents_of(idx);
        let name = &self.host.modules[idx].name;
        for (j, m) in self.host.modules.iter().enumerate() {
            if j != idx && m.loaded && !v.contains(&j) && m.package.as_ref().is_some_and(|p| p.manifest.dependencies.iter().any(|d| d.id == *name)) {
                v.push(j);
            }
        }
        v
    }

    /// Every loaded module that depends on `root`, transitively (not `root`).
    fn dependents_closure(&self, root: usize) -> BTreeSet<usize> {
        let mut set = BTreeSet::new();
        let mut todo = vec![root];
        while let Some(i) = todo.pop() {
            for d in self.direct_dependents(i) {
                if d != root && set.insert(d) {
                    todo.push(d);
                }
            }
        }
        set
    }

    /// Unload module `idx` for good: its instance, registrations, asset handlers and origin,
    /// and the component types it owned.
    fn unload_swap(&mut self, idx: usize, res: &mut Resources, types: &mut TypeRegistry) -> Result<String, ScriptError> {
        let name = self.unload_now(idx)?;
        self.release_module(ModuleId(idx as u32), res);
        types.remove_owner(&name);
        Ok(name)
    }

    /// Unload `idx` and its dependents (dependents first). Refused, with nothing unloaded,
    /// when a game module is among the dependents. Returns the unloaded mod ids.
    fn unload_closure(&mut self, idx: usize, res: &mut Resources, types: &mut TypeRegistry) -> Result<Vec<String>, ModError> {
        let mut all: Vec<usize> = self.dependents_closure(idx).into_iter().collect();
        all.push(idx);
        all.sort_unstable_by(|a, b| b.cmp(a));
        let games: Vec<String> = all.iter().filter(|&&i| self.host.modules[i].package.is_none()).map(|&i| self.host.modules[i].name.clone()).collect();
        if !games.is_empty() {
            return Err(ScriptError::Needed { module: self.host.modules[idx].name.clone(), by: games }.into());
        }
        let mut out = Vec::new();
        for i in all {
            out.push(self.unload_swap(i, res, types)?);
        }
        Ok(out)
    }

    /// `Engine::unload_mods`: every loaded mod, dependents before dependencies. A mod that a
    /// game module (or a mod that could not be unloaded) still depends on stays loaded and is
    /// reported in `failed`.
    pub(crate) fn unload_mods(&mut self, res: &mut Resources, types: &mut TypeRegistry) -> UnloadReport {
        let mut report = UnloadReport::default();
        for idx in (0..self.host.modules.len()).rev() {
            let slot = &self.host.modules[idx];
            if !slot.loaded || slot.package.is_none() {
                continue;
            }
            let by = self.direct_dependents(idx);
            if !by.is_empty() {
                let e = ScriptError::Needed { module: slot.name.clone(), by: by.iter().map(|&j| self.host.modules[j].name.clone()).collect() };
                report.failed.push((slot.name.clone(), e.into()));
                continue;
            }
            match self.unload_swap(idx, res, types) {
                Ok(name) => report.unloaded.push(name),
                Err(e) => report.failed.push((self.host.modules[idx].name.clone(), e.into())),
            }
        }
        report
    }

    /// A folder dependent is recompiled from the files its source kept.
    #[cfg(feature = "efa")]
    fn rebuild_origin(&mut self, o: &Origin) -> Option<Result<Built, ModError>> {
        let (files, assets) = o.folder.as_ref()?;
        Some(self.build_folder(&o.label, &**files, assets, None))
    }

    #[cfg(not(feature = "efa"))]
    fn rebuild_origin(&mut self, _o: &Origin) -> Option<Result<Built, ModError>> {
        None
    }

    /// Replace the loaded mods `targets` (see the module doc). Returns every mod that was
    /// loaded again, in load order, as `(id, new module)`; on failure the cause and whether
    /// the previous versions are back.
    pub(crate) fn swap_mods(
        &mut self,
        targets: Vec<Target<'_>>,
        res: &mut Resources,
        types: &mut TypeRegistry,
    ) -> Result<Vec<(String, ModuleId)>, (String, bool)> {
        let mut affected: BTreeSet<usize> = BTreeSet::new();
        for t in &targets {
            affected.insert(t.idx);
            affected.extend(self.dependents_closure(t.idx));
        }
        if let Some(&i) = affected.iter().find(|&&i| self.host.modules[i].package.is_none()) {
            let target = targets.first().map(|t| self.host.modules[t.idx].name.clone()).unwrap_or_default();
            let name = self.host.modules[i].name.clone();
            return Err((format!("`{name}` is a game module that depends on `{target}` and cannot be reloaded with it"), true));
        }
        let names: BTreeMap<usize, String> = affected.iter().map(|&i| (i, self.host.modules[i].name.clone())).collect();
        let affected_names: BTreeSet<&String> = names.values().collect();

        // Targets that need nothing that is being replaced are built now: a broken new
        // version fails before anything is touched.
        let mut pending: BTreeMap<usize, Target<'_>> = BTreeMap::new();
        let mut prepared: BTreeMap<usize, (Built, FolderKept)> = BTreeMap::new();
        let target_info: BTreeMap<usize, (Option<usize>, String)> = targets.iter().map(|t| (t.idx, (t.source, t.label.clone()))).collect();
        for mut t in targets {
            if t.deps.iter().all(|d| !affected_names.contains(d)) {
                let build = std::mem::replace(&mut t.build, Box::new(|_| Err(ModError::Build("already built".into()))));
                match build(self) {
                    Ok(b) => {
                        prepared.insert(t.idx, b);
                    }
                    Err(e) => return Err((e.to_string(), true)),
                }
            }
            pending.insert(t.idx, t);
        }

        let mut saved: Vec<Saved> = Vec::new();
        for &i in &affected {
            let id = ModuleId(i as u32);
            let built = self.host.modules[i].package.as_ref().map(|p| p.rebuilt()).expect("checked: a package");
            let origin = self.origins.iter().position(|o| o.id == id).map(|p| self.origins.remove(p));
            saved.push(Saved { idx: i, name: names[&i].clone(), built, origin });
        }
        for &i in affected.iter().rev() {
            if let Err(e) = self.unload_swap(i, res, types) {
                let cause = format!("unloading `{}`: {e}", names[&i]);
                let restored = self.restore(saved, res);
                return Err((cause, restored));
            }
        }

        // Load again, a mod after the mods it depends on.
        let mut remaining: Vec<usize> = affected.iter().copied().collect();
        let mut done: Vec<(usize, ModuleId)> = Vec::new();
        let mut kept: BTreeMap<usize, FolderKept> = BTreeMap::new();
        let mut failure: Option<String> = None;
        while !remaining.is_empty() {
            let blocked = |me: usize, deps: &[String]| deps.iter().any(|d| remaining.iter().any(|&o| o != me && names[&o] == *d));
            let pos = remaining.iter().position(|&i| match pending.get(&i) {
                Some(t) => !blocked(i, &t.deps),
                None => {
                    let s = saved.iter().find(|s| s.idx == i).expect("saved");
                    let deps: Vec<String> = s.built.pkg.manifest.dependencies.iter().map(|d| d.id.clone()).collect();
                    !blocked(i, &deps)
                }
            });
            let Some(pos) = pos else {
                failure = Some("the mods being reloaded depend on each other in a cycle".to_string());
                break;
            };
            let old = remaining.remove(pos);
            let step = (|| -> Result<(ModuleId, FolderKept), ModError> {
                let (built, folder) = match pending.remove(&old) {
                    Some(t) => match prepared.remove(&old) {
                        Some(b) => b,
                        None => (t.build)(self)?,
                    },
                    None => {
                        let s = saved.iter().find(|s| s.idx == old).expect("saved");
                        let rebuilt = match &s.origin {
                            Some(o) => self.rebuild_origin(o),
                            None => None,
                        };
                        match rebuilt {
                            Some(r) => (r?, None),
                            None => (Built { pkg: s.built.pkg.clone(), label: s.built.label.clone(), hash: s.built.hash.clone() }, None),
                        }
                    }
                };
                let id = self.load_package_item(built, res)?;
                Ok((id, folder))
            })();
            match step {
                Ok((id, folder)) => {
                    done.push((old, id));
                    kept.insert(old, folder);
                }
                Err(e) => {
                    failure = Some(format!("loading `{}`: {e}", names[&old]));
                    break;
                }
            }
        }
        if let Some(cause) = failure {
            for &(_, id) in done.iter().rev() {
                if let Err(e) = self.unload_swap(id.0 as usize, res, types) {
                    self.log(LogLevel::Error, format!("reload: undoing the load of module {} failed: {e}", id.0));
                }
            }
            let restored = self.restore(saved, res);
            return Err((cause, restored));
        }

        // Success: origins follow their mods to the new ids.
        let mut out = Vec::new();
        for (old, id) in &done {
            let s = saved.iter_mut().find(|s| s.idx == *old).expect("saved");
            let folder = kept.remove(old).flatten();
            match target_info.get(old) {
                Some((Some(source), label)) => self.origins.push(Origin { id: *id, source: *source, label: label.clone(), folder }),
                Some((None, _)) => {
                    if let Some(mut o) = s.origin.take() {
                        o.id = *id;
                        o.folder = None;
                        self.origins.push(o);
                    }
                }
                None => {
                    if let Some(mut o) = s.origin.take() {
                        o.id = *id;
                        self.origins.push(o);
                    }
                }
            }
            out.push((s.name.clone(), *id));
        }
        Ok(out)
    }

    /// Load the saved mods again, in load order, with their origins. True when all are back.
    fn restore(&mut self, saved: Vec<Saved>, res: &mut Resources) -> bool {
        let mut ok = true;
        for Saved { idx, name, built, origin } in saved {
            let id = match self.loaded_mod_idx(&name) {
                Some(i) => Ok(ModuleId(i as u32)),
                None => self.load_package_item(built, res),
            };
            match id {
                Ok(id) => {
                    if let Some(mut o) = origin {
                        o.id = id;
                        self.origins.push(o);
                    }
                }
                Err(e) => {
                    ok = false;
                    self.log(LogLevel::Error, format!("[{name}] restoring the previous version (module {idx}) failed, the mod is left unloaded: {e}"));
                }
            }
        }
        ok
    }
}

impl Scripting {
    /// Load one candidate that is not loaded yet.
    fn load_cand(&mut self, c: Cand, report: &mut LoadReport, failed_ids: &mut Vec<String>, res: &mut Resources) {
        let Cand { source, label, manifest, kind, .. } = c;
        if let Some(dep) = manifest.dependencies.iter().find(|d| failed_ids.contains(&d.id)) {
            failed_ids.push(manifest.id.clone());
            report.failed.push((label, ModError::Resolve(ResolveProblem::DependencyFailed { dep: dep.id.clone() })));
            return;
        }
        let r: Result<(ModuleId, FolderKept), ModError> = match kind {
            Kind::Pkg(b) => self.load_package_item(b, res).map(|id| (id, None)),
            #[cfg(feature = "efa")]
            Kind::Folder { files, assets, hash } => match self.build_folder(&label, &*files, &assets, Some(hash)) {
                Ok(b) => self.load_package_item(b, res).map(|id| (id, Some((files, assets)))),
                Err(e) => Err(e),
            },
        };
        match r {
            Ok((id, folder)) => {
                self.origins.push(Origin { id, source, label: label.clone(), folder });
                report.loaded.push((label, id));
            }
            Err(e) => {
                failed_ids.push(manifest.id);
                report.failed.push((label, e));
            }
        }
    }

    /// `Engine::load_mods` (`reload == false`) and `Engine::reload_mods` (`true`): list every
    /// source in registration order, read the manifests and hash the content, classify each
    /// item by the loaded mod with the same id (unchanged: `skipped`; changed: `AlreadyLoaded`
    /// in `failed`, or with `reload` replaced; absent: new), with `reload` unload the mods
    /// whose source no longer lists them, then one `efemod::resolve` over the items and the
    /// mods that stay loaded. New mods load in resolution order; changed mods are swapped
    /// together after the new mods they do not depend on, and the new mods that depend on a
    /// changed one load after the swap. `init`s are pending; the engine runs them after.
    pub(crate) fn load_mods(&mut self, res: &mut Resources, types: &mut TypeRegistry, reload: bool) -> LoadReport {
        let mut report = LoadReport::default();
        if self.sources.is_empty() {
            return report;
        }
        let mut sources = std::mem::take(&mut self.sources);
        let mut listed: Vec<BTreeSet<String>> = Vec::new();
        let mut read: Vec<Cand> = Vec::new();
        for (si, src) in sources.iter_mut().enumerate() {
            let mut labels = BTreeSet::new();
            for ModItem { label, body } in src.list() {
                labels.insert(label.clone());
                match body {
                    ModBody::Package(bytes) => match self.read_built(&label, &bytes) {
                        Ok(b) => read.push(Cand { source: si, label, manifest: b.pkg.manifest.clone(), hash: b.hash.clone(), kind: Kind::Pkg(b) }),
                        Err(e) => report.failed.push((label, e)),
                    },
                    #[cfg(feature = "efa")]
                    ModBody::Folder { files, assets } => match Self::peek_folder(&*files) {
                        Ok((manifest, hash)) => {
                            read.push(Cand { source: si, label, manifest, hash: hash.clone(), kind: Kind::Folder { files, assets, hash } })
                        }
                        Err(e) => report.failed.push((label, e)),
                    },
                }
            }
            listed.push(labels);
        }

        // Classify against what is loaded.
        let mut fresh: Vec<Cand> = Vec::new();
        let mut changed: Vec<Cand> = Vec::new();
        for c in read {
            let Some(idx) = self.loaded_mod_idx(&c.manifest.id) else {
                fresh.push(c);
                continue;
            };
            let module = ModuleId(idx as u32);
            if self.host.modules[idx].package.as_ref().is_some_and(|p| p.hash == c.hash) {
                report.skipped.push((c.label, module));
            } else if reload {
                changed.push(c);
            } else {
                report.failed.push((c.label, ModError::AlreadyLoaded { id: c.manifest.id, module, same_contents: false }));
            }
        }

        // Mods of a source that no longer lists them.
        let mut removed: Vec<(usize, String)> = Vec::new();
        if reload {
            for o in &self.origins {
                let id = &self.host.modules[o.id.0 as usize].name;
                let relisted = changed.iter().chain(&fresh).any(|c| c.manifest.id == *id);
                if self.is_loaded(o.id) && !relisted && listed.get(o.source).is_some_and(|l| !l.contains(&o.label)) {
                    removed.push((o.id.0 as usize, o.label.clone()));
                }
            }
            removed.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        }

        // One resolve over the candidates and the mods that stay as they are.
        let n_changed = changed.len();
        let all: Vec<Cand> = changed.into_iter().chain(fresh).collect();
        let leaving: BTreeSet<String> = removed.iter().map(|(i, _)| self.host.modules[*i].name.clone()).collect();
        let loaded = self.loaded_packages();
        let loaded_refs: Vec<(&str, &efemod::Version)> = loaded
            .iter()
            .filter(|(id, _)| !leaving.contains(id) && !all[..n_changed].iter().any(|c| c.manifest.id == *id))
            .map(|(i, v)| (i.as_str(), v))
            .collect();
        let cands: Vec<efemod::Candidate<'_>> = all
            .iter()
            .map(|c| efemod::Candidate { id: &c.manifest.id, version: &c.manifest.version, dependencies: &c.manifest.dependencies })
            .collect();
        let resolution = efemod::resolve(&cands, &loaded_refs);
        drop(cands);
        drop(loaded_refs);
        let mut slots: Vec<Option<Cand>> = all.into_iter().map(Some).collect();
        for (i, problem) in resolution.problems {
            if let Some(c) = slots[i].take() {
                report.failed.push((c.label, ModError::Resolve(problem)));
            }
        }

        for (idx, label) in removed {
            if !self.is_loaded(ModuleId(idx as u32)) {
                continue;
            }
            match self.unload_closure(idx, res, types) {
                Ok(ids) => {
                    for id in &ids {
                        self.log(LogLevel::Info, format!("[{id}] unloaded: its source `{label}` no longer lists it"));
                    }
                    report.unloaded.extend(ids);
                }
                Err(e) => report.failed.push((label, e)),
            }
        }

        let target_ids: BTreeSet<String> = slots[..n_changed].iter().flatten().map(|c| c.manifest.id.clone()).collect();
        let mut failed_ids: Vec<String> = Vec::new();
        let mut deferred: Vec<Cand> = Vec::new();
        let mut deferred_ids: BTreeSet<String> = BTreeSet::new();
        let mut swap_order: Vec<usize> = Vec::new();
        for &i in &resolution.order {
            if i < n_changed {
                swap_order.push(i);
                continue;
            }
            let Some(c) = slots[i].take() else { continue };
            if c.manifest.dependencies.iter().any(|d| target_ids.contains(&d.id) || deferred_ids.contains(&d.id)) {
                deferred_ids.insert(c.manifest.id.clone());
                deferred.push(c);
            } else {
                self.load_cand(c, &mut report, &mut failed_ids, res);
            }
        }

        // The changed mods, swapped together.
        let mut targets: Vec<Target<'_>> = Vec::new();
        let mut target_names: Vec<(String, String)> = Vec::new();
        for i in swap_order {
            let Some(c) = slots[i].take() else { continue };
            let Some(idx) = self.loaded_mod_idx(&c.manifest.id) else { continue };
            let Cand { source, label, manifest, kind, .. } = c;
            target_names.push((manifest.id.clone(), label.clone()));
            #[cfg_attr(not(feature = "efa"), allow(unused_variables))]
            let blabel = label.clone();
            let build: Build<'_> = Box::new(move |s: &mut Scripting| match kind {
                Kind::Pkg(b) => Ok((b, None)),
                #[cfg(feature = "efa")]
                Kind::Folder { files, assets, hash } => {
                    let b = s.build_folder(&blabel, &*files, &assets, Some(hash))?;
                    Ok((b, Some((files, assets))))
                }
            });
            targets.push(Target { idx, label, deps: manifest.dependencies.iter().map(|d| d.id.clone()).collect(), source: Some(source), build });
        }
        if !targets.is_empty() {
            match self.swap_mods(targets, res, types) {
                Ok(done) => {
                    for (name, id) in done {
                        let label = self.origins.iter().find(|o| o.id == id).map(|o| o.label.clone()).unwrap_or_else(|| name.clone());
                        self.log(LogLevel::Info, format!("[{name}] reloaded from {label}"));
                        report.reloaded.push((label, id));
                    }
                }
                Err((cause, restored)) => {
                    for (id, label) in target_names {
                        self.log(LogLevel::Error, format!("[{id}] reload failed: {cause}"));
                        failed_ids.push(id.clone());
                        report.failed.push((label, ModError::ReloadFailed { id, cause: cause.clone(), restored }));
                    }
                }
            }
        }
        for c in deferred {
            self.load_cand(c, &mut report, &mut failed_ids, res);
        }

        sources.append(&mut self.sources);
        self.sources = sources;
        report
    }

    /// `Engine::reload_mod_input` after the input was read: replace the loaded mod with
    /// manifest id `manifest.id`, load it when none is loaded, or report it unchanged.
    pub(crate) fn reload_one<'a>(
        &mut self,
        manifest: &Manifest,
        hash: &str,
        label: &str,
        build: Build<'a>,
        res: &mut Resources,
        types: &mut TypeRegistry,
    ) -> Result<Reloaded, ModError> {
        let Some(idx) = self.loaded_mod_idx(&manifest.id) else {
            let (b, _) = build(self)?;
            return Ok(Reloaded::Loaded(self.load_package_item(b, res)?));
        };
        if self.host.modules[idx].package.as_ref().is_some_and(|p| p.hash == hash) {
            return Ok(Reloaded::Unchanged(ModuleId(idx as u32)));
        }
        let target = Target { idx, label: label.to_string(), deps: manifest.dependencies.iter().map(|d| d.id.clone()).collect(), source: None, build };
        match self.swap_mods(vec![target], res, types) {
            Ok(done) => {
                let id = done.iter().find(|(n, _)| *n == manifest.id).map(|d| d.1).expect("the target is loaded again");
                let dependents = done.into_iter().map(|d| d.0).filter(|n| *n != manifest.id).collect();
                self.log(LogLevel::Info, format!("[{}] reloaded from {label}", manifest.id));
                Ok(Reloaded::Replaced { id, dependents })
            }
            Err((cause, restored)) => {
                self.log(LogLevel::Error, format!("[{}] reload failed: {cause}", manifest.id));
                Err(ModError::ReloadFailed { id: manifest.id.clone(), cause, restored })
            }
        }
    }
}

pub(crate) type BuildFn<'a> = Build<'a>;
