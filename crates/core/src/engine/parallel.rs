//! Parallel scheduling: the [`Executor`], declared [`Access`] and the [`ParCtx`] that parallel
//! systems receive. See `docs/PARALLEL_SPEC.md` (sections 2, 5).

use std::any::{Any, TypeId};
use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use hecs::{Component, ComponentError, ComponentRef, Entity, Query, QueryBorrow, QueryOne, World};

use crate::engine::{
    WindowState,
    commands::ParCommands,
    events::{EventRegistry, Events},
    input::{GamepadState, InputState},
    resources::{Res, ResMut, Resources},
    time::Time,
    types::TypeRegistry,
};
use crate::thread_pool::{GLOBAL_POOL, Jobs, ThreadPoolTrait, global_pool_size, init_global_pool, set_global_pool_size};

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Runs batches of tasks on the calling thread plus helpers from the global pool.
///
/// The caller never blocks on a channel: it takes tasks itself and then spin-waits on an atomic
/// counter. That is what makes it safe on the wasm main thread (no `Atomics.wait`) and lets
/// nested `run` calls from pool workers make progress without deadlocking.
pub(crate) struct Executor {
    threads: usize,
    /// Helper jobs sent by any `run` of this executor that have not finished yet. A `run` sends a
    /// helper only while this is below `threads - 1`, so nested `run` calls cannot push the number
    /// of participants over `threads`.
    helpers: Arc<AtomicUsize>,
}

/// The panic of the lowest task index seen so far.
type PanicSlot = Mutex<Option<(usize, Box<dyn Any + Send>)>>;

fn record_panic(slot: &PanicSlot, i: usize, p: Box<dyn Any + Send>) {
    let mut s = crate::thread_pool::lock_nb(slot);
    if s.as_ref().is_none_or(|(j, _)| i < *j) {
        *s = Some((i, p));
    }
}

/// State shared between the caller of [`Executor::run`] and its helper jobs.
struct Shared {
    next: AtomicUsize,
    done: AtomicUsize,
    len: usize,
    /// The caller's task slice with its lifetimes erased (see the SAFETY comment in `run`).
    tasks: *mut &'static mut (dyn FnMut() + Send),
    panic: PanicSlot,
}

// SAFETY: `tasks` is dereferenced only under the claim protocol described in `Executor::run`;
// every task it points to is `Send`, and each is touched by exactly one thread.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    /// Claim and run tasks until none are left.
    fn work(&self) {
        loop {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            if i >= self.len {
                return;
            }
            // SAFETY: `i < len` was claimed through the `fetch_add` above, so this thread is the
            // only one that ever dereferences slot `i`; `run` cannot have returned yet because
            // `done` counts this task only after it ran (see `Executor::run`).
            let task: &mut (dyn FnMut() + Send) = unsafe { &mut **self.tasks.add(i) };
            if let Err(p) = catch_unwind(AssertUnwindSafe(|| task())) {
                record_panic(&self.panic, i, p);
            }
            self.done.fetch_add(1, Ordering::Release);
        }
    }
}

impl Executor {
    /// An executor with `threads` participants (the caller plus `threads - 1` pool workers).
    /// For `threads > 1` the global pool grows to at least `threads - 1` workers (never shrinks).
    pub(crate) fn new(threads: usize) -> Self {
        let threads = threads.max(1);
        if threads > 1 {
            #[cfg(test)]
            let _g = crate::thread_pool::POOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let want = threads - 1;
            if once_cell::sync::Lazy::get(&GLOBAL_POOL).is_some() {
                if global_pool_size() < want {
                    set_global_pool_size(want);
                }
            } else {
                init_global_pool(global_pool_size().max(want));
            }
        }
        Self { threads, helpers: Arc::new(AtomicUsize::new(0)) }
    }

    /// Number of participants.
    pub(crate) fn threads(&self) -> usize {
        self.threads
    }

