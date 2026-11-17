//! Thread pool for background jobs.
//!
//! Jobs run on other threads: native OS threads, or Web Workers on wasm32.
//! wgpu objects, SDL windows and the event pump, and DOM objects (`web_sys`
//! handles, closures) must stay on the main thread and must never be moved into
//! pool jobs. Only `Send` data crosses.

use std::sync::atomic::{AtomicUsize, Ordering};

use once_cell::sync::Lazy;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

#[cfg(not(target_arch = "wasm32"))]
mod native;
use crate::global_info;
#[cfg(not(target_arch = "wasm32"))]
use crate::thread_pool::native::{ThreadPool,Worker};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use crate::thread_pool::web::{ThreadPool,Worker};


// ---------------------------------------------------------------------------
// 1. NATIVE IMPLEMENTASYONU (Gerçek Multi-thread)
// ---------------------------------------------------------------------------

pub enum Jobs{
    Job(Box<dyn FnOnce() + Send + 'static>),
    Kill
}

pub struct WorkerHandleOnce{
    sender:flume::Sender<Jobs>,
    worker:Worker
}
impl WorkerHandleOnce {
    pub fn spawn<F,T>(self,fun:F) -> JobHandle<T>
    where F: FnOnce() -> T + Send + 'static,
    T:Send + 'static
    {
        let (tx, rx) = oneshot::channel::<T>();
        let wrapper = move || {
            let res = fun();
            tx.send(res);
        };
        self.sender.send(Jobs::Job(Box::new(wrapper)));
        JobHandle { rx }
    }
}

/// How many workers to spawn, leaving headroom for main/render/audio threads.
pub fn worker_count(cores: usize) -> usize {
    let reserve = match cores {
        0..=3 => 1,
        4..=5 => 2,
        6..=9 => 3,
        _ => 4,
    };
    cores.saturating_sub(reserve).max(1)
}

fn detect_cores() -> usize {
    #[cfg(not(target_family = "wasm"))]
    {
        std::thread::available_parallelism().map_or(2, |n| n.get())
    }
    #[cfg(target_family = "wasm")]
    {
        let Some(win) = web_sys::window() else { return 1 };
        // Without cross-origin isolation there is no SharedArrayBuffer -> no threads.
        let isolated = js_sys::Reflect::get(&win, &"crossOriginIsolated".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !isolated {
            crate::debug_warn("page is not crossOriginIsolated; running single-threaded");
            return 1;
        }
        (win.navigator().hardware_concurrency() as usize).max(1)
    }
}

pub fn init_global_pool_auto() {
    let cores = detect_cores();
    let workers = std::env::var("EFE_THREADS")          // optional override (native)
        .ok().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| worker_count(cores));
    crate::debug_info(&format!("cores: {cores}, pool workers: {workers}"));
    init_global_pool(workers);
}



/// Handle to a dedicated worker. Not `Clone`: dropping it sends `Jobs::Kill`, which
/// stops the worker after the jobs already queued before it.
pub struct WorkerHandle{
    sender:flume::Sender<Jobs>,
}
impl WorkerHandle {
    pub fn spawn<F,T>(&self,fun:F) -> JobHandle<T>
    where F: FnOnce() -> T + Send + 'static,
    T:Send + 'static
    {
        let (tx, rx) = oneshot::channel::<T>();
        let wrapper = move || {
            let res = fun();
            tx.send(res);
        };
        self.sender.send(Jobs::Job(Box::new(wrapper)));
        JobHandle { rx }
    }
}
impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // Dedicated workers have no kill gate, so this always stops them. The send
        // fails (harmlessly) if the worker is already gone.
        let _ = self.sender.send(Jobs::Kill);
    }
}


/// Counts "please exit" requests for the resizable workers.
///
/// Shrinking the pool adds N to the counter and queues N `Jobs::Kill` on the shared
/// channel. A worker that pops a `Kill` only exits if it can take one request off the
/// counter; otherwise the `Kill` is stale (the shrink was cancelled by a later grow)
/// and it is ignored. This keeps the pool size right even if grow/shrink are mixed
/// quickly while old `Kill` jobs are still queued.
#[derive(Default)]
pub(crate) struct KillGate(AtomicUsize);
impl KillGate {
    pub(crate) fn request(&self, n: usize) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
    /// Cancel up to `n` pending requests, returns how many were cancelled.
    pub(crate) fn cancel(&self, n: usize) -> usize {
        let mut cur = self.0.load(Ordering::SeqCst);
        loop {
            let take = cur.min(n);
            if take == 0 { return 0; }
            match self.0.compare_exchange_weak(cur, cur - take, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return take,
                Err(actual) => cur = actual,
            }
        }
    }
    /// Called by a worker that received `Jobs::Kill`: true if it must exit.
    pub(crate) fn try_take(&self) -> bool {
        self.cancel(1) == 1
    }
}

