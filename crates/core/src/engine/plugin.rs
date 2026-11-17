//! Plugins: reusable bundles of components, resources and systems.
//!
//! A plugin crate depends on `efeengine-core` and uses its re-exports
//! (`efeengine_core::hecs`, `efeengine_core::glam`) so versions always match. Register
//! components with `register_component_owned::<T>(self.name())` so the API index shows
//! which plugin they belong to. Systems added inside [`Plugin::build`] are tagged with the
//! plugin's name.

use crate::Engine;

/// A unit of engine setup, added with [`Engine::add_plugin`].
pub trait Plugin: 'static {
    /// Unique name; default is the type name.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }
    /// Names of plugins that must be added before this one.
    fn dependencies(&self) -> &[&'static str] {
        &[]
    }
    /// Register components, resources and systems.
    fn build(&self, engine: &mut Engine);
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin `{0}` is already added")]
    AlreadyAdded(String),
    #[error("plugin `{plugin}` needs `{missing}` to be added first")]
    MissingDependency { plugin: String, missing: String },
    #[error("cannot add plugin `{0}`: API registration is closed once the game loop starts; register in setup or Plugin::build (mods declare theirs in mod.toml)")]
    Closed(String),
}
