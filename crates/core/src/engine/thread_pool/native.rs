use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use crate::{debug_info, thread_pool::{Jobs, KillGate, ThreadPoolTrait, WorkerHandle}};

/// Native pool: one shared MPMC channel, N workers pulling from it.
///
/// Locks: `workers` (resize bookkeeping, only taken by resize/Drop, never by
/// `execute`/`send_job`) and `dedicated_workers`. The hot path is a lock-free
/// `flume` send.
pub struct ThreadPool{
    workers: Mutex<Vec<Worker>>,
    /// Target size of the resizable set (what `change_worker_amount` asked for).
    target: AtomicUsize,
    next_id: AtomicUsize,
    kills: Arc<KillGate>,
    dedicated_workers: Mutex<Vec<Worker>>,
    sender: flume::Sender<Jobs>,
    receiver: flume::Receiver<Jobs>,
}

impl ThreadPool {
    /// Remove workers that already exited (they were told to via `Kill`).
    /// Joining is instant because the flag is set right before the thread returns.
    fn prune(workers: &mut Vec<Worker>) {
        let mut i = 0;
        while i < workers.len() {
            if workers[i].done.load(Ordering::SeqCst) {
                let mut w = workers.swap_remove(i);
                if let Some(t) = w.thread.take() {
                    let _ = t.join();
                }
            } else {
                i += 1;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn live_workers(&self) -> usize {
        let mut w = self.workers.lock().unwrap();
        Self::prune(&mut w);
        w.len()
    }
}

impl ThreadPoolTrait for ThreadPool {
    fn new(size:usize) -> Self{
        assert!(size > 0);
        let (tx,rx) = flume::unbounded::<Jobs>();
        let kills = Arc::new(KillGate::default());
        let mut workers = Vec::with_capacity(size);
        for id in 0..size{
            workers.push(Worker::new(id, rx.clone(), Some(kills.clone())));
        }
        Self {
            workers: Mutex::new(workers),
            target: AtomicUsize::new(size),
            next_id: AtomicUsize::new(size),
            kills,
            dedicated_workers: Mutex::new(Vec::new()),
            sender: tx,
            receiver: rx,
        }
    }

    fn execute<F>(&self, f: F) where F: FnOnce() + Send + 'static {
        // The pool owns a receiver, so the channel is never disconnected.
        let _ = self.sender.send(Jobs::Job(Box::new(f)));
    }

    fn send_job(&self,job:Jobs) {
        let _ = self.sender.send(job);
    }

    fn change_worker_amount(&self,new_size:usize) {
        let new_size = new_size.max(1);
        let mut workers = self.workers.lock().unwrap();
        Self::prune(&mut workers);
        let cur = self.target.load(Ordering::SeqCst);
        if new_size > cur {
            let need = new_size - cur;
            // First cancel shrink requests that no worker has taken yet.
            let cancelled = self.kills.cancel(need);
            for _ in 0..need - cancelled {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                workers.push(Worker::new(id, self.receiver.clone(), Some(self.kills.clone())));
            }
        } else if new_size < cur {
            let n = cur - new_size;
            self.kills.request(n);
            for _ in 0..n {
                let _ = self.sender.send(Jobs::Kill);
            }
        }
        self.target.store(new_size, Ordering::SeqCst);
    }

    fn worker_amount(&self) -> usize {
        self.target.load(Ordering::SeqCst)
    }

    fn new_dedicated_worker(&self) -> WorkerHandle{
        let (tx,rx) = flume::unbounded::<Jobs>();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        // No kill gate: a `Kill` sent to a dedicated worker always stops it.
        let w = Worker::new(id, rx, None);
        self.dedicated_workers.lock().unwrap().push(w);
        WorkerHandle { sender: tx }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        let workers = self.workers.get_mut().unwrap();
        let n = workers.iter().filter(|w| !w.done.load(Ordering::SeqCst)).count();
        self.kills.request(n);
        for _ in 0..n {
            let _ = self.sender.send(Jobs::Kill);
        }
        for worker in workers.iter_mut() {
            if let Some(thread) = worker.thread.take(){
                let _ = thread.join();
            }
        }
        if let Ok(mut extra) = self.dedicated_workers.lock() {
            for worker in extra.iter_mut() {
                if let Some(thread) = worker.thread.take(){
                    // Note: dedicated workers only stop when their WorkerHandle sender is
                    // dropped or a `Kill` was sent; otherwise this join can wait forever.
                    let _ = thread.join();
                }
            }
        }
    }
}

pub struct Worker {
    #[allow(dead_code)]
    id: usize,
    thread: Option<JoinHandle<()>>,
    done: Arc<AtomicBool>,
}

impl Worker {
    fn new(id:usize, rx:flume::Receiver<Jobs>, kills:Option<Arc<KillGate>>) -> Self{
        let done = Arc::new(AtomicBool::new(false));
        let done2 = done.clone();
        let th = thread::spawn(move|| {
            loop {
                match rx.recv() {
                    Ok(Jobs::Job(f)) => {
                        debug_info(&format!("thread {} yeni işi aldı",id));
                        // A panicking job must not take the worker (and the pool size) with it.
                        let _ = catch_unwind(AssertUnwindSafe(f));
                    },
                    Ok(Jobs::Kill) => {
                        // Stale kills (shrink was cancelled) are ignored.
                        if kills.as_ref().map_or(true, |k| k.try_take()) {
                            break;
                        }
                    },
                    // all senders dropped
                    Err(_) => break,
                }
            }
            done2.store(true, Ordering::SeqCst);
        });

        Self { id, thread: Some(th), done }
    }
}
