use crate::alloc::alloc::{alloc, dealloc, Layout};
use crate::alloc::boxed::Box;
use crate::alloc::{vec, vec::Vec};
use core::any::{type_name, TypeId};
use core::fmt;
use core::hash::{BuildHasher, BuildHasherDefault, Hasher};
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};

use hashbrown::HashMap;

use crate::borrow::AtomicBorrow;
use crate::query::Fetch;
use crate::tick::{ComponentTicks, Tick};
use crate::{Access, Component, ComponentRef, Query};

/// A collection of entities having the same component types
///
/// Accessing `Archetype`s is only required in niche cases. Typical use should go through the
/// [`World`](crate::World).
pub struct Archetype {
    types: Vec<TypeInfo>,
    type_ids: Box<[ComponentId]>,
    index: OrderedTypeIdMap<ComponentId, usize>,
    len: u32,
    entities: Box<[u32]>,
    /// One allocation per type, in the same order as `types`
    data: Box<[Data]>,
    /// Number of columns in `data` that have a tick array
    tracked: usize,
}

impl Archetype {
    fn assert_type_info(types: &[TypeInfo]) {
        types.windows(2).for_each(|x| match x[0].cmp(&x[1]) {
            core::cmp::Ordering::Less => (),
            #[cfg(debug_assertions)]
            core::cmp::Ordering::Equal => panic!(
                "attempted to allocate entity with duplicate {} components; \
                 each type must occur at most once!",
                x[0].type_name
            ),
            #[cfg(not(debug_assertions))]
            core::cmp::Ordering::Equal => panic!(
                "attempted to allocate entity with duplicate components; \
                 each type must occur at most once!"
            ),
            core::cmp::Ordering::Greater => panic!("type info is unsorted"),
        });
    }

