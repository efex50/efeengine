//! Editing (docs/IN_GAME_EDITOR.md, step 2): every change the editor makes to the world, and the
//! undo / redo log of them.
//!
//! All of it runs inside `Engine::engine_task` closures (`&mut Engine`, between systems). Each
//! edit records a [`Cmd`] with its inverse data; entities are named by `EditorId` in the log
//! (an entity without one gets one the first time it is edited), so a deleted entity that comes
//! back by undo is found again by the commands before it.

use efeengine_core::ecs::{SceneValue, entity_from_bits, entity_to_bits};
use efeengine_core::engine::inspect::{EntityData, UNMANAGED_COMPONENTS};
use efeengine_core::engine::prefab::{Prefab, PrefabInstance, valid_prefab_name};
use efeengine_core::math::Affine2;
use efeengine_core::prelude::*;

use super::viewport::{decompose, world_affine_of};
use super::{EditorSelection, EditorUi, FieldKey};

/// How many edits the log keeps (the oldest are dropped).
pub const HISTORY_LIMIT: usize = 500;

/// One recorded edit. `undo` and `redo` are its two directions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cmd {
    /// A field written: the whole field value before and after (a list for `[x, y]`).
    Set { id: u64, component: String, field: String, old: SceneValue, new: SceneValue },
    AddComponent { id: u64, component: String },
    /// A component removed, with its field values (put back by undo). `refs` says which of the
    /// fields are entity references and the `EditorId` of the entity each one pointed at: undo
    /// looks that entity up again instead of trusting the old raw bits (the target may have been
    /// deleted and brought back in between).
    RemoveComponent { id: u64, component: String, fields: Vec<(String, SceneValue)>, refs: Vec<(String, Option<u64>)> },
    Rename { id: u64, old: Option<String>, new: String },
    /// A new entity (New, Duplicate): undo deletes it, redo spawns it again. `refs`: see [`RefFix`].
    Spawn { data: EntityData, refs: Vec<RefFix> },
    /// An entity and its descendants deleted, in tree order.
    Delete { data: Vec<EntityData>, refs: Vec<RefFix> },
    /// A viewport drag (move, rotate, scale): the whole local `Transform` before and after.
    Transform { id: u64, old: Transform, new: Transform },
    /// A parent change that kept the world position: the parents by `EditorId` (`None`: a root)
    /// and the local `Transform` before and after.
    Reparent { id: u64, old_parent: Option<u64>, new_parent: Option<u64>, old: Transform, new: Transform },
    /// A prefab placed (its entities in tree order): undo deletes the tree, redo spawns it again.
    SpawnTree { data: Vec<EntityData>, refs: Vec<RefFix> },
    /// A prefab link broken: the entities by `EditorId` with the link they had.
    Unpack { links: Vec<(u64, PrefabInstance)> },
    /// A prefab file written from entities (Save as prefab, Apply): the file before (`None`: there
    /// was none) and after, the entities that were linked to it (in prefab order) and what the
    /// write changed in the world (links and values of other instances) for undo.
    PrefabWrite { name: String, before: Option<Prefab>, after: Prefab, source: Vec<u64>, tags: Vec<(u64, Option<PrefabInstance>)>, values: Vec<(u64, String, String, SceneValue)> },
    /// The scene's `systems` list.
    SceneSystems { old: Vec<String>, new: Vec<String> },
}

/// An entity reference of a saved copy that points outside the copied group, kept by the
/// target's `EditorId` and resolved when the copy is spawned again.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RefFix {
    index: usize,
    component: String,
    field: String,
    target: Option<u64>,
}

fn ref_bits(v: &SceneValue) -> Option<u64> {
    match v {
        SceneValue::UInt(u) => Some(*u),
        SceneValue::Int(i) => u64::try_from(*i).ok(),
        _ => None,
    }
}

/// The references of `data` that point outside the group (their `scene.id`s), by `EditorId`.
fn externalize(e: &mut Engine, data: &[EntityData]) -> Vec<RefFix> {
    let group: Vec<u64> = data.iter().filter_map(|d| d.scene.id).collect();
    let mut out = Vec::new();
    for (index, d) in data.iter().enumerate() {
        for (component, fields) in &d.scene.components {
            for field in e.entity_field_names(component) {
                let Some(bits) = fields.get(&field).and_then(ref_bits).filter(|b| *b != 0 && !group.contains(b)) else { continue };
                let target = entity_from_bits(bits).filter(|t| e.world().contains(*t)).and_then(|t| e.ensure_editor_id(t));
                out.push(RefFix { index, component: component.clone(), field, target });
            }
        }
    }
    out
}

