//! Mod-defined (runtime) components: declaration, registration, unload and migration.
//!
//! A mod declares components in the `[meta]` table of its manifest:
//!
//! ```toml
//! [[meta.component]]
//! name = "Health"
//! doc = "optional"
//! fields = [ { name = "hp", type = "f32" }, { name = "pos", type = "f32", count = 2 } ]
//! tracked = true   # optional, default true: change ticks on
//! ```
//!
//! The component is registered as `mod:<mod>/<Name>` (owner `mod:<mod>`, id =
//! `fnv1a32(name)`), in the [`TypeRegistry`], the API index and the world's dynamic columns
//! ([`World::register_dynamic`]), before the mod's `init` runs.
//!
//! Hot reload ("swap"): `unload_swap` keeps the world registration and the data of the old
//! component (the *stash*); the new version's registration adopts it (identical layout: data and
//! ticks stay) or queues a *migration* (changed layout); `finish_swap` runs the migrations once
//! the whole swap succeeded and drops what no new version claimed. A failed swap never touched
//! the data, so the restored previous version simply adopts it again.

use std::collections::BTreeMap;

use efevm::api::json::Value;
use efevm::{FieldDesc, Prim, StructDesc};
use hecs::{DynamicInfo, Entity, World};

use super::mods::same_layout;
use super::packages::ModError;
use super::{ModuleId, Scripting};
use crate::engine::types::TypeRegistry;

/// Most components one mod may declare, whatever the grant says.
pub const MAX_COMPONENTS_PER_MOD: usize = 256;
/// Most fields one component may have.
pub const MAX_FIELDS: usize = 64;
/// Largest size of one component, whatever the grant says.
pub const MAX_COMPONENT_BYTES: u32 = 64 * 1024;

/// Marks the module whose script spawned an entity. Rust-only and untracked (not in the
/// `TypeRegistry`, so scripts never see it); an unload despawns the entities of the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpawnedBy(pub ModuleId);

/// One component a mod declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModComponent {
    /// The name as declared (`Health`).
    pub name: String,
    /// Whether change ticks are recorded (`tracked`, default true).
    pub tracked: bool,
    /// The layout: name `mod:<mod>/<Name>`, owner `mod:<mod>`, `repr(C)` offsets.
    pub desc: StructDesc,
}

impl ModComponent {
    /// Size in bytes.
    pub fn size(&self) -> u32 {
        self.desc.size
    }
    /// Alignment in bytes.
    pub fn align(&self) -> u32 {
        self.desc.align
    }
    /// The id scripts and Rust use: `fnv1a32` of the full name.
    pub fn id(&self) -> u32 {
        TypeRegistry::id_of_name(&self.desc.name)
    }
}

/// `mod:<mod>/<component>`.
pub fn component_name(mod_id: &str, component: &str) -> String {
    format!("mod:{mod_id}/{component}")
}

fn bad<T>(id: &str, component: &str, reason: impl Into<String>) -> Result<T, ModError> {
    Err(ModError::Component { id: id.to_string(), component: component.to_string(), reason: reason.into() })
}

