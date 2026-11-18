//! Scene switching (docs/SCENES.md, phase S2). Headless.

use efeengine_core::ecs::{Scene, SceneError, SceneLoaded, SceneOwned, SceneUnloaded};
use efeengine_core::prelude::*;
use std::sync::{Arc, Mutex};

#[script_component(name = "Coin")]
#[derive(Default, Debug)]
struct Coin {
    value: i32,
}

type Log = Arc<Mutex<Vec<String>>>;

fn engine() -> (Engine, Log) {
    let mut e = Engine::with_config(EngineConfig::headless()).expect("headless engine");
    e.register::<Coin>().unwrap();
    for n in ["a_sys", "b_sys", "shared", "global_sys", "other"] {
        e.add_system(Stage::Update, |_| {}).name(n);
    }
    e.set_global_system("global_sys").unwrap();
    let log: Log = Arc::default();
    let l = log.clone();
    e.add_system(Stage::Update, move |ctx| {
        let mut l = l.lock().unwrap();
        for ev in ctx.events::<SceneUnloaded>().iter() {
            l.push(format!("{} unloaded {}", ev.seq, ev.name));
        }
        for ev in ctx.events::<SceneLoaded>().iter() {
            l.push(format!("{} loaded {}", ev.seq, ev.name));
        }
    })
    .name("log_events");
    (e, log)
}

fn scene(name: &str, systems: &[&str], coins: &[i32]) -> Scene {
    let mut s = Scene { format: 1, name: name.into(), requires: vec![], systems: systems.iter().map(|s| s.to_string()).collect(), entities: vec![] };
    let text = coins.iter().map(|v| format!("(components: {{ \"Coin\": {{ \"value\": {v} }} }})")).collect::<Vec<_>>().join(",");
    let full = format!("Scene(format: 1, name: \"{name}\", entities: [{text}])");
    s.entities = Scene::from_ron(&full).unwrap().entities;
    s
}

fn coins(e: &Engine) -> Vec<i32> {
    let mut v: Vec<i32> = e.world().query::<&Coin>().iter().map(|c| c.value).collect();
    v.sort();
    v
}

fn en(e: &Engine, n: &str) -> bool {
    e.is_system_enabled(n).unwrap()
}

#[test]
fn switch_removes_old_entities_and_toggles_only_its_systems() {
    let (mut e, _) = engine();
    for n in ["a_sys", "b_sys", "shared"] {
        e.set_system_enabled(n, false).unwrap();
    }
    e.load_scene(&scene("A", &["a_sys", "shared", "global_sys"], &[1, 2])).unwrap();
    assert_eq!(coins(&e), vec![1, 2]);
    assert!(en(&e, "a_sys") && en(&e, "shared") && !en(&e, "b_sys"));
    assert_eq!(e.current_scene(), Some("A"));
    // Unrelated and global systems are changed by hand; a switch must leave them alone.
    e.set_system_enabled("global_sys", false).unwrap();
    e.set_system_enabled("other", false).unwrap();

    e.load_scene(&scene("B", &["b_sys", "shared", "global_sys"], &[10])).unwrap();
    assert_eq!(coins(&e), vec![10]);
    assert_eq!(e.world().query::<&SceneOwned>().iter().count(), 1);
    assert!(!en(&e, "a_sys"), "A's own system is disabled");
    assert!(en(&e, "b_sys") && en(&e, "shared"));
    assert!(!en(&e, "global_sys"), "global is never toggled");
    assert!(!en(&e, "other"), "unrelated system untouched");
    assert_eq!(e.current_scene(), Some("B"));

    e.unload_scene();
    assert!(coins(&e).is_empty());
    assert!(!en(&e, "b_sys") && !en(&e, "shared"));
    assert_eq!(e.current_scene(), None);
}

#[test]
fn failed_load_keeps_old_scene() {
    let (mut e, _) = engine();
    e.set_system_enabled("a_sys", false).unwrap();
    e.load_scene(&scene("A", &["a_sys"], &[1, 2])).unwrap();
    let before = e.capture_scene("x");

    let bad_system = scene("B", &["nope"], &[9]);
    assert!(matches!(e.load_scene(&bad_system), Err(SceneError::System { .. })));
    let mut bad_comp = scene("C", &["b_sys"], &[9]);
    bad_comp.entities[0].components.get_mut("Coin").unwrap().insert("bogus".into(), efeengine_core::ecs::SceneValue::Int(1));
    assert!(matches!(e.load_scene(&bad_comp), Err(SceneError::Field { .. })));

    assert_eq!(e.capture_scene("x"), before);
    assert_eq!(coins(&e), vec![1, 2]);
    assert!(en(&e, "a_sys"));
    assert_eq!(e.current_scene(), Some("A"));
    assert_eq!(e.world().query::<&SceneOwned>().iter().count(), 2);
}

#[test]
fn events_fire_in_order() {
    let (mut e, log) = engine();
    e.load_scene(&scene("A", &[], &[1])).unwrap();
    e.load_scene(&scene("B", &[], &[2])).unwrap();
    e.run_frames(2, 1.0 / 60.0);
    // Events of different types are separate queues, so each carries a `seq` from one shared
    // counter. Sort the log by it to get the true global order.
    let log = log.lock().unwrap();
    let mut all: Vec<(u64, String)> = log
        .iter()
        .map(|l| {
            let (seq, rest) = l.split_once(' ').unwrap();
            (seq.parse().unwrap(), rest.to_string())
        })
        .collect();
    all.sort();
    let order: Vec<&str> = all.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(order, ["loaded A", "unloaded A", "loaded B"]);
    let seqs: Vec<u64> = all.iter().map(|(s, _)| *s).collect();
    assert_eq!(seqs, [1, 2, 3], "one counter, no gaps");
}

#[test]
fn switch_from_a_system_is_deferred() {
    let (mut e, _) = engine();
    e.load_scene(&scene("A", &[], &[1])).unwrap();
    let next = scene("B", &["b_sys"], &[5]);
    let fired = Arc::new(Mutex::new(false));
    e.add_system(Stage::Update, move |ctx| {
        let mut f = fired.lock().unwrap();
        if !*f {
            *f = true;
            ctx.commands.load_scene(next.clone());
        }
    });
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(coins(&e), vec![5]);
    assert_eq!(e.current_scene(), Some("B"));
}
