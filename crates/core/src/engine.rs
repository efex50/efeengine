use std::sync::{Arc, Mutex};

use once_cell::sync::{Lazy, OnceCell};
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use crate::{debug_info, engine::{messages::{Message}, window::{GameWindowManager, events::WindowEvents}}, log::{Logger, NewDefaultLogger}, recv_messages, renderer::GraphicsContext, set_global_logger, window::WindowManager, world::EngineWorld};
use crate::{global_info, thread_pool::init_global_pool_auto};
pub mod window;
pub mod messages;
pub mod log;
pub mod renderer;
pub mod flags;
pub mod world;
pub mod thread_pool;
pub mod systems;

pub static WASM_PATH:OnceCell<String> =OnceCell::new();
#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn set_wasmjs_path(path:String){
    WASM_PATH.set(path);
}

#[derive(Debug,PartialEq, Eq)]
pub enum EngineStatus{
    Uninitialized,
    Initializing,
    Ready,
    Stopped,
    Running,
    Kill,
}



pub struct EngineState{
    pub world : EngineWorld,

}

pub struct Engine{
    pub windows:GameWindowManager,
    status:EngineStatus,
    pub logger:Box<dyn Logger>,
    pub graphics_context: Option<GraphicsContext>,
    pub state:EngineState
}
impl Engine {

    /// create new game instance with title of the game and project name(for wasm)
    /// 
    /// ```no_run
    /// use efeengine_core::prelude::*;
    /// Engine::new("title",env!("CARGO_PKG_NAME"))
    /// ```
    pub fn new<S:Into<String>>(title:S) -> Self {
        let title = title.into();

        

        // init thread pool
        init_global_pool_auto();


        #[cfg(target_family = "wasm")]
        {
            std::panic::set_hook(Box::new(console_error_panic_hook::hook));
        }
        let w = GameWindowManager::new(title);
        let mut logger = NewDefaultLogger();
        debug_info("engine initilazition finished");
        
        
        let world = EngineWorld::new();
        
        let state = EngineState{
            world
        };

        Self {
            windows: w,
            status:EngineStatus::Uninitialized,
            logger:Box::new(logger),
            graphics_context:None,
            state,
        }
    }
    
    pub fn handle_messages(&mut self){
        for x in recv_messages(){
            match x {
                #[cfg(target_family = "wasm")]
                Message::SetCanvasId(s) => {
                    self.windows.set_canvas_id(s);
                }
                Message::Null => (),
                Message::Say(msg) =>{
                    self.logger.info(&msg);
                },
                Message::Stop =>{
                    self.status = EngineStatus::Stopped;
                },
                Message::Start =>{
                    self.status = EngineStatus::Running;
                },
                Message::Kill => {
                    self.status = EngineStatus::Kill; 
                }
                Message::SetFrameRate(_) => {
    
                },
                Message::ChangeTitle(tit) => {
                    self.windows.set_title(tit.to_string());
                },
                Message::Log(log_msg) => {
                    self.logger.log(&log_msg);
                },
                Message::WindowEvent(WindowEvents::Resized { width, height, .. }) => {
                    self.resize_surface((width, height));
                }
                // Key events etc. are not consumed by the engine yet.
                Message::WindowEvent(_) => {}
            }
        }

    }

    // YENİ: Async Grafik Bağlamı Başlatıcı
    pub async fn init_graphics(&mut self) {
        self.logger.info("Initializing Graphics Context...");
        let graphics_context = GraphicsContext::new().await;
        self.graphics_context = Some(graphics_context);
        self.logger.info("Graphics Context Initialized!");
        
        // SurfaceManager'ı oluştur ve Window'a ata
        {
            let sm = self.graphics_context.as_ref().unwrap().create_surface_manager(&self.windows);
            self.windows.surface_manager = Some(sm);
            self.logger.info("Window Surface Manager Initialized!");
        }
        
    }
    pub fn render(&mut self){
        
    }

