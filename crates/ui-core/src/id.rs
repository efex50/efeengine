//! Widget ids and owners.

use std::sync::Arc;

/// Generational id of a node in the [`UiTree`](crate::UiTree). Copy; an id whose node was
/// removed stays invalid forever (its slot may be reused, with a new generation).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WidgetId {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl WidgetId {
    /// Slot index (stable while the node lives).
    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the id into one `u64` (generation in the high half); for script host functions.
    pub fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    pub fn from_bits(bits: u64) -> Self {
        Self { index: bits as u32, generation: (bits >> 32) as u32 }
    }
}

/// Who created a node. Set from the tree's current owner (see [`UiTree::set_owner`](crate::UiTree::set_owner)).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Owner {
    #[default]
    Game,
    Plugin(Arc<str>),
    Mod(Arc<str>),
}

impl Owner {
    pub fn plugin(name: &str) -> Self {
        Owner::Plugin(name.into())
    }

    pub fn module(name: &str) -> Self {
        Owner::Mod(name.into())
    }

    /// Maps the engine's owner string (`Engine::current_owner`: a plugin name, or `engine`).
    pub fn from_engine_owner(name: &str) -> Self {
        if name == "engine" { Owner::Game } else { Owner::Plugin(name.into()) }
    }
}