    pub(crate) fn new(types: Vec<TypeInfo>) -> Self {
        let max_align = types.first().map_or(1, |ty| ty.layout.align());
        Self::assert_type_info(&types);
        let component_count = types.len();
        Self {
            index: OrderedTypeIdMap::new(types.iter().enumerate().map(|(i, ty)| (ty.id, i))),
            type_ids: types.iter().map(|ty| ty.id()).collect(),
            types,
            entities: Box::new([]),
            len: 0,
            data: (0..component_count)
                .map(|_| Data {
                    state: AtomicBorrow::new(),
                    storage: NonNull::new(max_align as *mut u8).unwrap(),
                    ticks: None,
                })
                .collect(),
            tracked: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        // Take the length before running any destructor. `Drop for Archetype`
        // calls this function, so if a component's `Drop` unwinds and the
        // length is still set, every component is destroyed a second time.
        // Components not reached before the unwind are leaked instead.
        let len = self.len;
        self.len = 0;
        for (ty, data) in self.types.iter().zip(&*self.data) {
            for index in 0..len {
                unsafe {
                    let removed = data.storage.as_ptr().add(index as usize * ty.layout.size());
                    (ty.drop)(removed);
                }
            }
        }
    }

    /// Whether this archetype contains `T` components
    pub fn has<T: Component>(&self) -> bool {
        self.has_component(ComponentId::of::<T>())
    }

    /// Whether this archetype contains components identified by `id`
    pub fn has_component(&self, id: ComponentId) -> bool {
        self.index.contains_key(&id)
    }

    /// Find the state index associated with `T`, if present
    pub(crate) fn get_state<T: Component>(&self) -> Option<usize> {
        self.index.get(&ComponentId::of::<T>()).copied()
    }

    /// Get the address of the first `T` component using an index from `get_state::<T>`
    ///
    /// # Safety
    /// - `state` must be associated with a component of type `T`
    pub(crate) unsafe fn get_base<T: Component>(&self, state: usize) -> NonNull<T> {
        debug_assert_eq!(self.types[state].id, ComponentId::of::<T>());

        unsafe {
            NonNull::new_unchecked(self.data.get_unchecked(state).storage.as_ptr().cast::<T>())
        }
    }

    /// Pointer to the first element of the tick array of column `state`, if it is tracked
    ///
    /// The array has [`capacity`](Self::capacity) slots, of which the first `len` are
    /// initialized. It may only be accessed while holding the column's dynamic borrow (shared to
    /// read, unique to write) or with exclusive access to the archetype.
    #[inline]
    pub(crate) fn get_ticks_base(&self, state: usize) -> Option<NonNull<ComponentTicks>> {
        self.data[state].ticks
    }

    /// The column index of `id`, if present
    pub(crate) fn state_of(&self, id: ComponentId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// Whether any column of this archetype is tracked
    #[inline]
    pub(crate) fn has_tracked(&self) -> bool {
        self.tracked != 0
    }

    /// Give column `state` a tick array if it lacks one, with `now` for all existing rows
    pub(crate) fn track_column(&mut self, state: usize, now: Tick) {
        if self.data[state].ticks.is_some() {
            return;
        }
        let cap = self.entities.len();
        // SAFETY: freed in `grow_exact`/`Drop` with the then-current capacity, which always
        // equals the capacity at allocation time because both change together.
        let ticks = unsafe { alloc_ticks(cap) };
        for i in 0..self.len as usize {
            // SAFETY: `i < len <= cap`, the array has `cap` slots
            unsafe {
                ticks.as_ptr().add(i).write(ComponentTicks {
                    added: now,
                    changed: now,
                });
            }
        }
        self.data[state].ticks = Some(ticks);
        self.tracked += 1;
    }

    /// Start tracking the column of `id`, if this archetype has one
    pub(crate) fn track_id(&mut self, id: ComponentId, now: Tick) {
        if let Some(&state) = self.index.get(&id) {
            self.track_column(state, now);
        }
    }

    /// The ticks of the `id` component at `index`, if the column is tracked
    ///
    /// # Safety
    /// `index < len`, and the column must not be uniquely borrowed.
    pub(crate) unsafe fn ticks_at(&self, id: ComponentId, index: u32) -> Option<ComponentTicks> {
        debug_assert!(index < self.len);
        let state = *self.index.get(&id)?;
        let ticks = self.data[state].ticks?;
        // SAFETY: `index < len`, so the slot is initialized; no unique borrow per the contract
        Some(unsafe { ticks.as_ptr().add(index as usize).read() })
    }

    /// Set `changed` of the `id` component at `index` to `now`, if the column is tracked
    ///
    /// # Safety
    /// `index < len`, and the caller must hold the column's unique borrow or `&mut` access.
    pub(crate) unsafe fn stamp_changed(&self, state: usize, index: u32, now: Tick) {
        debug_assert!(index < self.len);
        if let Some(ticks) = self.data[state].ticks {
            // SAFETY: `index < len`, so the slot is initialized; uniqueness per the contract
            unsafe { (*ticks.as_ptr().add(index as usize)).changed = now };
        }
    }

    /// Copy the ticks of every column `dst` has in common with `src` from row `si` of `src` to
    /// row `di` of `dst`, for the columns listed by `only` (all shared columns if `None`)
    ///
    /// Columns that `dst` does not track are skipped; if `dst` tracks a column that `src` does
    /// not, the destination is stamped with `now`.
    ///
    /// # Safety
    /// `si < src.len`, `di < dst.len`, neither archetype's relevant columns may be borrowed.
    pub(crate) unsafe fn copy_ticks_from(
        dst: &mut Archetype,
        di: u32,
        src: &Archetype,
        si: u32,
        only: Option<&[TypeInfo]>,
        now: Tick,
    ) {
        if dst.tracked == 0 {
            return;
        }
        debug_assert!(si < src.len && di < dst.len);
        let copy = |id: ComponentId| {
            let Some(&dst_state) = dst.index.get(&id) else {
                return;
            };
            let Some(dst_ticks) = dst.data[dst_state].ticks else {
                return;
            };
            let value = match src
                .index
                .get(&id)
                .and_then(|&state| src.data[state].ticks)
            {
                // SAFETY: `si < src.len`: initialized
                Some(src_ticks) => unsafe { src_ticks.as_ptr().add(si as usize).read() },
                None => ComponentTicks {
                    added: now,
                    changed: now,
                },
            };
            // SAFETY: `di < dst.len <= capacity`
            unsafe { dst_ticks.as_ptr().add(di as usize).write(value) };
        };
        match only {
            Some(list) => list.iter().for_each(|ty| copy(ty.id)),
            None => dst.types.iter().for_each(|ty| copy(ty.id)),
        }
    }

    /// Stamp `changed = now` (and `added = now` if `is_new`) for the `id` component at `index`
    ///
    /// # Safety
    /// `index < len`; no borrow of the column may be live (`&mut self` ensures exclusivity).
    pub(crate) unsafe fn stamp_in_place(
        &mut self,
        id: ComponentId,
        index: u32,
        now: Tick,
        is_new: bool,
    ) {
        debug_assert!(index < self.len);
        let Some(&state) = self.index.get(&id) else {
            return;
        };
        if let Some(ticks) = self.data[state].ticks {
            // SAFETY: `index < len`: initialized slot; `&mut self` is exclusive
            let t = unsafe { &mut *ticks.as_ptr().add(index as usize) };
            t.changed = now;
            if is_new {
                t.added = now;
            }
        }
    }

    /// Clamp every stored tick to at most `MAX_CHANGE_AGE` old relative to `now`
    pub(crate) fn check_ticks(&mut self, now: Tick) {
        let len = self.len as usize;
        for data in &*self.data {
            if let Some(ticks) = data.ticks {
                for i in 0..len {
                    // SAFETY: `i < len`: initialized; `&mut self` gives exclusive access
                    unsafe {
                        let t = &mut *ticks.as_ptr().add(i);
                        t.added.check_tick(now);
                        t.changed.check_tick(now);
                    }
                }
            }
        }
    }

    /// Copy row `si` of `src` into row `di` of `dst` (component data and ticks) for every
    /// component that both have
    ///
    /// Components only in `dst` are left untouched; components only in `src` are neither dropped
    /// nor moved, so the caller must account for them.
    ///
    /// # Safety
    /// `si < src.len`, `di < dst.len`, components must be moved (not duplicated) by the caller
    /// afterwards, e.g. by removing row `si` from `src` without dropping.
    pub(crate) unsafe fn move_shared_row(
        src: &Archetype,
        si: u32,
        dst: &mut Archetype,
        di: u32,
        now: Tick,
    ) {
        for (info, dst_data) in dst.types.iter().zip(&*dst.data) {
            let Some(&src_state) = src.index.get(&info.id) else {
                continue;
            };
            let size = info.layout.size();
            // SAFETY: both rows are in bounds of columns with identical layout for `info.id`
            // (the same id always has the same layout within a world), and distinct archetypes
            // never alias.
            unsafe {
                ptr::copy_nonoverlapping(
                    src.data[src_state].storage.as_ptr().add(si as usize * size),
                    dst_data.storage.as_ptr().add(di as usize * size),
                    size,
                );
            }
        }
        // SAFETY: same bounds contract as above
        unsafe { Self::copy_ticks_from(dst, di, src, si, None, now) };
    }

    /// Move every row of `self` into `dst`, which must have a subset of the components of `self`
    ///
    /// Rows keep their order and are appended to `dst`; returns the index of the first appended
    /// row. Components of `self` that `dst` lacks are discarded without being dropped, so the
    /// caller must ensure they need no drop. Entity IDs are moved along; the caller must fix up
    /// entity locations.
    ///
    /// # Safety
    /// Every type of `dst` must also be in `self` with an identical layout, and `self` must not
    /// hold droppable components that `dst` lacks.
    pub(crate) unsafe fn drain_into(&mut self, dst: &mut Archetype, now: Tick) -> u32 {
        let n = self.len;
        let base = dst.len;
        dst.reserve(n);
        for (info, dst_data) in dst.types.iter().zip(&*dst.data) {
            let src_state = *self.index.get(&info.id).expect("drain target is not a subset");
            let size = info.layout.size();
            // SAFETY: `dst` has capacity for `base + n` rows after `reserve`; `self` has `n`
            // initialized rows; the archetypes are distinct.
            unsafe {
                ptr::copy_nonoverlapping(
                    self.data[src_state].storage.as_ptr(),
                    dst_data.storage.as_ptr().add(base as usize * size),
                    n as usize * size,
                );
            }
        }
        dst.entities[base as usize..(base + n) as usize]
            .copy_from_slice(&self.entities[0..n as usize]);
        dst.len = base + n;
        // Tick arrays: copy where both sides are tracked, stamp `now` otherwise.
        for (info, dst_data) in dst.types.iter().zip(&*dst.data) {
            let Some(dst_ticks) = dst_data.ticks else {
                continue;
            };
            let src_ticks = self.index.get(&info.id).and_then(|&s| self.data[s].ticks);
            for i in 0..n as usize {
                let value = match src_ticks {
                    // SAFETY: `i < n = self.len`: initialized
                    Some(t) => unsafe { t.as_ptr().add(i).read() },
                    None => ComponentTicks {
                        added: now,
                        changed: now,
                    },
                };
                // SAFETY: `base + i < dst.len <= capacity`
                unsafe { dst_ticks.as_ptr().add(base as usize + i).write(value) };
            }
        }
        self.len = 0;
        base
    }

    /// Whole-column byte view of a dynamic component, with a shared dynamic borrow
    ///
    /// Returns `None` if `id` is not a [`ComponentId::Dynamic`] present in this archetype.
    /// Panics if the column is already uniquely borrowed.
    pub fn dynamic_column(&self, id: ComponentId) -> Option<DynColumn<'_>> {
        if !matches!(id, ComponentId::Dynamic(_)) {
            return None;
        }
        let state = *self.index.get(&id)?;
        if !self.data[state].state.borrow() {
            panic!("dynamic component {:?} already borrowed uniquely", id);
        }
        let size = self.types[state].layout.size();
        Some(DynColumn {
            archetype: self,
            state,
            ptr: self.data[state].storage,
            len: self.len as usize * size,
        })
    }

    /// Borrow all components of a single type from these entities, if present
    ///
    /// `T` must be a shared or unique reference to a component type.
    ///
    /// Useful for efficient serialization.
    pub fn get<'a, T: ComponentRef<'a>>(&'a self) -> Option<T::Column> {
        T::get_column(self)
    }

    pub(crate) fn borrow<T: Component>(&self, state: usize) {
        assert_eq!(self.types[state].id, ComponentId::of::<T>());

        if !self.data[state].state.borrow() {
            panic!("{} already borrowed uniquely", type_name::<T>());
        }
    }

    pub(crate) unsafe fn borrow_raw(&self, state: usize) {
        if !self.data[state].state.borrow() {
            panic!("state index {} already borrowed uniquely", state);
        }
    }

    pub(crate) fn borrow_mut<T: Component>(&self, state: usize) {
        assert_eq!(self.types[state].id, ComponentId::of::<T>());

        if !self.data[state].state.borrow_mut() {
            panic!("{} already borrowed", type_name::<T>());
        }
    }

    pub(crate) fn release<T: Component>(&self, state: usize) {
        assert_eq!(self.types[state].id, ComponentId::of::<T>());
        self.data[state].state.release();
    }

    pub(crate) fn release_mut<T: Component>(&self, state: usize) {
        assert_eq!(self.types[state].id, ComponentId::of::<T>());
        self.data[state].state.release_mut();
    }

    /// Try to take the unique borrow of column `state`; returns whether it succeeded
    pub(crate) unsafe fn borrow_mut_raw(&self, state: usize) -> bool {
        self.data[state].state.borrow_mut()
    }

    pub(crate) unsafe fn release_raw(&self, state: usize) {
        self.data[state].state.release();
    }

    pub(crate) unsafe fn release_raw_mut(&self, state: usize) {
        self.data[state].state.release_mut();
    }

    /// Number of entities in this archetype
    #[inline]
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether this archetype contains no entities
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn entities(&self) -> NonNull<u32> {
        unsafe { NonNull::new_unchecked(self.entities.as_ptr() as *mut _) }
    }

    pub(crate) fn entity_id(&self, index: u32) -> u32 {
        self.entities[index as usize]
    }

    #[inline]
    pub(crate) fn set_entity_id(&mut self, index: usize, id: u32) {
        self.entities[index] = id;
    }

    pub(crate) fn types(&self) -> &[TypeInfo] {
        &self.types
    }

    pub(crate) fn type_ids(&self) -> &[ComponentId] {
        &self.type_ids
    }

    /// Enumerate the types of the components of entities stored in this archetype.
    ///
    /// Convenient for dispatching logic which needs to be performed on sets of type ids.  For
    /// example, suppose you're building a scripting system, and you want to integrate the scripting
    /// language with your ECS. This functionality allows you to iterate through all of the
    /// archetypes of the world with [`World::archetypes()`](crate::World::archetypes()) and extract
    /// all possible combinations of component types which are currently stored in the `World`.
    /// From there, you can then create a mapping of archetypes to wrapper objects for your
    /// scripting language that provide functionality based off of the components of any given
    /// [`Entity`], and bind them onto an [`Entity`] when passed into your scripting language by
    /// looking up the [`Entity`]'s archetype using
    /// [`EntityRef::component_types`](crate::EntityRef::component_types).
    ///
    /// [`Entity`]: crate::Entity
    pub fn component_types(&self) -> impl ExactSizeIterator<Item = ComponentId> + '_ {
        self.types.iter().map(|typeinfo| typeinfo.id)
    }