/// Parse and lay out the components of `meta.component`. Nothing is registered.
pub fn parse_components(mod_id: &str, meta: Option<&Value>) -> Result<Vec<ModComponent>, ModError> {
    let Some(list) = meta.and_then(|m| m.get("component")) else { return Ok(Vec::new()) };
    let Some(list) = list.as_array() else {
        return bad(mod_id, "", "`meta.component` must be an array of tables (`[[meta.component]]`)");
    };
    if list.len() > MAX_COMPONENTS_PER_MOD {
        return bad(mod_id, "", format!("{} components declared, at most {MAX_COMPONENTS_PER_MOD}", list.len()));
    }
    let mut out: Vec<ModComponent> = Vec::new();
    for (i, c) in list.iter().enumerate() {
        let Some(entries) = c.entries() else { return bad(mod_id, &format!("#{i}"), "a component must be a table") };
        let name = match c.get("name").and_then(Value::as_str) {
            Some(n) => n.to_string(),
            None => return bad(mod_id, &format!("#{i}"), "`name` is missing or not a string"),
        };
        if !efevm::api::is_ident(&name) {
            return bad(mod_id, &name, "the name is not an identifier");
        }
        if out.iter().any(|o| o.name == name) {
            return bad(mod_id, &name, "declared twice");
        }
        for (k, _) in entries {
            if !["name", "doc", "fields", "tracked"].contains(&k.as_str()) {
                return bad(mod_id, &name, format!("unknown key `{k}` (known: name, doc, fields, tracked)"));
            }
        }
        let doc = match c.get("doc") {
            None => String::new(),
            Some(v) => match v.as_str() {
                Some(s) => s.to_string(),
                None => return bad(mod_id, &name, "`doc` must be a string"),
            },
        };
        let tracked = match c.get("tracked") {
            None => true,
            Some(v) => match v.as_bool() {
                Some(b) => b,
                None => return bad(mod_id, &name, "`tracked` must be true or false"),
            },
        };
        let Some(fields) = c.get("fields").and_then(Value::as_array) else {
            return bad(mod_id, &name, "`fields` is missing or not an array");
        };
        if fields.is_empty() {
            return bad(mod_id, &name, "a component needs at least one field");
        }
        if fields.len() > MAX_FIELDS {
            return bad(mod_id, &name, format!("{} fields, at most {MAX_FIELDS}", fields.len()));
        }
        let full = component_name(mod_id, &name);
        let mut desc = StructDesc::new(full, 0, 1).doc(doc);
        desc.owner = format!("mod:{mod_id}");
        let (mut offset, mut align) = (0u64, 1u32);
        for (j, f) in fields.iter().enumerate() {
            let Some(fe) = f.entries() else { return bad(mod_id, &name, format!("field #{j} must be a table")) };
            let Some(fname) = f.get("name").and_then(Value::as_str) else {
                return bad(mod_id, &name, format!("field #{j}: `name` is missing or not a string"));
            };
            for (k, _) in fe {
                if !["name", "type", "count", "doc"].contains(&k.as_str()) {
                    return bad(mod_id, &name, format!("field `{fname}`: unknown key `{k}` (known: name, type, count, doc)"));
                }
            }
            if !efevm::api::is_ident(fname) {
                return bad(mod_id, &name, format!("field name `{fname}` is not an identifier"));
            }
            if desc.fields.iter().any(|g| g.name == fname) {
                return bad(mod_id, &name, format!("duplicate field `{fname}`"));
            }
            let Some(tname) = f.get("type").and_then(Value::as_str) else {
                return bad(mod_id, &name, format!("field `{fname}`: `type` is missing or not a string"));
            };
            let Some(ty) = Prim::parse(tname) else {
                let known: Vec<&str> = Prim::ALL.iter().map(|p| p.name()).collect();
                return bad(mod_id, &name, format!("field `{fname}`: unknown type `{tname}` (known: {})", known.join(", ")));
            };
            let count = match f.get("count") {
                None => 1,
                Some(v) => match v.as_u64() {
                    Some(n) if (1..=u32::MAX as u64).contains(&n) => n as u32,
                    _ => return bad(mod_id, &name, format!("field `{fname}`: `count` must be an integer of at least 1")),
                },
            };
            let fdoc = match f.get("doc") {
                None => String::new(),
                Some(v) => match v.as_str() {
                    Some(s) => s.to_string(),
                    None => return bad(mod_id, &name, format!("field `{fname}`: `doc` must be a string")),
                },
            };
            let tsize = ty.size() as u64;
            offset = offset.div_ceil(tsize) * tsize;
            let end = offset + tsize * count as u64;
            if end > MAX_COMPONENT_BYTES as u64 {
                return bad(mod_id, &name, format!("larger than {MAX_COMPONENT_BYTES} bytes"));
            }
            desc.fields.push(FieldDesc::new(fname, offset as u32, ty, count).with_doc(fdoc));
            offset = end;
            align = align.max(tsize as u32);
        }
        let size = (offset as u32).div_ceil(align) * align;
        desc.size = size;
        desc.align = align;
        if let Err(e) = desc.validate() {
            return bad(mod_id, &name, e);
        }
        out.push(ModComponent { name, tracked, desc });
    }
    Ok(out)
}

