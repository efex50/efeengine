//! API registration lock, system enable / run conditions, late add and remove, deferred
//! schedule edits, and the per-mod `.efapi` fragment (docs/SYSTEMS.md, docs/MOD_LOADING.md).

use std::sync::{Arc, Mutex};

use efeengine_core::efa::asm::MemSource;
use efeengine_core::efevm::{ApiType, EventDesc, HostFn, HostOk, StructDesc};
use efeengine_core::prelude::*;
use efeengine_core::cache::{CacheConfig};
use efeengine_core::ecs::{PluginError};
use efeengine_core::cache::{CacheDir};
use efeengine_core::mods::{Grant, LoadInput, ModRequest};
use efeengine_core::ecs::{ScheduleError};

fn config() -> EngineConfig {
    EngineConfig {
        fixed_dt: 1.0 / 60.0,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    }
}

fn engine() -> Engine {
    let mut e = Engine::with_config(config()).expect("headless engine");
    e.insert_resource(Log::default());
    e
}

#[derive(Default)]
struct Log(Vec<&'static str>);

fn push(name: &'static str) -> impl FnMut(&mut Ctx<'_>) + Send + 'static {
    move |cx| cx.resources.get_mut::<Log>().unwrap().0.push(name)
}

fn take_log(e: &mut Engine) -> Vec<&'static str> {
    std::mem::take(&mut e.resources_mut().get_mut::<Log>().unwrap().0)
}

// ---- registration lock ----

#[script_component]
#[derive(Debug, Default, PartialEq)]
struct Late {
    v: f32,
    _pad: u32,
}

struct LatePlugin;
impl Plugin for LatePlugin {
    fn name(&self) -> &str {
        "late"
    }
    fn build(&self, _e: &mut Engine) {}
}

fn host_fn(name: &str) -> HostFn {
    HostFn::new("game", name).doc("A function.").param("v", ApiType::I64)
}

#[test]
fn registration_is_open_until_the_first_frame_then_errors() {
    let mut e = engine();
    assert!(e.api_open());
    e.define_namespace("game", 1, "g", "Game.").unwrap();
    e.register_host_fn(host_fn("early"), |_c, _a, _r| Ok(HostOk::Continue)).unwrap();
    e.add_plugin(LatePlugin).map(|_| ()).unwrap();

    e.run_frames(1, 0.016);
    assert!(!e.api_open());

    let msg = "API registration is closed once the game loop starts";
    let err = e.register_host_fn(host_fn("late"), |_c, _a, _r| Ok(HostOk::Continue)).unwrap_err();
    assert!(err.to_string().contains(msg), "{err}");
    assert!(e.register_component::<Late>().unwrap_err().to_string().contains(msg));
    assert!(e.register_component_owned::<Late>("me").is_err());
    assert!(e.register_struct(StructDesc::new("Late2", 4, 4)).unwrap_err().to_string().contains(msg));
    assert!(e.register_event(EventDesc::new("game", "late").doc("x")).unwrap_err().to_string().contains(msg));
    struct Other;
    impl Plugin for Other {
        fn build(&self, _e: &mut Engine) {}
    }
    match e.add_plugin(Other) {
        Err(PluginError::Closed(_)) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
    // nothing leaked into the index
    assert!(e.api_index().function("game", "late").is_none());
    assert!(e.types().by_name("Late2").is_none());

    // not locked: systems, resources, Rust events
    e.add_system(Stage::Update, |_| {});
    e.insert_resource(5u32);
    e.add_event::<u8>();

    // dev only: reopen
    e.reopen_api();
    assert!(e.api_open());
    e.register_component::<Late>().unwrap();
    e.run_frames(1, 0.016);
    assert!(e.api_open(), "frames after the first do not close it again");
}

const MOD_TOML: &str = "[mod]\nid = \"fx\"\nversion = \"1.0.0\"\n\n[[meta.component]]\nname = \"Hp\"\ndoc = \"Hit points.\"\nfields = [ { name = \"hp\", type = \"f32\" }, { name = \"pos\", type = \"i32\", count = 2 } ]\n";
const MOD_SRC: &str = "\
export struct Pt, \"A point.\" {
    x: i32
    y: i32
}

