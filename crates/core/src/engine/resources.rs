//! Type-indexed resource store.
//!
//! Stores at most one instance of any `Send + Sync + 'static` type in a type map. Every value
//! sits behind its own `RwLock` so parallel systems ([`ParCtx`](crate::engine::parallel::ParCtx))
//! can borrow different resources at once; exclusive code (`&mut Resources`) never locks.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

use hecs::Tick;

/// One stored resource: the value behind its lock plus the tick it was last changed at.
struct Slot {
    lock: RwLock<Box<dyn Any + Send + Sync>>,
    changed: AtomicU32,
    name: &'static str,
}

impl Slot {
    fn new<T: Send + Sync + 'static>(value: T, now: Tick) -> Self {
        Self { lock: RwLock::new(Box::new(value)), changed: AtomicU32::new(now.get()), name: std::any::type_name::<T>() }
    }
}

/// Shared borrow of a resource (a read guard that derefs to `T`).
pub struct Res<'a, T: 'static> {
    guard: RwLockReadGuard<'a, Box<dyn Any + Send + Sync>>,
    _p: PhantomData<&'a T>,
}

/// Exclusive borrow of a resource (a write guard that derefs to `T`).
///
/// Change detection: the resource is stamped as changed when this guard is dereferenced
/// mutably (`DerefMut`), not on the mere borrow, so `let r = cx.res_mut::<T>();` followed by
/// read-only use does not count as a change.
pub struct ResMut<'a, T: 'static> {
    guard: RwLockWriteGuard<'a, Box<dyn Any + Send + Sync>>,
    changed: &'a AtomicU32,
    tick: Tick,
    _p: PhantomData<&'a mut T>,
}

impl<T: 'static> Deref for Res<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        let any: &(dyn Any + Send + Sync) = &**self.guard;
        any.downcast_ref::<T>().expect("type map invariant")
    }
}

impl<T: 'static> Deref for ResMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        let any: &(dyn Any + Send + Sync) = &**self.guard;
        any.downcast_ref::<T>().expect("type map invariant")
    }
}

impl<T: 'static> DerefMut for ResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.changed.store(self.tick.get(), Ordering::Relaxed);
        let any: &mut (dyn Any + Send + Sync) = &mut **self.guard;
        any.downcast_mut::<T>().expect("type map invariant")
    }
}

impl<T: std::fmt::Debug + 'static> std::fmt::Debug for Res<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: std::fmt::Debug + 'static> std::fmt::Debug for ResMut<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

/// Type map: one value per type.
pub struct Resources {
    map: HashMap<TypeId, Slot>,
    /// The tick that exclusive writes (`insert`, `get_mut`, `get_or_insert_with`) stamp; kept in
    /// sync with the world's change tick by the engine and the schedule ([`set_tick`](Self::set_tick)).
    now: Tick,
}

impl Default for Resources {
    fn default() -> Self {
        Self::new()
    }
}