/// Lock a std `Mutex` without ever waiting in the OS: spin on `try_lock`. A contended std `Mutex`
/// waits with `Atomics.wait` on wasm, which throws on the browser main thread. Use this for locks
/// the main thread can take while another thread holds them (held only for very short sections).
/// A poisoned lock is recovered.
pub(crate) fn lock_nb<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    loop {
        match m.try_lock() {
            Ok(g) => return g,
            Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
        }
    }
}

pub trait ThreadPoolTrait{
    fn new(cores:usize) -> Self;
    fn execute<F>(&self, f: F) where F: FnOnce() + Send + 'static;
    fn send_job(&self,job:Jobs);
    /// Grow or shrink the resizable worker set. Never blocks on running jobs.
    /// Shrinking lets busy workers finish their current job (and any job queued
    /// before the `Kill`) and then exit. Call from one thread (the main thread).
    fn change_worker_amount(&self,new_size:usize);
    /// Number of resizable workers the pool is aiming for.
    fn worker_amount(&self) -> usize;
    fn new_dedicated_worker(&self) -> WorkerHandle;
}

/// Size requested before the pool exists (0 = nothing requested, use the default).
static CORES: AtomicUsize = AtomicUsize::new(0);
const DEFAULT_POOL_SIZE: usize = 2;

/// The global pool. It is created on first use with the size recorded by
/// `init_global_pool`/`set_global_pool_size` and can be resized afterwards
/// (`ThreadPool::change_worker_amount` takes `&self`).
pub static GLOBAL_POOL: Lazy<ThreadPool> = Lazy::new(|| {
    let mut cores = CORES.load(Ordering::SeqCst);
    if cores == 0 {
        cores = DEFAULT_POOL_SIZE;
        #[cfg(not(target_arch = "wasm32"))]
        println!("UYARI: init_global_pool çağrılmadı, varsayılan olarak 2 thread açılıyor.");
    }
    ThreadPool::new(cores)
});

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn __init_thread_pool(a:usize){
    init_global_pool(a);
}

pub fn init_global_pool(a:usize){
    CORES.store(a.max(1), Ordering::SeqCst);
    Lazy::force(&GLOBAL_POOL);
}

/// Change the number of global pool workers at runtime (clamped to >= 1).
/// Before the pool exists this only records the size. Call from the main thread.
/// On shrink, running jobs finish first; on wasm nothing blocks.
pub fn set_global_pool_size(n:usize){
    let n = n.max(1);
    CORES.store(n, Ordering::SeqCst);
    if let Some(pool) = Lazy::get(&GLOBAL_POOL) {
        pool.change_worker_amount(n);
    }
}

/// Current (target) number of global pool workers, or the recorded size if the pool
/// has not been created yet.
pub fn global_pool_size() -> usize{
    match Lazy::get(&GLOBAL_POOL) {
        Some(pool) => pool.worker_amount(),
        None => match CORES.load(Ordering::SeqCst) {
            0 => DEFAULT_POOL_SIZE,
            n => n,
        },
    }
}

