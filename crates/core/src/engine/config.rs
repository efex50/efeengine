//! One-shot startup configuration, set before `Engine::new` runs.
//! On the web it is passed from JS as `__engine_start({ ... })`.

use once_cell::sync::OnceCell;
use serde::Deserialize;

/// Startup options. Every field is optional; JS uses camelCase names.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartupConfig {
    /// Path of the wasm-bindgen JS glue, used to spawn thread-pool workers (web).
    pub wasm_js_path: Option<String>,
    /// Worker thread count; `None` means automatic.
    pub threads: Option<usize>,
    /// Id of the `<canvas>` to render to (web); `None` means "canvas".
    pub canvas_id: Option<String>,
    /// Reserved for a future external/extension API. Only stored for now.
    pub external_api: bool,
    /// Which browser graphics API to use (web only).
    pub graphics_backend: GraphicsBackend,
}

/// Browser graphics API choice. JS: `"auto" | "webgpu" | "webgl"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsBackend {
    /// WebGPU when the browser has a working adapter, else WebGL2.
    /// Desktop Linux uses WebGL2: Chrome's WebGPU there sets up fine but then
    /// fails to present to the canvas (Dawn `ImportMemory` errors every frame).
    #[default]
    Auto,
    /// WebGPU, falling back to WebGL2 only if the browser has no WebGPU adapter.
    Webgpu,
    /// Always WebGL2.
    Webgl,
}

static STARTUP: OnceCell<StartupConfig> = OnceCell::new();

/// Set the startup config. Only the first call wins; later calls warn and return `Err`.
pub fn set_startup_config(cfg: StartupConfig) -> Result<(), StartupConfig> {
    // thread_pool/web.rs reads `WASM_PATH` directly; keep it in sync with the config.
    let path = cfg.wasm_js_path.clone();
    let res = STARTUP.set(cfg);
    if let (Ok(()), Some(p)) = (&res, path) {
        let _ = crate::engine::WASM_PATH.set(p);
    }
    if res.is_err() {
        log::warn!("startup config was already set; ignoring the new one");
    }
    res
}

/// The startup config (default if none was set; this also locks it in).
pub fn startup_config() -> &'static StartupConfig {
    STARTUP.get_or_init(StartupConfig::default)
}

/// Path of the wasm-bindgen JS glue: the startup config, else the deprecated
/// `set_wasmjs_path` value (both also end up in `WASM_PATH`).
pub fn wasm_js_path() -> Option<&'static str> {
    startup_config()
        .wasm_js_path
        .as_deref()
        .or_else(|| crate::engine::WASM_PATH.get().map(String::as_str))
}

/// Used by the `#[main]` macro's generated `__engine_start`. Not public API.
#[doc(hidden)]
#[cfg(target_arch = "wasm32")]
pub fn __start_from_js(config: wasm_bindgen::JsValue) -> Result<(), wasm_bindgen::JsValue> {
    if !(config.is_undefined() || config.is_null()) {
        let cfg: StartupConfig = serde_wasm_bindgen::from_value(config)?;
        let _ = set_startup_config(cfg);
    }
    Ok(())
}
