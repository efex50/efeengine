//! Runtime-registered ("dynamic") components
//!
//! A dynamic component is a plain-bytes component whose size and alignment are chosen at runtime,
//! e.g. by a script or mod. It is identified by a user-chosen `u32` and stored in archetype
//! columns like any Rust component, but is only reachable through the byte-oriented
//! [`World`] methods in this module; typed queries never see it. Dynamic components are `Pod`
//! data: they are never dropped.

use core::fmt;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};
use core::slice;

use crate::alloc::boxed::Box;
use crate::alloc::vec::Vec;
use crate::archetype::{Archetype, ComponentId, DynamicId, TypeInfo};
use crate::entities::{Entity, Location, NoSuchEntity};
use crate::world::index2;
use crate::{DynamicBundle, World};

/// Largest alignment a dynamic component may request
pub const MAX_DYNAMIC_ALIGN: usize = 4096;
/// Largest size a dynamic component may have
pub const MAX_DYNAMIC_SIZE: usize = 1 << 30;

/// Layout of a dynamic component
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DynamicInfo {
    /// Size in bytes; a multiple of `align`. Zero makes the component a tag.
    pub size: usize,
    /// Alignment in bytes; a power of two, at most 4096
    pub align: usize,
}

/// Errors from the dynamic component API
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicError {
    /// The entity does not exist
    NoSuchEntity,
    /// No component is registered under this id
    Unregistered(u32),
    /// A component is already registered under this id
    AlreadyRegistered(u32),
    /// The provided bytes do not match the registered size
    WrongSize {
        /// The registered size
        expected: usize,
        /// The length of the supplied slice
        got: usize,
    },
    /// The requested size/alignment is invalid
    BadLayout,
    /// The entity does not have the component
    Missing,
}

impl fmt::Display for DynamicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DynamicError::NoSuchEntity => f.write_str("no such entity"),
            DynamicError::Unregistered(id) => {
                write!(f, "dynamic component {id} is not registered")
            }
            DynamicError::AlreadyRegistered(id) => {
                write!(f, "dynamic component {id} is already registered")
            }
            DynamicError::WrongSize { expected, got } => {
                write!(f, "expected {expected} bytes, got {got}")
            }
            DynamicError::BadLayout => f.write_str("invalid dynamic component layout"),
            DynamicError::Missing => f.write_str("entity does not have the dynamic component"),
        }
    }
}

impl core::error::Error for DynamicError {}

impl From<NoSuchEntity> for DynamicError {
    fn from(NoSuchEntity: NoSuchEntity) -> Self {
        DynamicError::NoSuchEntity
    }
}

/// What the world knows about a user-chosen dynamic id
///
/// Slots are never removed, so the generation of an id only ever increases.
pub(crate) struct DynamicSlot {
    pub(crate) generation: u32,
    pub(crate) size: usize,
    pub(crate) align: usize,
    pub(crate) registered: bool,
}

impl World {
    /// Register a dynamic component under the user-chosen `id`
    ///
    /// `info.align` must be a power of two no larger than 4096, `info.size` a multiple of it (zero
    /// is allowed, for tags) and at most 1 GiB. If `tracked`, change ticks are recorded for it
    /// (see [`track_changes_id`](Self::track_changes_id)). Returns the [`ComponentId`] used in
    /// archetypes; it carries a generation that changes whenever `id` is registered again after
    /// [`unregister_dynamic`](Self::unregister_dynamic).
    pub fn register_dynamic(
        &mut self,
        id: u32,
        info: DynamicInfo,
        tracked: bool,
    ) -> Result<ComponentId, DynamicError> {
        if info.align == 0
            || !info.align.is_power_of_two()
            || info.align > MAX_DYNAMIC_ALIGN
            || info.size % info.align != 0
            || info.size > MAX_DYNAMIC_SIZE
        {
            return Err(DynamicError::BadLayout);
        }
        let generation = match self.dynamic.get(&id) {
            Some(slot) if slot.registered => return Err(DynamicError::AlreadyRegistered(id)),
            // A fresh generation guarantees archetypes built for the old layout are never reused.
            Some(slot) => slot
                .generation
                .checked_add(1)
                .expect("dynamic component generation overflow"),
            None => 0,
        };
        self.dynamic.insert(
            id,
            DynamicSlot {
                generation,
                size: info.size,
                align: info.align,
                registered: true,
            },
        );
        let cid = ComponentId::Dynamic(DynamicId { id, generation });
        if tracked {
            self.track_changes_id(cid);
        }
        Ok(cid)
    }