/// Writes the current entity of every [`RefFix`] into `data` (0, no entity, when the target is gone).
fn internalize(e: &Engine, data: &mut [EntityData], refs: &[RefFix]) {
    for r in refs {
        let bits = r.target.and_then(|id| e.entity_by_editor_id(id)).map_or(0, entity_to_bits);
        if let Some(f) = data.get_mut(r.index).and_then(|d| d.scene.components.get_mut(&r.component)) {
            f.insert(r.field.clone(), SceneValue::UInt(bits));
        }
    }
}

impl Cmd {
    fn label(&self) -> String {
        match self {
            Cmd::Set { component, field, .. } => format!("edit {component}.{field}"),
            Cmd::AddComponent { component, .. } => format!("add {component}"),
            Cmd::RemoveComponent { component, .. } => format!("remove {component}"),
            Cmd::Rename { .. } => "rename".to_string(),
            Cmd::Spawn { .. } => "new entity".to_string(),
            Cmd::Delete { .. } => "delete entity".to_string(),
            Cmd::Transform { .. } => "transform".to_string(),
            Cmd::Reparent { .. } => "reparent".to_string(),
            Cmd::SpawnTree { .. } => "place prefab".to_string(),
            Cmd::Unpack { .. } => "unpack prefab".to_string(),
            Cmd::PrefabWrite { name, .. } => format!("write prefab {name}"),
            Cmd::SceneSystems { .. } => "scene systems".to_string(),
        }
    }
}

/// The command log. A resource; private to the plugin except for its sizes (in [`EditorUi`]).
#[derive(Default)]
pub struct EditorHistory {
    undo: Vec<Cmd>,
    redo: Vec<Cmd>,
}

impl EditorHistory {
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

/// Read-modify-write of [`EditorUi`] that stamps the resource changed only when `f` changed
/// something (a stamp wakes the view).
pub(crate) fn with_ui(e: &mut Engine, f: impl FnOnce(&mut EditorUi)) {
    let Some(cur) = e.resources().get::<EditorUi>().map(|u| (*u).clone()) else { return };
    let mut next = cur.clone();
    f(&mut next);
    if next != cur {
        if let Some(u) = e.resources_mut().get_mut::<EditorUi>() {
            *u = next;
        }
    }
    super::request_refresh(e);
}

/// Shows `msg` in the status line (an error also goes to the log).
pub(crate) fn say(e: &mut Engine, msg: impl Into<String>, error: bool) {
    let msg = msg.into();
    if error {
        global_warn(&format!("editor: {msg}"));
    }
    with_ui(e, |ui| {
        ui.status = msg;
        ui.status_error = error;
    });
}

fn history<'a>(e: &'a mut Engine) -> Option<&'a mut EditorHistory> {
    e.resources_mut().get_mut::<EditorHistory>()
}

fn sync_sizes(e: &mut Engine) {
    let (u, r) = history(e).map_or((0, 0), |h| (h.undo.len(), h.redo.len()));
    with_ui(e, |ui| {
        ui.undo_len = u;
        ui.redo_len = r;
    });
}

/// Records `cmd` (clears the redo side) and marks the scene dirty.
fn record(e: &mut Engine, cmd: Cmd) {
    if let Some(h) = history(e) {
        h.undo.push(cmd);
        h.redo.clear();
        if h.undo.len() > HISTORY_LIMIT {
            h.undo.remove(0);
        }
    }
    with_ui(e, |ui| ui.dirty = true);
    sync_sizes(e);
}

/// Forgets the log and every half-typed value: after the world was replaced (Stop restored the
/// snapshot, a scene was loaded).
pub(crate) fn reset(e: &mut Engine) {
    if let Some(h) = history(e) {
        h.undo.clear();
        h.redo.clear();
    }
    with_ui(e, |ui| {
        ui.drafts.clear();
        ui.errors.clear();
        ui.undo_len = 0;
        ui.redo_len = 0;
    });
}

/// The selected entity, if it is alive (found again by `EditorId` when its id changed).
pub(crate) fn selected(e: &Engine) -> Option<Entity> {
    let s = e.resources().get::<EditorSelection>()?;
    s.entity.filter(|x| e.world().contains(*x)).or_else(|| s.editor_id.and_then(|id| e.entity_by_editor_id(id)))
}

/// Selects `entity` (or nothing).
pub(crate) fn select(e: &mut Engine, entity: Option<Entity>) {
    let editor_id = entity.and_then(|x| e.world().get::<&EditorId>(x).ok().map(|i| i.id));
    let new = EditorSelection { entity, editor_id };
    if e.resources().get::<EditorSelection>().is_some_and(|s| *s != new) {
        if let Some(s) = e.resources_mut().get_mut::<EditorSelection>() {
            *s = new;
        }
    }
    with_ui(e, |ui| {
        ui.drafts.clear();
        ui.errors.clear();
    });
}