/// Tests that assert the exact global pool size hold this; `Executor::new` takes it while it grows the pool.
#[cfg(test)]
pub(crate) static POOL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct JobHandle<T>{
    rx:oneshot::Receiver<T>
}
impl<T> JobHandle<T> {
    fn new(rx:oneshot::Receiver<T>) -> JobHandle<T> {
        Self{rx}
    }
    pub fn join(self) -> T{
        #[cfg(not(target_family = "wasm"))]
        return self.rx.recv().unwrap();
        #[cfg(target_family = "wasm")]
        {
            use crate::debug_warn;
            debug_warn("thread join in wasm is experimental.");
            return self.rx.recv().unwrap();
        }
    }
    pub fn is_finished(&self)->bool{
        self.rx.has_message()
    }
    pub fn recv_timeout(&self,dur:std::time::Duration) -> Result<T, oneshot::RecvTimeoutError> {
        self.rx.recv_timeout(dur)
    }
    pub fn try_recv(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

// todo better return types
pub fn spawn_global<F,T>(mut a:F) -> JobHandle<T> 
    where F: FnOnce() -> T + Send + 'static,
    T:Send + 'static{
    let (tx, rx) = oneshot::channel::<T>();
    let wrapper = move || {
        let res = a();
        tx.send(res);
    };
    
    GLOBAL_POOL.execute(wrapper);
    JobHandle { rx }
}
pub fn spawn_job_global(job:Jobs){
    GLOBAL_POOL.send_job(job);
}
pub fn new_worker_global() -> WorkerHandle {
    GLOBAL_POOL.new_dedicated_worker()
}


#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if cond() { return true; }
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    #[test]
    fn grow_then_shrink() {
        let pool = ThreadPool::new(2);
        assert_eq!(pool.worker_amount(), 2);
        pool.change_worker_amount(5);
        assert_eq!(pool.worker_amount(), 5);
        assert_eq!(pool.live_workers(), 5);
        pool.change_worker_amount(5);
        assert_eq!(pool.worker_amount(), 5);
        pool.change_worker_amount(1);
        assert_eq!(pool.worker_amount(), 1);
        assert!(wait_until(|| pool.live_workers() == 1));
        assert_eq!(pool.live_workers(), 1);
        // still works with the single remaining worker
        let hit = Arc::new(AtomicUsize::new(0));
        let h = hit.clone();
        let (tx, rx) = oneshot::channel::<()>();
        pool.execute(move || { h.fetch_add(1, Ordering::SeqCst); let _ = tx.send(()); });
        rx.recv().unwrap();
        assert_eq!(hit.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropped_dedicated_workers_are_pruned() {
        let pool = ThreadPool::new(1);
        let handles: Vec<_> = (0..4).map(|_| pool.new_dedicated_worker()).collect();
        assert_eq!(pool.dedicated_count(), 4);
        // a handle that is still alive keeps its worker
        let keep = handles.into_iter().next().unwrap();
        assert!(wait_until(|| pool.dedicated_count() == 1));
        let (tx, rx) = oneshot::channel::<u32>();
        let _ = keep.spawn(move || { let _ = tx.send(7); });
        assert_eq!(rx.recv().unwrap(), 7);
        drop(keep);
        assert!(wait_until(|| pool.dedicated_count() == 0));
        // creating new ones prunes the finished ones too
        let a = pool.new_dedicated_worker();
        drop(a);
        assert!(wait_until(|| { pool.change_worker_amount(1); pool.dedicated_count() == 0 }));
    }

    #[test]
    fn jobs_complete_across_resizes() {
        let pool = ThreadPool::new(2);
        let count = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        let submit = |pool: &ThreadPool, n: usize, handles: &mut Vec<oneshot::Receiver<()>>| {
            for _ in 0..n {
                let c = count.clone();
                let (tx, rx) = oneshot::channel::<()>();
                pool.execute(move || {
                    std::thread::sleep(Duration::from_millis(1));
                    c.fetch_add(1, Ordering::SeqCst);
                    let _ = tx.send(());
                });
                handles.push(rx);
            }
        };
        submit(&pool, 50, &mut handles);
        pool.change_worker_amount(6);
        submit(&pool, 50, &mut handles);
        pool.change_worker_amount(1);
        submit(&pool, 50, &mut handles);
        pool.change_worker_amount(3);
        submit(&pool, 50, &mut handles);
        for h in handles { h.recv().unwrap(); }
        assert_eq!(count.load(Ordering::SeqCst), 200);
        assert_eq!(pool.worker_amount(), 3);
        assert!(wait_until(|| pool.live_workers() == 3));
        assert_eq!(pool.live_workers(), 3);
    }

    #[test]
    fn global_size_api() {
        // Uses the global pool; `Executor::new` (which may grow it) takes the same lock in tests.
        let _g = POOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        init_global_pool(2);
        set_global_pool_size(2); // the pool may already exist (and be larger) from other tests
        assert_eq!(global_pool_size(), 2);
        set_global_pool_size(0);
        assert_eq!(global_pool_size(), 1);
        set_global_pool_size(3);
        assert_eq!(global_pool_size(), 3);
        let count = Arc::new(AtomicUsize::new(0));
        let hs: Vec<_> = (0..20).map(|_| { let c = count.clone(); spawn_global(move || { c.fetch_add(1, Ordering::SeqCst); }) }).collect();
        set_global_pool_size(2);
        for h in hs { h.join(); }
        assert_eq!(count.load(Ordering::SeqCst), 20);
        assert_eq!(global_pool_size(), 2);
    }
}
