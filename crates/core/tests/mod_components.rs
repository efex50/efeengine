//! Mod-defined (runtime) components: declared in `mod.toml` `[[meta.component]]`, registered as
//! `mod:<mod>/<Name>`, used by scripts and Rust, removed or migrated on unload and reload,
//! limited by the grant. Headless; the script cache is off.

use std::sync::{Arc, Mutex};

use efeengine_core::efa::asm::MemSource;
use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::prelude::*;

type Reports = Arc<Mutex<Vec<i64>>>;

fn config(threads: Threads) -> EngineConfig {
    EngineConfig {
        fixed_dt: 1.0 / 60.0,
        threads,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    }
}

fn engine_with(threads: Threads, policy: impl FnMut(&ModRequest<'_>) -> Result<Grant, String> + 'static) -> (Engine, Reports) {
    let mut e = Engine::with_config(config(threads)).expect("headless engine");
    e.define_namespace("test", 1, "testgame", "Helpers of the test game.").unwrap();
    let got: Reports = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    e.register_host_fn(HostFn::new("test", "report").doc("Record `v`.").param("v", ApiType::I64), move |_c, a, _r| {
        g.lock().unwrap().push(a.i64(0)?);
        Ok(HostOk::Continue)
    })
    .unwrap();
    e.set_mod_policy(policy);
    (e, got)
}

fn grant_default(_: &ModRequest<'_>) -> Result<Grant, String> {
    Ok(Grant { namespaces: vec!["test".into(), "engine.world".into()], ..Grant::default() })
}

fn engine() -> (Engine, Reports) {
    engine_with(Threads::Single, grant_default)
}

fn take(got: &Reports) -> Vec<i64> {
    std::mem::take(&mut *got.lock().unwrap())
}

const PERM: &str = "\n[permissions]\nnamespaces = [\"test\", \"engine.world\"]\n";

fn health_decl(extra: &str) -> String {
    format!(
        "[[meta.component]]\nname = \"Health\"\ndoc = \"Hit points.\"\nfields = [ {{ name = \"hp\", type = \"f32\" }}, {{ name = \"max\", type = \"f32\" }}{extra} ]\n"
    )
}

/// The script of the `hp` mod. `init`: look the component up, spawn an entity and insert
/// `Health {100, 100}`. `frame`: report `changed`, the status of `get`, the bits of `hp`; on
/// the first frame also `set` hp to 55 and report the bits read back at once.
fn script() -> String {
    "import engine.world.component_id
import engine.world.spawn
import engine.world.insert
import engine.world.get
import engine.world.set
import engine.world.changed
import engine.world.tick
import test.report

hname db \"mod:hp/Health\"
buf resb 16
global mut cid: i32 = 0
global mut ent: i64 = 0
global mut last: i32 = 0

fn rep32(v: i32) {
    reg r
    i64.extend_i32_s r, v
    call test.report(r)
    ret
}

export fn init() {
    reg id, e, st
    call engine.world.component_id(hname, hname.len) -> id
    global.set cid, id
    call rep32(id)
    call engine.world.spawn() -> e
    global.set ent, e
    f32.store 100.0, buf
    f32.store 100.0, buf, 4
    call engine.world.insert(e, id, buf) -> st
    ret
}

export fn frame(dt: f32) {
    reg id, e, st, since, ch, t, bits
    global.get id, cid
    global.get e, ent
    global.get since, last
    call engine.world.changed(e, id, since) -> ch
    call rep32(ch)
    call engine.world.tick() -> t
    global.set last, t
    call engine.world.get(e, id, buf) -> st
    call rep32(st)
    i32.load bits, buf
    call rep32(bits)
    i32.ne st, since, 0
    br_if st, done
    f32.store 55.0, buf
    call engine.world.set(e, id, buf) -> st
    call rep32(st)
    call engine.world.get(e, id, buf) -> st
    i32.load bits, buf
    call rep32(bits)
done:
    ret
}
"
    .to_string()
}

fn mod_toml(id: &str, comps: &str, extra: &str) -> String {
    format!("[mod]\nid = \"{id}\"\nversion = \"1.0.0\"\n{extra}{PERM}\n{comps}")
}

fn hp_mod_with(comps: &str, script: &str) -> MemSource {
    MemSource::new().with("mod.toml", &mod_toml("hp", comps, "")).with("src/main.efa", script)
}

fn hp_mod() -> MemSource {
    hp_mod_with(&health_decl(""), &script())
}

fn no_assets() -> Vec<String> {
    Vec::new()
}

fn load(e: &mut Engine, files: &MemSource) -> Result<Loaded, LoadError> {
    e.load_input(LoadInput::Folder { label: "f", files, assets: &no_assets() })
}

fn reload(e: &mut Engine, files: &MemSource) -> Result<Reloaded, LoadError> {
    e.reload_mod_input(LoadInput::Folder { label: "f", files, assets: &no_assets() })
}

fn mod_err(e: LoadError) -> ModError {
    match e {
        LoadError::Mod(m) => m,
        other => panic!("expected a mod error, got: {other}"),
    }
}

const HEALTH: &str = "mod:hp/Health";

fn health_id(e: &Engine) -> u32 {
    e.types().by_name(HEALTH).expect("Health is registered").id()
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn f32_bits(v: f32) -> i64 {
    v.to_bits() as i64
}

fn health_bytes(hp: f32, max: f32) -> Vec<u8> {
    [hp.to_le_bytes(), max.to_le_bytes()].concat()
}

/// Every `(entity, bytes)` of component `id`, in world order.
fn dump(e: &Engine, id: u32) -> Vec<(u64, Vec<u8>)> {
    let w = e.world();
    w.query_dynamic(&[id]).map(|en| (entity_to_bits(en), w.get_dynamic(en, id).unwrap().to_vec())).collect()
}

#[test]
fn a_mod_declares_a_component_that_scripts_and_rust_use() {
    let (mut e, got) = engine();
    let game = e.world_mut().spawn((Transform::default(),));
    load(&mut e, &hp_mod()).unwrap();
    let id = health_id(&e);
    let entry = e.types().get(id).unwrap();
    assert!(entry.is_component() && entry.is_dynamic());
    assert_eq!((entry.size(), entry.align()), (8, 4));
    assert_eq!(entry.desc().owner, "mod:hp");
    assert_eq!(entry.desc().component, Some(id));
    assert_eq!(entry.desc().doc, "Hit points.");
    // the same layout is in the API index
    let ix = e.api_index();
    let t = ix.types.iter().find(|t| t.name == HEALTH).expect("in the index");
    assert_eq!((t.size, t.component), (8, Some(id)));
    // init reported the id the script found; a game entity gets the component next to its typed ones
    assert_eq!(take(&got), [id as i64]);
    let types = e.types().clone();
    types.component(id).unwrap().insert(e.world_mut(), game, &health_bytes(7.0, 9.0)).unwrap();
    assert!(e.world().get::<&Transform>(game).is_ok(), "typed components stay");
    assert!(types.component(id).unwrap().has(e.world(), game));

    // frame 1: the script's entity (spawned in init) has changed (1), get ok (0), hp = 100, set ok, read back 55
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(take(&got), [1, 0, f32_bits(100.0), 0, f32_bits(55.0)]);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    let scripted: Vec<_> = dump(&e, id).into_iter().filter(|(en, _)| *en != entity_to_bits(game)).collect();
    assert_eq!(scripted.len(), 1);
    assert_eq!(scripted[0].1, health_bytes(55.0, 100.0), "the set was applied after the stage");

    // Rust reads through Ctx::dyn_get and the DynComponent view
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    e.add_system(Stage::Update, move |cx| {
        let g = cx.dyn_get(game, id).unwrap();
        assert_eq!(g.len(), 8);
        let view = cx.types.view(cx.world, game, id).unwrap();
        s.lock().unwrap().push((view.field_f32("hp"), view.field::<f32>("max"), view.field_f32("nope"), view.field::<u64>("hp")));
    });
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(*seen.lock().unwrap(), [(Some(7.0), Some(9.0), None, None)]);

    // dyn_get_mut stamps the change; an untouched component does not report one
    let mut e2 = e;
    let before = e2.world().change_tick();
    e2.add_system(Stage::PreUpdate, move |cx| {
        let mut m = cx.dyn_get_mut(game, id).unwrap();
        m[0..4].copy_from_slice(&1.5f32.to_le_bytes());
    });
    e2.run_frames(1, 1.0 / 60.0);
    let t = e2.world().component_ticks(game, e2.types().get(id).unwrap().component_id(e2.world()).unwrap()).unwrap().unwrap();
    assert!(t.changed.is_newer_than(before, e2.world().change_tick()), "stamped by the write");
    assert!(errors(&e2).is_empty(), "{:?}", errors(&e2));
}

#[test]
fn script_changed_and_set_overlay_on_a_mod_component() {
    let (mut e, got) = engine();
    load(&mut e, &hp_mod()).unwrap();
    let id = health_id(&e);
    take(&got);
    // a Rust mover bumps the first entity's component every third frame, before the scripts
    let target = std::cell::Cell::new(None::<Entity>);
    let mut n = 0;
    e.add_system(Stage::PreUpdate, move |cx| {
        if target.get().is_none() {
            target.set(cx.world.query_dynamic(&[id]).next());
        }
        if n % 3 == 1
            && let Some(en) = target.get()
        {
            cx.dyn_get_mut(en, id).unwrap()[4] = 0;
        }
        n += 1;
    });
    e.run_frames(6, 1.0 / 60.0);
    let g = take(&got);
    // per frame: changed, get status, hp bits; the first frame adds the set status and read-back
    let changed: Vec<i64> = std::iter::once(g[0]).chain(g[5..].chunks(3).map(|c| c[0])).collect();
    assert_eq!(g[1], 0);
    assert_eq!(g[2], f32_bits(100.0));
    assert_eq!(g[3], 0);
    assert_eq!(g[4], f32_bits(55.0), "a module reads its own set at once");
    // frame 2 sees its own frame-1 set as changed, frames moved by Rust see a change, others do not
    assert_eq!(changed.len(), 6, "{g:?}");
    assert_eq!(changed[0], 1);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert!(changed.iter().all(|c| *c == 0 || *c == 1));
    assert!(changed.contains(&0), "an untouched frame reports no change: {changed:?}");
}

#[test]
fn unload_mods_removes_entities_components_and_types() {
    let (mut e, got) = engine();
    let game = e.world_mut().spawn((Transform::default(),));
    load(&mut e, &hp_mod()).unwrap();
    let id = health_id(&e);
    let gen1 = e.world().dynamic_info(id).unwrap().0;
    let types = e.types().clone();
    types.component(id).unwrap().insert(e.world_mut(), game, &health_bytes(1.0, 2.0)).unwrap();
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(dump(&e, id).len(), 2, "the game entity and the script's");
    let before = e.world().len();
    take(&got);

    let report = e.unload_mods();
    assert_eq!(report.unloaded, ["hp"]);
    assert!(report.failed.is_empty());
    assert_eq!(e.world().len(), before - 1, "the entity the script spawned is gone");
    assert!(e.world().contains(game) && e.world().get::<&Transform>(game).is_ok());
    assert!(!types.component(id).unwrap().has(e.world(), game), "the component left the game entity");
    assert!(e.world().dynamic_info(id).is_none());
    assert!(e.types().by_name(HEALTH).is_none() && e.types().get(id).is_none());
    assert!(e.api_index().types.iter().all(|t| t.name != HEALTH));

    // loading again works and registers a new generation
    load(&mut e, &hp_mod()).unwrap();
    assert_eq!(health_id(&e), id);
    let gen2 = e.world().dynamic_info(id).unwrap().0;
    assert_ne!(gen1, gen2);
    e.run_frames(1, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(dump(&e, id).len(), 1);
}

#[test]
fn reload_with_an_identical_layout_keeps_the_data() {
    let (mut e, got) = engine();
    let game = e.world_mut().spawn((Transform::default(),));
    let id = {
        load(&mut e, &hp_mod()).unwrap();
        health_id(&e)
    };
    let types = e.types().clone();
    types.component(id).unwrap().insert(e.world_mut(), game, &health_bytes(3.0, 4.0)).unwrap();
    let cid = e.world().dynamic_info(id).unwrap().0;
    let ticks = e.world().component_ticks(game, cid).unwrap();
    e.run_frames(2, 1.0 / 60.0);
    take(&got);
    // another script body, same declaration (only the doc differs)
    let mut changed = script();
    changed.push_str("; a comment\n");
    let v2 = hp_mod_with(&health_decl("").replace("Hit points.", "Hit points, v2."), &changed);
    let r = reload(&mut e, &v2).unwrap();
    assert!(matches!(r, Reloaded::Replaced { .. }), "{r:?}");
    assert_eq!(e.world().dynamic_info(id).unwrap().0, cid, "the registration (generation) is untouched");
    assert_eq!(e.world().component_ticks(game, cid).unwrap(), ticks, "ticks stay");
    assert_eq!(e.world().get_dynamic(game, id).unwrap().to_vec(), health_bytes(3.0, 4.0));
    // the old module's entity is despawned, its init spawned a new one
    assert_eq!(dump(&e, id).len(), 2);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
}

#[test]
fn reload_with_a_changed_layout_migrates_by_field_name_and_type() {
    let (mut e, _got) = engine();
    let game = e.world_mut().spawn((Transform::default(),));
    load(&mut e, &hp_mod()).unwrap();
    let id = health_id(&e);
    let types = e.types().clone();
    types.component(id).unwrap().insert(e.world_mut(), game, &health_bytes(3.0, 4.0)).unwrap();
    let cid1 = e.world().dynamic_info(id).unwrap().0;

    // add a field in front: hp and max move, `extra` is zero
    let v2 = hp_mod_with(
        "[[meta.component]]\nname = \"Health\"\nfields = [ { name = \"extra\", type = \"u32\" }, { name = \"hp\", type = \"f32\" }, { name = \"max\", type = \"f32\" }, { name = \"pos\", type = \"f32\", count = 2 } ]\n",
        &script(),
    );
    reload(&mut e, &v2).unwrap();
    let entry = e.types().get(id).unwrap();
    assert_eq!(entry.size(), 20);
    assert_ne!(e.world().dynamic_info(id).unwrap().0, cid1, "a new generation");
    let w = e.world();
    let v = e.types().view(w, game, id).unwrap();
    assert_eq!((v.field::<u32>("extra"), v.field_f32("hp"), v.field_f32("max")), (Some(0), Some(3.0), Some(4.0)));
    assert_eq!(v.field::<[f32; 2]>("pos"), Some([0.0, 0.0]));
    drop(v);
    assert!(e.world().get::<&Transform>(game).is_ok());

    // change a field's type (hp f32 -> i32): the old bytes are not reinterpreted; max stays
    let v3 = hp_mod_with(
        "[[meta.component]]\nname = \"Health\"\nfields = [ { name = \"hp\", type = \"i32\" }, { name = \"max\", type = \"f32\" } ]\n",
        &script(),
    );
    reload(&mut e, &v3).unwrap();
    let v = e.types().view(e.world(), game, id).unwrap();
    assert_eq!((v.field::<i32>("hp"), v.field_f32("max")), (Some(0), Some(4.0)));
    drop(v);

    // the component is dropped by the next version: removed from every entity and unregistered
    let v4 = hp_mod_with("", &script().replace("call engine.world.insert(e, id, buf) -> st", ""));
    reload(&mut e, &v4).unwrap();
    assert!(e.types().by_name(HEALTH).is_none());
    assert!(e.world().dynamic_info(id).is_none());
    assert!(e.world().get::<&Transform>(game).is_ok());
}

#[test]
fn a_failed_reload_keeps_the_declarations_and_the_data() {
    let (mut e, _got) = engine();
    let game = e.world_mut().spawn((Transform::default(),));
    load(&mut e, &hp_mod()).unwrap();
    let id = health_id(&e);
    let types = e.types().clone();
    types.component(id).unwrap().insert(e.world_mut(), game, &health_bytes(3.0, 4.0)).unwrap();
    // a new version that does not compile: fails up front
    let broken = hp_mod_with(&health_decl(", { name = \"z\", type = \"u8\" }"), "export fn init() {\n    nonsense\n}\n");
    let err = reload(&mut e, &broken).unwrap_err();
    assert!(matches!(mod_err(err), ModError::ReloadFailed { .. } | ModError::Build(_)));
    // a new version whose declaration is invalid: refused before anything is touched
    let bad = hp_mod_with("[[meta.component]]\nname = \"Health\"\nfields = []\n", &script());
    assert!(matches!(mod_err(reload(&mut e, &bad).unwrap_err()), ModError::Component { .. } | ModError::ReloadFailed { .. }));
    assert_eq!(e.types().get(id).unwrap().size(), 8);
    assert_eq!(e.world().get_dynamic(game, id).unwrap().to_vec(), health_bytes(3.0, 4.0));
}

#[test]
fn a_rollback_after_a_failed_swap_adopts_the_old_registration_again() {
    let (mut e, _got) = engine();
    let game = e.world_mut().spawn((Transform::default(),));
    load(&mut e, &hp_mod()).unwrap();
    let id = health_id(&e);
    let types = e.types().clone();
    types.component(id).unwrap().insert(e.world_mut(), game, &health_bytes(3.0, 4.0)).unwrap();
    let cid = e.world().dynamic_info(id).unwrap().0;
    // v2 changes the layout but imports engine.input, which the policy does not grant: the swap
    // fails at load, after the old version was unloaded, and the old version is restored
    let s2 = script().replace("import test.report", "import test.report\nimport engine.input.key_down\nhkey db \"W\"").replace(
        "    call engine.world.insert(e, id, buf) -> st\n    ret",
        "    call engine.world.insert(e, id, buf) -> st\n    call engine.input.key_down(hkey, hkey.len) -> st\n    ret",
    );
    let toml = mod_toml("hp", &health_decl(", { name = \"z\", type = \"u8\" }"), "").replace("[\"test\", \"engine.world\"]", "[\"test\", \"engine.world\", \"engine.input\"]");
    let v2 = MemSource::new().with("mod.toml", &toml).with("src/main.efa", &s2);
    let err = mod_err(reload(&mut e, &v2).unwrap_err());
    assert!(matches!(err, ModError::ReloadFailed { restored: true, .. }), "{err:?}");
    assert!(err.to_string().contains("not grant"), "{err}");
    // the old declaration is registered again, the world registration and the data were never touched
    assert_eq!(e.types().get(id).unwrap().size(), 8);
    assert_eq!(e.world().dynamic_info(id).unwrap().0, cid);
    assert_eq!(e.world().get_dynamic(game, id).unwrap().to_vec(), health_bytes(3.0, 4.0));
    assert_eq!(e.scripting().modules().iter().filter(|m| m.loaded && m.name == "hp").count(), 1);
}

#[test]
fn declaration_errors_fail_the_load() {
    let (mut e, _got) = engine();
    let try_decl = |e: &mut Engine, decl: &str| -> String {
        let r = load(e, &hp_mod_with(decl, &script()));
        let err = mod_err(r.unwrap_err());
        assert!(matches!(err, ModError::Component { .. }), "{err:?}");
        assert!(e.scripting().modules().iter().all(|m| !m.loaded || m.name != "hp"));
        assert!(e.world().dynamic_info(TypeRegistry::id_of_name(HEALTH)).is_none(), "nothing stays registered in the world");
        err.to_string()
    };
    let msg = try_decl(&mut e, "[[meta.component]]\nname = \"Health\"\nfields = [ { name = \"hp\", type = \"f33\" } ]\n");
    assert!(msg.contains("unknown type `f33`") && msg.contains("Health"), "{msg}");
    let msg = try_decl(&mut e, "[[meta.component]]\nname = \"Health\"\nfields = [ { name = \"hp\", type = \"f32\" }, { name = \"hp\", type = \"u8\" } ]\n");
    assert!(msg.contains("duplicate field `hp`"), "{msg}");
    let msg = try_decl(&mut e, "[[meta.component]]\nname = \"Health\"\nfields = []\n");
    assert!(msg.contains("at least one field"), "{msg}");
    let msg = try_decl(&mut e, &format!("{}{}", health_decl(""), health_decl("")));
    assert!(msg.contains("declared twice"), "{msg}");
    let msg = try_decl(&mut e, "[[meta.component]]\nname = \"not ok\"\nfields = [ { name = \"a\", type = \"u8\" } ]\n");
    assert!(msg.contains("not an identifier"), "{msg}");
    let msg = try_decl(&mut e, "[[meta.component]]\nname = \"Health\"\nfields = [ { name = \"a\", type = \"u8\", count = 0 } ]\n");
    assert!(msg.contains("count"), "{msg}");
    let msg = try_decl(&mut e, "[[meta.component]]\nname = \"Health\"\ncolour = 1\nfields = [ { name = \"a\", type = \"u8\" } ]\n");
    assert!(msg.contains("unknown key `colour`"), "{msg}");
    let msg = try_decl(&mut e, "[[meta.component]]\nname = \"Health\"\nfields = [ { name = \"a\", type = \"u8\", count = 100000 } ]\n");
    assert!(msg.contains("larger than"), "{msg}");
    // a name that clashes with a registered type
    e.register_struct(efeengine_core::efevm::StructDesc::new(HEALTH, 4, 4)).unwrap();
    let msg = try_decl(&mut e, &health_decl(""));
    assert!(msg.contains("already registered"), "{msg}");
}

#[test]
fn layout_follows_repr_c_rules() {
    let (mut e, _got) = engine();
    let comps = "[[meta.component]]\nname = \"Mixed\"\ntracked = false\nfields = [ { name = \"a\", type = \"u8\" }, { name = \"b\", type = \"f64\" }, { name = \"c\", type = \"i16\", count = 3 }, { name = \"who\", type = \"entity\" }, { name = \"ok\", type = \"bool\" } ]\n";
    load(&mut e, &hp_mod_with(comps, &script())).unwrap();
    let d = e.types().by_name("mod:hp/Mixed").unwrap().desc().clone();
    let off: Vec<(&str, u32)> = d.fields.iter().map(|f| (f.name.as_str(), f.offset)).collect();
    assert_eq!(off, [("a", 0), ("b", 8), ("c", 16), ("who", 24), ("ok", 32)]);
    assert_eq!((d.size, d.align), (40, 8));
    assert!(!e.world().is_tracked(e.types().by_name("mod:hp/Mixed").unwrap().component_id(e.world()).unwrap()));
}

#[test]
fn policy_limits_are_checked_and_a_policy_can_raise_them() {
    let two = format!("{}{}", health_decl(""), "[[meta.component]]\nname = \"Big\"\nfields = [ { name = \"blob\", type = \"u8\", count = 300 } ]\n");
    // defaults: 16 types of 256 bytes: `Big` (300) is too large
    let (mut e, _got) = engine();
    let err = mod_err(load(&mut e, &hp_mod_with(&two, &script())).unwrap_err());
    assert!(matches!(&err, ModError::Component { component, reason, .. } if component == "Big" && reason.contains("grant allows 256")), "{err:?}");
    assert!(e.types().by_name(HEALTH).is_none());

    // too many types
    let many: String = (0..17).map(|i| format!("[[meta.component]]\nname = \"C{i}\"\nfields = [ {{ name = \"a\", type = \"u8\" }} ]\n")).collect();
    let err = mod_err(load(&mut e, &hp_mod_with(&many, &script())).unwrap_err());
    assert!(err.to_string().contains("17 component types") && err.to_string().contains("allows 16"), "{err}");

    // the policy sees the declarations and can raise both limits
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let (mut e, _got) = engine_with(Threads::Single, move |r: &ModRequest<'_>| {
        *s.lock().unwrap() = r.components.iter().map(|c| (c.name.clone(), c.size())).collect::<Vec<_>>();
        Ok(Grant { namespaces: vec!["test".into(), "engine.world".into()], max_component_types: 32, max_component_size: 512, ..Grant::default() })
    });
    load(&mut e, &hp_mod_with(&two, &script())).unwrap();
    assert_eq!(*seen.lock().unwrap(), [("Health".to_string(), 8), ("Big".to_string(), 300)]);
    assert_eq!(e.types().by_name("mod:hp/Big").unwrap().size(), 300);
    let info = e.package(e.scripting().modules().iter().find(|m| m.name == "hp").unwrap().id).unwrap();
    assert_eq!(info.grant.max_component_types, 32);
    assert_eq!(info.components.len(), 2);

    // a policy can also refuse by what it sees
    let (mut e, _got) = engine_with(Threads::Single, |r: &ModRequest<'_>| {
        if r.components.iter().any(|c| c.name == "Health") { Err("no health".into()) } else { Ok(Grant::default()) }
    });
    let err = mod_err(load(&mut e, &hp_mod()).unwrap_err());
    assert!(matches!(err, ModError::Denied { .. }));
}

#[test]
fn hot_reload_in_place_with_changed_declarations_is_refused() {
    // a package watched on disk: same declarations reload in place, changed ones are refused
    let (mut e, _got) = engine();
    let files = hp_mod();
    let built = efeengine_core::efa::mod_folder::build_mod(&files, Some(&e.api_index()), &[], &Default::default()).unwrap();
    let bytes = efeengine_core::efemod::Package::write(&built.manifest, &built.module.encode(), &built.assets).unwrap();
    let dir = std::env::temp_dir().join(format!("efe-modcomp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hp.efmod");
    std::fs::write(&path, &bytes).unwrap();
    let id = e.load_input(LoadInput::Package { label: "hp.efmod", bytes: &bytes }).unwrap().id();
    e.watch_package(id, &path).unwrap();
    // same declarations, new code: allowed
    let mut s2 = script();
    s2.push_str("; v2\n");
    let b2 = efeengine_core::efa::mod_folder::build_mod(&hp_mod_with(&health_decl(""), &s2), Some(&e.api_index()), &[], &Default::default()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(&path, efeengine_core::efemod::Package::write(&b2.manifest, &b2.module.encode(), &b2.assets).unwrap()).unwrap();
    e.run_frames(20, 0.05);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    // changed declarations: refused, the old version keeps running
    let b3 = efeengine_core::efa::mod_folder::build_mod(&hp_mod_with(&health_decl(", { name = \"z\", type = \"u8\" }"), &s2), Some(&e.api_index()), &[], &Default::default()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(&path, efeengine_core::efemod::Package::write(&b3.manifest, &b3.module.encode(), &b3.assets).unwrap()).unwrap();
    e.run_frames(20, 0.05);
    assert!(errors(&e).iter().any(|l| l.contains("declared components changed")), "{:?}", errors(&e));
    assert_eq!(e.types().get(health_id(&e)).unwrap().size(), 8);
    let _ = std::fs::remove_dir_all(&dir);
}

/// One run: two mods with components in their own lanes spawn entities every frame; returns
/// every entity with its bytes, in world order.
fn determinism_run(threads: Threads) -> Vec<(u64, Vec<u8>)> {
    let (mut e, _got) = engine_with(threads, grant_default);
    load(&mut e, &hp_mod()).unwrap();
    // a second mod with its own component and a script that spawns two entities per frame
    let spawner = "import engine.world.component_id
import engine.world.spawn
import engine.world.insert
hname db \"mod:spawner/Tag\"
buf resb 8
global mut cid: i32 = 0
export fn init() {
    reg id
    call engine.world.component_id(hname, hname.len) -> id
    global.set cid, id
    ret
}
export fn frame(dt: f32) {
    reg id, e, st
    global.get id, cid
    call engine.world.spawn() -> e
    i32.store id, buf
    call engine.world.insert(e, id, buf) -> st
    call engine.world.spawn() -> e
    call engine.world.insert(e, id, buf) -> st
    ret
}
";
    let toml = mod_toml("spawner", "[[meta.component]]\nname = \"Tag\"\nfields = [ { name = \"n\", type = \"u32\" }, { name = \"m\", type = \"u32\" } ]\n", "");
    load(&mut e, &MemSource::new().with("mod.toml", &toml).with("src/main.efa", spawner)).unwrap();
    for i in 0..8 {
        let en = e.world_mut().spawn((Transform::default(),));
        let id = health_id(&e);
        e.types().clone().component(id).unwrap().insert(e.world_mut(), en, &health_bytes(i as f32, 1.0)).unwrap();
    }
    e.run_frames(20, 1.0 / 60.0);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    let mut out = dump(&e, health_id(&e));
    out.extend(dump(&e, e.types().by_name("mod:spawner/Tag").unwrap().id()));
    // reload one of them to exercise despawn + re-init at the same time
    let report = e.unload_mods();
    assert_eq!(report.unloaded.len(), 2);
    out.push((e.world().len() as u64, Vec::new()));
    out
}

#[test]
fn one_and_four_threads_give_identical_worlds() {
    let single = determinism_run(Threads::Single);
    assert!(single.len() > 40, "{}", single.len());
    assert_eq!(single, determinism_run(Threads::Fixed(4)));
    assert_eq!(single, determinism_run(Threads::Fixed(2)));
}

#[test]
fn the_index_with_a_mod_component_round_trips() {
    let (mut e, _got) = engine();
    load(&mut e, &hp_mod()).unwrap();
    let text = e.export_api_index();
    let ix = efeengine_core::efevm::ApiIndex::from_json(&text).expect("the exported index parses");
    assert!(ix.types.iter().any(|t| t.name == HEALTH));
}

#[test]
fn par_systems_can_read_mod_components() {
    let (mut e, _got) = engine();
    let game = e.world_mut().spawn((Transform::default(),));
    load(&mut e, &hp_mod()).unwrap();
    let id = health_id(&e);
    e.types().clone().component(id).unwrap().insert(e.world_mut(), game, &health_bytes(6.0, 8.0)).unwrap();
    let seen = Arc::new(Mutex::new(0.0f32));
    let s = seen.clone();
    e.add_par_system(Stage::Update, Access::new(), move |cx| {
        let g = cx.dyn_get(game, id).unwrap();
        *s.lock().unwrap() = f32::from_le_bytes(g[0..4].try_into().unwrap());
    });
    e.run_frames(1, 1.0 / 60.0);
    assert_eq!(*seen.lock().unwrap(), 6.0);
}