pub fn sum_impl(a: i64, b: i64) -> i64 {
    reg s
    i64.add s, a, b
    ret s
}

provide sum(a: i64, b: i64) -> (total: i64) = sum_impl, \"Add two numbers.\"

provide event pinged(&Pt), \"Pinged.\"
";

fn fx_mod() -> MemSource {
    MemSource::new().with("mod.toml", MOD_TOML).with("src/main.efa", MOD_SRC)
}

fn load_fx(e: &mut Engine) {
    let files = fx_mod();
    e.load_input(LoadInput::Folder { label: "fx", files: &files, assets: &[] }).unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn mods_register_after_the_game_started() {
    let mut e = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant::default()));
    e.run_frames(3, 0.016);
    assert!(!e.api_open());
    load_fx(&mut e);
    let ix = e.api_index();
    assert!(ix.function("fx", "sum").is_some());
    assert!(ix.event("fx", "pinged").is_some());
    assert!(ix.struct_type("fx.Pt").is_some() || ix.struct_type("Pt").is_some() || ix.types.iter().any(|t| t.owner == "fx"));
    assert!(ix.struct_type("mod:fx/Hp").is_some());
    assert!(e.types().by_name("mod:fx/Hp").is_some());
    // reload and unload keep working after start
    let files = fx_mod();
    e.reload_mod_input(LoadInput::Folder { label: "fx", files: &files, assets: &[] }).unwrap();
    let report = e.unload_mods();
    assert!(report.failed.is_empty());
    assert!(e.api_index().function("fx", "sum").is_none());
    assert!(e.api_index().struct_type("mod:fx/Hp").is_none());
}

// ---- mod fragment ----

#[test]
fn the_mod_fragment_equals_what_the_engine_registers() {
    use efeengine_core::efa::mod_folder::{build_mod, BuildLimits};
    // The fragment, from the declaration alone (no engine, no game loop).
    let built = build_mod(&fx_mod(), None, &[], &BuildLimits::default()).expect("builds");
    let frag = efeengine_core::efemod::api_fragment(&built.manifest, &built.module).expect("fragment");
    assert_eq!(frag.game, "fx");
    assert!(frag.function("fx", "sum").is_some());
    assert!(frag.event("fx", "pinged").is_some());
    assert!(frag.struct_type("mod:fx/Hp").is_some());
    frag.validate().expect("a valid index");
    assert_eq!(efeengine_core::efevm::ApiIndex::from_json(&frag.to_json()).unwrap(), frag);

    // The live index after loading the same mod.
    let mut e = engine();
    e.set_mod_policy(|_: &ModRequest<'_>| Ok(Grant::default()));
    load_fx(&mut e);
    let live = e.api_index();
    let owned = |o: &str| o == "fx" || o == "mod:fx";
    for n in &frag.namespaces {
        assert_eq!(live.namespace(&n.name), Some(n), "namespace {}", n.name);
    }
    for t in &frag.types {
        assert_eq!(live.struct_type(&t.name), Some(t), "struct {}", t.name);
    }
    for f in &frag.functions {
        assert_eq!(live.function(&f.namespace, &f.name), Some(f), "function {}", f.qualified());
    }
    for ev in &frag.events {
        assert_eq!(live.event(&ev.namespace, &ev.name), Some(ev), "event {}", ev.qualified());
    }
    // and nothing owned by the mod is missing from the fragment
    for n in live.namespaces.iter().filter(|n| owned(&n.owner)) {
        assert!(frag.namespace(&n.name).is_some(), "{}", n.name);
    }
    for t in live.types.iter().filter(|t| owned(&t.owner)) {
        assert!(frag.struct_type(&t.name).is_some(), "{}", t.name);
    }
    for f in live.functions.iter().filter(|f| owned(&f.owner)) {
        assert!(frag.function(&f.namespace, &f.name).is_some(), "{}", f.qualified());
    }
    for ev in live.events.iter().filter(|ev| owned(&ev.owner)) {
        assert!(frag.event(&ev.namespace, &ev.name).is_some(), "{}", ev.qualified());
    }
    // it merges with the game index
    efeengine_core::efevm::ApiIndex::merge(&[&Engine::with_config(config()).unwrap().api_index(), &frag]).expect("merges");
}

// ---- enabled / run_if ----

#[test]
fn a_disabled_system_is_skipped_and_can_be_toggled() {
    let mut e = engine();
    e.add_system(Stage::Update, push("a")).name("a");
    e.add_system(Stage::Update, push("b")).name("b").enabled(false);
    e.add_system(Stage::Update, push("c")).name("c");
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["a", "c"]);
    assert_eq!(e.is_system_enabled("b"), Some(false));
    e.set_system_enabled("b", true).unwrap();
    assert_eq!(e.is_system_enabled("b"), Some(true));
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["a", "b", "c"]);
    e.set_system_enabled("a", false).unwrap();
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["b", "c"]);
    assert_eq!(e.set_system_enabled("nope", true), Err(ScheduleError::UnknownSystem("nope".into())));
    assert_eq!(e.is_system_enabled("nope"), None);
    // listed whether enabled or not
    let states = e.system_states(Stage::Update);
    assert!(states.contains(&("a", false)) && states.contains(&("b", true)));
    assert!(e.system_names(Stage::Update).contains(&"a"));
}