/// Components the user may remove: not read-only, not `EditorId`.
pub fn removable(name: &str, readonly: bool) -> bool {
    !readonly && !UNMANAGED_COMPONENTS.contains(&name)
}

/// Text to a field value of the same shape as `shape` (`Bool`, `Int`, `UInt`, `Float`).
pub fn parse_scalar(shape: &SceneValue, text: &str) -> Result<SceneValue, String> {
    let t = text.trim();
    match shape {
        SceneValue::Bool(_) => match t.to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(SceneValue::Bool(true)),
            "false" | "0" | "no" | "off" => Ok(SceneValue::Bool(false)),
            _ => Err(format!("`{t}` is not true or false")),
        },
        SceneValue::Int(_) => t.parse::<i64>().map(SceneValue::Int).map_err(|_| format!("`{t}` is not a whole number")),
        SceneValue::UInt(_) => t.parse::<u64>().map(SceneValue::UInt).map_err(|_| format!("`{t}` is not a whole number of 0 or more")),
        SceneValue::Float(_) => t.parse::<f64>().ok().filter(|f| f.is_finite()).map(SceneValue::Float).ok_or_else(|| format!("`{t}` is not a number")),
        SceneValue::List(_) => Err("a list cannot be typed as one value".to_string()),
    }
}

/// `current` with the value at `index` (0 for a scalar) replaced by the parsed `text`.
pub fn with_element(current: &SceneValue, index: usize, text: &str) -> Result<SceneValue, String> {
    match current {
        SceneValue::List(items) => {
            let shape = items.get(index).ok_or_else(|| format!("no element {index}"))?;
            let mut out = items.clone();
            out[index] = parse_scalar(shape, text)?;
            Ok(SceneValue::List(out))
        }
        scalar if index == 0 => parse_scalar(scalar, text),
        _ => Err(format!("no element {index}")),
    }
}

/// The current value of `component.field` of `entity`, from the world.
fn current_field(e: &Engine, entity: Entity, component: &str, field: &str) -> Option<(SceneValue, bool)> {
    let comps = e.inspect_entity(entity)?;
    let c = comps.into_iter().find(|c| c.name == component)?;
    let is_ref = c.entity_fields.iter().any(|f| f == field);
    c.fields.into_iter().find(|(n, _)| n == field).map(|(_, v)| (v, is_ref))
}

/// Writes `component.field` of the selected entity to `new` (the whole value) and records it.
/// `Err` is the message the field shows; nothing changed then.
fn write_field(e: &mut Engine, entity: Entity, component: &str, field: &str, new: SceneValue) -> Result<(), String> {
    if UNMANAGED_COMPONENTS.contains(&component) {
        return Err(format!("{component} is managed by the editor"));
    }
    let (old, is_ref) = current_field(e, entity, component, field).ok_or_else(|| format!("{component}.{field} is not there"))?;
    if is_ref {
        return Err("entity references are read-only".to_string());
    }
    if old == new {
        return Ok(());
    }
    e.set_component_field(entity, component, field, new.clone()).map_err(|x| x.to_string())?;
    let id = e.ensure_editor_id(entity).ok_or("the entity is gone")?;
    record(e, Cmd::Set { id, component: component.to_string(), field: field.to_string(), old, new });
    Ok(())
}

/// A typed value arrived for a field (Enter or focus lost): parse it, write it, or keep the
/// error on the field.
pub(crate) fn commit_field(e: &mut Engine, key: FieldKey, text: String) {
    if key.is_name() {
        return rename(e, text);
    }
    let Some(entity) = selected(e) else { return };
    let result = (|| {
        let (cur, is_ref) = current_field(e, entity, &key.component, &key.field).ok_or_else(|| format!("{}.{} is not there", key.component, key.field))?;
        if is_ref {
            return Err("entity references are read-only".to_string());
        }
        let new = with_element(&cur, key.index, &text)?;
        write_field(e, entity, &key.component, &key.field, new)
    })();
    match result {
        Ok(()) => with_ui(e, |ui| {
            ui.drafts.remove(&key);
            ui.errors.remove(&key);
            ui.status_error = false;
            ui.status.clear();
        }),
        Err(msg) => {
            let line = format!("{}.{}: {msg}", key.component, key.field);
            with_ui(e, |ui| {
                ui.drafts.insert(key.clone(), text.clone());
                ui.errors.insert(key, msg);
            });
            say(e, line, true);
        }
    }
}

