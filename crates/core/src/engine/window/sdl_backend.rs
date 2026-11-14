use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sdl3::{EventPump, event::Event as SdlEvent};

use crate::Message;

use crate::{engine::window::InnerWindow, global_warn, send_message, window::events::{KeyCode, KeyMod}};

/// SDL window + event pump. Neither is `Send`/`Sync`: SDL requires windows and
/// event polling to stay on the main thread, so this type must not leave it.
pub struct SdlWindowManager{
    main_window:sdl3::video::Window,
    event_pump:EventPump,
    /// Keeps the SDL context alive for as long as the window exists.
    _sdl:sdl3::Sdl,
}

impl SdlWindowManager {
    pub fn new(title:String) -> Self {
        sdl3::hint::set("SDL_VIDEO_WAYLAND_PREFER_LIBDECOR", "0");
        sdl3::log::set_log_priorities(sdl3::log::Priority::Verbose);
        
        
        let s = sdl3::init().unwrap();
        let video = s.video().unwrap();
        let window: sdl3::video::Window = video
            .window(title.as_str(), 800, 600)
            .position_centered()
            .resizable()
            .build()
            .unwrap();


        let pump = s.event_pump().unwrap();
        Self{
            main_window: window,
            event_pump: pump,
            _sdl: s,
        }
    }
}

impl InnerWindow for SdlWindowManager {
    fn set_title(&mut self,title:String) {
        self.main_window.set_title(&title);        
    }
    /// Drawable size in physical pixels (matches the wgpu surface on HiDPI).
    fn size(&self) -> (u32, u32) {
        self.main_window.size_in_pixels()
    }
    
    fn poll_events(&mut self) {
        // Olayları tüket (pump) ki OS pencerenin donduğunu sanmasın
        for event in self.event_pump.poll_iter() {
            match event {
                SdlEvent::Quit { timestamp } => {
                    send_message(Message::Kill);
                    global_warn("warn: killing the game");
                },
                SdlEvent::KeyDown { timestamp, window_id, keycode, scancode, keymod, repeat, which, raw } => {
                    let engine_keycode = keycode.map(KeyCode::from);
                    let engine_scancode = scancode.map(KeyCode::from);
                    send_message(Message::WindowEvent(super::events::WindowEvents::KeyDown { timestamp, window_id, key_code:engine_keycode , scancode:engine_scancode, keymod: KeyMod::from(keymod), raw }));
                },
                SdlEvent::KeyUp { timestamp, window_id, keycode, scancode, keymod, repeat, which, raw } => {
                    let engine_keycode = keycode.map(KeyCode::from);
                    let engine_scancode = scancode.map(KeyCode::from);
                    send_message(Message::WindowEvent(super::events::WindowEvents::KeyUp { timestamp, window_id, key_code: engine_keycode, scancode: engine_scancode, keymod: KeyMod::from(keymod), raw }));
                }
                // Pixel size (not logical size) so the surface matches the drawable on HiDPI.
                SdlEvent::Window { window_id, win_event: sdl3::event::WindowEvent::PixelSizeChanged(w, h), .. } => {
                    send_message(Message::WindowEvent(super::events::WindowEvents::Resized {
                        window_id,
                        width: w.max(0) as u32,
                        height: h.max(0) as u32,
                    }));
                }

                _ => {}
            }
        }
    }

}


impl HasDisplayHandle for SdlWindowManager {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.main_window.display_handle()
    }
}

impl HasWindowHandle for SdlWindowManager {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.main_window.window_handle()
    }
}