    /// `index` must be in-bounds or just past the end
    pub(crate) unsafe fn get_ptr(
        &self,
        ty: ComponentId,
        size: usize,
        index: u32,
    ) -> Option<NonNull<u8>> {
        debug_assert!(index <= self.len);
        Some(NonNull::new_unchecked(
            self.data
                .get_unchecked(*self.index.get(&ty)?)
                .storage
                .as_ptr()
                .add(size * index as usize)
                .cast::<u8>(),
        ))
    }

    /// Every type must be written immediately after this call
    ///
    /// The ticks of every tracked column in the new row are set to `now` (added and changed).
    pub(crate) unsafe fn allocate(&mut self, id: u32, now: Tick) -> u32 {
        if self.len as usize == self.entities.len() {
            self.grow(64);
        }

        let index = self.len;
        self.entities[index as usize] = id;
        self.len += 1;
        self.stamp_new_row(index, now);
        index
    }

    /// Set added and changed of every tracked column at `index` to `now`
    #[inline]
    fn stamp_new_row(&mut self, index: u32, now: Tick) {
        if self.tracked == 0 {
            return;
        }
        debug_assert!(index < self.capacity());
        for data in &*self.data {
            if let Some(ticks) = data.ticks {
                // SAFETY: `index < capacity` (the entities array was grown by the caller if
                // necessary) and a tick array always has `capacity` slots; `&mut self` gives
                // exclusive access.
                unsafe {
                    ticks.as_ptr().add(index as usize).write(ComponentTicks {
                        added: now,
                        changed: now,
                    });
                }
            }
        }
    }

