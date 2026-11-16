//! Double-buffered typed event queues and type-erased event registry.
//!
//! Events sent during frame N are queued in a pending buffer, and become readable
//! during frame N+1 after `update()` is called by the engine loop.

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Double-buffered event queue. `send` writes to `pending`; `update()` makes `pending`
/// the readable buffer and drops the old readable one. An event sent in frame N is
/// therefore readable during all of frame N+1, exactly once.
pub struct Events<T> {
    readable: Vec<T>,
    pending: Vec<T>,
}

impl<T> Default for Events<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Events<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events")
            .field("readable", &self.readable)
            .field("pending", &self.pending)
            .finish()
    }
}

impl<T> Events<T> {
    /// Creates a new, empty double-buffered event queue.
    pub const fn new() -> Self {
        Self {
            readable: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Pushes an event to the pending buffer to become readable next frame.
    pub fn send(&mut self, ev: T) {
        self.pending.push(ev);
    }

    /// Returns an iterator over events in the readable buffer.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.readable.iter()
    }

    /// Returns `true` if the readable buffer contains no events.
    pub fn is_empty(&self) -> bool {
        self.readable.is_empty()
    }

    /// Returns the number of events in the readable buffer.
    pub fn len(&self) -> usize {
        self.readable.len()
    }

    /// Swaps the pending buffer into readable, dropping the old readable buffer.
    pub fn update(&mut self) {
        self.readable = std::mem::take(&mut self.pending);
    }

    /// Puts events straight into the readable buffer (same-frame delivery).
    ///
    /// The engine uses this for control `Message`s drained at the start of a frame.
    pub fn extend_readable(&mut self, evs: impl IntoIterator<Item = T>) {
        self.readable.extend(evs);
    }

    /// Clears both the readable and pending buffers.
    pub fn clear(&mut self) {
        self.readable.clear();
        self.pending.clear();
    }

    /// Returns the number of events queued in the pending buffer.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

impl<'a, T> IntoIterator for &'a Events<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Send + Sync + 'static> Events<T> {
    /// A `'static` empty queue, returned by readers when no events of type `T` exist.
    pub fn empty() -> &'static Events<T> {
        // inline const promotion; the value is never dropped
        const { &Events::<T>::new() }
    }
}