/// A checkbox: write `on` to a `Bool` field.
pub(crate) fn toggle_field(e: &mut Engine, key: FieldKey, on: bool) {
    let Some(entity) = selected(e) else { return };
    if let Err(msg) = write_field(e, entity, &key.component, &key.field, SceneValue::Bool(on)) {
        say(e, format!("{}.{}: {msg}", key.component, key.field), true);
    }
}

pub(crate) fn add_component(e: &mut Engine, component: String) {
    let Some(entity) = selected(e) else { return };
    match e.add_component_default(entity, &component) {
        Ok(()) => {
            if let Some(id) = e.ensure_editor_id(entity) {
                record(e, Cmd::AddComponent { id, component });
            }
            with_ui(e, |ui| ui.picker_open = false);
        }
        Err(x) => say(e, x.to_string(), true),
    }
}

pub(crate) fn remove_component(e: &mut Engine, component: String) {
    let Some(entity) = selected(e) else { return };
    let Some(c) = e.inspect_entity(entity).and_then(|v| v.into_iter().find(|c| c.name == component)) else { return };
    if !removable(&c.name, c.readonly) {
        return say(e, format!("{} cannot be removed", c.name), true);
    }
    // Entity references are remembered by the target's EditorId, not by the raw bits.
    let mut refs: Vec<(String, Option<u64>)> = Vec::new();
    for f in &c.entity_fields {
        let bits = c.fields.iter().find(|(n, _)| n == f).and_then(|(_, v)| ref_bits(v)).unwrap_or(0);
        let target = entity_from_bits(bits).filter(|t| e.world().contains(*t)).and_then(|t| e.ensure_editor_id(t));
        refs.push((f.clone(), target));
    }
    match e.remove_component(entity, &component) {
        Ok(()) => {
            if let Some(id) = e.ensure_editor_id(entity) {
                record(e, Cmd::RemoveComponent { id, component, fields: c.fields, refs });
            }
            with_ui(e, |ui| {
                ui.drafts.clear();
                ui.errors.clear();
            });
        }
        Err(x) => say(e, x.to_string(), true),
    }
}

pub(crate) fn rename(e: &mut Engine, text: String) {
    let Some(entity) = selected(e) else { return };
    let text = text.trim().to_string();
    let old = e.world().get::<&Name>(entity).ok().map(|n| n.0.clone());
    if old.as_deref().unwrap_or("") == text {
        return with_ui(e, |ui| {
            ui.drafts.remove(&FieldKey::name());
        });
    }
    let Some(id) = e.ensure_editor_id(entity) else { return };
    set_name(e, entity, Some(&text));
    record(e, Cmd::Rename { id, old, new: text });
    with_ui(e, |ui| {
        ui.drafts.remove(&FieldKey::name());
    });
}

/// `Some(name)` sets the `Name`, `None` removes it.
fn set_name(e: &mut Engine, entity: Entity, name: Option<&str>) {
    match name {
        Some(n) => {
            e.set_entity_name(entity, n);
        }
        None => {
            let _ = e.world_mut().remove_one::<Name>(entity);
        }
    }
}

pub(crate) fn new_entity(e: &mut Engine) {
    let n = e.new_editor_entity();
    if let Some(data) = e.capture_entity_data(n) {
        let refs = externalize(e, std::slice::from_ref(&data));
        record(e, Cmd::Spawn { data, refs });
    }
    select(e, Some(n));
}

pub(crate) fn duplicate(e: &mut Engine) {
    let Some(entity) = selected(e) else { return };
    e.ensure_editor_id(entity);
    match e.duplicate_entity(entity) {
        Ok(n) => {
            if let Some(data) = e.capture_entity_data(n) {
                let refs = externalize(e, std::slice::from_ref(&data));
                record(e, Cmd::Spawn { data, refs });
            }
            select(e, Some(n));
        }
        Err(x) => say(e, x.to_string(), true),
    }
}

pub(crate) fn delete(e: &mut Engine) {
    let Some(entity) = selected(e) else { return };
    // Everything that goes away gets an id, so undo (and the commands before it) can name it.
    for x in e.entity_tree(entity) {
        e.ensure_editor_id(x);
    }
    let data = e.delete_entity_tree(entity);
    if data.is_empty() {
        return;
    }
    let refs = externalize(e, &data);
    record(e, Cmd::Delete { data, refs });
    select(e, None);
}

fn write_transform(e: &mut Engine, ent: Entity, t: Transform) -> Result<(), String> {
    let mut cur = e.world_mut().get::<&mut Transform>(ent).map_err(|_| "the entity has no Transform".to_string())?;
    *cur = t;
    Ok(())
}