/// Same declarations (names, layouts, tracking); docs are not compared.
pub(crate) fn same_declarations(a: &[ModComponent], b: &[ModComponent]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.name == y.name && x.tracked == y.tracked && same_layout(&x.desc, &y.desc))
}

/// What the world held of a component before a swap.
pub(crate) struct Stashed {
    desc: StructDesc,
    tracked: bool,
}

/// A layout change to apply once a swap succeeded.
pub(crate) struct Migration {
    owner: String,
    id: u32,
    old: StructDesc,
    new: ModComponent,
}

/// Build the bytes of the new layout from the old: a field with the same name, type and count
/// copies its bytes, the rest is zero.
fn migrate_bytes(old: &StructDesc, new: &StructDesc, bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; new.size as usize];
    for f in &new.fields {
        let Some(o) = old.field_named(&f.name).filter(|o| o.ty == f.ty && o.count == f.count) else { continue };
        let n = f.size() as usize;
        let (from, to) = (o.offset as usize, f.offset as usize);
        if let (Some(src), Some(dst)) = (bytes.get(from..from + n), out.get_mut(to..to + n)) {
            dst.copy_from_slice(src);
        }
    }
    out
}

fn info_of(desc: &StructDesc) -> DynamicInfo {
    DynamicInfo { size: desc.size as usize, align: desc.align as usize }
}

impl Scripting {
    /// Register the components of mod `name`: world, [`TypeRegistry`] and API index. In a swap
    /// the stashed registration of an old version is adopted (see the module docs). On failure
    /// everything this call did is undone.
    pub(crate) fn register_components(
        &mut self,
        name: &str,
        comps: &[ModComponent],
        world: &mut World,
        types: &mut TypeRegistry,
    ) -> Result<(), ModError> {
        let mut fresh: Vec<u32> = Vec::new();
        let r = self.register_components_inner(name, comps, world, types, &mut fresh);
        if let Err(e) = r {
            self.migrations.retain(|m| m.owner != name);
            types.remove_mod_components(name);
            let _ = self.registry.unregister_owner(&format!("mod:{name}"));
            for id in fresh {
                let _ = world.unregister_dynamic(id);
            }
            return Err(e);
        }
        Ok(())
    }

    fn register_components_inner(
        &mut self,
        name: &str,
        comps: &[ModComponent],
        world: &mut World,
        types: &mut TypeRegistry,
        fresh: &mut Vec<u32>,
    ) -> Result<(), ModError> {
        let fail = |c: &ModComponent, reason: String| ModError::Component { id: name.to_string(), component: c.name.clone(), reason };
        for c in comps {
            let id = c.id();
            match (world.dynamic_info(id), self.stash.get(&id)) {
                (None, _) => {
                    world.register_dynamic(id, info_of(&c.desc), c.tracked).map_err(|e| fail(c, e.to_string()))?;
                    fresh.push(id);
                }
                (Some(_), Some(old)) => {
                    if !(same_layout(&old.desc, &c.desc) && old.tracked == c.tracked) {
                        self.migrations.push(Migration { owner: name.to_string(), id, old: old.desc.clone(), new: c.clone() });
                    }
                }
                (Some(_), None) => {
                    return Err(fail(c, format!("the component id {id:#010x} is already registered in the world")));
                }
            }
            types.register_dynamic(c.desc.clone()).map_err(|e| fail(c, e.to_string()))?;
            let id_in_types = TypeRegistry::id_of_name(&c.desc.name);
            let mut desc = c.desc.clone();
            desc.component = Some(id_in_types);
            self.register_type(desc).map_err(|e| fail(c, e.to_string()))?;
        }
        Ok(())
    }

