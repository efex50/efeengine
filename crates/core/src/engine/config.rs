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
    /// Id of the element the UI backend (`efeengine-ui`, web) builds its widgets in; `None`
    /// means "ui". Created inside the canvas's parent when the page has no such element.
    pub ui_layer_id: Option<String>,
    /// Reserved for a future external/extension API. Only stored for now.
    pub external_api: bool,
    /// Which browser graphics API to use (web only).
    pub graphics_backend: GraphicsBackend,
    /// Which GPU path to use (native; the web only has wgpu). On native the environment
    /// variable `EFE_GPU_MODE` (same values) is used when this is `auto`.
    pub gpu_mode: GpuMode,
}

/// GPU path, chosen once at startup. JS / env: `"auto" | "wgpu" | "wgpu-gl" | "sdl"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuMode {
    /// wgpu on Vulkan / Metal / DX12, then wgpu on GL when none works, then (native only) SDL mode
    /// when there is no wgpu adapter at all (a log line says so).
    #[default]
    Auto,
    /// Same native order as `auto`, but never falls back to SDL.
    Wgpu,
    /// wgpu on its GL backend only (GL 3.3+ / GLES 3.0); for old drivers and for testing.
    WgpuGl,
    /// SDL_Renderer, no wgpu context: 2D only, no iced UI (native only; on the web it warns and uses wgpu).
    Sdl,
}

impl GpuMode {
    /// Parse the config / env spelling.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "wgpu" => Some(Self::Wgpu),
            "wgpu-gl" => Some(Self::WgpuGl),
            "sdl" => Some(Self::Sdl),
            _ => None,
        }
    }

    /// The mode in effect: the startup config, or `EFE_GPU_MODE` (native) when that is `auto`.
    pub fn effective() -> Self {
        let cfg = startup_config().gpu_mode;
        #[cfg(not(target_arch = "wasm32"))]
        if cfg == Self::Auto {
            if let Ok(v) = std::env::var("EFE_GPU_MODE") {
                match Self::parse(&v) {
                    Some(m) => return m,
                    None => crate::global_warn(&format!("EFE_GPU_MODE={v:?} is not one of auto, wgpu, wgpu-gl, sdl; using auto")),
                }
            }
        }
        cfg
    }
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

/// How many threads run the parallel scheduler (see `docs/PARALLEL_SPEC.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Threads {
    /// Pool size from the core count (`worker_count`), or `EFE_THREADS`. Headless: `Single`.
    /// wasm without crossOriginIsolated: `Single`.
    #[default]
    Auto,
    /// `n` participants in total (the calling thread plus `n - 1` pool workers).
    /// `Fixed(1)` is the same as `Single`.
    Fixed(usize),
    /// Everything runs on the calling thread; the pool is never touched.
    Single,
}

/// Engine configuration passed to `Engine::with_config`.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// Window title (and `document.title` on the web).
    pub title: String,
    /// Initial window size in logical pixels (native only; the web canvas keeps its CSS size).
    pub size: (u32, u32),
    /// Request vsync. Stored now, consumed by the renderer in Phase 4.
    pub vsync: bool,
    /// FixedUpdate step in seconds.
    pub fixed_dt: f64,
    /// Frame-rate cap for the native loop; `None` = uncapped. `Message::SetFrameRate` changes it.
    pub max_fps: Option<f32>,
    /// Parallelism of the scheduler; `Auto` takes the startup config's value, else automatic.
    pub threads: Threads,
    /// Id of the `<canvas>` (web); `None` = the startup config's value, else `"canvas"`.
    pub canvas_id: Option<String>,
    /// No window, no graphics, no global message queue: for tests and tools.
    pub headless: bool,
    /// The script cache (memory and disk) of the engine.
    pub script_cache: crate::engine::scripting::CacheConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            title: "efeengine".to_string(),
            size: (800, 600),
            vsync: true,
            fixed_dt: 1.0 / 60.0,
            max_fps: Some(60.0),
            threads: Threads::Auto,
            canvas_id: None,
            headless: false,
            script_cache: Default::default(),
        }
    }
}

impl EngineConfig {
    /// Default config with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), ..Self::default() }
    }

    /// A headless config (see [`EngineConfig::headless`]).
    pub fn headless() -> Self {
        Self { headless: true, ..Self::default() }
    }

    /// Fill `threads` and `canvas_id` from the startup config (set from JS) where they are `None`.
    pub fn merged_with_startup(mut self) -> Self {
        let startup = startup_config();
        if self.threads == Threads::Auto {
            if let Some(n) = startup.threads {
                self.threads = Threads::Fixed(n);
            }
        }
        if self.canvas_id.is_none() {
            self.canvas_id = startup.canvas_id.clone();
        }
        self
    }

    /// Check the values; called by `Engine::with_config`.
    pub fn validate(&self) -> Result<(), crate::engine::error::EngineError> {
        use crate::engine::error::EngineError;
        if !(self.fixed_dt.is_finite() && self.fixed_dt > 0.0) {
            return Err(EngineError::Config(format!("fixed_dt must be > 0, got {}", self.fixed_dt)));
        }
        if !self.headless && (self.size.0 == 0 || self.size.1 == 0) {
            return Err(EngineError::Config(format!("window size must be non-zero, got {:?}", self.size)));
        }
        if self.threads == Threads::Fixed(0) {
            return Err(EngineError::Config("threads must be >= 1".to_string()));
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let c = EngineConfig::default();
        assert_eq!(c.title, "efeengine");
        assert_eq!(c.size, (800, 600));
        assert_eq!(c.fixed_dt, 1.0 / 60.0);
        assert_eq!(c.max_fps, Some(60.0));
        assert!(!c.headless);
    }

    #[test]
    fn validate_rejects_bad_fixed_dt() {
        let mut c = EngineConfig::default();
        c.fixed_dt = 0.0;
        assert!(c.validate().is_err());
        c.fixed_dt = f64::NAN;
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_size_depends_on_headless() {
        let mut c = EngineConfig::default();
        c.size = (0, 600);
        assert!(c.validate().is_err());

        let mut h = EngineConfig::headless();
        h.size = (0, 0);
        assert!(h.validate().is_ok());
    }

    #[test]
    fn validate_rejects_zero_threads() {
        let mut c = EngineConfig::default();
        c.threads = Threads::Fixed(0);
        assert!(c.validate().is_err());
    }

    #[test]
    fn new_sets_title() {
        assert_eq!(EngineConfig::new("x").title, "x");
    }
}

#[cfg(test)]
mod gpu_mode_tests {
    use super::*;

    #[test]
    fn gpu_mode_spellings() {
        assert_eq!(GpuMode::parse("wgpu-gl"), Some(GpuMode::WgpuGl));
        assert_eq!(GpuMode::parse(" SDL "), Some(GpuMode::Sdl));
        assert_eq!(GpuMode::parse("gl"), None);
        let cfg: StartupConfig = serde_json::from_str(r#"{"gpuMode":"wgpu-gl"}"#).unwrap();
        assert_eq!(cfg.gpu_mode, GpuMode::WgpuGl);
        assert_eq!(StartupConfig::default().gpu_mode, GpuMode::Auto);
    }
}