/// `parent` (an `EditorId`, resolved now) or no parent.
fn set_parent(e: &mut Engine, ent: Entity, parent: Option<u64>) -> Result<(), String> {
    match parent {
        Some(id) => {
            let p = find(e, id)?;
            e.world_mut().insert_one(ent, Parent::new(p)).map_err(|x| x.to_string())
        }
        None => {
            let _ = e.world_mut().remove_one::<Parent>(ent);
            Ok(())
        }
    }
}

/// Records a finished viewport drag as one undo entry (the entity already holds `new`).
pub(crate) fn record_transform(e: &mut Engine, entity: Entity, old: Transform, new: Transform) {
    if old == new {
        return;
    }
    let Some(id) = e.ensure_editor_id(entity) else { return };
    record(e, Cmd::Transform { id, old, new });
}

/// Makes the selected entity a child of `new_parent` (`None`: a root), keeping its world
/// transform; recorded for undo. Refuses a cycle (a parent that is the entity or one of its
/// descendants) and a parent without a `Transform`.
pub(crate) fn reparent(e: &mut Engine, new_parent: Option<Entity>) {
    let Some(ent) = selected(e) else { return };
    if let Err(msg) = reparent_entity(e, ent, new_parent) {
        say(e, format!("reparent: {msg}"), true);
    }
}

pub(crate) fn reparent_entity(e: &mut Engine, ent: Entity, new_parent: Option<Entity>) -> Result<(), String> {
    if let Some(p) = new_parent {
        if !e.world().contains(p) {
            return Err("that parent is gone".to_string());
        }
        if e.entity_tree(ent).contains(&p) {
            return Err("an entity cannot be its own parent or a child of its own descendant".to_string());
        }
        if e.world().get::<&Transform>(p).is_err() {
            return Err("the new parent has no Transform".to_string());
        }
    }
    let has_parent = e.world().get::<&Parent>(ent).is_ok();
    let old_parent = e.world().get::<&Parent>(ent).ok().and_then(|p| p.entity()).filter(|p| e.world().contains(*p));
    if old_parent == new_parent && has_parent == new_parent.is_some() {
        return Ok(());
    }
    let old = *e.world().get::<&Transform>(ent).map_err(|_| "the entity has no Transform".to_string())?;
    let world_aff = world_affine_of(e.world(), ent);
    let parent_aff = new_parent.map_or(Affine2::IDENTITY, |p| world_affine_of(e.world(), p));
    if parent_aff.matrix2.determinant().abs() < 1e-12 {
        return Err("the new parent has a zero scale".to_string());
    }
    let new = decompose(parent_aff.inverse() * world_aff);
    let id = e.ensure_editor_id(ent).ok_or("the entity is gone")?;
    let old_id = old_parent.and_then(|p| e.ensure_editor_id(p));
    let new_id = new_parent.and_then(|p| e.ensure_editor_id(p));
    match new_parent {
        Some(p) => {
            e.world_mut().insert_one(ent, Parent::new(p)).map_err(|x| x.to_string())?;
        }
        None => {
            let _ = e.world_mut().remove_one::<Parent>(ent);
        }
    }
    write_transform(e, ent, new)?;
    record(e, Cmd::Reparent { id, old_parent: old_id, new_parent: new_id, old, new });
    Ok(())
}

fn find(e: &Engine, id: u64) -> Result<Entity, String> {
    e.entity_by_editor_id(id).ok_or_else(|| format!("entity #{id} is gone"))
}

fn write_fields(e: &mut Engine, entity: Entity, component: &str, fields: &[(String, SceneValue)]) -> Result<(), String> {
    for (f, v) in fields {
        e.set_component_field(entity, component, f, v.clone()).map_err(|x| x.to_string())?;
    }
    Ok(())
}