struct R0;
struct R1;

#[test]
fn a_disabled_system_does_not_split_a_batch() {
    let mut e = engine();
    let base = e.batch_count(Stage::Update);
    e.add_par_system(Stage::Update, Access::new().write::<R0>(), |_| {}).name("p0").order(1);
    e.add_system(Stage::Update, |_| {}).name("x").order(2).enabled(false);
    e.add_par_system(Stage::Update, Access::new().write::<R1>(), |_| {}).name("p1").order(3);
    // p0 and p1 share one batch: the disabled exclusive system between them does not split it
    assert_eq!(e.batch_count(Stage::Update), base + 1);
    e.set_system_enabled("x", true).unwrap();
    assert_eq!(e.batch_count(Stage::Update), base + 3);
    e.set_system_enabled("x", false).unwrap();
    assert_eq!(e.batch_count(Stage::Update), base + 1);
    // a disabled conflicting par system does not split it either
    e.add_par_system(Stage::Update, Access::new().write::<R0>(), |_| {}).name("p2").order(2).enabled(false);
    assert_eq!(e.batch_count(Stage::Update), base + 1);
    e.run_frames(2, 0.016);
}

#[test]
fn run_if_decides_every_frame() {
    let mut e = engine();
    e.insert_resource(false);
    e.add_system(Stage::Update, push("gated")).run_if(|rc| rc.res::<bool>().is_some_and(|b| *b));
    e.add_par_system(Stage::Update, Access::new().res_mut::<Log>(), |cx| cx.res_mut::<Log>().0.push("par")).name("par").run_if(|rc| rc.time.frame() % 2 == 0);
    e.run_frames(1, 0.016);
    let l = take_log(&mut e);
    assert!(!l.contains(&"gated"));
    *e.resources_mut().get_mut::<bool>().unwrap() = true;
    e.run_frames(1, 0.016);
    assert!(take_log(&mut e).contains(&"gated"));
    // the par one only on some frames
    let mut runs = 0;
    for _ in 0..4 {
        e.run_frames(1, 0.016);
        if take_log(&mut e).contains(&"par") {
            runs += 1;
        }
    }
    assert_eq!(runs, 2);
}

