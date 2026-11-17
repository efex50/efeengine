//! Manual input test: logs InputState changes (keys, mouse, wheel, text, touch, focus, size).
//! Run natively with `cargo xtask run input_test native` (or `./build.py run input_test native`) and on the web with
//! `cargo xtask run input_test web`; watch the terminal / browser console.
use efeengine_core::prelude::*;
use efeengine_core::input::{ElementState, GamepadAxis, GamepadButton, GamepadId, InputEventKind};

#[main]
fn main() {
    const BUTTONS: [MouseButton; 5] = [
        MouseButton::Left,
        MouseButton::Middle,
        MouseButton::Right,
        MouseButton::Back,
        MouseButton::Forward,
    ];
    let mut engine = match Engine::new("input test") {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine init failed: {e}");
            return;
        }
    };

    engine.add_system(Stage::Startup, |_: &mut Ctx<'_>| {
        global_info("[input] input_test started: press keys, click, scroll, touch; Ctrl+Q quits");
    });

    // per-closure state for change detection
    let mut last_size = (0u32, 0u32);
    let mut last_scale = 0.0f64;
    let mut last_focus = true;
    let mut last_touches = 0usize;
    let mut last_pads: Vec<GamepadId> = Vec::new();
    // per pad: was the left stick past the 0.5 threshold on (x, y) last frame
    let mut stick_over: std::collections::HashMap<GamepadId, (bool, bool)> = std::collections::HashMap::new();

    engine.add_system(Stage::Update, move |cx: &mut Ctx<'_>| {
        for ev in cx.input.events() {
            if let InputEventKind::Key { physical, logical, state, repeat, mods } = &ev.kind {
                match state {
                    ElementState::Pressed if !*repeat => global_info(&format!(
                        "[input] key down physical={physical:?} logical={logical:?} mods={mods:?}"
                    )),
                    ElementState::Released => global_info(&format!(
                        "[input] key up physical={physical:?} logical={logical:?}"
                    )),
                    _ => {}
                }
            }
        }

        let pos = cx.input.mouse_pos();
        for b in BUTTONS {
            if cx.input.mouse_just_pressed(b) {
                global_info(&format!("[input] mouse {b:?} down at {pos:?}"));
            }
            if cx.input.mouse_just_released(b) {
                global_info(&format!("[input] mouse {b:?} up at {pos:?}"));
            }
        }

        let wheel = cx.input.wheel();
        if wheel.0 != 0.0 || wheel.1 != 0.0 {
            global_info(&format!("[input] wheel {wheel:?} (lines)"));
        }

        let text = cx.input.text();
        if !text.is_empty() {
            global_info(&format!("[input] text {text:?}"));
        }

        let touches: Vec<_> = cx.input.touches().collect();
        if touches.len() != last_touches {
            last_touches = touches.len();
            global_info(&format!("[input] touches: {touches:?}"));
        }

        let pad_ids: Vec<GamepadId> = cx.gamepads.pads().map(|p| p.id()).collect();
        if pad_ids != last_pads {
            last_pads = pad_ids;
            let list: Vec<String> =
                cx.gamepads.pads().map(|p| format!("{:?} \"{}\"", p.id(), p.name())).collect();
            global_info(&format!("[input] gamepads ({}): [{}]", cx.gamepads.len(), list.join(", ")));
            stick_over.retain(|id, _| last_pads.contains(id));
        }
        for pad in cx.gamepads.pads() {
            for b in GamepadButton::ALL {
                if pad.just_pressed(b) {
                    global_info(&format!("[input] pad {:?} {b:?} down", pad.id()));
                }
            }
            let (x, y) = (pad.axis(GamepadAxis::LeftX), pad.axis(GamepadAxis::LeftY));
            let over = (x.abs() > 0.5, y.abs() > 0.5);
            let prev = stick_over.insert(pad.id(), over).unwrap_or((false, false));
            if over != prev {
                global_info(&format!("[input] pad {:?} left stick ({x:.2}, {y:.2})", pad.id()));
            }
        }

        let focused = cx.input.focused();
        if focused != last_focus {
            last_focus = focused;
            global_info(&format!("[input] focus {focused}"));
        }

        let size = cx.window.size;
        let scale = cx.window.scale_factor;
        if size != last_size || scale != last_scale {
            last_size = size;
            last_scale = scale;
            global_info(&format!("[input] size {size:?} scale {scale}"));
        }

        if cx.input.modifiers().ctrl() && cx.input.just_pressed(KeyCode::Q) {
            global_info("[input] quit");
            cx.commands.quit();
        }
    });

    engine.game_loop();
}
