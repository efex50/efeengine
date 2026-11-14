use std::sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Arc, Mutex};

use js_sys::Array;
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};
use web_sys::WorkerOptions;

use crate::{WASM_PATH, debug_info, global_info, thread::Jobs, thread_pool::{KillGate, ThreadPoolTrait}};

/// Web Worker pool. Same design as the native one: one shared flume channel,
/// shrink = `Kill` jobs. Nothing here ever blocks the main thread: exited workers
/// are only `terminate()`d (by dropping them) on the next resize call.
pub struct ThreadPool{
    receiver:flume::Receiver<Jobs>,
    sender:flume::Sender<Jobs>,
    /// Only locked by resize and dedicated-worker creation on the main thread.
    workers:Mutex<Vec<Worker>>,
    target:AtomicUsize,
    next_id:AtomicUsize,
    kills:Arc<KillGate>,
    url:String,
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
        assert!(cores > 0);
        let (tx,rx) = flume::unbounded::<Jobs>();
        let mut workers = Vec::with_capacity(cores);
        let kills = Arc::new(KillGate::default());

        let url = gen_url(WorkerType::Standart);

        for x in 0..cores{
            let worker = Worker::new(x, rx.clone(),&url, Some(kills.clone()));
            workers.push(worker);
        };
        Self {
            dedicated_workers:Arc::new(Mutex::new(Vec::new())),
            workers:Mutex::new(workers),
            target:AtomicUsize::new(cores),
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
        let mut workers = self.workers.lock().unwrap();
        Self::prune(&mut workers);
        let cur = self.target.load(Ordering::SeqCst);
        if new_size > cur {
            let need = new_size - cur;
            let cancelled = self.kills.cancel(need);
            for _ in 0..need - cancelled {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                workers.push(Worker::new(id, self.receiver.clone(), &self.url, Some(self.kills.clone())));
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

    fn new_dedicated_worker(&self) -> super::WorkerHandle {
        let (tx,rx) = flume::unbounded::<Jobs>();
        let mut ex = self.dedicated_workers.lock().unwrap();
        // the new workers has id +31000 
        let w = Worker::new(ex.len() + 31000, rx, &gen_url(WorkerType::Standart), None);
        ex.push(w);
        super::WorkerHandle{sender:tx}

    }
}


fn gen_url(ty:WorkerType) -> String{
    let module_path= {
        let window = web_sys::window().expect("Global window objesi bulunamadı");
        let href = window.location().href().expect("Location href alınamadı");
        let url = web_sys::Url::new_with_base(&format!("{}",WASM_PATH.get().unwrap()), &href)
            .expect("URL parse edilemedi");
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
            
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &options)
        .expect("worker blob oluşturma hatası");

    web_sys::Url::create_object_url_with_blob(&blob)
        .expect("worker blob url oluşturma hatası") 

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
    fn new(id:usize,rx:flume::Receiver<Jobs>,url:&String,kills:Option<Arc<KillGate>>) -> Self{


        let module = wasm_bindgen::module();
        let memory = wasm_bindgen::memory();
        let done = Arc::new(AtomicBool::new(false));
        let init_data = Box::new(WorkerInit { id, rx, kills, done: done.clone() });

        let mut workeropt = WorkerOptions::new();
        workeropt.set_type(web_sys::WorkerType::Module);
        let worker = web_sys::Worker::new_with_options(url,&workeropt)
            .expect("worker oluşturulamadı");
        let mut msg_object = js_sys::Map::new();
        let ptr = Box::into_raw(init_data) as usize;

        msg_object.set(&JsValue::from_str("memory"), &memory);
        msg_object.set(&JsValue::from_str("id"), &JsValue::from(id));
        msg_object.set(&JsValue::from_str("module"), &module);
        msg_object.set(&JsValue::from_str("ptr"), &JsValue::from((ptr as usize)));
        worker.post_message(&msg_object);
        Self { 
            worker,
            id,
            done,
        }
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
    
}