impl Cmd {
    fn undo(&self, e: &mut Engine) -> Result<Option<Entity>, String> {
        match self {
            Cmd::Set { id, component, field, old, .. } => {
                let ent = find(e, *id)?;
                e.set_component_field(ent, component, field, old.clone()).map_err(|x| x.to_string())?;
                Ok(Some(ent))
            }
            Cmd::AddComponent { id, component } => {
                let ent = find(e, *id)?;
                e.remove_component(ent, component).map_err(|x| x.to_string())?;
                Ok(Some(ent))
            }
            Cmd::RemoveComponent { id, component, fields, refs } => {
                let ent = find(e, *id)?;
                e.add_component_default(ent, component).map_err(|x| x.to_string())?;
                let mut fields = fields.clone();
                for (name, target) in refs {
                    let bits = target.and_then(|t| e.entity_by_editor_id(t)).map_or(0, entity_to_bits);
                    if let Some(f) = fields.iter_mut().find(|(n, _)| n == name) {
                        f.1 = SceneValue::UInt(bits);
                    }
                }
                write_fields(e, ent, component, &fields)?;
                Ok(Some(ent))
            }
            Cmd::Rename { id, old, .. } => {
                let ent = find(e, *id)?;
                set_name(e, ent, old.as_deref());
                Ok(Some(ent))
            }
            Cmd::Spawn { data, .. } => {
                let ent = find(e, data.editor_id().ok_or("the entity has no id")?)?;
                e.delete_entity_tree(ent);
                Ok(None)
            }
            Cmd::Delete { data, refs } => {
                let mut data = data.clone();
                internalize(e, &mut data, refs);
                let spawned = e.spawn_entity_data(&data).map_err(|x| x.to_string())?;
                Ok(spawned.first().copied())
            }
            Cmd::Transform { id, old, .. } => {
                let ent = find(e, *id)?;
                write_transform(e, ent, *old)?;
                Ok(Some(ent))
            }
            Cmd::Reparent { id, old_parent, old, .. } => {
                let ent = find(e, *id)?;
                set_parent(e, ent, *old_parent)?;
                write_transform(e, ent, *old)?;
                Ok(Some(ent))
            }
            Cmd::SpawnTree { data, .. } => {
                let ent = find(e, data.first().and_then(|d| d.editor_id()).ok_or("the entity has no id")?)?;
                e.delete_entity_tree(ent);
                Ok(None)
            }
            Cmd::Unpack { links } => {
                let mut first = None;
                for (id, link) in links {
                    let ent = find(e, *id)?;
                    e.set_prefab_instance(ent, Some(link.clone()));
                    first.get_or_insert(ent);
                }
                Ok(first)
            }
            Cmd::PrefabWrite { name, before, source, tags, values, .. } => {
                match before {
                    Some(p) => {
                        e.save_prefab(p).map_err(|x| x.to_string())?;
                    }
                    None => e.remove_prefab(name).map_err(|x| x.to_string())?,
                }
                for (id, comp, field, old) in values.iter().rev() {
                    let ent = find(e, *id)?;
                    e.set_component_field(ent, comp, field, old.clone()).map_err(|x| x.to_string())?;
                }
                for (id, tag) in tags.iter().rev() {
                    let ent = find(e, *id)?;
                    e.set_prefab_instance(ent, tag.clone());
                }
                super::files::scan(e);
                source.first().map(|id| find(e, *id)).transpose()
            }
            Cmd::SceneSystems { old, .. } => {
                apply_scene_systems(e, old.clone())?;
                Ok(None)
            }
        }
    }

    fn redo(&self, e: &mut Engine) -> Result<Option<Entity>, String> {
        match self {
            Cmd::Set { id, component, field, new, .. } => {
                let ent = find(e, *id)?;
                e.set_component_field(ent, component, field, new.clone()).map_err(|x| x.to_string())?;
                Ok(Some(ent))
            }
            Cmd::AddComponent { id, component } => {
                let ent = find(e, *id)?;
                e.add_component_default(ent, component).map_err(|x| x.to_string())?;
                Ok(Some(ent))
            }
            Cmd::RemoveComponent { id, component, .. } => {
                let ent = find(e, *id)?;
                e.remove_component(ent, component).map_err(|x| x.to_string())?;
                Ok(Some(ent))
            }
            Cmd::Rename { id, new, .. } => {
                let ent = find(e, *id)?;
                set_name(e, ent, Some(new));
                Ok(Some(ent))
            }
            Cmd::Spawn { data, refs } => {
                let mut data = data.clone();
                internalize(e, std::slice::from_mut(&mut data), refs);
                let spawned = e.spawn_entity_data(std::slice::from_ref(&data)).map_err(|x| x.to_string())?;
                Ok(spawned.first().copied())
            }
            Cmd::Delete { data, .. } => {
                let ent = find(e, data.first().and_then(|d| d.editor_id()).ok_or("the entity has no id")?)?;
                e.delete_entity_tree(ent);
                Ok(None)
            }
            Cmd::Transform { id, new, .. } => {
                let ent = find(e, *id)?;
                write_transform(e, ent, *new)?;
                Ok(Some(ent))
            }
            Cmd::Reparent { id, new_parent, new, .. } => {
                let ent = find(e, *id)?;
                set_parent(e, ent, *new_parent)?;
                write_transform(e, ent, *new)?;
                Ok(Some(ent))
            }
            Cmd::SpawnTree { data, refs } => {
                let mut data = data.clone();
                internalize(e, &mut data, refs);
                let spawned = e.spawn_entity_data(&data).map_err(|x| x.to_string())?;
                Ok(spawned.first().copied())
            }
            Cmd::Unpack { links } => {
                let mut first = None;
                for (id, _) in links {
                    let ent = find(e, *id)?;
                    e.set_prefab_instance(ent, None);
                    first.get_or_insert(ent);
                }
                Ok(first)
            }
            Cmd::PrefabWrite { after, source, .. } => {
                e.save_prefab(after).map_err(|x| x.to_string())?;
                let ents: Vec<Entity> = source.iter().map(|id| find(e, *id)).collect::<Result<_, _>>()?;
                e.apply_prefab_to_world(after, &ents).map_err(|x| x.to_string())?;
                super::files::scan(e);
                Ok(ents.first().copied())
            }
            Cmd::SceneSystems { new, .. } => {
                apply_scene_systems(e, new.clone())?;
                Ok(None)
            }
        }
    }
}