    /// Run every task once and return when all are done. Tasks may borrow the caller's stack.
    /// Every task runs, even after a panic; then the panic of the lowest task index is re-raised
    /// (the same on the inline and the parallel path).
    pub(crate) fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        let len = tasks.len();
        if self.threads <= 1 || len <= 1 {
            let slot: PanicSlot = Mutex::new(None);
            for (i, t) in tasks.iter_mut().enumerate() {
                if let Err(p) = catch_unwind(AssertUnwindSafe(|| (**t)())) {
                    record_panic(&slot, i, p);
                }
            }
            if let Some((_, p)) = slot.into_inner().unwrap_or_else(|e| e.into_inner()) {
                resume_unwind(p);
            }
            return;
        }
        // SAFETY (the only unsafe lifetime erasure of the scheduler): the pointer to the caller's
        // task slice is stored as `'static` so it can sit in an `Arc` that pool jobs hold. It is
        // sound because
        //   1. a task pointer is dereferenced only by the thread that claimed an index `i < len`
        //      through `next.fetch_add`, so each `&mut` task is used by exactly one thread;
        //   2. `run` returns (and so the borrowed slice and everything the tasks borrow stay
        //      alive) only after `done == len`, i.e. after every claimed task has finished, and
        //      `done` is incremented (Release) only after a task ran; we read it with Acquire;
        //   3. a helper that starts late, even after `run` returned, finds `next >= len` and
        //      touches only the `Arc`'d atomics, never the erased pointer;
        //   4. every task is `Send`, so running it on another thread is allowed;
        //   5. the helper counter only bounds how many helpers exist; it plays no part in the
        //      argument above.
        let erased: *mut &'static mut (dyn FnMut() + Send) =
            unsafe { std::mem::transmute(tasks.as_mut_ptr()) };
        let shared = Arc::new(Shared {
            next: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            len,
            tasks: erased,
            panic: Mutex::new(None),
        });
        let cap = self.threads - 1;
        let want = cap.min(len - 1);
        for _ in 0..want {
            // Reserve a helper slot; if all `threads - 1` are in flight (an outer or nested run
            // holds them) the caller simply does more of the work itself.
            let reserved = self
                .helpers
                .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < cap).then_some(n + 1))
                .is_ok();
            if !reserved {
                break;
            }
            let s = shared.clone();
            let inflight = self.helpers.clone();
            GLOBAL_POOL.send_job(Jobs::Job(Box::new(move || {
                s.work();
                inflight.fetch_sub(1, Ordering::AcqRel);
            })));
        }
        shared.work();
        let mut spins = 0u32;
        while shared.done.load(Ordering::Acquire) < len {
            std::hint::spin_loop();
            spins = spins.wrapping_add(1);
            #[cfg(not(target_family = "wasm"))]
            if spins % 64 == 0 {
                std::thread::yield_now();
            }
        }
        let p = crate::thread_pool::lock_nb(&shared.panic).take();
        if let Some((_, p)) = p {
            resume_unwind(p);
        }
    }

    /// Split `0..len` into chunks of `chunk` items and call `f(range)` per chunk, in parallel.
    pub(crate) fn for_chunks(&self, len: usize, chunk: usize, f: &(dyn Fn(Range<usize>) + Sync)) {
        let chunk = chunk.max(1);
        let mut closures: Vec<_> = (0..len)
            .step_by(chunk)
            .map(|start| {
                let end = (start + chunk).min(len);
                move || f(start..end)
            })
            .collect();
        let mut tasks: Vec<&mut (dyn FnMut() + Send)> =
            closures.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
        self.run(&mut tasks);
    }
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

/// What a parallel system reads and writes. Systems whose accesses conflict (a write against a
/// read or write of the same component or resource) never share a batch.
#[derive(Clone, Default, Debug)]
pub struct Access {
    name: Option<&'static str>,
    /// Components read (includes the written ones).
    read: Vec<TypeId>,
    write: Vec<TypeId>,
    /// Resources read (includes the written ones).
    res: Vec<TypeId>,
    res_mut: Vec<TypeId>,
    names: HashMap<TypeId, &'static str>,
}

fn add(v: &mut Vec<TypeId>, id: TypeId) {
    if !v.contains(&id) {
        v.push(id);
    }
}

