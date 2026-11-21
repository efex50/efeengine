//! Reflection for tools (the in-game editor, docs/IN_GAME_EDITOR.md): list the entities a scene
//! would capture, read an entity's components as named fields and write one field back.
//!
//! It uses the scene codec ([`SceneValue`], the same one scene files and snapshots use), so what a
//! tool shows is what `save_scene` would write. Only components the [`TypeRegistry`](super::types::TypeRegistry)
//! knows are visible (Rust components marked `#[script_component]`, mod components); `Name`,
//! `Children` and other Rust-only components are not fields; `Name` is only read for the display
//! name (it travels in [`EntityData`] and in scene files as `SceneEntity::name`).

use std::collections::{BTreeMap, BTreeSet};

use efevm::Prim;
use hecs::Entity;

use super::Engine;
use super::components::{Camera, EditorId, Name, Parent, Sprite, Transform};
use super::scene::{Scene, SceneEntity, SceneError, SceneOwned, SceneValue, SCENE_FORMAT, decode_component, encode_component};
use super::types::{TypeEntry, entity_to_bits};

/// One component of an entity, its fields in declared order.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectedComponent {
    /// Registry name (`Transform`, `mod:<mod>/<Name>`).
    pub name: String,
    /// `(field, value)` in the order the layout declares them.
    pub fields: Vec<(String, SceneValue)>,
    /// The component is read-only (`GlobalTransform`): [`Engine::set_component_field`] refuses it.
    pub readonly: bool,
    /// Names of the fields that hold an entity reference (shown as numbers in `fields`; a tool
    /// should not offer them as editable text).
    pub entity_fields: Vec<String>,
}

/// A copy of one entity, enough to spawn it again (undo of a delete, duplicate): its registry
/// components and `Name` (`scene`, with `id` = its entity bits, so references among a group of
/// copies are remapped when the group is spawned) and its scene owner tag.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityData {
    pub scene: SceneEntity,
    pub(crate) owner: Option<SceneOwned>,
}

impl EntityData {
    /// The entity's `EditorId`, if it had one.
    pub fn editor_id(&self) -> Option<u64> {
        self.scene.components.get("EditorId").and_then(|f| f.get("id")).and_then(|v| match v {
            SceneValue::UInt(u) => Some(*u),
            SceneValue::Int(i) => u64::try_from(*i).ok(),
            _ => None,
        })
    }
    /// Gives the copy another `EditorId` (and forgets its old entity id, so nothing is remapped).
    pub fn set_editor_id(&mut self, id: u64) {
        self.scene.components.entry("EditorId".into()).or_default().insert("id".into(), SceneValue::UInt(id));
        self.scene.id = None;
    }
}

/// Components [`Engine::addable_components`] leaves out: `EditorId` identifies the entity for undo
/// and scene tools (use [`Engine::ensure_editor_id`]).
pub const UNMANAGED_COMPONENTS: [&str; 1] = ["EditorId"];

/// The bytes of a new component: the type's `Default` for the engine's own `Transform`, `Sprite`
/// and `Camera`, zeros for everything else (scripts' and mods' components are plain data).
fn default_bytes(entry: &TypeEntry) -> Vec<u8> {
    let size = entry.size() as usize;
    let known: Option<Vec<u8>> = match entry.name() {
        "Transform" => Some(bytemuck::bytes_of(&Transform::default()).to_vec()),
        "Sprite" => Some(bytemuck::bytes_of(&Sprite::default()).to_vec()),
        "Camera" => Some(bytemuck::bytes_of(&Camera::orthographic_2d()).to_vec()),
        _ => None,
    };
    known.filter(|b| b.len() == size).unwrap_or_else(|| vec![0u8; size])
}

/// One row of the entity list: enough for a tree view.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityRow {
    pub entity: Entity,
    /// The `Name` component; else the first registry component (`Sprite #3`); else `Entity(3)`.
    pub name: String,
    /// The entity's `EditorId`, if it has one.
    pub editor_id: Option<u64>,
    /// The entity's `Parent`, only when that parent is in the list too (else the row is a root).
    pub parent: Option<Entity>,
    /// Tagged `SceneOwned` by a scene (a scene switch despawns it).
    pub scene_owned: bool,
    /// Nesting level: 0 for roots.
    pub depth: usize,
    /// Direct children, in list order.
    pub children: Vec<Entity>,
}

