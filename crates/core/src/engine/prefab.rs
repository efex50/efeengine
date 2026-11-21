//! Prefabs (docs/SCENES.md, phase S5): a named bundle of entities in a file, placed by scenes.
//!
//! A prefab file (`<prefab dir>/<Name>.ron`) holds a tree of entities written with the same
//! component codec as scene files. The first entity is the root; every entity has a local `id`
//! and a child names its parent with a `Parent` component holding the parent's local id:
//!
//! ```text
//! Prefab(
//!     format: 1,
//!     name: "Crate",
//!     entities: [
//!         (id: Some(1), name: Some("Crate"), components: { "Transform": {..}, "Sprite": {..} }),
//!         (id: Some(2), name: Some("Lid"), components: { "Parent": { "entity": 1 }, "Transform": {..} }),
//!     ],
//! )
//! ```
//!
//! A scene places it with an entry `(prefab: "Crate", overrides: { "Transform": { "position": [3.0, 4.0] } })`.
//! Overrides name `component -> field -> value` of the root, `#<local id>:<component>` of another
//! entity of the prefab. They are checked against the registry when the scene is validated or
//! loaded (errors name the scene, the entry and the field). The prefab is read again on every
//! load, so a changed prefab changes every placement, and the overrides stay.
//!
//! A placed entity carries a [`PrefabInstance`] (the prefab as it was read, and the entity's local
//! id in it). Saving a scene ([`Engine::capture_world_scene`]) writes a complete instance back as
//! one `prefab:` entry holding only the fields that differ from that prefab. An instance that is no longer
//! complete (a member deleted, a component removed, an entity added under a member, a child renamed) is
//! written as plain entities: the link is lost, nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hecs::Entity;
use serde::{Deserialize, Serialize};

use super::Engine;
use super::components::Parent;
use super::log::global_warn;
use super::scene::{SCENE_FORMAT, Scene, SceneEntity, SceneError, SceneValue, decode_component, encode_component, int_of, renumber_entities};
use super::types::entity_to_bits;

/// The prefab file format this engine reads and writes.
pub const PREFAB_FORMAT: u32 = 1;

/// The default directory of prefab files (relative to the current directory).
pub const PREFABS_DIR: &str = "prefabs";

/// A prefab in memory. `entities[0]` is the root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefab {
    /// File format version (required); this engine reads [`PREFAB_FORMAT`].
    pub format: u32,
    /// The name scenes use (`prefab: "Crate"`): the file's stem.
    pub name: String,
    pub entities: Vec<SceneEntity>,
}

impl Prefab {
    /// Parse RON text (nothing is checked against the registry here, see [`Engine::register_prefab`]).
    pub fn from_ron(text: &str) -> Result<Prefab, SceneError> {
        ron::from_str(text).map_err(|e| SceneError::Parse { scene: "<prefab>".into(), msg: e.to_string() })
    }

    /// The RON text of the prefab (stable: the same prefab gives the same text).
    pub fn to_ron(&self) -> String {
        let cfg = ron::ser::PrettyConfig::new().struct_names(true);
        ron::ser::to_string_pretty(self, cfg).expect("a prefab always serialises") + "\n"
    }

    /// The local id of the root entity.
    pub fn root_id(&self) -> Option<u64> {
        self.entities.first().and_then(|e| e.id)
    }

    /// The entity with local id `local`.
    pub fn entity(&self, local: u64) -> Option<&SceneEntity> {
        self.entities.iter().find(|e| e.id == Some(local))
    }
}

