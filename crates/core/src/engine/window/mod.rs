use raw_window_handle::{HasDisplayHandle, HasWindowHandle};


#[cfg(target_family = "wasm")]
pub mod web_backend;
#[cfg(target_family = "wasm")]
mod web_impls;

#[cfg(not(target_family = "wasm"))]
pub mod sdl_backend;
#[cfg(not(target_family = "wasm"))]
mod sdl_impls;
pub mod events;

use crate::engine::renderer::SurfaceManager;

/// Platform window. Deliberately not `Send`/`Sync`: native (SDL) windows and the
/// event pump must stay on the main thread.
pub trait InnerWindow: HasWindowHandle + HasDisplayHandle{
    #[cfg(target_family = "wasm")]
    fn set_canvas_id(&mut self,canvas_id:String);
    #[cfg(target_family = "wasm")]
    fn canvas(&self) -> Option<web_sys::HtmlCanvasElement>;
    #[cfg(target_family = "wasm")]
    fn canvas_id(&self) -> &str;
    fn set_title(&mut self,title:String);
    fn size(&self) -> (u32, u32);
    /// Physical pixels per logical pixel (DPI scale / devicePixelRatio).
    fn scale_factor(&self) -> f64 { 1.0 }
    fn poll_events(&mut self);
}
pub trait WindowManager{
    fn size(&self) -> (u32,u32);
    fn scale_factor(&self) -> f64;
    fn poll_events(&mut self);
    fn set_title(&mut self,title:String);

}
pub struct GameWindowManager{
    // NOTE: field order matters. Fields drop in declaration order, and the
    // surface was created from `inner`'s raw window handle, so it must be
    // dropped before the window it renders to.
    pub surface_manager: Option<SurfaceManager>,
    inner: Box<dyn InnerWindow>,
}

impl GameWindowManager {
    pub fn new(
        title:String
    ) -> Self {
        #[cfg(target_family = "wasm")]
        let w = web_backend::WebWindow::new(title);
        #[cfg(not(target_family = "wasm"))]
        let w = sdl_backend::SdlWindowManager::new(title.clone());
        
        return Self {
            surface_manager: None, // Başlangıçta yok
            inner:Box::new(w),
        };
    }
    pub fn poll_events(&mut self ){
        self.inner.poll_events();
    }
    /// The canvas the window draws to (web only).
    #[cfg(target_family = "wasm")]
    pub fn canvas(&self) -> Option<web_sys::HtmlCanvasElement>{
        self.inner.canvas()
    }
    #[cfg(target_family = "wasm")]
    pub fn canvas_id(&self) -> &str{
        self.inner.canvas_id()
    }
    #[cfg(target_family = "wasm")]
    pub fn set_canvas_id(&mut self, canvas_id:String){
        self.inner.set_canvas_id(canvas_id);
    }

    
}

impl WindowManager for GameWindowManager {
    fn size(&self) -> (u32,u32) {
        self.inner.size()
    }

    fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }
    
    fn poll_events(&mut self) {
        self.inner.poll_events();
    }
    
    fn set_title(&mut self,title:String) {
        self.inner.set_title(title);
    }
}

// GameWindow üzerinden handle'lara erişim sağlamak için delegasyon yapıyoruz
impl HasWindowHandle for GameWindowManager {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for GameWindowManager {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.inner.display_handle()
    }
}