impl Engine {
    /// The entities a scene or snapshot covers (a scene owner tag or one registered, writable
    /// component), as rows in tree order: roots first by `EditorId` (rows without one last, then by
    /// entity bits), each followed by its children, depth first, children sorted the same way. The
    /// order is stable between calls while the world does not change.
    pub fn editable_entities(&self) -> Vec<EntityRow> {
        let world = self.world();
        let mut rows: BTreeMap<u64, EntityRow> = BTreeMap::new();
        for e in self.captured_entities(false) {
            let editor_id = world.get::<&EditorId>(e).ok().map(|i| i.id);
            let name = match world.get::<&Name>(e) {
                Ok(n) if !n.0.is_empty() => n.0.clone(),
                _ => {
                    let first = self.types().entries().iter().find(|t| t.is_component() && t.has(world, e) && !matches!(t.name(), "EditorId" | "Parent"));
                    match first {
                        Some(t) => format!("{} #{}", t.name(), e.id()),
                        None => format!("Entity({})", e.id()),
                    }
                }
            };
            let parent = world.get::<&Parent>(e).ok().and_then(|p| p.entity());
            rows.insert(
                entity_to_bits(e),
                EntityRow { entity: e, name, editor_id, parent, scene_owned: self.is_scene_owned(e), depth: 0, children: Vec::new() },
            );
        }
        // A parent that is not in the list (gone, or not a captured entity) makes the row a root.
        let listed: BTreeSet<u64> = rows.keys().copied().collect();
        for r in rows.values_mut() {
            if r.parent.is_some_and(|p| !listed.contains(&entity_to_bits(p)) || p == r.entity) {
                r.parent = None;
            }
        }
        let key = |r: &EntityRow| (r.editor_id.unwrap_or(u64::MAX), entity_to_bits(r.entity));
        let mut order: Vec<&EntityRow> = rows.values().collect();
        order.sort_by_key(|r| key(r));
        let mut kids: BTreeMap<u64, Vec<Entity>> = BTreeMap::new();
        for r in &order {
            if let Some(p) = r.parent {
                kids.entry(entity_to_bits(p)).or_default().push(r.entity);
            }
        }
        let roots: Vec<Entity> = order.iter().filter(|r| r.parent.is_none()).map(|r| r.entity).collect();
        let mut out = Vec::with_capacity(rows.len());
        let mut seen = BTreeSet::new();
        fn walk(e: Entity, depth: usize, rows: &BTreeMap<u64, EntityRow>, kids: &BTreeMap<u64, Vec<Entity>>, seen: &mut BTreeSet<u64>, out: &mut Vec<EntityRow>) {
            let bits = entity_to_bits(e);
            if !seen.insert(bits) {
                return;
            }
            let mut row = rows[&bits].clone();
            row.depth = depth;
            row.children = kids.get(&bits).cloned().unwrap_or_default();
            out.push(row);
            for c in kids.get(&bits).into_iter().flatten() {
                walk(*c, depth + 1, rows, kids, seen, out);
            }
        }
        for r in roots {
            walk(r, 0, &rows, &kids, &mut seen, &mut out);
        }
        // A parent cycle has no root: show its rows as roots, so nothing is hidden.
        for r in &order {
            if !seen.contains(&entity_to_bits(r.entity)) {
                let mut row = (*r).clone();
                row.parent = None;
                let bits = entity_to_bits(r.entity);
                seen.insert(bits);
                row.children = kids.get(&bits).cloned().unwrap_or_default();
                out.push(row);
            }
        }
        out
    }