/// A prefab name that can be a file name: ASCII letters, digits, `_` and `-`.
pub fn valid_prefab_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Marks an entity placed from a prefab: the prefab as it was read when the entity was placed (the
/// reference its overrides are measured against) and the entity's local id in it. Engine
/// bookkeeping: not in the type registry, never written to scene files as such (a scene writes a
/// `prefab:` entry instead).
#[derive(Clone, Debug)]
pub struct PrefabInstance {
    base: Arc<Prefab>,
    local: u64,
}

impl PartialEq for PrefabInstance {
    fn eq(&self, o: &Self) -> bool {
        self.local == o.local && (Arc::ptr_eq(&self.base, &o.base) || self.base == o.base)
    }
}

impl PrefabInstance {
    /// The prefab's name.
    pub fn prefab(&self) -> &str {
        &self.base.name
    }
    /// The entity's local id in the prefab.
    pub fn local(&self) -> u64 {
        self.local
    }
    /// The prefab as it was when this entity was placed or last refreshed.
    pub fn base(&self) -> &Prefab {
        &self.base
    }
    /// The entity is the prefab's root.
    pub fn is_root(&self) -> bool {
        self.base.root_id() == Some(self.local)
    }
}

/// What [`Engine::apply_prefab_to_world`] changed, so a tool can undo it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrefabApplyReport {
    /// Entities whose [`PrefabInstance`] was replaced, with the one they had (`None`: none).
    pub tags: Vec<(Entity, Option<PrefabInstance>)>,
    /// Fields of other instances that followed the prefab: entity, component, field, the old value.
    pub values: Vec<(Entity, String, String, SceneValue)>,
    /// Instances of the prefab whose structure (entities or parents) differs from the new
    /// version: left as they are until the scene is loaded again.
    pub stale: usize,
}

/// A scene with its prefab entries replaced by the entities of the prefabs.
pub(super) struct Expanded {
    pub scene: Scene,
    /// Per entity of `scene`: the index of the entry of the original scene it came from.
    pub origin: Vec<usize>,
    pub tags: Vec<Option<PrefabInstance>>,
}

fn prefab_err(scene: &str, entity: usize, prefab: &str, msg: impl Into<String>) -> SceneError {
    SceneError::Prefab { scene: scene.to_string(), entity, prefab: prefab.to_string(), msg: msg.into() }
}

/// `#<local>:<component>` or `<component>` (the root).
fn parse_key(key: &str) -> Result<(Option<u64>, &str), String> {
    match key.strip_prefix('#') {
        Some(rest) => {
            let (n, comp) = rest.split_once(':').ok_or_else(|| format!("override key `{key}`: expected `#<local id>:<component>`"))?;
            let local = n.parse::<u64>().map_err(|_| format!("override key `{key}`: `{n}` is not a local id"))?;
            Ok((Some(local), comp))
        }
        None => Ok((None, key)),
    }
}

fn override_key(local: u64, root: u64, component: &str) -> String {
    if local == root { component.to_string() } else { format!("#{local}:{component}") }
}

impl Engine {
    /// Set the directory prefab files are read from and written to. Relative paths are relative to
    /// the current directory.
    pub fn set_prefab_dir(&mut self, dir: impl Into<PathBuf>) {
        self.scene_state.prefab_dir = Some(dir.into());
    }

    /// The directory set with [`set_prefab_dir`](Self::set_prefab_dir).
    pub fn prefab_dir(&self) -> Option<&Path> {
        self.scene_state.prefab_dir.as_deref()
    }

    fn prefab_path(&self, name: &str) -> Option<PathBuf> {
        self.scene_state.prefab_dir.as_ref().map(|d| d.join(format!("{name}.ron")))
    }

    /// Check a prefab against the registry and normalise its values (every field of every
    /// component present, numbers as the codec prints them), so two prefabs can be compared
    /// field by field. Errors say why the prefab is not usable.
    fn canonical_prefab(&self, prefab: &Prefab) -> Result<Prefab, String> {
        if prefab.format != PREFAB_FORMAT {
            return Err(format!("format {} is not supported (this engine reads format {PREFAB_FORMAT})", prefab.format));
        }
        if !valid_prefab_name(&prefab.name) {
            return Err(format!("`{}` is not a valid prefab name (letters, digits, `_` and `-`)", prefab.name));
        }
        if prefab.entities.is_empty() {
            return Err("it has no entities".into());
        }
        let mut ids = BTreeSet::new();
        for (i, e) in prefab.entities.iter().enumerate() {
            let Some(id) = e.id else { return Err(format!("entity {i} has no `id`")) };
            if !ids.insert(id) {
                return Err(format!("entity {i}: id {id} is used by an earlier entity"));
            }
            if e.prefab.is_some() || !e.overrides.is_empty() {
                return Err(format!("entity {i}: a prefab cannot contain another prefab (not supported)"));
            }
        }
        let plain = Scene { format: SCENE_FORMAT, name: format!("prefab {}", prefab.name), requires: Vec::new(), systems: Vec::new(), entities: prefab.entities.clone() };
        self.validate_scene(&plain).map_err(|e| e.to_string())?;
        let mut out = prefab.clone();
        for (i, e) in out.entities.iter_mut().enumerate() {
            e.components.remove("EditorId");
            for (cname, fields) in &mut e.components {
                let entry = self.types().by_name(cname).expect("validated");
                let bytes = decode_component(entry, fields).map_err(|(f, m)| format!("entity {i}: `{cname}`.{f}: {m}"))?;
                *fields = encode_component(entry, &bytes);
            }
            if let Some(p) = e.components.get("Parent") {
                let target = p.get("entity").and_then(int_of).unwrap_or(0);
                if i == 0 {
                    return Err("the root entity cannot have a `Parent`".into());
                }
                if !ids.contains(&(target as u64)) {
                    return Err(format!("entity {i}: `Parent` names local id {target}, which is not in the prefab"));
                }
            }
        }
        Ok(out)
    }

