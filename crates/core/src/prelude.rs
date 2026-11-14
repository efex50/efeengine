

pub use crate::{
    Engine,
    EngineStatus,
    log::*,
    engine::*,
    config::{StartupConfig, set_startup_config, startup_config},
    messages::*,
};
pub mod thread{
    pub use crate::thread_pool::{GLOBAL_POOL,Jobs,init_global_pool,set_global_pool_size,global_pool_size,spawn_global,spawn_job_global};
    #[cfg(target_arch = "wasm32")]
    pub use gloo_timers::callback;
}
pub use efeengine_macros::main;
#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen;