/// Sets the scene's `systems` list. Only while the game runs do the enabled flags follow (Edit and
/// Pause keep the game frozen; the list is applied by the next scene load).
fn apply_scene_systems(e: &mut Engine, entries: Vec<String>) -> Result<(), String> {
    let live = e.resources().get::<super::EditorState>().is_some_and(|s| s.mode == super::EditorMode::Play);
    e.set_scene_system_entries(entries, live).map_err(|x| x.to_string())
}

/// Lists (`on`) or unlists system `name` in the active scene; recorded for undo.
pub(crate) fn toggle_scene_system(e: &mut Engine, name: String, on: bool) {
    let old = e.scene_system_entries();
    let new = e.scene_entries_with(&name, on);
    if old == new {
        return;
    }
    match apply_scene_systems(e, new.clone()) {
        Ok(()) => record(e, Cmd::SceneSystems { old, new }),
        Err(msg) => say(e, format!("systems: {msg}"), true),
    }
}

/// Where a placed prefab goes when nothing says otherwise: the centre of the editor camera.
fn placement_point(e: &Engine) -> Vec2 {
    super::viewport::camera_center(e).unwrap_or(Vec2::ZERO)
}

/// Places a copy of prefab `name` at the centre of the editor camera and selects its root.
pub(crate) fn place_prefab(e: &mut Engine, name: String) {
    let spawned = match e.place_prefab(&name) {
        Ok(s) => s,
        Err(x) => return say(e, format!("place prefab: {x}"), true),
    };
    let Some(&root) = spawned.first() else { return };
    let at = placement_point(e);
    if let Ok(mut t) = e.world_mut().get::<&mut Transform>(root) {
        t.position = at;
    }
    let data: Vec<EntityData> = spawned.iter().filter_map(|x| e.capture_entity_data(*x)).collect();
    let refs = externalize(e, &data);
    record(e, Cmd::SpawnTree { data, refs });
    select(e, Some(root));
    say(e, format!("Placed prefab {name}"), false);
}

/// Break the prefab link of the selected entity's whole instance.
pub(crate) fn unpack_prefab(e: &mut Engine) {
    let Some(sel) = selected(e) else { return };
    let removed = e.unpack_prefab(sel);
    if removed.is_empty() {
        return say(e, "The selected entity is not part of a prefab instance", true);
    }
    let mut links = Vec::new();
    for (ent, link) in removed {
        if let Some(id) = e.ensure_editor_id(ent) {
            links.push((id, link));
        }
    }
    let n = links.len();
    record(e, Cmd::Unpack { links });
    say(e, format!("Unpacked {n} entities"), false);
}