    pub(crate) unsafe fn set_len(&mut self, len: u32) {
        debug_assert!(len <= self.capacity());
        self.len = len;
    }

    pub(crate) fn reserve(&mut self, additional: u32) {
        if additional > (self.capacity() - self.len()) {
            let increment = additional - (self.capacity() - self.len());
            self.grow(increment.max(64));
        }
    }

    pub(crate) fn capacity(&self) -> u32 {
        self.entities.len() as u32
    }

    /// Increase capacity by at least `min_increment`
    fn grow(&mut self, min_increment: u32) {
        // Double capacity or increase it by `min_increment`, whichever is larger.
        self.grow_exact(self.capacity().max(min_increment))
    }

    /// Increase capacity by exactly `increment`
    fn grow_exact(&mut self, increment: u32) {
        let old_count = self.len as usize;
        let old_cap = self.entities.len();
        let new_cap = self.entities.len() + increment as usize;
        let mut new_entities = vec![!0; new_cap].into_boxed_slice();
        new_entities[0..old_count].copy_from_slice(&self.entities[0..old_count]);
        // `self.entities.len()` is the capacity `Drop` frees the columns with, so it is only
        // replaced together with `self.data` below: if building the new columns panics
        // (capacity overflow), the archetype still describes the old allocations.

        let new_data = self
            .types
            .iter()
            .zip(&*self.data)
            .map(|(info, old)| {
                let storage = if info.layout.size() == 0 {
                    NonNull::new(info.layout.align() as *mut u8).unwrap()
                } else {
                    let layout = info
                        .layout
                        .size()
                        .checked_mul(new_cap)
                        .and_then(|size| Layout::from_size_align(size, info.layout.align()).ok())
                        .expect("capacity overflow");
                    unsafe {
                        let mem = alloc(layout);
                        let mem = NonNull::new(mem)
                            .unwrap_or_else(|| alloc::alloc::handle_alloc_error(layout));
                        ptr::copy_nonoverlapping(
                            old.storage.as_ptr(),
                            mem.as_ptr(),
                            info.layout.size() * old_count,
                        );
                        mem
                    }
                };
                let ticks = old.ticks.map(|old_ticks| unsafe {
                    // SAFETY: `old_ticks` is a tick array of `old_cap` slots whose first
                    // `old_count <= old_cap` entries are initialized (tracked-column invariant);
                    // the new array has `new_cap >= old_count` slots and does not overlap.
                    let new_ticks = alloc_ticks(new_cap);
                    ptr::copy_nonoverlapping(old_ticks.as_ptr(), new_ticks.as_ptr(), old_count);
                    new_ticks
                });
                Data {
                    state: AtomicBorrow::new(), // &mut self guarantees no outstanding borrows
                    storage,
                    ticks,
                }
            })
            .collect::<Box<[_]>>();

        // Now that we've successfully constructed a replacement, we can
        // deallocate the old column data without risking `self.data` being left
        // partially deallocated on OOM.
        if old_cap > 0 {
            for (info, data) in self.types.iter().zip(&*self.data) {
                if let Some(ticks) = data.ticks {
                    // SAFETY: allocated with `old_cap` slots by `alloc_ticks`
                    unsafe { free_ticks(ticks, old_cap) };
                }
                if info.layout.size() == 0 {
                    continue;
                }
                unsafe {
                    dealloc(
                        data.storage.as_ptr(),
                        Layout::from_size_align(info.layout.size() * old_cap, info.layout.align())
                            .unwrap(),
                    );
                }
            }
        }

        self.entities = new_entities;
        self.data = new_data;
    }

