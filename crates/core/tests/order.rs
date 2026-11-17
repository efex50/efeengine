//! System order numbers and plugins.

use std::sync::{Arc, Mutex};

use efeengine_core::prelude::*;
use efeengine_core::ecs::{PluginError};

#[derive(Default)]
struct Log(Vec<&'static str>);

fn engine() -> Engine {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
    e.insert_resource(Log::default());
    e
}

fn push(name: &'static str) -> impl FnMut(&mut Ctx<'_>) + 'static {
    move |cx| cx.resources.get_mut::<Log>().unwrap().0.push(name)
}

fn log(e: &Engine) -> Vec<&'static str> {
    e.resources().get::<Log>().unwrap().0.clone()
}

#[test]
fn order_sorts_systems() {
    let mut e = engine();
    e.add_system(Stage::Update, push("B")).order(10);
    e.add_system(Stage::Update, push("A")).order(-10);
    e.add_system(Stage::Update, push("C"));
    e.run_frames(1, 0.016);
    assert_eq!(log(&e), ["A", "C", "B"]);
}

#[test]
fn same_order_keeps_registration_order() {
    let mut e = engine();
    e.add_system(Stage::Update, push("1")).order(5);
    e.add_system(Stage::Update, push("2")).order(5);
    e.add_system(Stage::Update, push("0")).order(4);
    e.add_system(Stage::Update, push("3")).order(5);
    e.run_frames(1, 0.016);
    assert_eq!(log(&e), ["0", "1", "2", "3"]);
}

struct M0(#[allow(dead_code)] u32);
struct M1(#[allow(dead_code)] u32);

#[test]
fn non_conflicting_par_systems_share_a_batch_regardless_of_order_numbers() {
    let mut e = engine();
    let base = e.batch_count(Stage::Update);
    e.add_par_system(Stage::Update, Access::new().write::<M0>(), |_| {}).order(7);
    e.add_par_system(Stage::Update, Access::new().write::<M1>(), |_| {}).order(7);
    e.add_par_system(Stage::Update, Access::new().write::<M0>(), |_| {}).order(7);
    // script_system (exclusive, -100) | {M0, M1} | {M0}
    assert_eq!(e.batch_count(Stage::Update), base + 2);
    let mut e = engine();
    let base = e.batch_count(Stage::Update);
    e.add_par_system(Stage::Update, Access::new().write::<M0>(), |_| {}).order(1);
    e.add_par_system(Stage::Update, Access::new().write::<M1>(), |_| {}).order(2);
    assert_eq!(e.batch_count(Stage::Update), base + 1);
}

#[test]
fn builtin_script_system_runs_before_default_order() {
    let mut e = engine();
    e.add_system(Stage::Update, push("user"));
    let names = e.system_names(Stage::Update);
    assert!(names[0].contains("script_system"), "{names:?}");
    assert!(names[1].contains("push"), "{names:?}");
}

#[test]
fn after_transform_order_sees_propagated_global_transform() {
    let mut e = engine();
    let parent = e.world_mut().spawn((Transform::from_position(Vec2::new(10.0, 0.0)), GlobalTransform::__identity_for_tests()));
    let child = e.world_mut().spawn((Transform::from_position(Vec2::new(1.0, 0.0)), GlobalTransform::__identity_for_tests(), Parent::new(parent)));
    let seen = Arc::new(Mutex::new((f32::NAN, f32::NAN)));
    for (slot, ord) in [(0usize, order::TRANSFORM - 1), (1, order::TRANSFORM + 1)] {
        let seen = seen.clone();
        e.add_system(Stage::PostUpdate, move |cx| {
            let x = cx.world.get::<&GlobalTransform>(child).unwrap().translation().x;
            let mut s = seen.lock().unwrap();
            if slot == 0 { s.0 = x } else { s.1 = x }
        })
        .order(ord);
    }
    e.run_frames(1, 0.016);
    let s = *seen.lock().unwrap();
    assert_eq!(s.1, 11.0);
    assert_ne!(s.0, 11.0);
}

#[derive(Clone, Copy)]
struct Ev(u32);

#[test]
fn events_and_commands_merge_in_sorted_order() {
    for threads in [Threads::Single, Threads::Fixed(4)] {
        let cfg = EngineConfig { threads, ..EngineConfig::headless() };
        let mut e = Engine::with_config(cfg).unwrap();
        // registered 0,1,2 but ordered 2,0,1
        for (i, ord) in [(0u32, 5), (1, 6), (2, -5)] {
            let acc = match i { 0 => Access::new().write::<M0>(), 1 => Access::new().write::<M1>(), _ => Access::new() };
            e.add_par_system(Stage::Update, acc, move |cx| cx.send(Ev(i))).order(ord);
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        e.add_system(Stage::Update, move |cx| {
            let v: Vec<u32> = cx.events::<Ev>().iter().map(|e| e.0).collect();
            if !v.is_empty() {
                *s2.lock().unwrap() = v;
            }
        })
        .order(100);
        e.run_frames(2, 0.016);
        assert_eq!(*seen.lock().unwrap(), vec![2, 0, 1], "{threads:?}");
    }
}

struct Base;
impl Plugin for Base {
    fn name(&self) -> &str { "base" }
    fn build(&self, e: &mut Engine) {
        e.add_system(Stage::Update, push("base")).order(order::EARLY);
    }
}
struct Needs;
impl Plugin for Needs {
    fn name(&self) -> &str { "needs" }
    fn dependencies(&self) -> &[&'static str] { &["base"] }
    fn build(&self, e: &mut Engine) {
        e.add_system(Stage::Update, push("needs"));
    }
}
struct Outer;
impl Plugin for Outer {
    fn name(&self) -> &str { "outer" }
    fn build(&self, e: &mut Engine) {
        e.add_system(Stage::Update, push("outer-1")).order(1);
        e.add_plugin(Base).unwrap();
        e.add_system(Stage::Update, push("outer-2")).order(2);
    }
}

#[test]
fn plugin_errors() {
    let mut e = engine();
    assert!(matches!(e.add_plugin(Needs), Err(PluginError::MissingDependency { .. })));
    assert!(!e.has_plugin("needs"));
    e.add_plugin(Base).unwrap();
    assert!(matches!(e.add_plugin(Base), Err(PluginError::AlreadyAdded(n)) if n == "base"));
    e.add_plugin(Needs).unwrap().add_plugin(Needs).err().unwrap();
    assert_eq!(e.plugins(), ["base", "needs"]);
}

#[test]
fn nested_plugins_tag_their_systems() {
    let mut e = engine();
    e.add_plugin(Outer).unwrap();
    e.add_system(Stage::Update, push("game")).order(3);
    assert_eq!(e.plugins(), ["outer", "base"]);
    let names = e.system_names(Stage::Update);
    let owners = e.system_owners(Stage::Update);
    assert_eq!(names.len(), owners.len());
    // execution order: base(-500), script(-100), outer-1, outer-2, game
    assert_eq!(owners, [Some("base"), None, Some("outer"), Some("outer"), None]);
    e.run_frames(1, 0.016);
    assert_eq!(log(&e), ["base", "outer-1", "outer-2", "game"]);
}