/// Writes the prefab file for the tree under `root` and links the tree to it.
fn write_prefab(e: &mut Engine, root: Entity, name: &str, keep_root_transform_of: Option<&Prefab>) {
    if !super::files::in_edit(e) {
        return say(e, "Prefab files can only be changed in Edit mode (press Stop first)", true);
    }
    if !valid_prefab_name(name) {
        return say(e, format!("`{name}` is not a prefab name: use letters, digits, _ and -"), true);
    }
    let (mut prefab, tree) = match e.capture_prefab(root, name) {
        Ok(x) => x,
        Err(x) => return say(e, format!("prefab: {x}"), true),
    };
    // The root's own placement is no part of the prefab: a new prefab starts at the origin, an
    // applied one keeps the transform it had.
    if let Some(t) = prefab.entities[0].components.get_mut("Transform") {
        match keep_root_transform_of.and_then(|p| p.entities.first()).and_then(|r| r.components.get("Transform")) {
            Some(old) => *t = old.clone(),
            None => {
                t.insert("position".into(), SceneValue::List(vec![SceneValue::Float(0.0), SceneValue::Float(0.0)]));
            }
        }
    }
    let before = e.load_prefab(name).ok();
    if let Err(x) = e.save_prefab(&prefab) {
        return say(e, format!("prefab: {x}"), true);
    }
    for t in &tree {
        e.ensure_editor_id(*t);
    }
    let rep = match e.apply_prefab_to_world(&prefab, &tree) {
        Ok(r) => r,
        Err(x) => return say(e, format!("prefab: {x}"), true),
    };
    let id_of = |e: &mut Engine, ent: Entity| e.ensure_editor_id(ent).unwrap_or(0);
    let tags: Vec<(u64, Option<PrefabInstance>)> = rep.tags.iter().map(|(ent, t)| (id_of(e, *ent), t.clone())).collect();
    let values: Vec<(u64, String, String, SceneValue)> = rep.values.iter().map(|(ent, c, f, v)| (id_of(e, *ent), c.clone(), f.clone(), v.clone())).collect();
    let source: Vec<u64> = tree.iter().map(|t| id_of(e, *t)).collect();
    let after = e.load_prefab(name).unwrap_or(prefab);
    record(e, Cmd::PrefabWrite { name: name.to_string(), before, after, source, tags, values });
    super::files::scan(e);
    let note = if rep.stale > 0 { format!(" ({} other instances change when the scene is loaded again)", rep.stale) } else { String::new() };
    say(e, format!("Saved prefab {name} ({} entities){note}", tree.len()), false);
}

/// Save As prefab: the selected entity and its children become prefab `name`, and the entity an
/// instance of it.
pub(crate) fn save_as_prefab(e: &mut Engine, name: String) {
    let Some(sel) = selected(e) else { return say(e, "Select an entity first", true) };
    let name = name.trim().to_string();
    write_prefab(e, sel, &name, None);
}

/// Apply to prefab: write the selected instance back to its prefab file.
pub(crate) fn apply_prefab(e: &mut Engine) {
    let Some(sel) = selected(e) else { return };
    let Some(root) = e.prefab_instance_root(sel) else {
        return say(e, "The selected entity is not part of a complete prefab instance", true);
    };
    let Some(tag) = e.prefab_instance(root) else { return };
    let base = tag.base().clone();
    write_prefab(e, root, &tag.prefab().to_string(), Some(&base));
}

/// Revert: put the prefab's value back into an overridden field of the selected entity.
pub(crate) fn revert_field(e: &mut Engine, key: FieldKey) {
    let Some(sel) = selected(e) else { return };
    let Some(base) = e.prefab_base_field(sel, &key.component, &key.field) else {
        return say(e, format!("{}.{} has no prefab value", key.component, key.field), true);
    };
    match write_field(e, sel, &key.component, &key.field, base) {
        Ok(()) => with_ui(e, |ui| {
            ui.drafts.retain(|k, _| !(k.component == key.component && k.field == key.field));
            ui.errors.retain(|k, _| !(k.component == key.component && k.field == key.field));
        }),
        Err(msg) => say(e, format!("{}.{}: {msg}", key.component, key.field), true),
    }
}

pub(crate) fn undo(e: &mut Engine) {
    step(e, true);
}

pub(crate) fn redo(e: &mut Engine) {
    step(e, false);
}

fn step(e: &mut Engine, is_undo: bool) {
    let cmd = history(e).and_then(|h| if is_undo { h.undo.pop() } else { h.redo.pop() });
    let Some(cmd) = cmd else { return };
    let result = if is_undo { cmd.undo(e) } else { cmd.redo(e) };
    match result {
        Ok(focus) => {
            if let Some(h) = history(e) {
                if is_undo { h.redo.push(cmd.clone()) } else { h.undo.push(cmd.clone()) }
            }
            // Spawns and deletes move the selection to what came back (or nothing).
            match (&cmd, focus) {
                (Cmd::Spawn { .. } | Cmd::Delete { .. } | Cmd::SpawnTree { .. }, f) => select(e, f),
                (_, Some(ent)) => select(e, Some(ent)),
                _ => {}
            }
            with_ui(e, |ui| {
                ui.dirty = true;
                ui.drafts.clear();
                ui.errors.clear();
                ui.status = format!("{}: {}", if is_undo { "Undo" } else { "Redo" }, cmd.label());
                ui.status_error = false;
            });
        }
        Err(msg) => {
            // The entity it needs is gone: the command is dropped, the older ones stay.
            say(e, format!("{} {} failed: {msg}", if is_undo { "undo" } else { "redo" }, cmd.label()), true);
        }
    }
    sync_sizes(e);
}