fn intersects(a: &[TypeId], b: &[TypeId]) -> bool {
    a.iter().any(|x| b.contains(x))
}

impl Access {
    pub fn new() -> Self {
        Self::default()
    }

    /// The system's name, shown in panics (default: the type name of the closure).
    pub fn named(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    /// Declare a component read.
    pub fn read<T: Component>(mut self) -> Self {
        let id = TypeId::of::<T>();
        add(&mut self.read, id);
        self.names.insert(id, std::any::type_name::<T>());
        self
    }

    /// Declare a component write (implies read).
    pub fn write<T: Component>(mut self) -> Self {
        let id = TypeId::of::<T>();
        add(&mut self.read, id);
        add(&mut self.write, id);
        self.names.insert(id, std::any::type_name::<T>());
        self
    }

    /// Declare a resource read.
    pub fn res<T: Send + Sync + 'static>(mut self) -> Self {
        let id = TypeId::of::<T>();
        add(&mut self.res, id);
        self.names.insert(id, std::any::type_name::<T>());
        self
    }

    /// Declare a resource write (implies read).
    pub fn res_mut<T: Send + Sync + 'static>(mut self) -> Self {
        let id = TypeId::of::<T>();
        add(&mut self.res, id);
        add(&mut self.res_mut, id);
        self.names.insert(id, std::any::type_name::<T>());
        self
    }

    /// Write/write or read/write overlap on a component or a resource.
    pub fn conflicts(&self, other: &Access) -> bool {
        intersects(&self.write, &other.read)
            || intersects(&other.write, &self.read)
            || intersects(&self.res_mut, &other.res)
            || intersects(&other.res_mut, &self.res)
    }

    pub(crate) fn set_name(&mut self, name: &'static str) {
        self.name = Some(name);
    }

    pub(crate) fn name_or(&self, default: &'static str) -> &'static str {
        self.name.unwrap_or(default)
    }

    pub(crate) fn set_default_name(&mut self, name: &'static str) {
        self.name.get_or_insert(name);
    }

    fn display_name(&self) -> &'static str {
        self.name.unwrap_or("<unnamed>")
    }

    fn type_name(&self, id: TypeId) -> String {
        match self.names.get(&id) {
            Some(n) => (*n).to_string(),
            None => format!("{id:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// ParCtx
// ---------------------------------------------------------------------------

/// A buffered event send, replayed when the batch merges.
pub(crate) type SendFn = Box<dyn FnOnce(&mut EventRegistry) + Send>;

/// What a parallel system produced: its queued commands and event sends.
#[derive(Default)]
pub(crate) struct ParOutput {
    pub(crate) commands: crate::engine::commands::Commands,
    pub(crate) sends: Vec<SendFn>,
}

/// The context of a parallel system. It shares the world with the other systems of its batch,
/// so everything goes through the checked accessors: component access must be declared in the
/// system's [`Access`] (checked in debug builds), resources always (checked in release too).
/// Structural changes and event sends are buffered and applied in registration order.
pub struct ParCtx<'a> {
    world: &'a World,
    /// Registered script-visible types.
    pub types: &'a TypeRegistry,
    pub time: &'a Time,
    pub input: &'a InputState,
    pub gamepads: &'a GamepadState,
    pub window: &'a WindowState,
    resources: &'a Resources,
    events: &'a EventRegistry,
    access: &'a Access,
    commands: RefCell<ParCommands>,
    sends: RefCell<Vec<SendFn>>,
    exec: &'a Executor,
}

impl<'a> ParCtx<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        world: &'a World,
        types: &'a TypeRegistry,
        time: &'a Time,
        input: &'a InputState,
        gamepads: &'a GamepadState,
        window: &'a WindowState,
        resources: &'a Resources,
        events: &'a EventRegistry,
        access: &'a Access,
        exec: &'a Executor,
    ) -> Self {
        Self {
            world,
            types,
            time,
            input,
            gamepads,
            window,
            resources,
            events,
            access,
            commands: RefCell::new(ParCommands::new()),
            sends: RefCell::new(Vec::new()),
            exec,
        }
    }

    pub(crate) fn into_output(self) -> ParOutput {
        ParOutput { commands: self.commands.into_inner().take(), sends: self.sends.into_inner() }
    }

    #[cfg(debug_assertions)]
    fn check_component(&self, id: TypeId, unique: bool) {
        let a = self.access;
        let ok = if unique { a.write.contains(&id) } else { a.read.contains(&id) };
        if !ok {
            panic!(
                "par system `{}` accesses `{}` ({}) without declaring it in its Access",
                a.display_name(),
                a.type_name(id),
                if unique { "write" } else { "read" }
            );
        }
    }

    fn check_query<Q: Query>(&self) {
        #[cfg(debug_assertions)]
        <Q::Fetch as hecs::Fetch>::for_each_borrow(|id, unique| self.check_component(id, unique));
    }

    /// Query the world. Debug builds check the query against the declared [`Access`].
    pub fn query<Q: Query>(&self) -> QueryBorrow<'_, Q> {
        self.check_query::<Q>();
        self.world.query::<Q>()
    }

    /// Query one entity (checked like [`query`](Self::query)).
    pub fn query_one<Q: Query>(&self, e: Entity) -> QueryOne<'_, Q> {
        self.check_query::<Q>();
        self.world.query_one::<Q>(e)
    }

    /// Get `&T` or `&mut T` of one entity (checked like [`query`](Self::query)).
    pub fn get<'b, T: ComponentRef<'b> + Query>(&'b self, e: Entity) -> Result<T::Ref, ComponentError> {
        self.check_query::<T>();
        self.world.get::<T>(e)
    }

    /// The raw world. Nothing is checked against the access declaration; hecs still checks
    /// borrows at runtime (a conflicting borrow panics, it is never unsound).
    pub fn world_unchecked(&self) -> &World {
        self.world
    }

    fn check_res(&self, id: TypeId, unique: bool) {
        let a = self.access;
        let ok = if unique { a.res_mut.contains(&id) } else { a.res.contains(&id) };
        if !ok {
            panic!(
                "par system `{}` accesses resource `{}` ({}) without declaring it in its Access",
                a.display_name(),
                a.type_name(id),
                if unique { "write" } else { "read" }
            );
        }
    }

    /// Shared borrow of resource `T`. Panics if `T` was not declared with `Access::res` (or
    /// `res_mut`) or is missing.
    pub fn res<T: Send + Sync + 'static>(&self) -> Res<'_, T> {
        self.try_res::<T>()
            .unwrap_or_else(|| panic!("resource `{}` is missing", std::any::type_name::<T>()))
    }

    /// Exclusive borrow of resource `T`. Panics if `T` was not declared with `Access::res_mut`
    /// or is missing.
    pub fn res_mut<T: Send + Sync + 'static>(&self) -> ResMut<'_, T> {
        self.try_res_mut::<T>()
            .unwrap_or_else(|| panic!("resource `{}` is missing", std::any::type_name::<T>()))
    }

    /// Like [`res`](Self::res); `None` if the resource is missing.
    pub fn try_res<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        self.check_res(TypeId::of::<T>(), false);
        self.resources.read::<T>()
    }

    /// Like [`res_mut`](Self::res_mut); `None` if the resource is missing.
    pub fn try_res_mut<T: Send + Sync + 'static>(&self) -> Option<ResMut<'_, T>> {
        self.check_res(TypeId::of::<T>(), true);
        self.resources.write::<T>()
    }

    /// Events of type `T` readable this frame.
    pub fn events<T: Send + Sync + 'static>(&self) -> &Events<T> {
        self.events.get_or_empty::<T>()
    }

    /// Send an event; buffered, replayed in registration order when the batch merges, readable
    /// during the next frame.
    pub fn send<T: Send + Sync + 'static>(&self, ev: T) {
        self.sends.borrow_mut().push(Box::new(move |reg: &mut EventRegistry| reg.send(ev)));
    }

    /// This system's command buffer (spawns are deferred: no entity handle is returned).
    pub fn commands(&self) -> RefMut<'_, ParCommands> {
        self.commands.borrow_mut()
    }

    /// Run `f` on every item of query `Q`, in parallel batches of about `batch` entities.
    /// The batches are disjoint, so `f` may write the query items in place; it must not touch
    /// any other data.
    pub fn par_for_each<Q, F>(&self, batch: u32, f: F)
    where
        Q: Query,
        for<'q> Q::Item<'q>: Send,
        F: Fn(Q::Item<'_>) + Send + Sync,
    {
        let mut q = self.query::<Q>();
        if self.exec.threads() <= 1 {
            for item in q.iter() {
                f(item);
            }
            return;
        }
        let mut batches: Vec<_> = q.iter_batched(batch.max(1)).collect();
        if batches.len() <= 1 {
            for b in batches {
                for item in b {
                    f(item);
                }
            }
            return;
        }
        let f = &f;
        let mut tasks: Vec<_> = batches
            .iter_mut()
            .map(|b| {
                move || {
                    for item in b.by_ref() {
                        f(item);
                    }
                }
            })
            .collect();
        let mut refs: Vec<&mut (dyn FnMut() + Send)> =
            tasks.iter_mut().map(|t| t as &mut (dyn FnMut() + Send)).collect();
        self.exec.run(&mut refs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    #[derive(Clone, Copy)]
    struct A(#[allow(dead_code)] u32);
    #[derive(Clone, Copy)]
    struct B(#[allow(dead_code)] u32);
    struct R1;
    struct R2;

    /// A burst far larger than the pool (the case that filled the channel on a long frame): the
    /// caller must finish it itself without waiting on a channel, and every task runs once.
    #[test]
    fn large_burst_completes_without_blocking() {
        let ex = Executor::new(2);
        let hits = AtomicUsize::new(0);
        let n = 20_000;
        let mut closures: Vec<_> = (0..n).map(|_| || { hits.fetch_add(1, Ordering::Relaxed); }).collect();
        let mut tasks: Vec<&mut (dyn FnMut() + Send)> =
            closures.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
        let t = std::time::Instant::now();
        for _ in 0..20 {
            ex.run(&mut tasks);
        }
        assert_eq!(hits.load(Ordering::Relaxed), n * 20);
        assert!(t.elapsed() < Duration::from_secs(20));
    }

    /// The panic slot lock is taken by the main thread while a helper may hold it: `lock_nb`
    /// spins instead of waiting in the OS, and recovers a poisoned lock.
    #[test]
    fn lock_nb_spins_and_recovers_poison() {
        let m = Arc::new(Mutex::new(0u32));
        let m2 = m.clone();
        let h = std::thread::spawn(move || {
            let mut g = m2.lock().unwrap();
            std::thread::sleep(Duration::from_millis(50));
            *g = 7;
        });
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(*crate::thread_pool::lock_nb(&m), 7);
        h.join().unwrap();
        let m3 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m3.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert_eq!(*crate::thread_pool::lock_nb(&m), 7);
    }

    #[test]
    fn access_conflicts() {
        let ra = Access::new().read::<A>();
        let ra2 = Access::new().read::<A>();
        let wa = Access::new().write::<A>();
        let wb = Access::new().write::<B>();
        assert!(!ra.conflicts(&ra2), "read/read is fine");
        assert!(ra.conflicts(&wa) && wa.conflicts(&ra), "read/write conflicts both ways");
        assert!(wa.conflicts(&wa.clone()), "write/write conflicts");
        assert!(!wa.conflicts(&wb), "disjoint components");
        let r = Access::new().res::<R1>();
        let w = Access::new().res_mut::<R1>();
        assert!(!r.conflicts(&r.clone()));
        assert!(r.conflicts(&w) && w.conflicts(&r) && w.conflicts(&w.clone()));
        assert!(!w.conflicts(&Access::new().res_mut::<R2>()));
        assert!(!w.conflicts(&wa), "a resource never conflicts with a component");
        assert!(!Access::new().conflicts(&wa));
    }

    fn run_counts(threads: usize, n: usize) {
        let exec = Executor::new(threads);
        let counts: Vec<AtomicUsize> = (0..n).map(|_| AtomicUsize::new(0)).collect();
        let mut closures: Vec<_> = counts
            .iter()
            .map(|c| move || {
                c.fetch_add(1, Ordering::SeqCst);
            })
            .collect();
        let mut tasks: Vec<&mut (dyn FnMut() + Send)> = closures.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
        exec.run(&mut tasks);
        assert!(counts.iter().all(|c| c.load(Ordering::SeqCst) == 1), "threads={threads}");
    }

    #[test]
    fn executor_runs_every_task_once() {
        for threads in [1, 2, 8] {
            for n in [0, 1, 2, 7, 100] {
                run_counts(threads, n);
            }
        }
    }

    #[test]
    fn executor_runs_in_order_when_single() {
        let exec = Executor::new(1);
        let log = Mutex::new(Vec::new());
        let mut cs: Vec<_> = (0..5).map(|i| { let log = &log; move || log.lock().unwrap().push(i) }).collect();
        let mut tasks: Vec<&mut (dyn FnMut() + Send)> = cs.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
        exec.run(&mut tasks);
        assert_eq!(*log.lock().unwrap(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn executor_reraises_panic_after_all_tasks() {
        let exec = Executor::new(4);
        let ran = AtomicUsize::new(0);
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut cs: Vec<_> = (0..8)
                .map(|i| {
                    let ran = &ran;
                    move || {
                        ran.fetch_add(1, Ordering::SeqCst);
                        if i == 3 {
                            panic!("boom");
                        }
                    }
                })
                .collect();
            let mut tasks: Vec<&mut (dyn FnMut() + Send)> = cs.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
            exec.run(&mut tasks);
        }));
        let err = r.expect_err("panic is re-raised");
        assert_eq!(*err.downcast_ref::<&str>().unwrap(), "boom");
        assert_eq!(ran.load(Ordering::SeqCst), 8, "the other tasks still ran");
    }

    #[test]
    fn executor_nested_run() {
        let exec = Executor::new(4);
        let total = AtomicUsize::new(0);
        let exec_ref = &exec;
        let total_ref = &total;
        let mut outer: Vec<_> = (0..4)
            .map(|_| move || {
                let mut inner: Vec<_> = (0..4).map(|_| move || { total_ref.fetch_add(1, Ordering::SeqCst); }).collect();
                let mut tasks: Vec<&mut (dyn FnMut() + Send)> = inner.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
                exec_ref.run(&mut tasks);
            })
            .collect();
        let mut tasks: Vec<&mut (dyn FnMut() + Send)> = outer.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
        exec.run(&mut tasks);
        assert_eq!(total.load(Ordering::SeqCst), 16);
    }

    fn panic_order(threads: usize) {
        let exec = Executor::new(threads);
        let ran = AtomicUsize::new(0);
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut cs: Vec<_> = (0..8usize)
                .map(|i| {
                    let ran = &ran;
                    move || {
                        // make the higher index panic first in time on the parallel path
                        if i == 2 {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        ran.fetch_add(1, Ordering::SeqCst);
                        if i == 2 {
                            panic!("task two");
                        }
                        if i == 5 {
                            panic!("task five");
                        }
                    }
                })
                .collect();
            let mut tasks: Vec<&mut (dyn FnMut() + Send)> = cs.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
            exec.run(&mut tasks);
        }));
        let err = r.expect_err("panic is re-raised");
        assert_eq!(*err.downcast_ref::<&str>().unwrap(), "task two", "threads={threads}");
        assert_eq!(ran.load(Ordering::SeqCst), 8, "every task ran, threads={threads}");
    }

    #[test]
    fn panic_of_lowest_index_wins_inline() {
        panic_order(1);
    }

    #[test]
    fn panic_of_lowest_index_wins_parallel() {
        for _ in 0..5 {
            panic_order(4);
        }
    }

    #[test]
    fn nested_runs_never_exceed_thread_bound() {
        for threads in [2usize, 3] {
            for _ in 0..5 {
                let exec = Executor::new(threads);
                let cur = AtomicUsize::new(0);
                let max = AtomicUsize::new(0);
                let (exec_ref, cur_ref, max_ref) = (&exec, &cur, &max);
                let mut outer: Vec<_> = (0..4)
                    .map(|_| move || {
                        let mut inner: Vec<_> = (0..4)
                            .map(|_| move || {
                                let now = cur_ref.fetch_add(1, Ordering::SeqCst) + 1;
                                max_ref.fetch_max(now, Ordering::SeqCst);
                                std::thread::sleep(Duration::from_millis(2));
                                cur_ref.fetch_sub(1, Ordering::SeqCst);
                            })
                            .collect();
                        let mut tasks: Vec<&mut (dyn FnMut() + Send)> = inner.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
                        exec_ref.run(&mut tasks);
                    })
                    .collect();
                let mut tasks: Vec<&mut (dyn FnMut() + Send)> = outer.iter_mut().map(|c| c as &mut (dyn FnMut() + Send)).collect();
                exec.run(&mut tasks);
                let m = max.load(Ordering::SeqCst);
                assert!(m <= threads, "max concurrent {m} > threads {threads}");
                assert_eq!(exec.helpers.load(Ordering::SeqCst), 0, "all helper slots released");
            }
        }
    }

    #[test]
    fn for_chunks_covers_range_once() {
        for threads in [1, 4] {
            let exec = Executor::new(threads);
            let hits: Vec<AtomicUsize> = (0..1000).map(|_| AtomicUsize::new(0)).collect();
            exec.for_chunks(1000, 64, &|r| {
                for i in r {
                    hits[i].fetch_add(1, Ordering::SeqCst);
                }
            });
            assert!(hits.iter().all(|h| h.load(Ordering::SeqCst) == 1));
        }
    }

    #[test]
    fn slow_task_does_not_deadlock_caller() {
        // a task that finishes late must still be waited for
        let exec = Executor::new(2);
        let flag = AtomicBool::new(false);
        let mut a = || { std::thread::sleep(Duration::from_millis(30)); flag.store(true, Ordering::SeqCst); };
        let mut b = || {};
        exec.run(&mut [&mut a, &mut b]);
        assert!(flag.load(Ordering::SeqCst));
    }

    // ---- engine-level tests (headless engine, par systems) ----

    use crate::engine::{Engine, config::{EngineConfig, Threads}, schedule::Stage};
    use crate::engine::components::{Transform, Velocity};

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct M0(u32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct M1(u32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct M2(u32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct M3(u32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Tag(u32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Ev(u32);

    fn engine(threads: Threads) -> Engine {
        let mut cfg = EngineConfig::headless();
        cfg.threads = threads;
        Engine::with_config(cfg).expect("headless engine")
    }

    #[test]
    fn engine_threads_follow_config() {
        assert_eq!(engine(Threads::Auto).threads(), 1, "headless Auto is Single");
        assert_eq!(engine(Threads::Single).threads(), 1);
        assert_eq!(engine(Threads::Fixed(1)).threads(), 1);
        let mut e = engine(Threads::Fixed(3));
        assert_eq!(e.threads(), 3);
        e.set_threads(Threads::Single);
        assert_eq!(e.threads(), 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "par system `badquery` accesses")]
    fn undeclared_query_panics_with_system_name() {
        let mut e = engine(Threads::Single);
        e.world_mut().spawn((M0(1),));
        e.add_par_system(Stage::Update, Access::new().named("badquery").read::<M0>(), |cx| {
            for m in cx.query::<&mut M0>().iter() {
                m.0 += 1;
            }
        });
        e.run_frames(1, 0.016);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "par system `badquery2` accesses")]
    fn undeclared_component_read_panics() {
        let mut e = engine(Threads::Single);
        e.add_par_system(Stage::Update, Access::new().named("badquery2").read::<M0>(), |cx| {
            let _ = cx.query::<&M1>().iter().count();
        });
        e.run_frames(1, 0.016);
    }

    #[test]
    #[should_panic(expected = "par system `badres` accesses resource")]
    fn undeclared_resource_panics_with_system_name() {
        let mut e = engine(Threads::Single);
        e.insert_resource(5u32);
        e.add_par_system(Stage::Update, Access::new().named("badres"), |cx| {
            let _ = cx.res::<u32>();
        });
        e.run_frames(1, 0.016);
    }

    #[test]
    #[should_panic(expected = "par system `badresmut` accesses resource")]
    fn read_declared_resource_cannot_be_written() {
        let mut e = engine(Threads::Single);
        e.insert_resource(5u32);
        e.add_par_system(Stage::Update, Access::new().named("badresmut").res::<u32>(), |cx| {
            *cx.res_mut::<u32>() += 1;
        });
        e.run_frames(1, 0.016);
    }

    #[test]
    fn declared_resource_access_works() {
        let mut e = engine(Threads::Fixed(2));
        e.insert_resource(5u32);
        e.add_par_system(Stage::Update, Access::new().res_mut::<u32>(), |cx| {
            *cx.res_mut::<u32>() += 1;
        });
        e.run_frames(3, 0.016);
        assert_eq!(e.resources().get::<u32>().as_deref(), Some(&8));
    }

    /// Four non-conflicting par systems finish in the reverse of registration order (sleeps);
    /// their commands and events must still merge in registration order.
    #[test]
    fn merge_order_is_registration_order() {
        for threads in [Threads::Single, Threads::Fixed(4)] {
            let mut e = engine(threads);
            macro_rules! sys {
                ($m:ident, $i:expr) => {
                    e.add_par_system(Stage::Update, Access::new().write::<$m>(), move |cx| {
                        std::thread::sleep(Duration::from_millis(60 - 15 * $i as u64));
                        cx.commands().spawn((Tag($i), $m($i)));
                        cx.send(Ev($i));
                    });
                };
            }
            sys!(M0, 0);
            sys!(M1, 1);
            sys!(M2, 2);
            sys!(M3, 3);
            let seen = Arc::new(Mutex::new(Vec::new()));
            let seen2 = seen.clone();
            e.add_system(Stage::Update, move |cx| {
                let evs: Vec<u32> = cx.events::<Ev>().iter().map(|e| e.0).collect();
                if !evs.is_empty() {
                    *seen2.lock().unwrap() = evs;
                }
            });
            e.run_frames(2, 0.016);
            assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3], "event order, {threads:?}");
            let mut tags: Vec<(u32, u32)> =
                e.state.world.query::<(hecs::Entity, &Tag)>().iter().map(|(en, t)| (en.id(), t.0)).collect();
            tags.sort();
            let tags: Vec<u32> = tags.into_iter().map(|(_, t)| t).collect();
            assert_eq!(tags.len(), 8, "two frames of four spawns");
            assert_eq!(tags[..4], [0, 1, 2, 3], "spawn order, {threads:?}");
        }
    }

    #[test]
    fn velocity_is_the_same_for_every_thread_count() {
        let run = |threads: Threads| -> Vec<(f32, f32, f32)> {
            let mut e = engine(threads);
            for i in 0..5000u32 {
                let f = i as f32;
                e.world_mut().spawn((
                    Transform { position: glam::Vec2::new(f, -f), ..Default::default() },
                    Velocity { linear: glam::Vec2::new(1.0 + f * 0.001, 2.0), angular: 0.1 },
                ));
            }
            e.run_frames(30, 1.0 / 60.0);
            let mut out: Vec<(u32, (f32, f32, f32))> = e
                .state
                .world
                .query::<(hecs::Entity, &Transform)>()
                .iter()
                .map(|(en, t)| (en.id(), (t.position.x, t.position.y, t.rotation)))
                .collect();
            out.sort_by_key(|(id, _)| *id);
            out.into_iter().map(|(_, v)| v).collect()
        };
        let single = run(Threads::Single);
        assert!(single.iter().any(|(x, _, _)| *x != 0.0));
        assert_eq!(single, run(Threads::Fixed(4)));
        assert_eq!(single, run(Threads::Fixed(2)));
    }
}
