use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sdl3::{EventPump, event::Event as SdlEvent};

use super::sdl_impls::{logical_key, modifiers, mouse_button, physical_key};
use crate::engine::{
    error::EngineError,
    input::{
        axis_from_i16, trigger_from_i16, ElementState, GamepadAxis, GamepadButton, GamepadId, GamepadState, InputEvent, InputEventKind, TouchPhase, WheelUnit, WindowEvent as EngineWindowEvent,
        WindowId,
    },
    window::InnerWindow,
};

/// SDL window + event pump. Neither is `Send`/`Sync`: SDL requires windows and
/// event polling to stay on the main thread, so this type must not leave it.
pub struct SdlWindowManager{
    // NOTE: field order matters: open pads must drop before the gamepad subsystem
    // (they also hold their own subsystem clone, so this is belt and braces).
    /// Open gamepads with the joystick id they were opened with (kept because the
    /// id can no longer be queried from a pad once SDL has detached it).
    open_pads: Vec<(sdl3::joystick::JoystickId, sdl3::gamepad::Gamepad)>,
    gamepad_subsystem: Option<sdl3::GamepadSubsystem>,
    main_window:sdl3::video::Window,
    event_pump:EventPump,
    video: sdl3::VideoSubsystem,
    mouse: sdl3::mouse::MouseUtil,
    /// Keeps the SDL context alive for as long as the window exists.
    _sdl:sdl3::Sdl,
    /// Set once the initial Resized/ScaleFactorChanged have been sent.
    sent_initial_state: bool,
    /// Last display scale reported, to emit `ScaleFactorChanged` only on change.
    last_scale: f64,
}

fn now_ns() -> u64 {
    // SAFETY: plain tick counter query, valid once SDL is initialised (and harmless before).
    unsafe { sdl3::sys::timer::SDL_GetTicksNS() }
}

impl SdlWindowManager {
    /// Display scale of the main window. `SDL_GetWindowDisplayScale` returns 0.0 on
    /// failure, which would collapse the UI, so anything not finite and positive is 1.0.
    fn current_scale(&self) -> f64 {
        let s = self.main_window.display_scale() as f64;
        if s.is_finite() && s > 0.0 { s } else { 1.0 }
    }

    /// `size` is the logical window size.
    pub fn new(title: &str, size: (u32, u32)) -> Result<Self, EngineError> {
        sdl3::hint::set("SDL_VIDEO_WAYLAND_PREFER_LIBDECOR", "0");
        // Touch is reported through Finger* events; do not also synthesise mouse events.
        sdl3::hint::set(sdl3::hint::names::TOUCH_MOUSE_EVENTS, "0");
        sdl3::log::set_log_priorities(sdl3::log::Priority::Verbose);

        let werr = |e: &dyn std::fmt::Display| EngineError::Window(e.to_string());
        let s = sdl3::init().map_err(|e| werr(&e))?;
        let video = s.video().map_err(|e| werr(&e))?;
        let window: sdl3::video::Window = video
            .window(title, size.0, size.1)
            .position_centered()
            .resizable()
            .build()
            .map_err(|e| werr(&e))?;
        let pump = s.event_pump().map_err(|e| werr(&e))?;
        let mouse = s.mouse();

        video.text_input().start(&window);

        // Gamepads are optional: never fail window creation over them.
        let gamepad_subsystem = match s.gamepad() {
            Ok(g) => Some(g),
            Err(e) => {
                log::warn!("gamepad subsystem unavailable: {e}");
                None
            }
        };
        let mut open_pads = Vec::new();
        if let Some(g) = &gamepad_subsystem {
            match g.gamepads() {
                Ok(ids) => {
                    for id in ids {
                        match g.open(id) {
                            Ok(pad) => open_pads.push((id, pad)),
                            Err(e) => log::warn!("failed to open gamepad {id}: {e}"),
                        }
                    }
                }
                Err(e) => log::warn!("failed to list gamepads: {e}"),
            }
        }

        Ok(Self{
            open_pads,
            gamepad_subsystem,
            main_window: window,
            event_pump: pump,
            video,
            mouse,
            _sdl: s,
            sent_initial_state: false,
            last_scale: 0.0,
        })
    }
}