    /// Reconfigure the window surface to `size` (physical pixels).
    /// No-op without a graphics context/surface, or for a 0x0 (minimized) size.
    fn resize_surface(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if let (Some(context), Some(sm)) = (&self.graphics_context, &mut self.windows.surface_manager) {
            sm.resize(size, &context.device);
        }
    }
    
    pub fn tick(&mut self){
        // Poll first so input/resize events from this frame are handled this frame.
        self.windows.poll_events();
        self.handle_messages();
        // Çizim Mantığı:
        let mut render_result = Ok(());
        if let Some(ref context) = self.graphics_context {
            if let Some(ref mut sm) = self.windows.surface_manager {
                render_result = sm.render(&context.device, &context.queue,&self.state);
            }
        }
        match render_result {
            Ok(()) => {}
            // Surface no longer matches the window: reconfigure, next frame renders.
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                let size = self.windows.size();
                self.resize_surface(size);
            }
            Err(e) => self.logger.error(&format!("Render error: {:?}", e)),
        }
        
    }

    /// takes ownership of the game and starts the game loop until killed
    pub fn game_loop(mut self){
        #[cfg(not(target_family = "wasm"))]
        self.sdl_loop();
        #[cfg(target_family = "wasm")]
        self.wasm_loop_start();
    }
    #[cfg(target_family = "wasm")]
    pub fn wasm_loop_start(mut self){
        use std::rc::Rc;
        use std::cell::RefCell;

        use crate::global_info;

        // 1. Wrap engine in Rc<RefCell> so it can be shared with the closure
        let engine = Rc::new(RefCell::new(self));
        let f = Rc::new(RefCell::new(None));    
        let g = f.clone();
        // 2. Create the animation frame closure
        *g.borrow_mut() = Some(Closure::new(move || {
            use crate::global_info;

            let mut engine_ref = engine.borrow_mut();
                
            // Run logic
            engine_ref.tick();
            // Request next frame if running
            if engine_ref.status != EngineStatus::Kill {
                request_animation_frame(f.borrow().as_ref().unwrap());
            }else {
                engine_ref.logger.info("Killing the engine");
            }
        }));

        // 3. Start the loop
        request_animation_frame(g.borrow().as_ref().unwrap());
    }


    //#[cfg(not(target_family = "wasm"))]
    fn sdl_loop(&mut self) {
        
        
        pollster::block_on(self.init_graphics());
        'main:loop {
            self.tick();
            std::thread::sleep(std::time::Duration::from_millis(10));
            if self.status == EngineStatus::Stopped { break 'main; }




            match self.status {
                EngineStatus::Uninitialized |
                EngineStatus::Initializing |
                EngineStatus::Ready |
                EngineStatus::Stopped |
                EngineStatus::Running => {},
                EngineStatus::Kill => {
                    std::process::exit(0)
                },
            }
        }
    }
}

#[cfg(target_family = "wasm")]
fn request_animation_frame(f: &Closure<dyn FnMut()>) {
    use wasm_bindgen::JsCast;
    use wgpu::web_sys;

    web_sys::window()
        .expect("no global `window` exists")
        .request_animation_frame(f.as_ref().unchecked_ref())
        .expect("should register `requestAnimationFrame` OK");
}


#[cfg(test)]
mod tests{
    use std::{pin::Pin, sync::mpsc};

    #[test]
    #[cfg(not(target_family = "wasm"))]

    fn platform(){
        let s = sdl3::get_platform();
        println!("{}",s);
    }
    #[test]
    fn mpsc_test(){
        let (tx,rx) = mpsc::channel::<String>();
        
        std::thread::spawn(move||{
            let mut i = 0;
            for x in 0..10{
                tx.send(format!("asfasfas no {i}"));
                i+=1;
            };
        });

        std::thread::sleep(std::time::Duration::from_secs(2));
        loop {
            match rx.recv() {
                Ok(msg) => println!("received \"{}\"",msg),
                Err(err) => {println!("err:{:?}",err);break;},
            }
            
            
        }

    }


    #[test]
    fn parrrralel(){
        let t = std::thread::available_parallelism().unwrap();
        let h = std::thread::spawn(|| {
            
        });
        println!("{}",t)
    }

}