//! Reflection for tools (the in-game editor, docs/IN_GAME_EDITOR.md): list the entities a scene
//! would capture, read an entity's components as named fields and write one field back.
//!
//! It uses the scene codec ([`SceneValue`], the same one scene files and snapshots use), so what a
//! tool shows is what `save_scene` would write. Only components the [`TypeRegistry`](super::types::TypeRegistry)
//! knows are visible (Rust components marked `#[script_component]`, mod components); `Name`,
//! `Children` and other Rust-only components are not fields, `Name` is only read for the display
//! name.

use std::collections::{BTreeMap, BTreeSet};

use hecs::Entity;

use super::Engine;
use super::components::{EditorId, Name, Parent};
use super::scene::{SceneError, SceneValue, decode_component, encode_component};
use super::types::entity_to_bits;

/// One component of an entity, its fields in declared order.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectedComponent {
    /// Registry name (`Transform`, `mod:<mod>/<Name>`).
    pub name: String,
    /// `(field, value)` in the order the layout declares them.
    pub fields: Vec<(String, SceneValue)>,
    /// The component is read-only (`GlobalTransform`): [`Engine::set_component_field`] refuses it.
    pub readonly: bool,
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
            out.push(InspectedComponent { name: entry.name().to_string(), fields, readonly: entry.readonly() });
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