    /// Remove dynamic component `id` from every entity that has it, then forget the id
    ///
    /// Affected entities are moved, in archetype order and keeping their relative order, to the
    /// archetype without the component. Returns how many entities were affected. Registering `id`
    /// again later yields a new generation, so a different layout is sound.
    pub fn unregister_dynamic(&mut self, id: u32) -> Result<u32, DynamicError> {
        let (did, _, _) = self.dynamic_slot(id)?;
        let cid = ComponentId::Dynamic(did);
        self.flush();
        let now = self.change_tick;
        let mut affected = 0u32;
        // Archetypes created below never contain `cid`, so the range can be fixed up front.
        for src_id in 0..self.archetypes.archetypes.len() {
            let src = &self.archetypes.archetypes[src_id];
            if src.is_empty() || !src.has_component(cid) {
                continue;
            }
            let target = self.dyn_remove_target(src_id as u32, cid);
            let (src, dst) = index2(
                &mut self.archetypes.archetypes,
                src_id,
                target as usize,
            );
            let moved = src.len();
            // SAFETY: `target` is `src` without `cid`, so every type of `dst` is in `src` with
            // identical layout, and the only component `dst` lacks is the dynamic one, which has
            // no destructor.
            let base = unsafe { src.drain_into(dst, now) };
            for i in 0..moved {
                let entity_id = dst.entity_id(base + i);
                self.entities.meta[entity_id as usize].location = Location {
                    archetype: target,
                    index: base + i,
                };
            }
            affected += moved;
        }
        if let Some(slot) = self.dynamic.get_mut(&id) {
            slot.registered = false;
        }
        self.archetypes.tracked.remove(&cid);
        Ok(affected)
    }

    /// The component id and layout registered under `id`
    pub fn dynamic_info(&self, id: u32) -> Option<(ComponentId, DynamicInfo)> {
        let (did, size, align) = self.dynamic_slot(id).ok()?;
        Some((ComponentId::Dynamic(did), DynamicInfo { size, align }))
    }

    /// Add dynamic component `id` to `entity`, or overwrite it if present
    ///
    /// `bytes` must be exactly the registered size; it may be unaligned. Change ticks are stamped
    /// like [`insert_one`](Self::insert_one) does (new: added and changed; replaced: changed).
    pub fn insert_dynamic(
        &mut self,
        entity: Entity,
        id: u32,
        bytes: &[u8],
    ) -> Result<(), DynamicError> {
        let (did, size, align) = self.dynamic_slot(id)?;
        if bytes.len() != size {
            return Err(DynamicError::WrongSize {
                expected: size,
                got: bytes.len(),
            });
        }
        self.flush();
        let loc = self.entities.get(entity)?;
        let cid = ComponentId::Dynamic(did);
        let now = self.change_tick;

        if self.archetypes.archetypes[loc.archetype as usize].has_component(cid) {
            let archetype = &mut self.archetypes.archetypes[loc.archetype as usize];
            // SAFETY: `loc.index < len` after `flush`; the column holds `size`-byte elements and
            // `bytes` has exactly `size` bytes; `&mut self` rules out overlap with live borrows.
            unsafe {
                let dst = archetype
                    .get_ptr(cid, size, loc.index)
                    .expect("component present");
                ptr::copy_nonoverlapping(bytes.as_ptr(), dst.as_ptr(), size);
                archetype.stamp_in_place(cid, loc.index, now, false);
            }
            return Ok(());
        }

        let target = self.dyn_insert_target(loc.archetype, cid, TypeInfo::dynamic(did, size, align));
        let (src, dst) = index2(
            &mut self.archetypes.archetypes,
            loc.archetype as usize,
            target as usize,
        );
        // SAFETY: `target` is `src` plus `cid`, so everything `src` has is in `dst`; the source
        // row is moved (not dropped) and then removed without dropping; `index` of the new row is
        // fully written (shared columns by `move_shared_row`, the new column from `bytes`).
        unsafe {
            let index = dst.allocate(entity.id, now);
            Archetype::move_shared_row(src, loc.index, dst, index, now);
            let new = dst.get_ptr(cid, size, index).expect("component present");
            ptr::copy_nonoverlapping(bytes.as_ptr(), new.as_ptr(), size);
            if let Some(moved) = src.remove(loc.index, false) {
                self.entities.meta[moved as usize].location.index = loc.index;
            }
            self.entities.meta[entity.id as usize].location = Location {
                archetype: target,
                index,
            };
        }
        Ok(())
    }

