use std::sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Arc, Mutex};

use js_sys::Array;
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};
use web_sys::WorkerOptions;

use crate::{debug_info, global_info, thread::Jobs, thread_pool::{KillGate, ThreadPoolTrait}};

/// Web Worker pool. Same design as the native one: one shared flume channel,
/// shrink = `Kill` jobs. Nothing here ever blocks the main thread: a worker that leaves its
/// loop closes its own JS global scope (so it does not linger), and its handle is dropped
/// (`terminate()` as a backstop) the next time the pool prunes.
///
/// If the wasm JS path is not known when the pool is created (`url` is `None`), the
/// pool has no workers. `worker_amount` is then 0, and jobs sent to it stay queued
/// and never run, so a `JobHandle::join` on them blocks. A warning is logged.
pub struct ThreadPool{
    receiver:flume::Receiver<Jobs>,
    sender:flume::Sender<Jobs>,
    /// Only locked by resize and dedicated-worker creation on the main thread.
    workers:Mutex<Vec<Worker>>,
    target:AtomicUsize,
    next_id:AtomicUsize,
    kills:Arc<KillGate>,
    url:Option<String>,
    dedicated_workers:Arc<Mutex<Vec<Worker>>>,

}
unsafe impl Sync for ThreadPool {}
unsafe impl Send for ThreadPool {}

impl ThreadPool {
    /// Terminate workers that already left their loop (flag set by the worker).
    fn prune(workers:&mut Vec<Worker>) {
        workers.retain(|w| !w.done.load(Ordering::SeqCst));
    }
}

impl ThreadPoolTrait for ThreadPool{
    fn new(cores:usize) -> Self {
        let cores = cores.max(1);
        let (tx,rx) = flume::unbounded::<Jobs>();
        let mut workers = Vec::with_capacity(cores);
        let kills = Arc::new(KillGate::default());

        let url = gen_url(WorkerType::Standart);
        match &url {
            Some(url) => {
                for x in 0..cores{
                    if let Some(worker) = Worker::new(x, rx.clone(),url, Some(kills.clone())) {
                        workers.push(worker);
                    }
                };
            }
            None => log::warn!("web thread pool created with no workers: jobs will not run"),
        }
        Self {
            dedicated_workers:Arc::new(Mutex::new(Vec::new())),
            target:AtomicUsize::new(workers.len()),
            workers:Mutex::new(workers),
            next_id:AtomicUsize::new(cores),
            kills,
            url,
            receiver: rx,
            sender: tx,
        }
    }

    fn execute<F>(&self, f: F) where F: FnOnce() + Send + 'static {
        let _ = self.sender.send(Jobs::Job(Box::new(f)));
    }

    fn send_job(&self,job:super::Jobs) {
        let _ = self.sender.send(job);
    }

    fn change_worker_amount(&self,new_size:usize) {
        let new_size = new_size.max(1);
        let mut workers = crate::thread_pool::lock_nb(&self.workers);
        Self::prune(&mut workers);
        let cur = self.target.load(Ordering::SeqCst);
        let Some(url) = self.url.as_deref() else {
            // No worker script URL: nothing can be spawned, so the pool stays empty.
            log::warn!("web thread pool has no workers (no wasm JS path); resize ignored");
            self.target.store(0, Ordering::SeqCst);
            return;
        };
        let mut new_target = new_size;
        if new_size > cur {
            let need = new_size - cur;
            let cancelled = self.kills.cancel(need);
            let mut created = 0;
            for _ in 0..need - cancelled {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                if let Some(w) = Worker::new(id, self.receiver.clone(), url, Some(self.kills.clone())) {
                    workers.push(w);
                    created += 1;
                }
            }
            // Count only what actually exists (worker creation can fail).
            new_target = cur + cancelled + created;
        } else if new_size < cur {
            let n = cur - new_size;
            self.kills.request(n);
            for _ in 0..n {
                let _ = self.sender.send(Jobs::Kill);
            }
        }
        self.target.store(new_target, Ordering::SeqCst);
    }

    fn worker_amount(&self) -> usize {
        // Cheap prune: only when the lock is free (never blocks the main thread).
        if let Ok(mut w) = self.workers.try_lock() {
            Self::prune(&mut w);
        }
        self.target.load(Ordering::SeqCst)
    }

    fn new_dedicated_worker(&self) -> super::WorkerHandle {
        let (tx,rx) = flume::unbounded::<Jobs>();
        let mut ex = crate::thread_pool::lock_nb(&self.dedicated_workers);
        // Dedicated workers whose handle dropped have left their loop and closed themselves.
        Self::prune(&mut ex);
        match gen_url(WorkerType::Standart) {
            Some(url) => {
                // the new workers has id +31000
                match Worker::new(ex.len() + 31000, rx, &url, None) {
                    Some(w) => ex.push(w),
                    // `rx` was moved into the failed worker and dropped: jobs sent are discarded.
                    None => log::warn!("dedicated web worker could not be created; its jobs are discarded"),
                }
            }
            None => {
                // No script URL: the worker is not created and `rx` is dropped, so jobs
                // sent through this handle are discarded.
                log::warn!("dedicated web worker not created (no wasm JS path); its jobs are discarded");
            }
        }
        super::WorkerHandle{sender:tx}

    }
}


