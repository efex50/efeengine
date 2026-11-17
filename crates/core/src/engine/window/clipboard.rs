//! Clipboard text, main thread only.
//!
//! Native: SDL's clipboard while SDL video is initialised; otherwise (headless engines,
//! tests) an in-process store, so a `set` followed by a `get` round-trips. Web: not supported,
//! `clipboard_text()` is `None` and `set_clipboard_text` is `false` (DOM inputs handle
//! copy / paste themselves).

#[cfg(not(target_family = "wasm"))]
mod imp {
    use std::sync::Mutex;

    static FALLBACK: Mutex<Option<String>> = Mutex::new(None);

    fn video_up() -> bool {
        // SAFETY: SDL_WasInit only reads init state; fine before SDL_Init.
        unsafe { sdl3::sys::init::SDL_WasInit(sdl3::sys::init::SDL_INIT_VIDEO).0 != 0 }
    }

    pub fn get() -> Option<String> {
        let text = if video_up() {
            // SAFETY: returns an owned NUL-terminated string (or null) that we free.
            unsafe {
                let p = sdl3::sys::clipboard::SDL_GetClipboardText();
                if p.is_null() {
                    None
                } else {
                    let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
                    sdl3::sys::stdinc::SDL_free(p.cast());
                    Some(s)
                }
            }
        } else {
            FALLBACK.lock().ok().and_then(|g| g.clone())
        };
        text.filter(|s| !s.is_empty())
    }

    pub fn set(text: &str) -> bool {
        if video_up() {
            match std::ffi::CString::new(text) {
                // SAFETY: valid NUL-terminated string for the call.
                Ok(c) => unsafe { sdl3::sys::clipboard::SDL_SetClipboardText(c.as_ptr()) },
                Err(_) => false,
            }
        } else if let Ok(mut g) = FALLBACK.lock() {
            *g = Some(text.to_owned());
            true
        } else {
            false
        }
    }
}

#[cfg(target_family = "wasm")]
mod imp {
    pub fn get() -> Option<String> { None }
    pub fn set(_text: &str) -> bool { false }
}

/// The clipboard text, `None` if empty or unavailable (always `None` on the web).
pub fn clipboard_text() -> Option<String> {
    imp::get()
}

/// Replace the clipboard text; `false` on failure (always `false` on the web).
pub fn set_clipboard_text(text: &str) -> bool {
    imp::set(text)
}