    /// Returns the ID of the entity moved into `index`, if any
    pub(crate) unsafe fn remove(&mut self, index: u32, drop: bool) -> Option<u32> {
        let last = self.len - 1;
        // The guard backfills every type from `last` and commits the length, on
        // both the straight-line path and on unwind. Destructors run before it,
        // so if one unwinds, the components of the removed entity that were not
        // reached are overwritten by the backfill and leaked, which is safe.
        struct Guard<'a> {
            archetype: &'a mut Archetype,
            index: u32,
            last: u32,
        }
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if self.index != self.last {
                    for (ty, data) in self.archetype.types.iter().zip(&*self.archetype.data) {
                        unsafe {
                            let removed = data
                                .storage
                                .as_ptr()
                                .add(self.index as usize * ty.layout.size());
                            let moved = data
                                .storage
                                .as_ptr()
                                .add(self.last as usize * ty.layout.size());
                            ptr::copy_nonoverlapping(moved, removed, ty.layout.size());
                            if let Some(ticks) = data.ticks {
                                // SAFETY: `index < last < len <= capacity`, so both slots are
                                // initialized and distinct.
                                ptr::copy_nonoverlapping(
                                    ticks.as_ptr().add(self.last as usize),
                                    ticks.as_ptr().add(self.index as usize),
                                    1,
                                );
                            }
                        }
                    }
                    self.archetype.entities[self.index as usize] =
                        self.archetype.entities[self.last as usize];
                }
                self.archetype.len = self.last;
            }
        }
        {
            let guard = Guard {
                archetype: self,
                index,
                last,
            };
            if drop {
                for (ty, data) in guard.archetype.types.iter().zip(&*guard.archetype.data) {
                    let removed = data.storage.as_ptr().add(index as usize * ty.layout.size());
                    (ty.drop)(removed);
                }
            }
        }

        if index != last {
            Some(self.entities[last as usize])
        } else {
            None
        }
    }

    /// Returns the ID of the entity moved into `index`, if any
    pub(crate) unsafe fn move_to(
        &mut self,
        index: u32,
        mut f: impl FnMut(*mut u8, ComponentId, usize),
    ) -> Option<u32> {
        let last = self.len - 1;
        for (ty, data) in self.types.iter().zip(&*self.data) {
            let moved_out = data.storage.as_ptr().add(index as usize * ty.layout.size());
            f(moved_out, ty.id(), ty.layout().size());
            if index != last {
                let moved = data.storage.as_ptr().add(last as usize * ty.layout.size());
                ptr::copy_nonoverlapping(moved, moved_out, ty.layout.size());
                if let Some(ticks) = data.ticks {
                    // SAFETY: `index < last < len <= capacity`: both slots initialized, distinct.
                    ptr::copy_nonoverlapping(
                        ticks.as_ptr().add(last as usize),
                        ticks.as_ptr().add(index as usize),
                        1,
                    );
                }
            }
        }
        self.len -= 1;
        if index != last {
            self.entities[index as usize] = self.entities[last as usize];
            Some(self.entities[last as usize])
        } else {
            None
        }
    }

    pub(crate) unsafe fn put_raw(
        &mut self,
        component: *mut u8,
        ty: ComponentId,
        size: usize,
        index: u32,
    ) {
        let ptr = self
            .get_ptr(ty, size, index)
            .unwrap()
            .as_ptr()
            .cast::<u8>();
        ptr::copy_nonoverlapping(component, ptr, size);
    }

    /// How, if at all, `Q` will access entities in this archetype
    pub fn access<Q: Query>(&self) -> Option<Access> {
        Q::Fetch::access(self)
    }

    /// Determine whether this archetype would satisfy the query `Q`
    pub fn satisfies<Q: Query>(&self) -> bool {
        self.access::<Q>().is_some()
    }

    /// Add components from another archetype with identical components
    ///
    /// # Safety
    ///
    /// Component types must match exactly.
    ///
    /// Tracked columns of `self` get `now` as added and changed tick for the merged rows,
    /// regardless of any ticks `other` may have.
    pub(crate) unsafe fn merge(&mut self, mut other: Archetype, now: Tick) {
        self.reserve(other.len);
        let base = self.len;
        for ((info, dst), src) in self.types.iter().zip(&*self.data).zip(&*other.data) {
            dst.storage
                .as_ptr()
                .add(self.len as usize * info.layout.size())
                .copy_from_nonoverlapping(
                    src.storage.as_ptr(),
                    other.len as usize * info.layout.size(),
                )
        }
        // Capacity now covers `base + other.len`, so the stamped rows are in bounds.
        for i in 0..other.len {
            self.stamp_new_row(base + i, now);
        }
        self.len += other.len;
        other.len = 0;
    }

    /// Raw IDs of the entities in this archetype
    ///
    /// Convertible into [`Entity`](crate::Entity)s with
    /// [`World::find_entity_from_id()`](crate::World::find_entity_from_id). Useful for efficient
    /// serialization.
    #[inline]
    pub fn ids(&self) -> &[u32] {
        &self.entities[0..self.len as usize]
    }
}