    /// Add `prefab` to the in-memory prefabs (they win over files of the same name; for tests,
    /// tools and builds without a file system). The prefab is checked against the registry.
    pub fn register_prefab(&mut self, prefab: Prefab) -> Result<(), SceneError> {
        let canon = self.canonical_prefab(&prefab).map_err(|m| prefab_err("<prefab>", 0, &prefab.name, m))?;
        self.scene_state.prefabs.insert(canon.name.clone(), Arc::new(canon));
        Ok(())
    }

    /// The prefab `name`: registered in memory, else read from `<prefab dir>/<name>.ron`; checked.
    /// (Read from disk on every call: edits show up on the next load.)
    pub fn find_prefab(&self, name: &str) -> Result<Arc<Prefab>, String> {
        if let Some(p) = self.scene_state.prefabs.get(name) {
            return Ok(p.clone());
        }
        if !valid_prefab_name(name) {
            return Err(format!("`{name}` is not a valid prefab name"));
        }
        let Some(path) = self.prefab_path(name) else {
            return Err("no such prefab registered and no prefab directory set (Engine::set_prefab_dir)".into());
        };
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut prefab = Prefab::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        prefab.name = name.to_string();
        self.canonical_prefab(&prefab).map(Arc::new).map_err(|m| format!("{}: {m}", path.display()))
    }

    /// [`find_prefab`](Self::find_prefab) as an owned value.
    pub fn load_prefab(&self, name: &str) -> Result<Prefab, SceneError> {
        self.find_prefab(name).map(|p| (*p).clone()).map_err(|m| prefab_err("<prefab>", 0, name, m))
    }

