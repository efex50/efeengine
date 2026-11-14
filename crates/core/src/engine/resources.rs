//! Type-indexed resource store.
//!
//! Stores at most one instance of any `'static` type in a type map, suitable for
//! engine-wide state and singleton resources accessed from the main thread.

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Type map: one value per type. Main-thread only (no `Send` bound).
#[derive(Default)]
pub struct Resources {
    map: HashMap<TypeId, Box<dyn Any>>,
}

impl Resources {
    /// Creates an empty resource container.
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Inserts a resource of type `T`, returning the previous value of that type if any.
    pub fn insert<T: 'static>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|boxed| boxed.downcast::<T>().ok().map(|b| *b))
    }

    /// Returns a shared reference to the resource of type `T`, or `None` if not present.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    /// Returns a mutable reference to the resource of type `T`, or `None` if not present.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// Removes and returns the resource of type `T`, or `None` if not present.
    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.map
            .remove(&TypeId::of::<T>())
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    /// Returns `true` if a resource of type `T` is present.
    pub fn contains<T: 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }

    /// Returns a mutable reference to the resource of type `T`, inserting the result of `f`
    /// if it does not yet exist.
    pub fn get_or_insert_with<T: 'static>(&mut self, f: impl FnOnce() -> T) -> &mut T {
        // The entry API ensures the entry under TypeId::of::<T>() holds a Box<T>.
        // Downcasting will not fail because the stored box holds a T under its own TypeId.
        let boxed = self
            .map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(f()));
        boxed
            .downcast_mut::<T>()
            .expect("type map invariant")
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
        assert_eq!(resources.get::<i32>(), Some(&42));
        assert_eq!(resources.get::<String>(), None);
    }

    #[test]
    fn test_insert_replaces_and_returns_old() {
        let mut resources = Resources::new();
        assert_eq!(resources.insert(10_u32), None);
        assert_eq!(resources.insert(20_u32), Some(10));
        assert_eq!(resources.get::<u32>(), Some(&20));
        assert_eq!(resources.len(), 1);
    }

    #[test]
    fn test_get_mut_mutation() {
        let mut resources = Resources::new();
        resources.insert(vec![1, 2, 3]);

        if let Some(v) = resources.get_mut::<Vec<i32>>() {
            v.push(4);
        }

        assert_eq!(resources.get::<Vec<i32>>(), Some(&vec![1, 2, 3, 4]));
    }

    #[test]
    fn test_remove() {
        let mut resources = Resources::new();
        resources.insert(String::from("hello"));
        assert!(resources.contains::<String>());

        let removed = resources.remove::<String>();
        assert_eq!(removed, Some(String::from("hello")));
        assert!(!resources.contains::<String>());
        assert_eq!(resources.get::<String>(), None);
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
        assert_eq!(resources.get::<i32>(), Some(&123));
        assert_eq!(resources.get::<String>(), Some(&String::from("world")));
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