impl Drop for Archetype {
    fn drop(&mut self) {
        self.clear();
        if self.entities.is_empty() {
            return;
        }
        for (info, data) in self.types.iter().zip(&*self.data) {
            if let Some(ticks) = data.ticks {
                // SAFETY: tick arrays always have exactly `entities.len()` slots
                unsafe { free_ticks(ticks, self.entities.len()) };
            }
            if info.layout.size() != 0 {
                unsafe {
                    dealloc(
                        data.storage.as_ptr(),
                        Layout::from_size_align_unchecked(
                            info.layout.size() * self.entities.len(),
                            info.layout.align(),
                        ),
                    );
                }
            }
        }
    }
}

struct Data {
    state: AtomicBorrow,
    storage: NonNull<u8>,
    /// Added/changed ticks, one per row, if this column is tracked. When present, always has
    /// exactly `Archetype::capacity()` slots (dangling if that is zero) and slots `0..len` are
    /// initialized. Unique access to the column (via `state`) implies unique access to these.
    ticks: Option<NonNull<ComponentTicks>>,
}

fn ticks_layout(cap: usize) -> Layout {
    Layout::array::<ComponentTicks>(cap).expect("capacity overflow")
}

/// Allocate an uninitialized tick array of `cap` slots
///
/// # Safety
/// Must be released with `free_ticks` using the same `cap`.
unsafe fn alloc_ticks(cap: usize) -> NonNull<ComponentTicks> {
    if cap == 0 {
        return NonNull::dangling();
    }
    let layout = ticks_layout(cap);
    // SAFETY: `cap != 0` and `ComponentTicks` is not zero-sized, so `layout.size() != 0`
    let mem = unsafe { alloc(layout) };
    NonNull::new(mem.cast::<ComponentTicks>())
        .unwrap_or_else(|| alloc::alloc::handle_alloc_error(layout))
}

/// # Safety
/// `ptr` must come from `alloc_ticks(cap)`
unsafe fn free_ticks(ptr: NonNull<ComponentTicks>, cap: usize) {
    if cap != 0 {
        // SAFETY: caller contract
        unsafe { dealloc(ptr.as_ptr().cast(), ticks_layout(cap)) };
    }
}

