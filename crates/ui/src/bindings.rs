//! Bindings: read-only functions from game state to one node property (API.md section 8).
//!
//! The model is `efeui_core::bindings`; this adds the engine side: [`engine_bind_cx`] builds
//! a [`BindCx`] from the engine's resources, world and time, [`BindCxExt`] reads them back
//! and [`BindExt`] gives `Bind::res` and `Bind::query` (bring the trait into scope).

use efeengine_core::engine::resources::{Res, Resources};
use efeengine_core::engine::time::Time;
use efeengine_core::hecs::{Component, Entity, World};

pub use efeui_core::bindings::*;

/// Evaluates `f` with a [`BindCx`] that carries the engine's resources, world and time.
pub fn engine_bind_cx<R>(resources: &Resources, world: &World, time: &Time, f: impl FnOnce(&BindCx) -> R) -> R {
    let items: [&dyn std::any::Any; 3] = [resources, world, time];
    f(&BindCx::new(&items))
}

/// Typed access to the engine values inside a [`BindCx`] the UI plugin built.
pub trait BindCxExt {
    fn resources(&self) -> &Resources;
    fn world(&self) -> &World;
    fn time(&self) -> &Time;
    fn res<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        self.resources().get::<T>()
    }
}

impl BindCxExt for BindCx<'_> {
    fn resources(&self) -> &Resources {
        self.get::<Resources>().expect("BindCx built without the engine resources")
    }

    fn world(&self) -> &World {
        self.get::<World>().expect("BindCx built without the engine world")
    }

    fn time(&self) -> &Time {
        self.get::<Time>().expect("BindCx built without the engine time")
    }
}

/// Engine constructors for [`Bind`].
pub trait BindExt<T>: Sized {
    /// From resource `R`: `Bind::res(|s: &Score| format!("Score: {}", s.value))`.
    fn res<R: Send + Sync + 'static>(f: impl Fn(&R) -> T + Send + Sync + 'static) -> Self;

    /// From component `C` of `entity` (`None` while the entity or component is missing):
    /// `Bind::query::<Health>(player, |h| h.hp / h.max)`.
    fn query<C: Component>(entity: Entity, f: impl Fn(&C) -> T + Send + Sync + 'static) -> Self;
}

impl<T: 'static> BindExt<T> for Bind<T> {
    fn res<R: Send + Sync + 'static>(f: impl Fn(&R) -> T + Send + Sync + 'static) -> Self {
        Bind::with(move |cx| cx.resources().get::<R>().map(|r| f(&r)))
    }

    fn query<C: Component>(entity: Entity, f: impl Fn(&C) -> T + Send + Sync + 'static) -> Self {
        Bind::with(move |cx| cx.world().get::<&C>(entity).ok().map(|c| f(&c)))
    }
}
