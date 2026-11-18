// Lets `#[derive(ScriptType)]` (which names `::efeengine_core::...`) work inside this crate.
extern crate self as efeengine_core;

#[allow(unused)]
pub mod engine;
pub mod prelude;

// Internal paths (`crate::Engine`, `crate::debug_info`, ...); not part of the public surface.
#[allow(unused_imports)]
pub(crate) use engine::*;
#[allow(unused_imports)]
pub(crate) use engine::{log::*, messages::*};

//---- domain facades (extra front doors; the full internals stay under `engine`) ----
pub mod math {
    //! Math: all of `glam` (matrices, quaternions, integer vectors, ...).
    pub use glam::*;
}

pub mod render {
    //! Writing a renderer or touching the GPU: `Renderer2D`, `Renderer3D`, `CustomRenderer`, `Gpu`, `Target`, the `RenderFrame` and its parts, textures, `Draw2D` and its `DrawList` / wgpu backend, the `DebugDraw2D` resource, offscreen rendering and the SDL-mode 2D renderer `SdlDraw2D` / `SdlSpriteRenderer` (native), plus the `wgpu` and `bytemuck` crates.
    pub use crate::engine::render::{Renderer2D, Renderer3D, CustomRenderer, Target, Frame2D, Frame3D, RenderOverlay, Draw2DOverlay, Overlay2D, OverlayTextures, OVERLAY_TEXTURE_BASE, Gpu, ExtraSurface, SurfaceFrame, ClearRenderer, PngError, RenderFrame, CameraView, SpriteInstance, MeshInstance, TextureChanges, Textures, Image, TextureError, RenderWarnings, WindowFrame, SpriteRenderer, Draw2D, DrawList, DrawCommand, DrawState, Vertex2D, Rect, RectI, Blend, Filter, DebugDraw2D, GpuTextures, WgpuDraw2D};
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::engine::render::{OffscreenGpu, SdlDraw2D, SdlSpriteRenderer, encode_png, write_png};
    pub use bytemuck;
    pub use wgpu;
}

pub mod input {
    //! The input types: events, keys, modifiers, mouse and gamepads.
    pub use crate::engine::input::{
        InputEvent, InputEventKind, WindowEvent, WindowId, WheelUnit, TouchPhase, NamedKey, Modifiers, ElementState,
        InputState, InputCapture, GameInput, Filter, InputFilter, GamepadState, Gamepad, GamepadId, GamepadButton, GamepadAxis, WHEEL_PIXELS_PER_LINE, WHEEL_LINES_PER_PAGE,
        Key, KeyCode, MouseButton,
    };
}

pub mod scripting {
    //! Running scripts: the VM host, script components, events and errors.
    pub use crate::engine::scripting::{Scripting, ScriptHost, ScriptStats, Script, ScriptEvent, ScriptEventError, ScriptError, ModuleId, ModuleInfo, LogLevel, LogLine};
}

pub mod mods {
    //! Loading mods: policy, sources, reports and errors.
    pub use crate::engine::scripting::{
        ModPolicy, ModRequest, Grant, DefaultPolicy, ModSource, AssetHandler, LoadInput, Loaded, LoadError, LoadReport, Reloaded,
        UnloadReport, ModError, ModOptions, PackageInfo, ModBody, ModItem, ModComponent, SpawnedBy,
    };
    #[cfg(not(target_family = "wasm"))]
    pub use crate::engine::scripting::DirSource;
}

pub mod cache {
    //! The compiled-script cache.
    pub use crate::engine::scripting::{CacheConfig, CacheDir, CacheStats, ScriptCache, CACHE_FORMAT};
}

pub mod ecs {
    //! ECS and type-registry details: hecs, change ticks, the schedule builders and errors.
    pub use hecs::*;
    pub use crate::engine::types::{TypeRegistry, TypeEntry, ScriptField, CompError, TypeError, entity_to_bits, entity_from_bits};
    pub use crate::engine::schedule::{ScheduleCommands, PendingSystem, ScheduleError};
    pub use crate::engine::plugin::PluginError;
    pub use crate::engine::resources::Resources;
    pub use crate::engine::commands::{EngineCommands, ParCommands};
    pub use crate::engine::{EngineState, WindowState, Windows};
}

pub mod window {
    //! The window layer: `CursorIcon`, the clipboard functions, the window traits and backends.
    pub use crate::engine::window::*;
}

pub mod clipboard {
    //! System clipboard text (main thread). Native: SDL; headless / SDL not initialised: an
    //! in-process store; web: unsupported (`None` / `false`, DOM inputs handle it).
    pub use crate::engine::window::{clipboard_text, set_clipboard_text};
}

pub mod thread {
    //! The engine's global thread pool.
    pub use crate::engine::thread_pool::{GLOBAL_POOL, Jobs, init_global_pool, set_global_pool_size, global_pool_size, spawn_global, spawn_job_global};
    #[cfg(target_arch = "wasm32")]
    pub use gloo_timers::callback;
}

pub mod log {
    //! The logger trait and message types (`global_*` / `debug_*` are also in the prelude).
    pub use crate::engine::log::*;
}

/// The ECS used for the game world (ROADMAP Phase 3), re-exported as-is.
pub use hecs;
/// Math types used by components, re-exported as-is.
pub use glam;
/// The GPU API the renderer traits is written against (re-exported so versions always match).
pub use wgpu;
/// Re-exported so `#[script_component]` and renderers need no direct dependency.
pub use bytemuck;
/// The VM crate; its `StructDesc` / `FieldDesc` / `Prim` describe script-visible layouts.
pub use efevm;
pub use efemod;
#[cfg(feature = "efa")]
pub use efa;
#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen;

#[cfg(target_family = "wasm")]
mod wasm{

    use wasm_bindgen::prelude::{Closure, wasm_bindgen};
    use wgpu::web_sys;
    #[wasm_bindgen(js_namespace = console)]
    unsafe extern "C" {
        pub fn log(s: &str);
        pub fn warn(s: &str);
        pub fn error(s: &str);
    }

    #[wasm_bindgen]
    pub fn zort() -> wasm_bindgen::JsValue{


        web_sys::console::log_1(&"safasfi".into());
        let closure = Closure::wrap(Box::new(move |message: String| {
            web_sys::console::log_1(&message.into());
        }) as Box<dyn FnMut(String)>);

        // 2. Extract the JS function wrapper
        let js_func = closure.as_ref().clone();

        // 3. IMPORTANT: Forget the closure to keep it alive
        // If you don't do this, Rust will drop the 'closure' variable 
        // at the end of this scope, and the JS function will become invalid.
        closure.forget();

        js_func
    }
}



#[cfg(test)]
mod tests{
    use serde_json::json;

    use crate::engine::messages::Message;

    /// JS sends messages in serde's default (externally tagged) shape;
    /// pin it so a refactor can't silently break `send_message` from JS.
    #[test]
    fn message_json_shape(){
        let shape = |m: &Message| serde_json::to_value(m).unwrap();
        assert_eq!(shape(&Message::Start), json!("Start"));
        assert_eq!(shape(&Message::Say("hi".into())), json!({ "Say": "hi" }));
        assert_eq!(shape(&Message::SetThreads(4)), json!({ "SetThreads": 4 }));

        let parsed: Message = serde_json::from_value(json!({ "ChangeTitle": "x" })).unwrap();
        assert!(matches!(parsed, Message::ChangeTitle(t) if t == "x"));
    }
}