trait AnyEvents: Send + Sync {
    fn update(&mut self);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Send + Sync + 'static> AnyEvents for Events<T> {
    fn update(&mut self) {
        Events::update(self);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Type-erased storage of every registered [`Events<T>`].
#[derive(Default)]
pub struct EventRegistry {
    map: HashMap<TypeId, Box<dyn AnyEvents>>,
}

impl EventRegistry {
    /// Creates an empty event registry.
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Register event type `T` (no-op if already registered).
    pub fn register<T: Send + Sync + 'static>(&mut self) {
        self.map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Events::<T>::new()));
    }

    /// Returns `true` if event type `T` is registered.
    pub fn is_registered<T: Send + Sync + 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }

    /// Returns a reference to the event queue for type `T`, or `None` if not registered.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&Events<T>> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.as_any().downcast_ref::<Events<T>>())
    }

    /// Returns a reference to the event queue for type `T`, or [`Events::<T>::empty()`] when
    /// `T` was never registered.
    pub fn get_or_empty<T: Send + Sync + 'static>(&self) -> &Events<T> {
        self.get::<T>().unwrap_or_else(|| Events::<T>::empty())
    }

    /// Returns a mutable reference to the event queue for type `T`, registering `T` on first use.
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> &mut Events<T> {
        let boxed = self
            .map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Events::<T>::new()));
        boxed
            .as_any_mut()
            .downcast_mut::<Events<T>>()
            .expect("event registry invariant: queue type matches TypeId")
    }

    /// Sends an event of type `T`, registering `T` on first send.
    pub fn send<T: Send + Sync + 'static>(&mut self, ev: T) {
        self.get_mut::<T>().send(ev);
    }

    /// Calls `update()` on every registered queue (the engine does this at the start of each frame).
    pub fn update_all(&mut self) {
        for queue in self.map.values_mut() {
            queue.update();
        }
    }

    /// Returns the number of registered event types.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns `true` if no event types are registered.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl std::fmt::Debug for EventRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventRegistry")
            .field("len", &self.map.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_iter_empty_before_update() {
        let mut events = Events::<i32>::new();
        events.send(42);
        assert!(events.is_empty());
        assert_eq!(events.len(), 0);
        assert_eq!(events.pending_len(), 1);
        assert_eq!(events.iter().count(), 0);
    }

    #[test]
    fn test_readable_exactly_once_then_gone() {
        let mut events = Events::<i32>::new();
        events.send(42);
        assert!(events.is_empty());

        events.update();
        assert!(!events.is_empty());
        assert_eq!(events.len(), 1);
        assert_eq!(events.pending_len(), 0);
        assert_eq!(events.iter().copied().collect::<Vec<_>>(), vec![42]);

        events.update();
        assert!(events.is_empty());
        assert_eq!(events.len(), 0);
        assert_eq!(events.iter().count(), 0);
    }

    #[test]
    fn test_events_between_updates_in_order() {
        let mut events = Events::<i32>::new();
        events.send(1);
        events.send(2);
        events.send(3);
        assert_eq!(events.pending_len(), 3);

        events.update();
        assert_eq!(events.len(), 3);
        let received: Vec<i32> = events.iter().copied().collect();
        assert_eq!(received, vec![1, 2, 3]);

        // Using IntoIterator for &'a Events<T>
        let mut into_iter_received = Vec::new();
        for &ev in &events {
            into_iter_received.push(ev);
        }
        assert_eq!(into_iter_received, vec![1, 2, 3]);
    }

    #[test]
    fn test_extend_readable() {
        let mut events = Events::<i32>::new();
        events.extend_readable([10, 20, 30]);
        assert_eq!(events.len(), 3);
        assert_eq!(events.iter().copied().collect::<Vec<_>>(), vec![10, 20, 30]);

        events.update();
        assert!(events.is_empty());
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_registry_get_or_empty_unknown() {
        let registry = EventRegistry::new();
        assert!(!registry.is_registered::<String>());
        let empty = registry.get_or_empty::<String>();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.iter().count(), 0);
    }

    #[test]
    fn test_registry_send_update_all_get() {
        let mut registry = EventRegistry::new();
        registry.send(100_i32);
        assert!(registry.is_registered::<i32>());

        // Before update_all: queue readable is empty
        let queue_before = registry.get::<i32>().expect("should be registered");
        assert!(queue_before.is_empty());

        registry.update_all();

        let queue_after = registry.get::<i32>().expect("should be registered");
        assert_eq!(queue_after.len(), 1);
        assert_eq!(queue_after.iter().copied().collect::<Vec<_>>(), vec![100]);
    }

    #[test]
    fn test_registry_two_types() {
        let mut registry = EventRegistry::new();
        registry.send(77_u64);
        registry.send(String::from("engine_event"));

        assert!(registry.is_registered::<u64>());
        assert!(registry.is_registered::<String>());
        assert_eq!(registry.len(), 2);

        registry.update_all();

        let u64_events = registry.get::<u64>().unwrap();
        assert_eq!(u64_events.iter().copied().collect::<Vec<_>>(), vec![77]);

        let str_events = registry.get::<String>().unwrap();
        assert_eq!(
            str_events.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["engine_event"]
        );
    }

    #[test]
    fn test_events_empty() {
        let empty = Events::<String>::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.pending_len(), 0);
        assert_eq!(empty.iter().count(), 0);
    }

    #[test]
    fn test_events_clear() {
        let mut events = Events::<i32>::new();
        events.send(1);
        events.extend_readable([2]);
        assert_eq!(events.len(), 1);
        assert_eq!(events.pending_len(), 1);

        events.clear();
        assert_eq!(events.len(), 0);
        assert_eq!(events.pending_len(), 0);
    }

    #[test]
    fn test_debug_format() {
        let mut events = Events::<i32>::new();
        events.send(5);
        events.extend_readable([1]);
        let debug_str = format!("{events:?}");
        assert!(debug_str.contains("Events"));
        assert!(debug_str.contains("readable: [1]"));
        assert!(debug_str.contains("pending: [5]"));

        let mut registry = EventRegistry::new();
        assert_eq!(format!("{registry:?}"), "EventRegistry { len: 0 }");
        registry.register::<i32>();
        assert_eq!(format!("{registry:?}"), "EventRegistry { len: 1 }");
    }
}