    /// The registry-known components of `e` with their fields, in registry order (read-only ones
    /// included, flagged). `None` when `e` is not alive.
    pub fn inspect_entity(&self, e: Entity) -> Option<Vec<InspectedComponent>> {
        let world = self.world();
        if !world.contains(e) {
            return None;
        }
        let mut out = Vec::new();
        for entry in self.types().entries() {
            if !entry.is_component() || !entry.has(world, e) {
                continue;
            }
            let mut buf = vec![0u8; entry.size() as usize];
            if entry.read(world, e, &mut buf).is_err() {
                continue;
            }
            let mut by_name = encode_component(entry, &buf);
            let fields = entry.desc().fields.iter().filter_map(|f| by_name.remove(&f.name).map(|v| (f.name.clone(), v))).collect();
            let entity_fields = entry.desc().fields.iter().filter(|f| f.ty == Prim::Entity).map(|f| f.name.clone()).collect();
            out.push(InspectedComponent { name: entry.name().to_string(), fields, readonly: entry.readonly(), entity_fields });
        }
        Some(out)
    }

    /// Write one field of a component of `e` through the scene codec: `value` has the shape
    /// `inspect_entity` returns (a number or bool; a list for a field with several values).
    /// Errors (nothing is written): no such component, the entity lacks it, the component is
    /// read-only, no such field, or a value that does not fit the field's type or range. The error
    /// names scene `<inspect>` and entity 0.
    pub fn set_component_field(&mut self, e: Entity, component: &str, field: &str, value: SceneValue) -> Result<(), SceneError> {
        let scene = "<inspect>".to_string();
        let fail = |msg: &str| SceneError::Field { scene: scene.clone(), entity: 0, component: component.to_string(), field: field.to_string(), msg: msg.to_string() };
        let (types, world) = self.types_and_world_mut();
        let entry = types.by_name(component).ok_or_else(|| SceneError::UnknownComponent { scene: scene.clone(), entity: 0, component: component.to_string() })?;
        if !entry.is_component() {
            return Err(SceneError::NotComponent { scene: scene.clone(), entity: 0, component: component.to_string() });
        }
        if entry.readonly() {
            return Err(fail("the component is read-only"));
        }
        if !world.contains(e) || !entry.has(world, e) {
            return Err(SceneError::Insert { scene: scene.clone(), entity: 0, component: component.to_string(), msg: "the entity has no such component".into() });
        }
        if !entry.desc().fields.iter().any(|f| f.name == field) {
            return Err(fail("no such field"));
        }
        let mut buf = vec![0u8; entry.size() as usize];
        entry.read(world, e, &mut buf).map_err(|x| fail(&x.to_string()))?;
        let mut fields = encode_component(entry, &buf);
        fields.insert(field.to_string(), value);
        let bytes = decode_component(entry, &fields).map_err(|(f, msg)| SceneError::Field { scene: scene.clone(), entity: 0, component: component.to_string(), field: f, msg })?;
        entry.write(world, e, &bytes).map_err(|x| fail(&x.to_string()))
    }
}

fn inspect_err(msg: &str, component: &str) -> SceneError {
    SceneError::Insert { scene: "<inspect>".into(), entity: 0, component: component.to_string(), msg: msg.to_string() }
}

impl Engine {
    /// An `EditorId` no entity has: one more than the largest in the world (1 for an empty world).
    pub fn next_editor_id(&self) -> u64 {
        self.world().query::<&EditorId>().iter().map(|i| i.id).max().map_or(1, |m| m.saturating_add(1))
    }

    /// The `EditorId` of `e`; an entity without one gets a new one ([`next_editor_id`](Self::next_editor_id)).
    /// `None` when `e` is not alive.
    pub fn ensure_editor_id(&mut self, e: Entity) -> Option<u64> {
        if !self.world().contains(e) {
            return None;
        }
        if let Ok(i) = self.world().get::<&EditorId>(e) {
            return Some(i.id);
        }
        let id = self.next_editor_id();
        self.world_mut().insert_one(e, EditorId { id }).ok()?;
        Some(id)
    }

