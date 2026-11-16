//! Type-indexed resource store.
//!
//! Stores at most one instance of any `Send + Sync + 'static` type in a type map. Every value
//! sits behind its own `RwLock` so parallel systems ([`ParCtx`](crate::engine::parallel::ParCtx))
//! can borrow different resources at once; exclusive code (`&mut Resources`) never locks.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

type Slot = RwLock<Box<dyn Any + Send + Sync>>;

/// Shared borrow of a resource (a read guard that derefs to `T`).
pub struct Res<'a, T: 'static> {
    guard: RwLockReadGuard<'a, Box<dyn Any + Send + Sync>>,
    _p: PhantomData<&'a T>,
}

/// Exclusive borrow of a resource (a write guard that derefs to `T`).
pub struct ResMut<'a, T: 'static> {
    guard: RwLockWriteGuard<'a, Box<dyn Any + Send + Sync>>,
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
#[derive(Default)]
pub struct Resources {
    map: HashMap<TypeId, Slot>,
}

impl Resources {
    /// Creates an empty resource container.
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Inserts a resource of type `T`, returning the previous value of that type if any.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), RwLock::new(Box::new(value)))
            .and_then(|slot| {
                let boxed = slot.into_inner().unwrap_or_else(|e| e.into_inner());
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
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        let slot = self.map.get_mut(&TypeId::of::<T>())?;
        let boxed = slot.get_mut().unwrap_or_else(|_| poisoned::<T>());
        let any: &mut (dyn Any + Send + Sync) = &mut **boxed;
        any.downcast_mut::<T>()
    }

    /// Shared borrow through `try_read` (used by `ParCtx`). Panics on contention or poison.
    pub(crate) fn read<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        let slot = self.map.get(&TypeId::of::<T>())?;
        match slot.try_read() {
            Ok(guard) => Some(Res { guard, _p: PhantomData }),
            Err(TryLockError::WouldBlock) => panic!("resource `{}` is already mutably borrowed", std::any::type_name::<T>()),
            Err(TryLockError::Poisoned(_)) => poisoned::<T>(),
        }
    }

    /// Exclusive borrow through `try_write` (used by `ParCtx`). Panics on contention or poison.
    pub(crate) fn write<T: Send + Sync + 'static>(&self) -> Option<ResMut<'_, T>> {
        let slot = self.map.get(&TypeId::of::<T>())?;
        match slot.try_write() {
            Ok(guard) => Some(ResMut { guard, _p: PhantomData }),
            Err(TryLockError::WouldBlock) => panic!("resource `{}` is already borrowed", std::any::type_name::<T>()),
            Err(TryLockError::Poisoned(_)) => poisoned::<T>(),
        }
    }

    /// Removes and returns the resource of type `T`, or `None` if not present.
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.map.remove(&TypeId::of::<T>()).and_then(|slot| {
            let boxed = slot.into_inner().unwrap_or_else(|_| poisoned::<T>());
            (boxed as Box<dyn Any>).downcast::<T>().ok().map(|b| *b)
        })
    }

    /// Returns `true` if a resource of type `T` is present.
    pub fn contains<T: 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }

    /// Returns a mutable reference to the resource of type `T`, inserting the result of `f`
    /// if it does not yet exist.
    pub fn get_or_insert_with<T: Send + Sync + 'static>(&mut self, f: impl FnOnce() -> T) -> &mut T {
        let slot = self
            .map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| RwLock::new(Box::new(f())));
        let boxed = slot.get_mut().unwrap_or_else(|_| poisoned::<T>());
        let any: &mut (dyn Any + Send + Sync) = &mut **boxed;
        any.downcast_mut::<T>().expect("type map invariant")
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
    fn test_debug_format() {
        let mut resources = Resources::new();
        assert_eq!(format!("{resources:?}"), "Resources { len: 0 }");
        resources.insert(1_i32);
        resources.insert(2.0_f32);
        resources.insert("three");
        assert_eq!(format!("{resources:?}"), "Resources { len: 3 }");
    }
}