    /// Everything a module leaves behind when it unloads: the entities its scripts spawned
    /// (collected in query order, then despawned), then its components. With `keep` (a swap) the
    /// components' world registration and data stay, stashed for the next version; otherwise
    /// they are unregistered (removing them from every entity). The types leave the registry and
    /// the API index in both cases.
    pub(crate) fn release_module_state(&mut self, id: ModuleId, name: &str, world: &mut World, types: &mut TypeRegistry, keep: bool) {
        let victims: Vec<Entity> = world.query::<(Entity, &SpawnedBy)>().iter().filter(|(_, s)| s.0 == id).map(|(e, _)| e).collect();
        for e in victims {
            let _ = world.despawn(e);
        }
        let comps: Vec<(u32, StructDesc)> = types.mod_components(name).map(|t| (t.id(), t.desc().clone())).collect();
        for (cid, desc) in comps {
            if keep {
                let tracked = world.dynamic_info(cid).is_some_and(|(c, _)| world.is_tracked(c));
                self.stash.entry(cid).or_insert(Stashed { desc, tracked });
            } else {
                let _ = world.unregister_dynamic(cid);
            }
        }
        self.migrations.retain(|m| m.owner != name);
        types.remove_owner(name);
        let _ = self.registry.unregister_owner(&format!("mod:{name}"));
    }

    /// End of a swap (success or rollback): run the pending migrations, unregister the
    /// components no loaded version claims.
    pub(crate) fn finish_swap(&mut self, world: &mut World, types: &TypeRegistry) {
        let stash = std::mem::take(&mut self.stash);
        let migrations = std::mem::take(&mut self.migrations);
        for m in migrations {
            let ents: Vec<Entity> = world.query_dynamic(&[m.id]).collect();
            let data: Vec<Vec<u8>> =
                ents.iter().map(|&e| world.get_dynamic(e, m.id).map(|r| migrate_bytes(&m.old, &m.new.desc, &r)).unwrap_or_default()).collect();
            let _ = world.unregister_dynamic(m.id);
            if let Err(e) = world.register_dynamic(m.id, info_of(&m.new.desc), m.new.tracked) {
                self.log(super::LogLevel::Error, format!("[{}] migrating `{}`: {e}", m.owner, m.new.desc.name));
                continue;
            }
            let n = ents.len();
            for (e, bytes) in ents.into_iter().zip(data) {
                let _ = world.insert_dynamic(e, m.id, &bytes);
            }
            self.log(super::LogLevel::Info, format!("[{}] migrated `{}` on {n} entities to the new layout", m.owner, m.new.desc.name));
        }
        for id in stash.keys() {
            if !types.get(*id).is_some_and(|t| t.is_dynamic()) {
                let _ = world.unregister_dynamic(*id);
            }
        }
    }

    /// Undo a registration whose module then failed to load (nothing of it was used yet).
    pub(crate) fn undo_components(&mut self, name: &str, world: &mut World, types: &mut TypeRegistry) {
        let ids: Vec<u32> = types.mod_components(name).map(|t| t.id()).collect();
        self.migrations.retain(|m| m.owner != name);
        types.remove_mod_components(name);
        let _ = self.registry.unregister_owner(&format!("mod:{name}"));
        for id in ids {
            if !self.stash.contains_key(&id) {
                let _ = world.unregister_dynamic(id);
            }
        }
    }

    /// The declared components of the loaded mod `name`, from its package.
    pub(crate) fn declared_components(&self, name: &str) -> Vec<ModComponent> {
        self.loaded_mod_idx(name)
            .and_then(|i| self.modules[i].package.as_ref())
            .map(|p| p.components.clone())
            .unwrap_or_default()
    }
}

/// Order of the stash is by id (a `BTreeMap`), never hash order.
pub(crate) type StashMap = BTreeMap<u32, Stashed>;