    /// Registry components `e` lacks and a tool may add by name, sorted by name: not read-only,
    /// not in [`UNMANAGED_COMPONENTS`]. Empty for a dead entity.
    pub fn addable_components(&self, e: Entity) -> Vec<String> {
        let world = self.world();
        if !world.contains(e) {
            return Vec::new();
        }
        let mut out: Vec<String> = self
            .types()
            .entries()
            .iter()
            .filter(|t| t.is_component() && !t.readonly() && !UNMANAGED_COMPONENTS.contains(&t.name()) && !t.has(world, e))
            .map(|t| t.name().to_string())
            .collect();
        out.sort();
        out
    }

    /// Add the registry component `component` to `e` with default content: `Default` for
    /// `Transform`, `Sprite` and `Camera`, zeroed bytes for any other component (a script's or a
    /// mod's). Errors (nothing changes): unknown name, not a component, read-only, the entity is
    /// dead or already has it.
    pub fn add_component_default(&mut self, e: Entity, component: &str) -> Result<(), SceneError> {
        let scene = "<inspect>".to_string();
        let (types, world) = self.types_and_world_mut();
        let entry = types.by_name(component).ok_or_else(|| SceneError::UnknownComponent { scene: scene.clone(), entity: 0, component: component.to_string() })?;
        if !entry.is_component() {
            return Err(SceneError::NotComponent { scene, entity: 0, component: component.to_string() });
        }
        if entry.readonly() {
            return Err(inspect_err("the component is read-only", component));
        }
        if !world.contains(e) {
            return Err(inspect_err("no such entity", component));
        }
        if entry.has(world, e) {
            return Err(inspect_err("the entity already has this component", component));
        }
        let bytes = default_bytes(entry);
        entry.insert(world, e, &bytes).map_err(|x| inspect_err(&x.to_string(), component))
    }

    /// Remove the registry component `component` from `e`. Errors (nothing changes): unknown
    /// name, not a component, read-only (derived data), dead entity, the entity lacks it.
    /// (`EditorId` can be removed here; a tool decides not to offer it, see [`UNMANAGED_COMPONENTS`].)
    pub fn remove_component(&mut self, e: Entity, component: &str) -> Result<(), SceneError> {
        let scene = "<inspect>".to_string();
        let (types, world) = self.types_and_world_mut();
        let entry = types.by_name(component).ok_or_else(|| SceneError::UnknownComponent { scene: scene.clone(), entity: 0, component: component.to_string() })?;
        if !entry.is_component() {
            return Err(SceneError::NotComponent { scene, entity: 0, component: component.to_string() });
        }
        if entry.readonly() {
            return Err(inspect_err("the component is read-only", component));
        }
        if !world.contains(e) || !entry.has(world, e) {
            return Err(inspect_err("the entity has no such component", component));
        }
        entry.remove(world, e).map_err(|x| inspect_err(&x.to_string(), component))
    }

    /// Set (or replace) the `Name` of `e`. `false` when `e` is not alive.
    pub fn set_entity_name(&mut self, e: Entity, name: &str) -> bool {
        self.world_mut().insert_one(e, Name::new(name)).is_ok()
    }

    /// Spawn a new editable entity: `Transform` (default), `EditorId` (the next free one) and
    /// `Name` `entity <id>`. It is tagged as owned by the active scene when one is loaded (so
    /// `save_scene` includes it and a scene switch removes it), a free entity otherwise.
    pub fn new_editor_entity(&mut self) -> Entity {
        let id = self.next_editor_id();
        let e = if self.current_scene().is_some() { self.spawn_scene_owned() } else { self.world_mut().spawn(()) };
        let _ = self.world_mut().insert(e, (Transform::default(), EditorId { id }, Name::new(format!("entity {id}"))));
        e
    }

    /// Names of the fields of registry component `component` that hold an entity reference
    /// (empty for an unknown component). For tools that keep references across a delete / respawn.
    pub fn entity_field_names(&self, component: &str) -> Vec<String> {
        self.types().by_name(component).map(|t| t.desc().fields.iter().filter(|f| f.ty == Prim::Entity).map(|f| f.name.clone()).collect()).unwrap_or_default()
    }