fn gen_url(ty:WorkerType) -> Option<String>{
    let Some(js_path) = crate::engine::config::wasm_js_path() else {
        log::warn!("wasm JS path is not set; cannot create a web worker");
        return None;
    };
    let module_path= {
        let Some(window) = web_sys::window() else {
            log::warn!("global window object not found; cannot create a web worker");
            return None;
        };
        let Ok(href) = window.location().href() else {
            log::warn!("location href could not be read; cannot create a web worker");
            return None;
        };
        let Ok(url) = web_sys::Url::new_with_base(js_path, &href) else {
            log::warn!("worker URL could not be parsed; cannot create a web worker");
            return None;
        };
        url.href()
    };

    let worker_str = {
        match ty {
            WorkerType::Standart => include_str!("./worker.js"),
            WorkerType::Once => include_str!("./worker_once.js"),
        }
    };
    let worker_str = worker_str.replace("*project_path*", &module_path);
    let mut blob_parts = Array::new();
    blob_parts.push(&JsValue::from_str(&worker_str));
    let mut options = web_sys::BlobPropertyBag::new();
    options.set_type("application/javascript");
            
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &options) else {
        log::warn!("worker blob could not be created");
        return None;
    };

    match web_sys::Url::create_object_url_with_blob(&blob) {
        Ok(url) => Some(url),
        Err(_) => {
            log::warn!("worker blob URL could not be created");
            None
        }
    }

}

struct WorkerInit {
    id: usize,
    rx: flume::Receiver<Jobs>,
    /// None for dedicated workers: any `Kill` stops them.
    kills: Option<Arc<KillGate>>,
    /// Set by the worker right before it leaves its loop.
    done: Arc<AtomicBool>,
}


pub(super) struct Worker{
    worker:web_sys::Worker,
    id:usize,
    done:Arc<AtomicBool>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.worker.terminate();
        global_info("worker dropping");
    }
}

enum WorkerType{
    Standart,
    Once
}

impl Worker{
    /// `None` when the browser refuses to create or message the worker (CSP, no module workers).
    fn new(id:usize,rx:flume::Receiver<Jobs>,url:&str,kills:Option<Arc<KillGate>>) -> Option<Self>{
        let module = wasm_bindgen::module();
        let memory = wasm_bindgen::memory();
        let done = Arc::new(AtomicBool::new(false));

        let mut workeropt = WorkerOptions::new();
        workeropt.set_type(web_sys::WorkerType::Module);
        let worker = match web_sys::Worker::new_with_options(url,&workeropt) {
            Ok(w) => w,
            Err(err) => {
                log::warn!("web worker could not be created: {err:?}");
                return None;
            }
        };
        let init_data = Box::new(WorkerInit { id, rx, kills, done: done.clone() });
        let ptr = Box::into_raw(init_data) as usize;
        let mut msg_object = js_sys::Map::new();

        msg_object.set(&JsValue::from_str("memory"), &memory);
        msg_object.set(&JsValue::from_str("id"), &JsValue::from(id));
        msg_object.set(&JsValue::from_str("module"), &module);
        msg_object.set(&JsValue::from_str("ptr"), &JsValue::from(ptr));
        if let Err(err) = worker.post_message(&msg_object) {
            log::warn!("web worker init message failed: {err:?}");
            // SAFETY: `ptr` came from `Box::into_raw` above and the message was not delivered,
            // so the worker never takes ownership of it.
            drop(unsafe { Box::from_raw(ptr as *mut WorkerInit) });
            worker.terminate();
            return None;
        }
        Some(Self {
            worker,
            id,
            done,
        })
    }
}


// the function that worker thread calls
#[wasm_bindgen]
pub fn worker_loop(ptr: usize){
    let init_data = unsafe { Box::from_raw(ptr as *mut WorkerInit) };
    let id = init_data.id;
    let rx = &init_data.rx;
    loop {
        let job = rx.recv();
        if let Ok(job) = job{
            debug_info(&format!("thread {} yeni işi aldı",id));

            match job {
                Jobs::Job(fun)=>{fun()},
                Jobs::Kill=>{
                    // Stale kills (shrink was cancelled) are ignored.
                    if init_data.kills.as_ref().map_or(true, |k| k.try_take()) {
                        break;
                    }
                },
            }
        }else {
            break
        }
    }
    init_data.done.store(true, Ordering::SeqCst);
    close_self();
}

/// Close this worker's own global scope: the JS Worker ends right after its loop does, not on
/// the next prune. The `Drop` of `Worker` (terminate) stays as a backstop.
fn close_self() {
    use wasm_bindgen::JsCast;
    let global = js_sys::global();
    if let Some(scope) = global.dyn_ref::<web_sys::DedicatedWorkerGlobalScope>() {
        scope.close();
    }
}
#[wasm_bindgen]
pub fn worker_loop_once(ptr: usize){
    let init_data = unsafe { Box::from_raw(ptr as *mut WorkerInit) };
    let id = init_data.id;
    let rx = &init_data.rx;
    let job = rx.recv();

    if let Ok(job) = job{
        debug_info(&format!("thread {} yeni işi aldı",id));
        match job {
            Jobs::Job(fun)=>{fun()},
            Jobs::Kill=>(),
        }
    }
    close_self();
}