#[test]
fn remove_system_and_late_add() {
    let mut e = engine();
    e.add_system(Stage::Update, push("a")).name("a");
    e.add_system(Stage::PostUpdate, push("a")).name("a");
    e.add_system(Stage::Update, push("b")).name("b");
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["a", "b", "a"]);
    assert_eq!(e.remove_system("a"), Ok(2));
    assert_eq!(e.remove_system("a"), Err(ScheduleError::UnknownSystem("a".into())));
    // late add after the game started: next registration index, so after equal-order systems
    e.add_system(Stage::Update, push("late")).name("late");
    e.add_system(Stage::Update, push("early")).name("early").order(-1);
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["early", "b", "late"]);
}

#[test]
fn deferred_edits_apply_at_the_end_of_the_stage() {
    let mut e = engine();
    e.add_system(Stage::Update, push("victim")).name("victim");
    e.add_system(Stage::Update, |cx| {
        cx.resources.get_mut::<Log>().unwrap().0.push("spawner");
        cx.schedule().add_system(Stage::Update, push("added")).name("added");
        cx.schedule().remove("victim");
    })
    .name("spawner")
    .order(1);
    e.run_frames(1, 0.016);
    // the edit is queued, applied after the stage: this frame still ran the victim, not "added"
    assert_eq!(take_log(&mut e), ["victim", "spawner"]);
    e.set_system_enabled("spawner", false).unwrap();
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["added"]);
    assert_eq!(e.remove_system("victim"), Err(ScheduleError::UnknownSystem("victim".into())));
}

#[test]
fn deferred_enable_and_par_queues_merge_in_order() {
    let mut e = engine();
    e.add_system(Stage::Update, push("sleeper")).name("sleeper").enabled(false);
    e.add_par_system(Stage::Update, Access::new().res_mut::<Log>(), |cx| {
        cx.commands().schedule().set_enabled("sleeper", true);
        cx.commands().schedule().add_par_system(Stage::PostUpdate, Access::new().res_mut::<Log>(), |cx| cx.res_mut::<Log>().0.push("par-late")).name("par-late");
    })
    .name("waker")
    .order(-5);
    // frame 1: the waker's edits apply at the end of Update, so PostUpdate already runs the new system
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["par-late"]);
    // frame 2: the sleeper is awake; the waker queued a second copy of the late system
    e.run_frames(1, 0.016);
    assert_eq!(take_log(&mut e), ["sleeper", "par-late", "par-late"]);
    assert_eq!(e.is_system_enabled("sleeper"), Some(true));
}

// ---- change detection ----

#[test]
fn a_re_enabled_system_sees_changes_since_it_last_ran() {
    let mut e = engine();
    e.register_component::<Late>().unwrap();
    e.track_changes::<Late>();
    let ent = e.world_mut().spawn((Late::default(),));
    let seen: Arc<Mutex<Vec<usize>>> = Arc::default();
    let s = seen.clone();
    e.add_system(Stage::Update, move |cx| {
        s.lock().unwrap().push(cx.query::<(Entity, Changed<Late>)>().iter().count());
    })
    .name("watcher");
    e.run_frames(1, 0.016);
    e.run_frames(1, 0.016);
    assert_eq!(*seen.lock().unwrap(), [1, 0], "added once, then quiet");
    e.set_system_enabled("watcher", false).unwrap();
    // a change while the watcher sleeps
    e.world_mut().get::<&mut Late>(ent).unwrap().v = 3.0;
    e.run_frames(3, 0.016);
    assert_eq!(seen.lock().unwrap().len(), 2, "disabled: did not run");
    e.set_system_enabled("watcher", true).unwrap();
    e.run_frames(2, 0.016);
    assert_eq!(*seen.lock().unwrap(), [1, 0, 1, 0], "sees the change from while it was disabled, once");
}

#[test]
fn host_fn_registered_before_start_still_works_after() {
    // the lock only stops new registrations, calls keep working
    let mut e = engine();
    e.define_namespace("game", 1, "g", "Game.").unwrap();
    e.register_host_fn(host_fn("ok"), |_c, _a, _r| Ok(HostOk::Continue)).unwrap();
    e.run_frames(2, 0.016);
    assert!(e.api_index().function("game", "ok").is_some());
}
