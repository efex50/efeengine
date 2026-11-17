//! `use efeengine_core::prelude::*;` brings in what a typical game file uses. The domains
//! have their own front doors at the crate root: [`render`](crate::render),
//! [`input`](crate::input), [`scripting`](crate::scripting), [`mods`](crate::mods),
//! [`ecs`](crate::ecs), [`math`](crate::math), [`cache`](crate::cache), [`log`](crate::log) and
//! [`thread`](crate::thread). Everything else, in full, is under [`engine`](crate::engine).

pub use crate::engine::{
    Engine,
    config::{EngineConfig, Threads},
    order,
    plugin::Plugin,
    schedule::{Stage, Ctx, SystemEntry, RunCtx},
    parallel::{Access, ParCtx},
    commands::Commands,
    components::{Transform, Velocity, MeshHandle, Name, Camera, TextureHandle, Sprite, Visibility, Parent, Children, GlobalTransform},
    types::{ScriptType, ComponentInfo},
    resources::{Res, ResMut},
    events::Events,
    time::Time,
    input::{Key, KeyCode, MouseButton, InputState},
    render::{SpriteRenderer, Textures},
    log::{global_info, global_warn, global_alert, global_error, debug_info, debug_warn, debug_alert, debug_error},
};
pub use efeengine_macros::{main, Component, script_component, ScriptType};
pub use hecs::{Entity, World, Mut, Changed, Added};
pub use glam::{Vec2, Vec3, Vec4, vec2, vec3, vec4};
#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen;