    /// The names of the prefabs a scene can place: in-memory ones and the `*.ron` files of the
    /// prefab directory, sorted, each once.
    pub fn prefab_names(&self) -> Vec<String> {
        let mut out: BTreeSet<String> = self.scene_state.prefabs.keys().cloned().collect();
        if let Some(dir) = &self.scene_state.prefab_dir {
            for d in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = d.path();
                if p.extension().is_some_and(|x| x == "ron") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()).filter(|s| valid_prefab_name(s)) {
                        out.insert(stem.to_string());
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// Write `prefab` to `<prefab dir>/<name>.ron` (creating the directory) and, when a prefab of
    /// that name is registered in memory, replace it. Checked first; nothing is written on error.
    /// Returns the normalised prefab.
    pub fn save_prefab(&mut self, prefab: &Prefab) -> Result<Arc<Prefab>, SceneError> {
        let canon = self.canonical_prefab(prefab).map_err(|m| prefab_err("<prefab>", 0, &prefab.name, m))?;
        let canon = Arc::new(canon);
        if let Some(path) = self.prefab_path(&canon.name) {
            let io = |e: std::io::Error| SceneError::Io { path: path.display().to_string(), msg: e.to_string() };
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            std::fs::write(&path, canon.to_ron()).map_err(io)?;
        } else if !self.scene_state.prefabs.contains_key(&canon.name) {
            return Err(prefab_err("<prefab>", 0, &canon.name, "no prefab directory set (Engine::set_prefab_dir)"));
        }
        if self.scene_state.prefabs.contains_key(&canon.name) {
            self.scene_state.prefabs.insert(canon.name.clone(), canon.clone());
        }
        Ok(canon)
    }

    /// Delete the prefab file `name` and forget the in-memory one. Missing files are not an error.
    pub fn remove_prefab(&mut self, name: &str) -> Result<(), SceneError> {
        self.scene_state.prefabs.remove(name);
        if valid_prefab_name(name) {
            if let Some(path) = self.prefab_path(name) {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(SceneError::Io { path: path.display().to_string(), msg: e.to_string() }),
                }
            }
        }
        Ok(())
    }

    /// The link a snapshot or a restart kept for `name`, `local`, if the prefab is still there.
    pub(super) fn prefab_tag(&self, name: &str, local: u64) -> Option<PrefabInstance> {
        self.find_prefab(name).ok().filter(|b| b.entity(local).is_some()).map(|base| PrefabInstance { base, local })
    }

    /// Replace the prefab entries of `scene` by the prefabs' entities (`None` when it has none).
    /// Overrides are merged into the copies; a prefab that cannot be read or an override that
    /// names an unknown component is an error naming the scene and the entry.
    pub(super) fn expand_prefabs(&self, scene: &Scene) -> Result<Option<Expanded>, SceneError> {
        if !scene.entities.iter().any(|e| e.prefab.is_some() || !e.overrides.is_empty()) {
            return Ok(None);
        }
        let sname = &scene.name;
        let mut next_id = scene.entities.iter().filter_map(|e| e.id).max().unwrap_or(0);
        let mut cache: BTreeMap<String, Arc<Prefab>> = BTreeMap::new();
        let mut out: Vec<SceneEntity> = Vec::new();
        let mut origin = Vec::new();
        let mut tags = Vec::new();
        for (i, ent) in scene.entities.iter().enumerate() {
            let Some(pname) = &ent.prefab else {
                if !ent.overrides.is_empty() {
                    return Err(SceneError::Insert { scene: sname.clone(), entity: i, component: "<overrides>".into(), msg: "`overrides` needs a `prefab`".into() });
                }
                out.push(ent.clone());
                origin.push(i);
                tags.push(None);
                continue;
            };
            if !ent.components.is_empty() {
                return Err(prefab_err(sname, i, pname, "a prefab entry cannot list `components`; use `overrides`"));
            }
            let base = match cache.get(pname) {
                Some(b) => b.clone(),
                None => {
                    let b = self.find_prefab(pname).map_err(|m| prefab_err(sname, i, pname, m))?;
                    cache.insert(pname.clone(), b.clone());
                    b
                }
            };
            let root_local = base.root_id().unwrap_or(0);
            let mut map: BTreeMap<u64, u64> = BTreeMap::new();
            for pe in &base.entities {
                let local = pe.id.unwrap_or(0);
                let new = if local == root_local && ent.id.is_some() {
                    ent.id.unwrap_or(0)
                } else {
                    next_id += 1;
                    next_id
                };
                map.insert(local, new);
            }
            let first = out.len();
            for pe in &base.entities {
                let mut c = pe.clone();
                c.id = c.id.and_then(|l| map.get(&l).copied());
                for (cname, fields) in &mut c.components {
                    let Some(entry) = self.types().by_name(cname) else { continue };
                    for f in entry.desc().fields.iter().filter(|f| f.ty == efevm::Prim::Entity) {
                        if let Some(v) = fields.get_mut(&f.name) {
                            renumber_entities(v, &map);
                        }
                    }
                }
                out.push(c);
                origin.push(i);
                tags.push(Some(PrefabInstance { base: base.clone(), local: pe.id.unwrap_or(0) }));
            }
            if let Some(n) = &ent.name {
                out[first].name = Some(n.clone());
            }
            for (key, fields) in &ent.overrides {
                let (local, cname) = parse_key(key).map_err(|m| prefab_err(sname, i, pname, m))?;
                let local = local.unwrap_or(root_local);
                let Some(at) = base.entities.iter().position(|e| e.id == Some(local)) else {
                    global_warn(&format!("scene `{sname}`: entity {i}: prefab `{pname}` has no entity with local id {local} any more; override `{key}` ignored"));
                    continue;
                };
                let entry = self
                    .types()
                    .by_name(cname)
                    .ok_or_else(|| SceneError::UnknownComponent { scene: sname.clone(), entity: i, component: cname.to_string() })?;
                if !entry.is_component() {
                    return Err(SceneError::NotComponent { scene: sname.clone(), entity: i, component: cname.to_string() });
                }
                let target = out[first + at].components.entry(cname.to_string()).or_default();
                for (f, v) in fields {
                    target.insert(f.clone(), v.clone());
                }
            }
        }
        Ok(Some(Expanded { scene: Scene { format: scene.format, name: scene.name.clone(), requires: scene.requires.clone(), systems: scene.systems.clone(), entities: out }, origin, tags }))
    }

    /// Spawn one copy of prefab `name` (all its entities, root first), owned by the active scene
    /// when there is one, each with an `EditorId`. The entities carry a [`PrefabInstance`].
    pub fn place_prefab(&mut self, name: &str) -> Result<Vec<Entity>, SceneError> {
        let scene = Scene {
            format: SCENE_FORMAT,
            name: "<place>".into(),
            requires: Vec::new(),
            systems: Vec::new(),
            entities: vec![SceneEntity { prefab: Some(name.to_string()), ..Default::default() }],
        };
        let x = self.expand_prefabs(&scene)?.expect("one prefab entry");
        let (decoded, _) = self.decode_scene_full(&x.scene).map_err(|e| e.remap_entity(&x.origin))?;
        let owner = self.active_owner();
        let owners = vec![owner; decoded.len()];
        let spawned = self.spawn_decoded(&x.scene, &decoded, &owners, &x.tags)?;
        for e in &spawned {
            self.ensure_editor_id(*e);
        }
        Ok(spawned)
    }

    /// A prefab made of `root` and its descendants, with the entities in tree order (the same order
    /// as the returned prefab's entities). `EditorId` is left out and `root` loses its `Parent`.
    /// Entities keep the local id they have in a prefab of the same `name` (so scenes that refer
    /// to them by `#<id>:` keep working); the others get new ones. References to entities outside the tree
    /// are written as no entity.
    pub fn capture_prefab(&self, root: Entity, name: &str) -> Result<(Prefab, Vec<Entity>), SceneError> {
        let tree = self.entity_tree(root);
        if tree.is_empty() {
            return Err(prefab_err("<prefab>", 0, name, "no such entity"));
        }
        let mut ids: Vec<Option<u64>> = tree
            .iter()
            .map(|e| self.world().get::<&PrefabInstance>(*e).ok().filter(|t| t.prefab() == name).map(|t| t.local()))
            .collect();
        // Duplicates (an entity copied from an instance) are numbered again.
        let mut seen = BTreeSet::new();
        for id in &mut ids {
            if id.is_some_and(|v| !seen.insert(v)) {
                *id = None;
            }
        }
        let mut next = seen.iter().max().copied().unwrap_or(0);
        let ids: Vec<u64> = ids
            .into_iter()
            .map(|i| {
                i.unwrap_or_else(|| {
                    next += 1;
                    next
                })
            })
            .collect();
        let map: BTreeMap<u64, u64> = tree.iter().zip(&ids).map(|(e, l)| (entity_to_bits(*e), *l)).collect();
        let mut entities = Vec::with_capacity(tree.len());
        for (i, e) in tree.iter().enumerate() {
            let mut ent = self.capture_entity(*e);
            ent.id = Some(ids[i]);
            ent.components.remove("EditorId");
            if i == 0 {
                ent.components.remove("Parent");
            }
            for (cname, fields) in &mut ent.components {
                for f in self.entity_field_names(cname) {
                    if let Some(v) = fields.get_mut(&f) {
                        if let SceneValue::UInt(u) = v {
                            *u = map.get(u).copied().unwrap_or(0);
                        } else {
                            renumber_entities(v, &map);
                        }
                    }
                }
            }
            entities.push(ent);
        }
        let prefab = Prefab { format: PREFAB_FORMAT, name: name.to_string(), entities };
        let canon = self.canonical_prefab(&prefab).map_err(|m| prefab_err("<prefab>", 0, name, m))?;
        Ok((canon, tree))
    }

    /// Link `entities` (in the order of `prefab.entities`) to `prefab` as instances of it.
    pub fn link_prefab(&mut self, entities: &[Entity], prefab: &Prefab) -> Result<(), SceneError> {
        let canon = Arc::new(self.canonical_prefab(prefab).map_err(|m| prefab_err("<prefab>", 0, &prefab.name, m))?);
        if entities.len() != canon.entities.len() {
            return Err(prefab_err("<prefab>", 0, &prefab.name, format!("{} entities for a prefab of {}", entities.len(), canon.entities.len())));
        }
        for (e, pe) in entities.iter().zip(&canon.entities) {
            let _ = self.world_mut().insert_one(*e, PrefabInstance { base: canon.clone(), local: pe.id.unwrap_or(0) });
        }
        Ok(())
    }

    /// The prefab link of `e`, if it has one.
    pub fn prefab_instance(&self, e: Entity) -> Option<PrefabInstance> {
        self.world().get::<&PrefabInstance>(e).ok().map(|t| (*t).clone())
    }

    /// Set (`Some`) or remove (`None`) the prefab link of `e`.
    pub fn set_prefab_instance(&mut self, e: Entity, tag: Option<PrefabInstance>) {
        match tag {
            Some(t) => {
                let _ = self.world_mut().insert_one(e, t);
            }
            None => {
                let _ = self.world_mut().remove_one::<PrefabInstance>(e);
            }
        }
    }

    /// The root of the instance `e` belongs to: `e` itself for a root, else the nearest ancestor
    /// that is the root of the same prefab. `None` for an entity with no link or whose chain is
    /// broken (reparented out of its instance).
    pub fn prefab_instance_root(&self, e: Entity) -> Option<Entity> {
        let name = self.prefab_instance(e)?.prefab().to_string();
        let mut cur = e;
        for _ in 0..256 {
            let t = self.prefab_instance(cur)?;
            if t.prefab() != name {
                return None;
            }
            if t.is_root() {
                return Some(cur);
            }
            cur = self.world().get::<&Parent>(cur).ok().and_then(|p| p.entity())?;
        }
        None
    }

    /// The members of the instance rooted at `root`: the entities of its tree whose
    /// [`prefab_instance_root`](Self::prefab_instance_root) is `root`, root first.
    pub fn prefab_instance_entities(&self, root: Entity) -> Vec<Entity> {
        self.entity_tree(root).into_iter().filter(|e| self.prefab_instance_root(*e) == Some(root)).collect()
    }

    /// Break the link of the whole instance `e` belongs to; returns what was removed (entity and
    /// link) so a tool can put it back. Nothing happens for an entity without a link.
    pub fn unpack_prefab(&mut self, e: Entity) -> Vec<(Entity, PrefabInstance)> {
        let Some(root) = self.prefab_instance_root(e) else {
            return self.prefab_instance(e).map(|t| {
                self.set_prefab_instance(e, None);
                vec![(e, t)]
            }).unwrap_or_default();
        };
        let mut out = Vec::new();
        for m in self.prefab_instance_entities(root) {
            if let Some(t) = self.prefab_instance(m) {
                self.set_prefab_instance(m, None);
                out.push((m, t));
            }
        }
        out
    }

    /// `(component, field)` of `e` whose value differs from its prefab (the overrides a save
    /// would write), sorted. Entity references are not listed (they are not comparable between
    /// a prefab and a world); a component the prefab lacks is not listed either.
    pub fn prefab_overrides(&self, e: Entity) -> Vec<(String, String)> {
        let Some(tag) = self.prefab_instance(e) else { return Vec::new() };
        let Some(base) = tag.base.entity(tag.local) else { return Vec::new() };
        let cur = self.capture_entity(e);
        let mut out = Vec::new();
        for (comp, fields) in &cur.components {
            let Some(bc) = base.components.get(comp) else { continue };
            let refs = self.entity_field_names(comp);
            for (f, v) in fields {
                if !refs.contains(f) && bc.get(f).is_some_and(|b| b != v) {
                    out.push((comp.clone(), f.clone()));
                }
            }
        }
        out
    }

    /// The prefab's value of `component.field` for `e` (what a revert writes).
    pub fn prefab_base_field(&self, e: Entity, component: &str, field: &str) -> Option<SceneValue> {
        let tag = self.prefab_instance(e)?;
        if self.entity_field_names(component).iter().any(|f| f == field) {
            return None;
        }
        tag.base.entity(tag.local)?.components.get(component)?.get(field).cloned()
    }

    /// Make `prefab` (already saved) the prefab of `source` (entities in the order of its entities)
    /// and let the other instances of it in the world follow: a field that still has the old
    /// prefab's value takes the new one, overrides stay, and the link is replaced. Instances
    /// whose entities or parents differ between the two versions are left alone (counted in
    /// `stale`; they change when the scene is loaded again). Components the new prefab added are not
    /// added to live instances. Returns what changed, for undo.
    pub fn apply_prefab_to_world(&mut self, prefab: &Prefab, source: &[Entity]) -> Result<PrefabApplyReport, SceneError> {
        let new_base = Arc::new(self.canonical_prefab(prefab).map_err(|m| prefab_err("<prefab>", 0, &prefab.name, m))?);
        if source.len() != new_base.entities.len() {
            return Err(prefab_err("<prefab>", 0, &prefab.name, format!("{} entities for a prefab of {}", source.len(), new_base.entities.len())));
        }
        let mut rep = PrefabApplyReport::default();
        let src: BTreeSet<u64> = source.iter().map(|e| entity_to_bits(*e)).collect();
        let others: Vec<(Entity, PrefabInstance)> = self
            .world()
            .query::<(Entity, &PrefabInstance)>()
            .iter()
            .filter(|(e, t)| t.prefab() == new_base.name && !src.contains(&entity_to_bits(*e)))
            .map(|(e, t)| (e, t.clone()))
            .collect();
        for (e, pe) in source.iter().zip(&new_base.entities) {
            rep.tags.push((*e, self.prefab_instance(*e)));
            let _ = self.world_mut().insert_one(*e, PrefabInstance { base: new_base.clone(), local: pe.id.unwrap_or(0) });
        }
        for (e, t) in others {
            if !same_structure(&t.base, &new_base) {
                if t.is_root() {
                    rep.stale += 1;
                }
                continue;
            }
            let Some(new_ent) = new_base.entity(t.local) else { continue };
            let old_ent = t.base.entity(t.local);
            let cur = self.capture_entity(e);
            for (comp, nfields) in &new_ent.components {
                let Some(cf) = cur.components.get(comp) else { continue };
                let refs = self.entity_field_names(comp);
                for (f, nv) in nfields {
                    if refs.contains(f) {
                        continue;
                    }
                    let cv = cf.get(f);
                    let ov = old_ent.and_then(|o| o.components.get(comp)).and_then(|c| c.get(f));
                    if let (Some(cv), Some(ov)) = (cv, ov) {
                        if cv == ov && cv != nv && self.set_component_field(e, comp, f, nv.clone()).is_ok() {
                            rep.values.push((e, comp.clone(), f.clone(), cv.clone()));
                        }
                    }
                }
            }
            rep.tags.push((e, Some(t.clone())));
            let _ = self.world_mut().insert_one(e, PrefabInstance { base: new_base.clone(), local: t.local });
        }
        Ok(rep)
    }

    /// Turn complete prefab instances among `items` (an entity and its capture) into one `prefab:`
    /// entry each at the place of the instance's root (see the module docs); other items, and members of
    /// incomplete instances, pass through. The root's `id` is kept.
    pub(super) fn collapse_prefab_instances(&self, items: Vec<(Entity, SceneEntity)>) -> Vec<(Entity, SceneEntity)> {
        let tags: Vec<Option<PrefabInstance>> = items.iter().map(|(e, _)| self.prefab_instance(*e)).collect();
        if tags.iter().all(Option::is_none) {
            return items;
        }
        let index: BTreeMap<u64, usize> = items.iter().enumerate().map(|(i, (e, _))| (entity_to_bits(*e), i)).collect();
        let parent_idx = |i: usize| -> Option<usize> {
            self.world().get::<&Parent>(items[i].0).ok().and_then(|p| p.entity()).and_then(|p| index.get(&entity_to_bits(p)).copied())
        };
        let root_of = |i: usize| -> Option<usize> {
            let name = tags[i].as_ref()?.prefab().to_string();
            let mut cur = i;
            for _ in 0..256 {
                let t = tags[cur].as_ref()?;
                if t.prefab() != name {
                    return None;
                }
                if t.is_root() {
                    return Some(cur);
                }
                cur = parent_idx(cur)?;
            }
            None
        };
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..items.len() {
            if let Some(r) = root_of(i) {
                groups.entry(r).or_default().push(i);
            }
        }
        let mut placements: BTreeMap<usize, SceneEntity> = BTreeMap::new();
        let mut consumed: BTreeSet<usize> = BTreeSet::new();
        for (r, members) in &groups {
            if let Some(p) = self.placement_of(&items, &tags, &parent_idx, *r, members) {
                placements.insert(*r, p);
                consumed.extend(members.iter().filter(|m| *m != r));
            }
        }
        items
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !consumed.contains(i))
            .map(|(i, (e, ent))| match placements.remove(&i) {
                Some(p) => (e, p),
                None => (e, ent),
            })
            .collect()
    }