    /// Remove dynamic component `id` from `entity`
    pub fn remove_dynamic(&mut self, entity: Entity, id: u32) -> Result<(), DynamicError> {
        let (did, _, _) = self.dynamic_slot(id)?;
        self.flush();
        let loc = self.entities.get(entity)?;
        let cid = ComponentId::Dynamic(did);
        let now = self.change_tick;
        if !self.archetypes.archetypes[loc.archetype as usize].has_component(cid) {
            return Err(DynamicError::Missing);
        }
        let target = self.dyn_remove_target(loc.archetype, cid);
        let (src, dst) = index2(
            &mut self.archetypes.archetypes,
            loc.archetype as usize,
            target as usize,
        );
        // SAFETY: `target` is `src` minus `cid`; every column of `dst` is copied from `src`'s row,
        // the discarded dynamic component needs no drop, and the source row is then removed
        // without dropping.
        unsafe {
            let index = dst.allocate(entity.id, now);
            Archetype::move_shared_row(src, loc.index, dst, index, now);
            if let Some(moved) = src.remove(loc.index, false) {
                self.entities.meta[moved as usize].location.index = loc.index;
            }
            self.entities.meta[entity.id as usize].location = Location {
                archetype: target,
                index,
            };
        }
        Ok(())
    }

    /// Whether `entity` exists and has dynamic component `id`
    pub fn has_dynamic(&self, entity: Entity, id: u32) -> bool {
        let Ok((did, _, _)) = self.dynamic_slot(id) else {
            return false;
        };
        let Ok(loc) = self.entities.get(entity) else {
            return false;
        };
        self.archetypes.archetypes[loc.archetype as usize].has_component(ComponentId::Dynamic(did))
    }

    /// Borrow the bytes of `entity`'s dynamic component `id`
    ///
    /// Dynamically borrow-checked like [`EntityRef::get`](crate::EntityRef::get): panics if the
    /// column is uniquely borrowed. The slice is exactly `size` bytes and aligned to the
    /// registered alignment.
    pub fn get_dynamic(&self, entity: Entity, id: u32) -> Result<DynRef<'_>, DynamicError> {
        let (did, size, _) = self.dynamic_slot(id)?;
        let loc = self.entities.get(entity)?;
        let cid = ComponentId::Dynamic(did);
        let archetype = &self.archetypes.archetypes[loc.archetype as usize];
        let state = archetype.state_of(cid).ok_or(DynamicError::Missing)?;
        // SAFETY: the column exists, so the entity (flushed, since it has a component) has a
        // valid row; the borrow is released by `DynRef`'s drop.
        unsafe {
            archetype.borrow_raw(state);
            let ptr = archetype
                .get_ptr(cid, size, loc.index)
                .expect("component present");
            Ok(DynRef {
                archetype,
                state,
                ptr,
                size,
            })
        }
    }

    /// Mutably borrow the bytes of `entity`'s dynamic component `id`
    ///
    /// Panics if the column is already borrowed. If the component is tracked, `changed` is
    /// stamped with the world's current tick when the guard is created.
    pub fn get_dynamic_mut(&self, entity: Entity, id: u32) -> Result<DynRefMut<'_>, DynamicError> {
        let (did, size, _) = self.dynamic_slot(id)?;
        let loc = self.entities.get(entity)?;
        let cid = ComponentId::Dynamic(did);
        let archetype = &self.archetypes.archetypes[loc.archetype as usize];
        let state = archetype.state_of(cid).ok_or(DynamicError::Missing)?;
        // SAFETY: as in `get_dynamic`; the unique borrow taken here covers the tick array that
        // `stamp_changed` writes.
        unsafe {
            if !archetype.borrow_mut_raw(state) {
                panic!("dynamic component {id} already borrowed");
            }
            archetype.stamp_changed(state, loc.index, self.change_tick);
            let ptr = archetype
                .get_ptr(cid, size, loc.index)
                .expect("component present");
            Ok(DynRefMut {
                archetype,
                state,
                ptr,
                size,
            })
        }
    }

    /// Entities having all of the dynamic components `ids`
    ///
    /// Yielded in archetype order, then row order, which is deterministic for a given sequence of
    /// world operations. An unregistered id yields nothing. Does not borrow any component.
    pub fn query_dynamic(&self, ids: &[u32]) -> impl Iterator<Item = Entity> + '_ {
        let cids: Option<Vec<ComponentId>> = ids
            .iter()
            .map(|&id| {
                self.dynamic_slot(id)
                    .ok()
                    .map(|(did, _, _)| ComponentId::Dynamic(did))
            })
            .collect();
        let valid = cids.is_some();
        let cids = cids.unwrap_or_default();
        let meta = &self.entities.meta;
        self.archetypes
            .archetypes
            .iter()
            .filter(move |archetype| valid && cids.iter().all(|&c| archetype.has_component(c)))
            .flat_map(move |archetype| {
                archetype.ids().iter().map(move |&id| Entity {
                    id,
                    generation: meta[id as usize].generation,
                })
            })
    }

    fn dynamic_slot(&self, id: u32) -> Result<(DynamicId, usize, usize), DynamicError> {
        match self.dynamic.get(&id) {
            Some(slot) if slot.registered => Ok((
                DynamicId {
                    id,
                    generation: slot.generation,
                },
                slot.size,
                slot.align,
            )),
            _ => Err(DynamicError::Unregistered(id)),
        }
    }

    fn dyn_insert_target(&mut self, src: u32, cid: ComponentId, info: TypeInfo) -> u32 {
        if let Some(&target) = self.dyn_insert_edges.get(&(src, cid)) {
            return target;
        }
        let mut types = self.archetypes.archetypes[src as usize].types().to_vec();
        types.push(info);
        types.sort_unstable();
        let ids = types.iter().map(|ty| ty.id()).collect::<Box<_>>();
        let target = self.archetypes.get(ids, move || types);
        self.dyn_insert_edges.insert((src, cid), target);
        target
    }

    fn dyn_remove_target(&mut self, src: u32, cid: ComponentId) -> u32 {
        if let Some(&target) = self.dyn_remove_edges.get(&(src, cid)) {
            return target;
        }
        let types = self.archetypes.archetypes[src as usize]
            .types()
            .iter()
            .filter(|ty| ty.id() != cid)
            .copied()
            .collect::<Vec<_>>();
        let ids = types.iter().map(|ty| ty.id()).collect::<Box<_>>();
        let target = self.archetypes.get(ids, move || types);
        self.dyn_remove_edges.insert((src, cid), target);
        target
    }

    /// Panic unless every dynamic component in `components` matches this world's registry
    ///
    /// Bundles from other worlds (e.g. via [`take`](Self::take)) or hand-built `TypeInfo`s could
    /// otherwise smuggle in a layout that disagrees with the registered one, and the byte copies
    /// that follow would be out of bounds. Called before anything is mutated.
    pub(crate) fn check_dynamic_bundle(&self, components: &impl DynamicBundle) {
        // Statically typed bundles can only contain Rust component types.
        if components.key().is_some() {
            return;
        }
        let any_dynamic = components.with_ids(|ids| {
            ids.iter()
                .any(|id| matches!(id, ComponentId::Dynamic(_)))
        });
        if any_dynamic {
            self.check_dynamic_types(&components.type_info());
        }
    }

    /// See [`check_dynamic_bundle`](Self::check_dynamic_bundle)
    pub(crate) fn check_dynamic_types(&self, types: &[TypeInfo]) {
        for ty in types {
            if let ComponentId::Dynamic(did) = ty.id() {
                let ok = self.dynamic.get(&did.id).is_some_and(|slot| {
                    slot.registered
                        && slot.generation == did.generation
                        && slot.size == ty.layout().size()
                        && slot.align == ty.layout().align()
                });
                assert!(
                    ok,
                    "dynamic component {:?} is not registered in this world with this layout",
                    did
                );
            }
        }
    }
}

