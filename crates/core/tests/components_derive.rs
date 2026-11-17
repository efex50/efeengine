//! `#[script_component]` and `#[derive(Component)]` (docs/SYSTEMS.md, "Components").

use efeengine_core::efevm::Prim;
use efeengine_core::prelude::*;
use efeengine_core::ecs::{TypeError};

fn engine() -> Engine {
    Engine::with_config(EngineConfig::headless()).expect("headless engine")
}

/// Hand-written long form.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
#[script(name = "Long")]
struct LongForm {
    /// Where.
    pos: Vec2,
    #[script(entity)]
    target: u64,
    speed: f32,
    _pad: u32,
}

/// Short form of the same layout.
#[script_component(name = "Short", tracked)]
#[derive(Default)]
struct ShortForm {
    /// Where.
    pos: Vec2,
    #[script(entity)]
    target: u64,
    speed: f32,
    _pad: u32,
}

#[test]
fn script_component_matches_the_long_form() {
    assert_eq!(size_of::<ShortForm>(), size_of::<LongForm>());
    assert_eq!(align_of::<ShortForm>(), align_of::<LongForm>());
    let (a, b) = (<LongForm as ScriptType>::layout(), <ShortForm as ScriptType>::layout());
    assert_eq!((a.size, a.align), (b.size, b.align));
    assert_eq!(a.fields.len(), b.fields.len());
    for (x, y) in a.fields.iter().zip(&b.fields) {
        assert_eq!((&x.name, x.offset, x.ty, x.count, &x.doc), (&y.name, y.offset, y.ty, y.count, &y.doc));
    }
    assert_eq!(b.field_named("target").unwrap().ty, Prim::Entity);
    assert_eq!(b.name, "Short");
}

#[test]
fn name_and_tracked_take_effect() {
    let mut e = engine();
    let id = e.register::<ShortForm>().unwrap().expect("script component has an id");
    assert_eq!(e.types().get(id).unwrap().name(), "Short");
    assert!(e.world().is_tracked(hecs::ComponentId::of::<ShortForm>()));
    assert_eq!(<ShortForm as ComponentInfo>::NAME, "Short");
    assert!(<ShortForm as ComponentInfo>::SCRIPT);
    assert_eq!(e.types().get(id).unwrap().desc().owner, "engine");

    // untracked by default, the long form still registers
    let id = e.register_component::<LongForm>().unwrap();
    assert_eq!(e.types().get(id).unwrap().name(), "Long");
    assert!(!e.world().is_tracked(hecs::ComponentId::of::<LongForm>()));
}

/// The user's own derive may list what the attribute derives.
#[script_component]
#[derive(Clone, Debug, PartialEq, bytemuck::Pod)]
struct Dup {
    v: f32,
}

#[test]
fn derives_are_not_duplicated() {
    let d = Dup { v: 1.0 };
    assert_eq!(d.clone(), d);
    assert_eq!(<Dup as ScriptType>::NAME, "Dup");
}

#[derive(Component)]
#[component(tracked)]
#[allow(dead_code)]
struct Target(Entity);

#[derive(Component)]
#[component(name = "Wrapped")]
struct Wrap<T: Send + Sync + 'static>(T);

#[derive(Component)]
enum Mood {
    Calm,
}

#[test]
fn derive_component_on_any_type() {
    let mut e = engine();
    assert_eq!(e.register::<Target>().unwrap(), None);
    assert!(e.world().is_tracked(hecs::ComponentId::of::<Target>()));
    assert_eq!(e.register::<Wrap<u8>>().unwrap(), None);
    assert!(!e.world().is_tracked(hecs::ComponentId::of::<Wrap<u8>>()));
    assert_eq!(<Wrap<u8> as ComponentInfo>::NAME, "Wrapped");
    assert!(!<Wrap<u8> as ComponentInfo>::SCRIPT);
    e.register::<Mood>().unwrap();
    let _ = (Mood::Calm, Wrap(1u8).0);
}

struct Owned;
impl Plugin for Owned {
    fn name(&self) -> &str {
        "owned_plugin"
    }
    fn build(&self, engine: &mut Engine) {
        engine.register::<Dup>().unwrap();
    }
}

#[test]
fn register_in_a_plugin_records_the_owner() {
    let mut e = engine();
    e.add_plugin(Owned).map(|_| ()).unwrap();
    let id = e.types().by_name("Dup").unwrap().id();
    assert_eq!(e.types().get(id).unwrap().desc().owner, "owned_plugin");
}

#[test]
fn register_after_start_is_closed() {
    let mut e = engine();
    e.run_frames(1, 0.016);
    assert!(matches!(e.register::<Dup>(), Err(TypeError::Closed { .. })));
    assert!(matches!(e.register::<Target>(), Err(TypeError::Closed { .. })));
}