impl InnerWindow for SdlWindowManager {
    fn set_title(&mut self,title:String) {
        if let Err(e) = self.main_window.set_title(&title) {
            log::warn!("failed to set window title: {e}");
        }
    }
    /// Drawable size in physical pixels (matches the wgpu surface on HiDPI).
    fn size(&self) -> (u32, u32) {
        self.main_window.size_in_pixels()
    }

    /// Window display scale (SDL_GetWindowDisplayScale).
    fn scale_factor(&self) -> f64 {
        self.current_scale()
    }

    fn logical_size(&self) -> (u32, u32) {
        self.main_window.size()
    }

    fn set_text_input(&mut self, enabled: bool) {
        let ti = self.video.text_input();
        if enabled {
            ti.start(&self.main_window);
        } else {
            ti.stop(&self.main_window);
        }
    }

    fn set_cursor_visible(&mut self, visible: bool) {
        self.mouse.show_cursor(visible);
    }

    fn set_cursor_grab(&mut self, grab: bool) {
        self.mouse.set_relative_mouse_mode(&self.main_window, grab);
    }

    fn poll_events(&mut self, out: &mut Vec<InputEvent>) {
        use sdl3::event::{DisplayEvent, WindowEvent};

        if !self.sent_initial_state {
            self.sent_initial_state = true;
            let ts = now_ns();
            let (width, height) = self.main_window.size_in_pixels();
            out.push(InputEvent::with_time(
                InputEventKind::Window(EngineWindowEvent::Resized { width, height }),
                ts,
                WindowId::MAIN,
            ));
            self.last_scale = self.current_scale();
            out.push(InputEvent::with_time(
                InputEventKind::Window(EngineWindowEvent::ScaleFactorChanged { scale: self.last_scale }),
                ts,
                WindowId::MAIN,
            ));
        }

        let mut close_sent = false;
        // Set on pixel-size changes: fractional-scale changes on the same display are not
        // reported by sdl3 0.14.42 as DISPLAY_SCALE_CHANGED, so the scale is re-read once
        // at the end of the poll.
        let mut scale_dirty = false;
        // Drain the pump (also keeps the OS from deeming the window frozen).
        // Collected first: handling needs `&self` while the pump borrows `&mut self`.
        let events: Vec<SdlEvent> = self.event_pump.poll_iter().collect();
        for event in events {
            let mut push = |ts: u64, window: WindowId, kind: InputEventKind| {
                out.push(InputEvent::with_time(kind, ts, window));
            };
            match event {
                SdlEvent::Quit { timestamp } => {
                    if !close_sent {
                        close_sent = true;
                        push(timestamp, WindowId::MAIN, InputEventKind::Window(EngineWindowEvent::CloseRequested));
                    }
                }
                SdlEvent::Window { timestamp, window_id, win_event } => {
                    let window = win_id(&self.main_window, window_id);
                    match win_event {
                        WindowEvent::CloseRequested => {
                            if !close_sent {
                                close_sent = true;
                                push(timestamp, window, InputEventKind::Window(EngineWindowEvent::CloseRequested));
                            }
                        }
                        WindowEvent::FocusGained => {
                            push(timestamp, window, InputEventKind::Window(EngineWindowEvent::Focused(true)));
                        }
                        WindowEvent::FocusLost => {
                            push(timestamp, window, InputEventKind::Window(EngineWindowEvent::Focused(false)));
                        }
                        // Pixel size (not logical size) so the surface matches the drawable on HiDPI.
                        WindowEvent::PixelSizeChanged(w, h) => {
                            push(timestamp, window, InputEventKind::Window(EngineWindowEvent::Resized {
                                width: w.max(0) as u32,
                                height: h.max(0) as u32,
                            }));
                            scale_dirty = true;
                        }
                        WindowEvent::DisplayChanged(_) => {
                            let scale = self.current_scale();
                            if scale != self.last_scale {
                                self.last_scale = scale;
                                push(timestamp, window, InputEventKind::Window(EngineWindowEvent::ScaleFactorChanged { scale }));
                            }
                        }
                        _ => {}
                    }
                }
                SdlEvent::Display { timestamp, display_event: DisplayEvent::ContentScaleChanged, .. } => {
                    let scale = self.current_scale();
                    if scale != self.last_scale {
                        self.last_scale = scale;
                        push(timestamp, WindowId::MAIN, InputEventKind::Window(EngineWindowEvent::ScaleFactorChanged { scale }));
                    }
                }
                SdlEvent::KeyDown { timestamp, window_id, keycode, scancode, keymod, repeat, .. } => {
                    let kind = InputEventKind::Key {
                        physical: physical_key(scancode),
                        logical: logical_key(keycode, scancode, keymod),
                        state: ElementState::Pressed,
                        repeat,
                        mods: modifiers(keymod),
                    };
                    push(timestamp, win_id(&self.main_window, window_id), kind);
                }
                SdlEvent::KeyUp { timestamp, window_id, keycode, scancode, keymod, .. } => {
                    let kind = InputEventKind::Key {
                        physical: physical_key(scancode),
                        logical: logical_key(keycode, scancode, keymod),
                        state: ElementState::Released,
                        repeat: false,
                        mods: modifiers(keymod),
                    };
                    push(timestamp, win_id(&self.main_window, window_id), kind);
                }
                SdlEvent::TextInput { timestamp, window_id, text } => {
                    if !text.is_empty() {
                        push(timestamp, win_id(&self.main_window, window_id), InputEventKind::Text(text));
                    }
                }
                SdlEvent::MouseMotion { timestamp, window_id, which, x, y, xrel, yrel, .. } => {
                    if which == sdl3::sys::touch::SDL_TOUCH_MOUSEID { continue; }
                    let (sx, sy) = pixel_ratio_of(&self.main_window);
                    push(timestamp, win_id(&self.main_window, window_id), InputEventKind::MouseMotion {
                        pos: (x * sx, y * sy),
                        delta: (xrel * sx, yrel * sy),
                    });
                }
                SdlEvent::MouseButtonDown { timestamp, window_id, which, mouse_btn, x, y, .. } => {
                    if which == sdl3::sys::touch::SDL_TOUCH_MOUSEID { continue; }
                    let (sx, sy) = pixel_ratio_of(&self.main_window);
                    push(timestamp, win_id(&self.main_window, window_id), InputEventKind::MouseButton {
                        button: mouse_button(mouse_btn),
                        state: ElementState::Pressed,
                        pos: (x * sx, y * sy),
                    });
                }
                SdlEvent::MouseButtonUp { timestamp, window_id, which, mouse_btn, x, y, .. } => {
                    if which == sdl3::sys::touch::SDL_TOUCH_MOUSEID { continue; }
                    let (sx, sy) = pixel_ratio_of(&self.main_window);
                    push(timestamp, win_id(&self.main_window, window_id), InputEventKind::MouseButton {
                        button: mouse_button(mouse_btn),
                        state: ElementState::Released,
                        pos: (x * sx, y * sy),
                    });
                }
                SdlEvent::MouseWheel { timestamp, window_id, which, x, y, direction, .. } => {
                    if which == sdl3::sys::touch::SDL_TOUCH_MOUSEID { continue; }
                    let sign = if matches!(direction, sdl3::mouse::MouseWheelDirection::Flipped) { -1.0 } else { 1.0 };
                    push(timestamp, win_id(&self.main_window, window_id), InputEventKind::MouseWheel {
                        delta: (x * sign, y * sign),
                        unit: WheelUnit::Line,
                    });
                }
                // Touch: SDL (sdl3 0.14.42) exposes no finger-cancel variant;
                // SDL_EVENT_FINGER_CANCELED arrives as `Event::Unknown` without finger id,
                // so `TouchPhase::Cancelled` is never produced natively.
                SdlEvent::FingerDown { timestamp, finger_id, x, y, .. } => {
                    let (pw, ph) = self.main_window.size_in_pixels();
                    push(timestamp, WindowId::MAIN, InputEventKind::Touch {
                        id: finger_id, phase: TouchPhase::Started, pos: (x * pw as f32, y * ph as f32),
                    });
                }
                SdlEvent::FingerMotion { timestamp, finger_id, x, y, .. } => {
                    let (pw, ph) = self.main_window.size_in_pixels();
                    push(timestamp, WindowId::MAIN, InputEventKind::Touch {
                        id: finger_id, phase: TouchPhase::Moved, pos: (x * pw as f32, y * ph as f32),
                    });
                }
                SdlEvent::FingerUp { timestamp, finger_id, x, y, .. } => {
                    let (pw, ph) = self.main_window.size_in_pixels();
                    push(timestamp, WindowId::MAIN, InputEventKind::Touch {
                        id: finger_id, phase: TouchPhase::Ended, pos: (x * pw as f32, y * ph as f32),
                    });
                }
                SdlEvent::GamepadAdded { which, .. } => {
                    // SDL3: `which` is the joystick instance id.
                    if let Some(g) = &self.gamepad_subsystem {
                        let known = self.open_pads.iter().any(|(id, _)| *id == which);
                        if !known {
                            match g.open(which) {
                                Ok(pad) => self.open_pads.push((which, pad)),
                                Err(e) => log::warn!("failed to open gamepad {which}: {e}"),
                            }
                        }
                    }
                }
                SdlEvent::GamepadRemoved { which, .. } => {
                    self.open_pads.retain(|(id, _)| *id != which);
                }
                _ => {}
            }
        }

        if scale_dirty {
            let scale = self.current_scale();
            if scale != self.last_scale {
                self.last_scale = scale;
                out.push(InputEvent::with_time(
                    InputEventKind::Window(EngineWindowEvent::ScaleFactorChanged { scale }),
                    now_ns(),
                    WindowId::MAIN,
                ));
            }
        }
    }

