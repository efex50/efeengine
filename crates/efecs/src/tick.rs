//! Change detection ticks
//!
//! A [`Tick`] is a wrapping 32-bit counter owned by a [`World`](crate::World). Tracked components
//! remember the tick at which they were added and last changed, and queries compare those against
//! the tick at which they last ran.

// Adapted from Bevy (https://github.com/bevyengine/bevy), crates/bevy_ecs/src/component.rs and change_detection.rs at v0.14.0, MIT OR Apache-2.0, Copyright (c) Bevy contributors.
/// A point in a world's change history
///
/// Ticks wrap around; comparisons are only meaningful between ticks that are less than
/// [`MAX_CHANGE_AGE`] apart, which is maintained by
/// [`World::check_change_ticks`](crate::World::check_change_ticks).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Tick(u32);

impl Tick {
    /// Construct a tick from a raw counter value
    pub const fn new(v: u32) -> Self {
        Self(v)
    }

    /// The raw counter value
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Distance from `other` to `self`, wrapping
    #[inline]
    fn relative_to(self, other: Tick) -> Tick {
        Tick(self.0.wrapping_sub(other.0))
    }

    /// Whether `self` is newer than `last_run`, as seen from `this_run`
    ///
    /// Wrap-aware: equivalent to `this_run - self < this_run - last_run` in wrapping arithmetic,
    /// with both distances clamped to [`MAX_CHANGE_AGE`].
    #[inline]
    pub fn is_newer_than(self, last_run: Tick, this_run: Tick) -> bool {
        let ticks_since_insert = this_run.relative_to(self).0.min(MAX_CHANGE_AGE);
        let ticks_since_system = this_run.relative_to(last_run).0.min(MAX_CHANGE_AGE);
        ticks_since_system > ticks_since_insert
    }

    /// Clamp `self` if it is older than [`MAX_CHANGE_AGE`] relative to `now`
    ///
    /// Returns whether `self` was modified.
    #[inline]
    pub fn check_tick(&mut self, now: Tick) -> bool {
        let age = now.relative_to(*self);
        if age.0 > MAX_CHANGE_AGE {
            *self = now.relative_to(Tick(MAX_CHANGE_AGE));
            true
        } else {
            false
        }
    }
}

/// How far the world's change tick may advance between calls to
/// [`World::check_change_ticks`](crate::World::check_change_ticks)
pub const CHECK_TICK_THRESHOLD: u32 = 518_400_000;

/// Ticks older than this, relative to the current tick, are clamped to this age
///
/// Keeps wrapping comparisons unambiguous.
pub const MAX_CHANGE_AGE: u32 = u32::MAX - (2 * CHECK_TICK_THRESHOLD - 1);

/// The ticks at which a tracked component was added and last changed
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ComponentTicks {
    /// When the component was added to its entity
    pub added: Tick,
    /// When the component was last (possibly) mutated
    pub changed: Tick,
}

/// The tick window a query evaluates change filters in
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct QueryTicks {
    /// Changes at or before this tick are not considered new
    pub last_run: Tick,
    /// The current tick; also what mutable access stamps into `changed`
    pub this_run: Tick,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_than_basic() {
        let last = Tick::new(10);
        let now = Tick::new(20);
        assert!(Tick::new(11).is_newer_than(last, now));
        assert!(Tick::new(20).is_newer_than(last, now));
        assert!(!Tick::new(10).is_newer_than(last, now));
        assert!(!Tick::new(5).is_newer_than(last, now));
    }

    #[test]
    fn newer_than_wraps() {
        let last = Tick::new(u32::MAX - 2);
        let now = Tick::new(3);
        assert!(Tick::new(u32::MAX).is_newer_than(last, now));
        assert!(Tick::new(0).is_newer_than(last, now));
        assert!(Tick::new(3).is_newer_than(last, now));
        assert!(!Tick::new(u32::MAX - 2).is_newer_than(last, now));
        assert!(!Tick::new(u32::MAX - 10).is_newer_than(last, now));
    }

    #[test]
    fn check_tick_clamps() {
        let now = Tick::new(MAX_CHANGE_AGE + 100);
        let mut old = Tick::new(0);
        assert!(old.check_tick(now));
        assert_eq!(old, Tick::new(100));
        let mut fresh = Tick::new(101);
        assert!(!fresh.check_tick(now));
        assert_eq!(fresh, Tick::new(101));
    }
}