    /// `e` and its descendants (entities whose `Parent` chain leads to it), depth first, parents
    /// before children, each once. Empty for a dead entity.
    pub fn entity_tree(&self, e: Entity) -> Vec<Entity> {
        let world = self.world();
        if !world.contains(e) {
            return Vec::new();
        }
        let mut kids: BTreeMap<u64, Vec<Entity>> = BTreeMap::new();
        for (c, p) in world.query::<(Entity, &Parent)>().iter() {
            if let Some(p) = p.entity() {
                kids.entry(entity_to_bits(p)).or_default().push(c);
            }
        }
        for v in kids.values_mut() {
            v.sort_by_key(|c| entity_to_bits(*c));
        }
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![e];
        while let Some(cur) = stack.pop() {
            if !seen.insert(entity_to_bits(cur)) {
                continue;
            }
            out.push(cur);
            if let Some(k) = kids.get(&entity_to_bits(cur)) {
                stack.extend(k.iter().rev().copied());
            }
        }
        out
    }

    /// A copy of `e` (see [`EntityData`]); `None` when it is not alive.
    pub fn capture_entity_data(&self, e: Entity) -> Option<EntityData> {
        if !self.world().contains(e) {
            return None;
        }
        let mut scene = self.capture_entity(e);
        scene.id = Some(entity_to_bits(e));
        let owner = self.world().get::<&SceneOwned>(e).ok().map(|o| (*o).clone());
        Some(EntityData { scene, owner })
    }

    /// Spawn the entities `data` describes (references among them are remapped to the new
    /// entities), with their `Name` and owner tag; returns them in order. Nothing is spawned on
    /// error: invalid component data, or an `EditorId` that the world or the group already uses.
    pub fn spawn_entity_data(&mut self, data: &[EntityData]) -> Result<Vec<Entity>, SceneError> {
        let scene = Scene { format: SCENE_FORMAT, name: "<inspect>".into(), requires: Vec::new(), systems: Vec::new(), entities: data.iter().map(|d| d.scene.clone()).collect() };
        let (decoded, _) = self.decode_scene_full(&scene)?;
        for (i, d) in data.iter().enumerate() {
            if let Some(id) = d.editor_id().filter(|id| self.entity_by_editor_id(*id).is_some()) {
                return Err(SceneError::DuplicateEditorId { scene: scene.name.clone(), entity: i, id });
            }
        }
        let owners: Vec<Option<SceneOwned>> = data.iter().map(|d| d.owner.clone()).collect();
        self.spawn_decoded(&scene, &decoded, &owners)
    }

    /// Copy `e` (all its registry components, its `Name` with ` copy` added, its owner tag) as a
    /// new entity with a new `EditorId`; `Parent` is copied, so the copy is a sibling. Children
    /// are not copied. Errors when `e` is not alive.
    pub fn duplicate_entity(&mut self, e: Entity) -> Result<Entity, SceneError> {
        let mut data = self.capture_entity_data(e).ok_or_else(|| inspect_err("no such entity", "<entity>"))?;
        data.set_editor_id(self.next_editor_id());
        if let Some(n) = &mut data.scene.name {
            n.push_str(" copy");
        }
        Ok(self.spawn_entity_data(&[data])?[0])
    }

    /// Despawn `e` and its descendants ([`entity_tree`](Self::entity_tree)); returns copies of them
    /// in tree order (what [`spawn_entity_data`](Self::spawn_entity_data) needs to bring them back).
    /// Empty when `e` is not alive.
    pub fn delete_entity_tree(&mut self, e: Entity) -> Vec<EntityData> {
        let tree = self.entity_tree(e);
        let data: Vec<EntityData> = tree.iter().filter_map(|x| self.capture_entity_data(*x)).collect();
        for x in tree {
            let _ = self.world_mut().despawn(x);
        }
        data
    }

    /// Unload the active scene and despawn every entity [`editable_entities`](Self::editable_entities)
    /// lists: an empty editable world.
    pub fn clear_editable_world(&mut self) {
        self.unload_scene();
        for e in self.captured_entities(false) {
            let _ = self.world_mut().despawn(e);
        }
    }
}