    fn poll_gamepads(&mut self, state: &mut GamepadState) {
        use sdl3::gamepad::{Axis, Button};
        const BUTTONS: [(GamepadButton, Button); 15] = [
            (GamepadButton::South, Button::South),
            (GamepadButton::East, Button::East),
            (GamepadButton::West, Button::West),
            (GamepadButton::North, Button::North),
            (GamepadButton::Back, Button::Back),
            (GamepadButton::Guide, Button::Guide),
            (GamepadButton::Start, Button::Start),
            (GamepadButton::LeftStick, Button::LeftStick),
            (GamepadButton::RightStick, Button::RightStick),
            (GamepadButton::LeftShoulder, Button::LeftShoulder),
            (GamepadButton::RightShoulder, Button::RightShoulder),
            (GamepadButton::DPadUp, Button::DPadUp),
            (GamepadButton::DPadDown, Button::DPadDown),
            (GamepadButton::DPadLeft, Button::DPadLeft),
            (GamepadButton::DPadRight, Button::DPadRight),
        ];
        let mut ids = Vec::with_capacity(self.open_pads.len());
        for (joystick_id, pad) in &self.open_pads {
            let id = GamepadId(joystick_id.raw());
            ids.push(id);
            if state.get(id).is_none() {
                state.connect(id, pad.name().unwrap_or_else(|| "gamepad".to_string()));
            }
            for (b, sb) in BUTTONS {
                state.set_button(id, b, pad.button(sb));
            }
            let stick = |a: Axis| axis_from_i16(pad.axis(a));
            state.set_axis(id, GamepadAxis::LeftX, stick(Axis::LeftX));
            state.set_axis(id, GamepadAxis::LeftY, stick(Axis::LeftY));
            state.set_axis(id, GamepadAxis::RightX, stick(Axis::RightX));
            state.set_axis(id, GamepadAxis::RightY, stick(Axis::RightY));
            let lt = trigger_from_i16(pad.axis(Axis::TriggerLeft));
            let rt = trigger_from_i16(pad.axis(Axis::TriggerRight));
            state.set_axis(id, GamepadAxis::LeftTrigger, lt);
            state.set_axis(id, GamepadAxis::RightTrigger, rt);
            state.set_button(id, GamepadButton::LeftTrigger, lt > 0.5);
            state.set_button(id, GamepadButton::RightTrigger, rt > 0.5);
        }
        state.retain(&ids);
    }
}

fn win_id(main: &sdl3::video::Window, id: u32) -> WindowId {
    if id == main.id() { WindowId::MAIN } else { WindowId(id) }
}

/// Physical pixels per logical window unit, per axis (1.0 when a size is 0).
fn pixel_ratio_of(w: &sdl3::video::Window) -> (f32, f32) {
    let (lw, lh) = w.size();
    let (pw, ph) = w.size_in_pixels();
    let r = |p: u32, l: u32| if l == 0 { 1.0 } else { p as f32 / l as f32 };
    (r(pw, lw), r(ph, lh))
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