impl Resources {
    /// Creates an empty resource container.
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            now: Tick::new(1),
        }
    }

    /// Sets the tick stamped by exclusive writes. The engine keeps it equal to the world's
    /// change tick; call it before mutating resources if you drive `Resources` yourself.
    pub fn set_tick(&mut self, now: Tick) {
        self.now = now;
    }

    /// The tick at which the resource `T` was last inserted or changed (see [`ResMut`] for
    /// what counts as a change), or `None` if absent. Compare with
    /// `tick.is_newer_than(last_run, this_run)`.
    pub fn changed_tick<T: 'static>(&self) -> Option<Tick> {
        self.map.get(&TypeId::of::<T>()).map(|s| Tick::new(s.changed.load(Ordering::Relaxed)))
    }

    /// Whether `T` was inserted or changed since `last_run` (as of `this_run`); `false` if absent.
    pub fn is_changed<T: 'static>(&self, last_run: Tick, this_run: Tick) -> bool {
        self.changed_tick::<T>().is_some_and(|t| t.is_newer_than(last_run, this_run))
    }

    /// Clamps stored change ticks that have become too old to compare reliably (wraparound
    /// handling, like the world's `maybe_check_change_ticks`). Called by the engine every frame.
    pub fn check_ticks(&mut self, now: Tick) {
        for slot in self.map.values_mut() {
            let mut t = Tick::new(*slot.changed.get_mut());
            if t.check_tick(now) {
                *slot.changed.get_mut() = t.get();
            }
        }
    }

    /// Inserts a resource of type `T`, returning the previous value of that type if any.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Slot::new(value, self.now))
            .and_then(|slot| {
                let boxed = slot.lock.into_inner().unwrap_or_else(|e| e.into_inner());
                (boxed as Box<dyn Any>).downcast::<T>().ok().map(|b| *b)
            })
    }

    /// Shared borrow of the resource of type `T`, or `None` if not present. Panics if the
    /// resource is exclusively borrowed or its lock is poisoned (a parallel system whose
    /// access declaration is correct cannot cause that).
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        self.read::<T>()
    }

    /// Mutable reference to the resource of type `T`, or `None` if not present (no lock).
    /// A plain `&mut T` cannot observe writes, so this counts as a change immediately.
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        let slot = self.map.get_mut(&TypeId::of::<T>())?;
        *slot.changed.get_mut() = self.now.get();
        let boxed = slot.lock.get_mut().unwrap_or_else(|_| poisoned::<T>());
        let any: &mut (dyn Any + Send + Sync) = &mut **boxed;
        any.downcast_mut::<T>()
    }

    /// Shared borrow through `try_read` (used by `ParCtx`). Panics on contention or poison.
    pub(crate) fn read<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        let slot = self.map.get(&TypeId::of::<T>())?;
        match slot.lock.try_read() {
            Ok(guard) => Some(Res { guard, _p: PhantomData }),
            Err(TryLockError::WouldBlock) => panic!("resource `{}` is already mutably borrowed", std::any::type_name::<T>()),
            Err(TryLockError::Poisoned(_)) => poisoned::<T>(),
        }
    }

    /// Exclusive borrow through `try_write` (used by `ParCtx`), stamping `tick` on mutable
    /// deref. Panics on contention or poison.
    pub(crate) fn write<T: Send + Sync + 'static>(&self, tick: Tick) -> Option<ResMut<'_, T>> {
        let slot = self.map.get(&TypeId::of::<T>())?;
        match slot.lock.try_write() {
            Ok(guard) => Some(ResMut { guard, changed: &slot.changed, tick, _p: PhantomData }),
            Err(TryLockError::WouldBlock) => panic!("resource `{}` is already borrowed", std::any::type_name::<T>()),
            Err(TryLockError::Poisoned(_)) => poisoned::<T>(),
        }
    }

    /// Removes and returns the resource of type `T`, or `None` if not present.
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.map.remove(&TypeId::of::<T>()).and_then(|slot| {
            let boxed = slot.lock.into_inner().unwrap_or_else(|_| poisoned::<T>());
            (boxed as Box<dyn Any>).downcast::<T>().ok().map(|b| *b)
        })
    }

    /// Returns `true` if a resource of type `T` is present.
    pub fn contains<T: 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }

    /// Returns a mutable reference to the resource of type `T`, inserting the result of `f`
    /// if it does not yet exist. Counts as a change (like [`get_mut`](Self::get_mut)).
    pub fn get_or_insert_with<T: Send + Sync + 'static>(&mut self, f: impl FnOnce() -> T) -> &mut T {
        let now = self.now;
        let slot = self
            .map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Slot::new(f(), now));
        *slot.changed.get_mut() = now.get();
        let boxed = slot.lock.get_mut().unwrap_or_else(|_| poisoned::<T>());
        let any: &mut (dyn Any + Send + Sync) = &mut **boxed;
        any.downcast_mut::<T>().expect("type map invariant")
    }

    /// `(type id, type name)` of every stored resource, in no particular order.
    pub(crate) fn type_names(&self) -> impl Iterator<Item = (TypeId, &'static str)> + '_ {
        self.map.iter().map(|(id, slot)| (*id, slot.name))
    }

    /// Returns the number of resources currently stored.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns `true` if no resources are currently stored.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cold]
fn poisoned<T>() -> ! {
    panic!("resource `{}` is poisoned (a system panicked while borrowing it)", std::any::type_name::<T>())
}

impl std::fmt::Debug for Resources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resources")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let mut resources = Resources::new();
        assert!(resources.is_empty());
        assert_eq!(resources.len(), 0);

        assert_eq!(resources.insert(42_i32), None);
        assert!(!resources.is_empty());
        assert_eq!(resources.len(), 1);
        assert_eq!(resources.get::<i32>().as_deref(), Some(&42));
        assert_eq!(resources.get::<String>().as_deref(), None);
    }

    #[test]
    fn test_insert_replaces_and_returns_old() {
        let mut resources = Resources::new();
        assert_eq!(resources.insert(10_u32), None);
        assert_eq!(resources.insert(20_u32), Some(10));
        assert_eq!(resources.get::<u32>().as_deref(), Some(&20));
        assert_eq!(resources.len(), 1);
    }

    #[test]
    fn test_get_mut_mutation() {
        let mut resources = Resources::new();
        resources.insert(vec![1, 2, 3]);

        if let Some(v) = resources.get_mut::<Vec<i32>>() {
            v.push(4);
        }

        assert_eq!(resources.get::<Vec<i32>>().as_deref(), Some(&vec![1, 2, 3, 4]));
    }

    #[test]
    fn test_remove() {
        let mut resources = Resources::new();
        resources.insert(String::from("hello"));
        assert!(resources.contains::<String>());

        let removed = resources.remove::<String>();
        assert_eq!(removed, Some(String::from("hello")));
        assert!(!resources.contains::<String>());
        assert_eq!(resources.get::<String>().as_deref(), None);
        assert_eq!(resources.remove::<String>(), None);
    }

    #[test]
    fn test_contains() {
        let mut resources = Resources::new();
        assert!(!resources.contains::<f64>());
        resources.insert(3.14_f64);
        assert!(resources.contains::<f64>());
        resources.remove::<f64>();
        assert!(!resources.contains::<f64>());
    }

    #[test]
    fn test_get_or_insert_with() {
        let mut resources = Resources::new();
        let mut call_count = 0;

        let val = resources.get_or_insert_with(|| {
            call_count += 1;
            100_i32
        });
        assert_eq!(*val, 100);
        assert_eq!(call_count, 1);

        let val2 = resources.get_or_insert_with(|| {
            call_count += 1;
            200_i32
        });
        assert_eq!(*val2, 100);
        assert_eq!(call_count, 1);
        assert_eq!(resources.len(), 1);
    }

    #[test]
    fn test_two_different_types_coexist() {
        let mut resources = Resources::new();
        resources.insert(123_i32);
        resources.insert(String::from("world"));

        assert_eq!(resources.len(), 2);
        assert!(resources.contains::<i32>());
        assert!(resources.contains::<String>());
        assert_eq!(resources.get::<i32>().as_deref(), Some(&123));
        assert_eq!(resources.get::<String>().as_deref(), Some(&String::from("world")));
    }

    #[test]
    fn test_change_ticks() {
        let mut r = Resources::new();
        r.set_tick(Tick::new(5));
        r.insert(1_u32);
        assert_eq!(r.changed_tick::<u32>(), Some(Tick::new(5)));
        assert_eq!(r.changed_tick::<i8>(), None);
        r.set_tick(Tick::new(7));
        {
            let g = r.write::<u32>(Tick::new(6)).unwrap();
            assert_eq!(*g, 1); // read-only deref: not a change
        }
        assert_eq!(r.changed_tick::<u32>(), Some(Tick::new(5)));
        *r.write::<u32>(Tick::new(6)).unwrap() += 1;
        assert_eq!(r.changed_tick::<u32>(), Some(Tick::new(6)));
        *r.get_mut::<u32>().unwrap() += 1;
        assert_eq!(r.changed_tick::<u32>(), Some(Tick::new(7)));
        assert!(r.is_changed::<u32>(Tick::new(6), Tick::new(8)));
        assert!(!r.is_changed::<u32>(Tick::new(7), Tick::new(8)));
    }

    #[test]
    fn test_debug_format() {
        let mut resources = Resources::new();
        assert_eq!(format!("{resources:?}"), "Resources { len: 0 }");
        resources.insert(1_i32);
        resources.insert(2.0_f32);
        resources.insert("three");
        assert_eq!(format!("{resources:?}"), "Resources { len: 3 }");
    }
}