    /// The `prefab:` entry for the instance with root `r` and `members` (indices into `items`), or
    /// `None` when the instance is not complete or not expressible.
    fn placement_of(
        &self,
        items: &[(Entity, SceneEntity)],
        tags: &[Option<PrefabInstance>],
        parent_idx: &dyn Fn(usize) -> Option<usize>,
        r: usize,
        members: &[usize],
    ) -> Option<SceneEntity> {
        let root_tag = tags[r].as_ref()?;
        let base = root_tag.base.clone();
        let root_local = root_tag.local;
        // Same set of entities as the prefab.
        let mut locals: Vec<u64> = members.iter().filter_map(|m| tags[*m].as_ref().map(|t| t.local)).collect();
        locals.sort_unstable();
        let mut want: Vec<u64> = base.entities.iter().filter_map(|e| e.id).collect();
        want.sort_unstable();
        if locals != want {
            return None;
        }
        let bits_to_local: BTreeMap<u64, u64> = members.iter().filter_map(|m| tags[*m].as_ref().map(|t| (entity_to_bits(items[*m].0), t.local))).collect();
        let member_set: BTreeSet<usize> = members.iter().copied().collect();
        // Nothing outside the instance refers to one of its non-root members.
        let inner: BTreeSet<u64> = members.iter().filter(|m| **m != r).map(|m| entity_to_bits(items[*m].0)).collect();
        for (i, (_, ent)) in items.iter().enumerate() {
            if member_set.contains(&i) {
                continue;
            }
            for (cname, fields) in &ent.components {
                for f in self.entity_field_names(cname) {
                    if fields.get(&f).and_then(int_of).is_some_and(|b| inner.contains(&(b as u64))) {
                        return None;
                    }
                }
            }
        }
        let mut overrides: BTreeMap<String, BTreeMap<String, SceneValue>> = BTreeMap::new();
        let mut name = None;
        for &m in members {
            let tag = tags[m].as_ref()?;
            let be = base.entity(tag.local)?;
            let ent = &items[m].1;
            if m != r {
                // The parent link must be the prefab's.
                let want_parent = be.components.get("Parent").and_then(|p| p.get("entity")).and_then(int_of)? as u64;
                let got = parent_idx(m).and_then(|p| tags[p].as_ref()).map(|t| t.local)?;
                if want_parent != got || ent.name != be.name {
                    return None;
                }
            } else if ent.name != be.name && ent.name.is_some() {
                name = ent.name.clone();
            }
            for cname in be.components.keys() {
                if !ent.components.contains_key(cname) {
                    return None;
                }
            }
            for (cname, fields) in &ent.components {
                // The root keeps its editor id (an override: prefabs have none), so a scene
                // lists the same entities in the same order after a reload; the members' ids are
                // handed out again when the editor needs them.
                if cname == "EditorId" && m != r {
                    continue;
                }
                let refs = self.entity_field_names(cname);
                let bc = be.components.get(cname);
                let mut diff: BTreeMap<String, SceneValue> = BTreeMap::new();
                for (f, v) in fields {
                    let mut v = v.clone();
                    if refs.contains(f) {
                        // Group members are compared (and saved) by local id.
                        let map: BTreeMap<u64, u64> = bits_to_local.clone();
                        renumber_entities(&mut v, &map);
                    }
                    if bc.and_then(|c| c.get(f)) != Some(&v) {
                        diff.insert(f.clone(), v);
                    }
                }
                if !diff.is_empty() {
                    overrides.insert(override_key(tag.local, root_local, cname), diff);
                }
            }
        }
        Some(SceneEntity { id: items[r].1.id, name, prefab: Some(base.name.clone()), overrides, components: BTreeMap::new() })
    }
}

/// The same entities (local ids) with the same parents.
fn same_structure(a: &Prefab, b: &Prefab) -> bool {
    let shape = |p: &Prefab| -> BTreeMap<u64, Option<u64>> {
        p.entities.iter().filter_map(|e| e.id.map(|id| (id, e.components.get("Parent").and_then(|c| c.get("entity")).and_then(int_of).map(|v| v as u64)))).collect()
    };
    shape(a) == shape(b)
}