/// A hasher optimized for hashing a single TypeId.
///
/// TypeId is already thoroughly hashed, so there's no reason to hash it again.
/// Just leave the bits unchanged.
#[derive(Default)]
pub struct TypeIdHasher {
    hash: u64,
}

impl Hasher for TypeIdHasher {
    fn write_u64(&mut self, n: u64) {
        // Only a single value can be hashed, so the old hash should be zero.
        debug_assert_eq!(self.hash, 0);
        self.hash = n;
    }

    // Tolerate TypeId being either u64 or u128.
    fn write_u128(&mut self, n: u128) {
        debug_assert_eq!(self.hash, 0);
        self.hash = n as u64;
    }

    fn write(&mut self, bytes: &[u8]) {
        debug_assert_eq!(self.hash, 0);

        // This will only be called if TypeId is neither u64 nor u128, which is not anticipated.
        // In that case we'll just fall back to using a different hash implementation.
        let mut hasher = foldhash::fast::FixedState::with_seed(0xb334867b740a29a5).build_hasher();
        hasher.write(bytes);
        self.hash = hasher.finish();
    }

    fn finish(&self) -> u64 {
        self.hash
    }
}

/// A HashMap with TypeId keys
///
/// Because TypeId is already a fully-hashed u64 (including data in the high seven bits,
/// which hashbrown needs), there is no need to hash it again. Instead, this uses the much
/// faster no-op hash.
pub type TypeIdMap<V> = HashMap<TypeId, V, BuildHasherDefault<TypeIdHasher>>;

struct OrderedTypeIdMap<K, V>(Box<[(K, V)]>);

impl<K: Ord + Copy, V> OrderedTypeIdMap<K, V> {
    fn new(iter: impl Iterator<Item = (K, V)>) -> Self {
        let mut vals = iter.collect::<Box<[_]>>();
        vals.sort_unstable_by_key(|(id, _)| *id);
        Self(vals)
    }

    fn search(&self, id: &K) -> Option<usize> {
        self.0.binary_search_by_key(id, |(id, _)| *id).ok()
    }

    fn contains_key(&self, id: &K) -> bool {
        self.search(id).is_some()
    }

    fn get(&self, id: &K) -> Option<&V> {
        self.search(id).map(move |idx| &self.0[idx].1)
    }
}

/// Identifies a kind of component: either a Rust type, or a runtime-registered dynamic component
///
/// Ordering is `Rust(_)` before `Dynamic(_)`, then by the contained id; it is only meaningful for
/// determinism, not for any semantic purpose.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComponentId {
    /// A component backed by a Rust type
    Rust(TypeId),
    /// A component whose layout was registered at runtime, see
    /// [`World::register_dynamic`](crate::World::register_dynamic)
    Dynamic(DynamicId),
}

impl ComponentId {
    /// The id of the Rust component type `T`
    pub fn of<T: 'static>() -> Self {
        ComponentId::Rust(TypeId::of::<T>())
    }
}

impl From<TypeId> for ComponentId {
    fn from(id: TypeId) -> Self {
        ComponentId::Rust(id)
    }
}

/// Identifies a dynamic component: the user's `id`, plus a generation that changes every time the
/// same `id` is registered again, so storage built for an old layout is never reused
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DynamicId {
    /// The id chosen by the user at registration
    pub id: u32,
    /// Bumped on every re-registration of `id`
    pub generation: u32,
}

/// Metadata required to store a component.
///
/// All told, this means a [`ComponentId`], to be able to dynamically name/check the component type; a
/// [`Layout`], so that we know how to allocate memory for this component type; and a drop function
/// which internally calls [`core::ptr::drop_in_place`] with the correct type parameter.
#[derive(Debug, Copy, Clone)]
pub struct TypeInfo {
    id: ComponentId,
    layout: Layout,
    drop: unsafe fn(*mut u8),
    #[cfg(debug_assertions)]
    type_name: &'static str,
}

impl TypeInfo {
    /// Construct a `TypeInfo` directly from the static type.
    pub fn of<T: 'static>() -> Self {
        unsafe fn drop_ptr<T>(x: *mut u8) {
            x.cast::<T>().drop_in_place()
        }