/// Shared borrow of a dynamic component's bytes, see [`World::get_dynamic`]
pub struct DynRef<'a> {
    archetype: &'a Archetype,
    state: usize,
    ptr: NonNull<u8>,
    size: usize,
}

// SAFETY: behaves like `&[u8]` plus an atomic borrow count release.
unsafe impl Send for DynRef<'_> {}
unsafe impl Sync for DynRef<'_> {}

impl Deref for DynRef<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: `ptr` addresses `size` initialized bytes of one row, valid while the shared
        // dynamic borrow held by `self` prevents any structural or unique access.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
    }
}

impl Drop for DynRef<'_> {
    fn drop(&mut self) {
        // SAFETY: balances the `borrow_raw` in `get_dynamic`
        unsafe { self.archetype.release_raw(self.state) };
    }
}

impl fmt::Debug for DynRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// Unique borrow of a dynamic component's bytes, see [`World::get_dynamic_mut`]
pub struct DynRefMut<'a> {
    archetype: &'a Archetype,
    state: usize,
    ptr: NonNull<u8>,
    size: usize,
}

// SAFETY: behaves like `&mut [u8]` plus an atomic borrow count release.
unsafe impl Send for DynRefMut<'_> {}
unsafe impl Sync for DynRefMut<'_> {}

impl Deref for DynRefMut<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: see `DynRef::deref`; the unique borrow additionally excludes other readers.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
    }
}

impl DerefMut for DynRefMut<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: the unique dynamic borrow held by `self` guarantees exclusive access to the
        // row's bytes, and `&mut self` prevents aliasing through this guard.
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size) }
    }
}

impl Drop for DynRefMut<'_> {
    fn drop(&mut self) {
        // SAFETY: balances the unique borrow taken in `get_dynamic_mut`
        unsafe { self.archetype.release_raw_mut(self.state) };
    }
}

impl fmt::Debug for DynRefMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}
