//! Engine-level error type.

/// Errors returned by fallible engine setup (`Engine::new`, window creation, ...).
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The platform window (SDL window, web canvas) could not be created or used.
    #[error("window error: {0}")]
    Window(String),
    /// The graphics backend could not be initialised.
    #[error("graphics error: {0}")]
    Graphics(String),
    /// The engine configuration is invalid.
    #[error("config error: {0}")]
    Config(String),
    /// The thread pool could not be set up.
    #[error("thread pool error: {0}")]
    ThreadPool(String),
}

/// Shorthand for `Result<T, EngineError>`.
pub type EngineResult<T> = Result<T, EngineError>;