        Self {
            id: ComponentId::of::<T>(),
            layout: Layout::new::<T>(),
            drop: drop_ptr::<T>,
            #[cfg(debug_assertions)]
            type_name: core::any::type_name::<T>(),
        }
    }

    /// Construct a `TypeInfo` from its components. This is useful in the rare case that you have
    /// some kind of pointer to raw bytes/erased memory holding a component type, coming from a
    /// source unrelated to hecs, and you want to treat it as an insertable component by
    /// implementing the `DynamicBundle` API.
    pub fn from_parts(
        id: impl Into<ComponentId>,
        layout: Layout,
        drop: unsafe fn(*mut u8),
    ) -> Self {
        Self {
            id: id.into(),
            layout,
            drop,
            #[cfg(debug_assertions)]
            type_name: "<unknown> (TypeInfo constructed from parts)",
        }
    }

    /// Construct a `TypeInfo` for a runtime-defined (dynamic) component of `size` bytes aligned to
    /// `align`.
    ///
    /// Dynamic components are plain bytes: dropping them is a no-op.
    pub(crate) fn dynamic(id: DynamicId, size: usize, align: usize) -> Self {
        unsafe fn drop_noop(_: *mut u8) {}

        Self {
            id: ComponentId::Dynamic(id),
            layout: Layout::from_size_align(size, align).expect("invalid dynamic component layout"),
            drop: drop_noop,
            #[cfg(debug_assertions)]
            type_name: "<dynamic component>",
        }
    }

    /// Access the [`ComponentId`] for this component type.
    pub fn id(&self) -> ComponentId {
        self.id
    }

    /// Access the `Layout` of this component type.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Directly call the destructor on a pointer to data of this component type.
    ///
    /// # Safety
    ///
    /// All of the caveats of [`core::ptr::drop_in_place`] apply, with the additional requirement
    /// that this method is being called on a pointer to an object of the correct component type.
    pub unsafe fn drop(&self, data: *mut u8) {
        (self.drop)(data)
    }

    /// Get the function pointer encoding the destructor for the component type this `TypeInfo`
    /// represents.
    pub fn drop_shim(&self) -> unsafe fn(*mut u8) {
        self.drop
    }
}

impl PartialOrd for TypeInfo {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TypeInfo {
    /// Order by alignment, descending. Ties broken with TypeId.
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.layout
            .align()
            .cmp(&other.layout.align())
            .reverse()
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialEq for TypeInfo {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeInfo {}

/// Shared reference to a single column of component data in an [`Archetype`]
pub struct ArchetypeColumn<'a, T: Component> {
    archetype: &'a Archetype,
    column: &'a [T],
}

impl<'a, T: Component> ArchetypeColumn<'a, T> {
    pub(crate) fn new(archetype: &'a Archetype) -> Option<Self> {
        let state = archetype.get_state::<T>()?;
        let ptr = unsafe { archetype.get_base::<T>(state) };
        // Borrow first: creating the slice while the column is uniquely borrowed would alias it.
        archetype.borrow::<T>(state);
        let column = unsafe { core::slice::from_raw_parts(ptr.as_ptr(), archetype.len() as usize) };
        Some(Self { archetype, column })
    }
}

impl<T: Component> Deref for ArchetypeColumn<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.column
    }
}

impl<T: Component> Drop for ArchetypeColumn<'_, T> {
    fn drop(&mut self) {
        let state = self.archetype.get_state::<T>().unwrap();
        self.archetype.release::<T>(state);
    }
}

impl<T: Component> Clone for ArchetypeColumn<'_, T> {
    fn clone(&self) -> Self {
        let state = self.archetype.get_state::<T>().unwrap();
        self.archetype.borrow::<T>(state);
        Self {
            archetype: self.archetype,
            column: self.column,
        }
    }
}

impl<T: Component + fmt::Debug> fmt::Debug for ArchetypeColumn<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.column.fmt(f)
    }
}

/// Unique reference to a single column of component data in an [`Archetype`]
pub struct ArchetypeColumnMut<'a, T: Component> {
    archetype: &'a Archetype,
    column: &'a mut [T],
}

impl<'a, T: Component> ArchetypeColumnMut<'a, T> {
    pub(crate) fn new(archetype: &'a Archetype) -> Option<Self> {
        let state = archetype.get_state::<T>()?;
        let ptr = unsafe { archetype.get_base::<T>(state) };
        // Borrow first: creating the slice while the column is borrowed would alias it.
        archetype.borrow_mut::<T>(state);
        let column =
            unsafe { core::slice::from_raw_parts_mut(ptr.as_ptr(), archetype.len() as usize) };
        Some(Self { archetype, column })
    }
}

impl<T: Component> Deref for ArchetypeColumnMut<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.column
    }
}

impl<T: Component> DerefMut for ArchetypeColumnMut<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.column
    }
}

impl<T: Component> Drop for ArchetypeColumnMut<'_, T> {
    fn drop(&mut self) {
        let state = self.archetype.get_state::<T>().unwrap();
        self.archetype.release_mut::<T>(state);
    }
}

impl<T: Component + fmt::Debug> fmt::Debug for ArchetypeColumnMut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.column.fmt(f)
    }
}

/// Shared, borrow-checked byte view of a whole dynamic component column of an [`Archetype`]
///
/// Contains `archetype.len() * size` bytes; row `i` starts at `i * size` and is aligned to the
/// component's registered alignment.
pub struct DynColumn<'a> {
    archetype: &'a Archetype,
    state: usize,
    ptr: NonNull<u8>,
    len: usize,
}

impl Deref for DynColumn<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: `ptr` is the column storage (non-null, aligned for u8); `len = rows * size`
        // bytes of it are initialized (every row was written on insert/move) and the shared
        // dynamic borrow held by `self` prevents unique access for the lifetime of `self`.
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for DynColumn<'_> {
    fn drop(&mut self) {
        // SAFETY: balances the `borrow` in `dynamic_column`
        unsafe { self.archetype.release_raw(self.state) };
    }
}
