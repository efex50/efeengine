//! Scripts and mods building UI through the `ui` host functions: ownership and grant rules,
//! signals as script events, mount points, limits, unload cleanup. Headless; the modules are
//! `.efa` sources compiled in memory.

use std::sync::{Arc, Mutex};

use efeengine_core::efa::asm::MemSource;
use efeengine_core::efevm::{ApiType, HostFn, HostOk};
use efeengine_core::mods::{Grant, LoadInput, ModRequest};
use efeengine_core::prelude::*;
use efeengine_core::scripting::{LogLevel, ModuleId};
use efeengine_ui::{Owner, UiScripts, UiTest, UiTree};

#[derive(Clone, Default)]
struct Probe {
    /// Everything the modules `test.report`ed.
    reports: Arc<Mutex<Vec<i64>>>,
    /// Commands the next frame of every test module runs (one per call of `test.cmd`).
    cmds: Arc<Mutex<Vec<(i32, i64)>>>,
}

impl Probe {
    fn take(&self) -> Vec<i64> {
        std::mem::take(&mut *self.reports.lock().unwrap())
    }
    fn cmd(&self, op: i32, arg: i64) {
        self.cmds.lock().unwrap().push((op, arg));
    }
}

/// The module: `init` builds a small panel; `frame` runs at most one command per frame and
/// reports every `ui.signal` (widget, kind, value bits) it sees. Commands:
/// 1 set_text(arg) 2 remove(arg) 3 find("hud/slot") 4 modal layer 5 button under arg
/// 6 set_value(arg) 7 create 300 texts under the panel, report the node count
/// 8 find("go") 9 get_text(arg) length 10 set_key(arg, "go") 11 set_size(arg)
const PROGRAM: &str = r#"
import ui.layer
import ui.column
import ui.text
import ui.button
import ui.checkbox
import ui.slider
import ui.text_input
import ui.set_text
import ui.get_text
import ui.set_value
import ui.set_key
import ui.set_size
import ui.remove
import ui.find
import ui.node_count
import event ui.signal
import engine.events.count
import engine.events.read
import test.report
import test.cmd
import test.arg

s_hud db "hud"
s_go db "Go"
s_go_key db "go"
s_slot db "hud/slot"
s_modal db "modal"
s_txt db "txt"
s_name db "name"
s_changed db "changed"
global mut panel: i64 = 0
global mut button: i64 = 0
global mut slider: i64 = 0
global mut input: i64 = 0
ev resb UiSignal.size
buf resb 64

export fn init() {
    reg root, col, b, t, c, s, i, f
    const.i32 f, 0
    call ui.layer(s_hud, s_hud.len, 10, 2, f) -> root
    call ui.column(root) -> col
    global.set panel, col
    call ui.text(col, s_txt, s_txt.len) -> t
    call ui.button(col, s_go, s_go.len) -> b
    global.set button, b
    call ui.set_key(b, s_go_key, s_go_key.len) -> f
    call ui.checkbox(col, s_txt, s_txt.len, f) -> c
    const.f32 s, 0.0
    call ui.slider(col, s, 1.0, 0.25) -> i
    global.set slider, i
    call ui.text_input(col, s_name, s_name.len) -> i
    global.set input, i
    call test.report(root)
    call test.report(b)
    ret
}

export fn frame(dt: f32) {
    reg n, i, st, op, arg, w, k, kind, val, h, f, c
    ; signals first
    call engine.events.count(ui.signal.id) -> n
    const.i32 i, 0
sig:
    i32.lt_s st, i, n
    br_ifz st, cmds
    call engine.events.read(ui.signal.id, i, ev) -> st
    br_if st, cmds
    i64.load w, ev, UiSignal.widget
    i32.load k, ev, UiSignal.kind
    i32.load val, ev, UiSignal.value
    call test.report(w)
    i64.extend_i32_s h, k
    call test.report(h)
    i64.extend_i32_s h, val
    call test.report(h)
    i32.add i, i, 1
    jmp sig
cmds:
    call test.cmd() -> op
    br_ifz op, out
    call test.arg() -> arg
    i32.eq st, op, 1
    br_ifz st, c2
    call ui.set_text(arg, s_changed, s_changed.len) -> st
    jmp rep32
c2:
    i32.eq st, op, 2
    br_ifz st, c3
    call ui.remove(arg) -> st
    jmp rep32
c3:
    i32.eq st, op, 3
    br_ifz st, c4
    call ui.find(s_slot, s_slot.len) -> h
    call test.report(h)
    ret
c4:
    i32.eq st, op, 4
    br_ifz st, c5
    const.i32 f, 1
    call ui.layer(s_modal, s_modal.len, 50, 1, f) -> h
    call test.report(h)
    ret
c5:
    i32.eq st, op, 5
    br_ifz st, c6
    call ui.button(arg, s_go, s_go.len) -> h
    call test.report(h)
    ret
c6:
    i32.eq st, op, 6
    br_ifz st, c7
    call ui.set_value(arg, 0.5) -> st
    jmp rep32
c7:
    i32.eq st, op, 7
    br_ifz st, c8
    const.i32 i, 0
spam:
    i32.lt_s st, i, 300
    br_ifz st, spamdone
    global.get h, panel
    call ui.text(h, s_txt, s_txt.len) -> h
    i32.add i, i, 1
    jmp spam
spamdone:
    call ui.node_count() -> st
    jmp rep32
c8:
    i32.eq st, op, 8
    br_ifz st, c9
    call ui.find(s_go_key, s_go_key.len) -> h
    call test.report(h)
    ret
c9:
    i32.eq st, op, 9
    br_ifz st, c10
    call ui.get_text(arg, buf, 64) -> st
    jmp rep32
c10:
    i32.eq st, op, 10
    br_ifz st, c11
    call ui.set_key(arg, s_go_key, s_go_key.len) -> st
    jmp rep32
c11:
    call ui.set_size(arg, 100.0, 40.0) -> st
rep32:
    i64.extend_i32_s h, st
    call test.report(h)
out:
    ret
}
"#;

fn engine_setup(probe: &Probe) -> impl FnOnce(&mut Engine) + use<> {
    let probe = probe.clone();
    move |e: &mut Engine| {
        e.define_namespace("test", 1, "testgame", "Test helpers.").unwrap();
        let p = probe.clone();
        e.register_host_fn(HostFn::new("test", "report").doc("Record v.").param("v", ApiType::I64), move |_c, a, _r| {
            p.reports.lock().unwrap().push(a.i64(0)?);
            Ok(HostOk::Continue)
        })
        .unwrap();
        let p = probe.clone();
        e.register_host_fn(HostFn::new("test", "cmd").doc("Next command.").result("op", ApiType::I32), move |c, _a, r| {
            // one command per module per frame: keyed by the module so every module runs it
            let _ = c;
            let q = p.cmds.lock().unwrap();
            let op = if q.is_empty() { 0 } else { q[0].0 };
            r.set_i32(0, op)?;
            Ok(HostOk::Continue)
        })
        .unwrap();
        let p = probe.clone();
        e.register_host_fn(HostFn::new("test", "arg").doc("Argument of the command.").result("arg", ApiType::I64), move |_c, _a, r| {
            let q = p.cmds.lock().unwrap();
            r.set_i64(0, q.first().map_or(0, |c| c.1))?;
            Ok(HostOk::Continue)
        })
        .unwrap();
    }
}

const MOD_TOML: &str = "[mod]\nid = \"{id}\"\nversion = \"1.0.0\"\n\n[permissions]\nnamespaces = [\"ui\", \"engine.events\", \"test\"]\n";

fn mod_files(id: &str) -> MemSource {
    MemSource::new().with("mod.toml", &MOD_TOML.replace("{id}", id)).with("src/main.efa", PROGRAM)
}

fn load_mod(t: &mut UiTest, id: &str) -> ModuleId {
    let files = mod_files(id);
    t.engine
        .load_input(LoadInput::Folder { label: "m", files: &files, assets: &[] })
        .unwrap_or_else(|e| panic!("load mod {id}: {e}"))
        .id()
}

fn grant_ui(t: &mut UiTest) {
    t.engine.set_mod_policy(|_: &ModRequest<'_>| {
        Ok(Grant { namespaces: vec!["ui".into(), "engine.events".into(), "test".into()], ..Grant::default() })
    });
}

fn setup() -> (UiTest, Probe) {
    let probe = Probe::default();
    let mut t = UiTest::new(engine_setup(&probe));
    grant_ui(&mut t);
    (t, probe)
}

/// Run one frame with command `op` queued.
fn run_cmd(t: &mut UiTest, probe: &Probe, op: i32, arg: i64) -> Vec<i64> {
    probe.cmd(op, arg);
    t.step(1);
    probe.cmds.lock().unwrap().clear();
    probe.take()
}

fn errors(t: &UiTest) -> Vec<String> {
    t.engine.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

#[test]
fn a_mod_builds_its_own_panel_with_owner_mod() {
    let (mut t, probe) = setup();
    load_mod(&mut t, "hudmod");
    t.step(2);
    assert!(errors(&t).is_empty(), "{:?}", errors(&t));
    let r = probe.take();
    assert_eq!(r.len(), 2, "{r:?}");
    let ui = t.ui();
    let layer = ui.find("mod:hudmod/hud").expect("the mod's layer exists under its prefix");
    assert_eq!(ui.owner_of(layer), Some(&Owner::module("hudmod")));
    let go = ui.find("mod:hudmod/go").expect("keyed button");
    assert_eq!(ui.owner_of(go), Some(&Owner::module("hudmod")));
    // layer, column, text, button, checkbox, slider, text input
    assert_eq!(ui.nodes_of(&Owner::module("hudmod")).len(), 7);
    assert_eq!(ui.layer_of(go), Some(layer));
    assert_eq!(t.resource::<UiScripts>().nodes_of("hudmod"), 7);
}

#[test]
fn click_reaches_the_owning_mod_as_a_script_event() {
    let (mut t, probe) = setup();
    load_mod(&mut t, "hudmod");
    t.step(2);
    let handles = probe.take();
    let button = handles[1];
    t.click("mod:hudmod/go");
    t.step(1);
    let r = probe.take();
    assert_eq!(r, vec![button, 1, 0], "widget, kind click, value");
    assert!(errors(&t).is_empty(), "{:?}", errors(&t));
}

#[test]
fn slider_changes_arrive_with_the_value_and_text_is_readable() {
    let (mut t, probe) = setup();
    load_mod(&mut t, "hudmod");
    t.step(2);
    probe.take();
    t.set_value("mod:hudmod/go", 0.0); // wrong type for a button: ignored by the plugin
    t.step(1);
    assert!(probe.take().is_empty());
    let slider = {
        let ui = t.ui();
        let layer = ui.find("mod:hudmod/hud").unwrap();
        ui.subtree(layer).into_iter().find(|id| matches!(ui.kind(*id), Some(efeengine_ui::NodeKind::Slider { .. }))).unwrap()
    };
    t.ui_mut().node(slider).unwrap().key("slider");
    t.set_value("slider", 0.75);
    t.step(1);
    let r = probe.take();
    assert_eq!(r.len(), 3, "{r:?}");
    assert_eq!(r[1], 2, "changed");
    assert_eq!(f32::from_bits(r[2] as u32), 0.75);
    let input = {
        let ui = t.ui();
        let layer = ui.find("mod:hudmod/hud").unwrap();
        ui.subtree(layer).into_iter().find(|id| matches!(ui.kind(*id), Some(efeengine_ui::NodeKind::TextInput { .. }))).unwrap()
    };
    t.ui_mut().node(input).unwrap().key("input");
    t.type_text("input", "Ada");
    t.step(1);
    let r = probe.take();
    assert!(r.chunks(3).any(|c| c[1] == 2), "{r:?}");
}

#[test]
fn a_mod_cannot_touch_the_games_widgets_or_forged_handles() {
    let (mut t, probe) = setup();
    // a game widget with a key the mod knows
    {
        let ui = t.ui_mut();
        let layer = ui.layer("game", efeengine_ui::Layer::centered(1));
        ui.button(layer, "Play").key("game/play");
    }
    t.step(1);
    load_mod(&mut t, "hudmod");
    t.step(2);
    probe.take();
    // find: not exposed -> negative status 1
    probe.cmd(3, 0);
    t.step(1);
    probe.cmds.lock().unwrap().clear();
    assert_eq!(probe.take(), vec![-1]);
    // forged handles, the game's WidgetId bits and handle 0 are all bad handles
    let game_play = t.ui().find("game/play").unwrap().to_bits() as i64;
    for forged in [game_play, 0, -5, 0x0000_0002_0000_0001, 0x7fff_ffff_0000_0001, 0x0000_0001_0000_0063, 1] {
        assert_eq!(run_cmd(&mut t, &probe, 1, forged), vec![1], "forged {forged:#x}");
        assert_eq!(run_cmd(&mut t, &probe, 2, forged), vec![1], "remove {forged:#x}");
    }
    assert_eq!(t.ui().text_of(t.ui().find("game/play").unwrap()), Some("Play"));
    assert!(t.ui().find("game/play").is_some());
    assert!(t.resource::<UiScripts>().violations().len() >= 12);
    // the refusal was logged once per function
    let warns = t.engine.scripting().log_lines().filter(|l| l.text.contains("refused")).count();
    assert!(warns >= 1 && warns <= 4, "{warns}");
    assert!(errors(&t).is_empty(), "{:?}", errors(&t));
}

#[test]
fn a_mod_cannot_change_another_mods_widgets() {
    let (mut t, probe) = setup();
    load_mod(&mut t, "mod_a");
    t.step(2);
    let a_handles = probe.take();
    load_mod(&mut t, "mod_b");
    t.step(2);
    let _ = probe.take();
    // both modules run every command; module B's handle is positive and different from A's
    probe.cmd(1, a_handles[1]);
    t.step(1);
    probe.cmds.lock().unwrap().clear();
    let r = probe.take();
    // A: ok (0); B: bad handle (1). Order is module order.
    assert_eq!(r, vec![0, 1], "{r:?}");
    {
        let ui = t.ui();
        let a_go = ui.find("mod:mod_a/go").unwrap();
        assert_eq!(ui.text_of(a_go), Some("changed"));
        let b_go = ui.find("mod:mod_b/go").unwrap();
        assert_eq!(ui.text_of(b_go), Some("Go"));
    }
    // removing A's panel root by B is refused as well
    probe.cmd(2, a_handles[0]);
    t.step(1);
    probe.cmds.lock().unwrap().clear();
    assert_eq!(probe.take(), vec![0, 1]);
    assert!(t.ui().find("mod:mod_a/hud").is_none(), "A removed its own layer");
    assert!(t.ui().find("mod:mod_b/hud").is_some());
}

#[test]
fn signals_only_reach_widgets_of_the_module_that_owns_them() {
    let (mut t, probe) = setup();
    load_mod(&mut t, "mod_a");
    load_mod(&mut t, "mod_b");
    t.step(2);
    probe.take();
    t.click("mod:mod_b/go");
    t.step(1);
    let r = probe.take();
    // Both modules read the broadcast event, but the widget handle is the one of mod_b: its
    // high half names module b, and mod_a gets no event for its own widgets.
    assert_eq!(r.len(), 6, "{r:?}");
    let b_module = (r[0] >> 32) - 1;
    assert_eq!(r[0], r[3]);
    assert!(b_module >= 0);
}

#[test]
fn modal_layers_limits_and_mount_points() {
    let (mut t, probe) = setup();
    {
        let ui = t.ui_mut();
        let layer = ui.layer("game", efeengine_ui::Layer::top_left(1));
        ui.column(layer).key("hud/slot");
        ui.column(layer).key("hud/other");
    }
    t.step(1);
    load_mod(&mut t, "hudmod");
    t.step(2);
    probe.take();
    // a modal layer is denied for mods (status 4)
    assert_eq!(run_cmd(&mut t, &probe, 4, 0), vec![-4]);
    assert!(t.ui().find("mod:hudmod/modal").is_none());
    assert!(!t.ui().modal_active());
    // mount point: exposed by the game, found by the mod, usable as a parent only
    t.resource::<UiScripts>().expose("hud/slot");
    t.step(1);
    let r = run_cmd(&mut t, &probe, 3, 0);
    let mount = r[0];
    assert!(mount > 0, "{r:?}");
    let r = run_cmd(&mut t, &probe, 5, mount);
    assert!(r[0] > 0, "button under the mount point: {r:?}");
    let slot = t.ui().find("hud/slot").unwrap();
    assert_eq!(t.ui().children(slot).len(), 1);
    assert_eq!(t.ui().owner_of(t.ui().children(slot)[0]), Some(&Owner::module("hudmod")));
    // but not changed or removed or renamed
    assert_eq!(run_cmd(&mut t, &probe, 1, mount), vec![4]);
    assert_eq!(run_cmd(&mut t, &probe, 2, mount), vec![4]);
    assert_eq!(run_cmd(&mut t, &probe, 10, mount), vec![4]);
    assert_eq!(run_cmd(&mut t, &probe, 11, mount), vec![4]);
    assert!(t.ui().find("hud/slot").is_some());
    // not exposed: still refused
    assert_eq!(run_cmd(&mut t, &probe, 3, 0).len(), 1);
    // the node limit: 7 + 1 nodes, then 300 more texts
    t.resource::<UiScripts>().set_max_nodes(50);
    let r = run_cmd(&mut t, &probe, 7, 0);
    assert_eq!(r, vec![50], "the count stops at the limit");
    assert_eq!(t.ui().nodes_of(&Owner::module("hudmod")).len(), 50 - 0);
    assert!(t.resource::<UiScripts>().violations().iter().any(|v| v.function == "text" && v.status == 3));
}

#[test]
fn unloading_a_mod_removes_its_widgets() {
    let (mut t, probe) = setup();
    load_mod(&mut t, "hudmod");
    t.step(2);
    probe.take();
    assert_eq!(t.ui().nodes_of(&Owner::module("hudmod")).len(), 7);
    t.engine.unload_mods();
    t.step(2);
    assert_eq!(t.ui().nodes_of(&Owner::module("hudmod")).len(), 0);
    assert!(t.ui().find("mod:hudmod/hud").is_none());
}

#[test]
fn a_mod_without_the_ui_grant_does_not_load() {
    let probe = Probe::default();
    let mut t = UiTest::new(engine_setup(&probe));
    t.engine.set_mod_policy(|_: &ModRequest<'_>| {
        Ok(Grant { namespaces: vec!["engine.events".into(), "test".into()], ..Grant::default() })
    });
    let files = mod_files("nogrant");
    let err = t.engine.load_input(LoadInput::Folder { label: "m", files: &files, assets: &[] });
    assert!(err.is_err(), "the mod imports ui.* without a grant");
    assert_eq!(t.ui().nodes_of(&Owner::module("nogrant")).len(), 0);
}

#[test]
fn a_game_script_is_trusted_and_finds_any_widget() {
    let (mut t, probe) = setup();
    {
        let ui = t.ui_mut();
        let layer = ui.layer("game", efeengine_ui::Layer::top_left(1));
        ui.button(layer, "Play").key("go");
    }
    t.step(1);
    let files = MemSource::new().with("main.efa", PROGRAM);
    let id = t.engine.load_input(LoadInput::Efa { name: "gamescript", files: &files, root: "main.efa" }).expect("load script").id();
    let _ = id;
    t.step(2);
    probe.take();
    // find("go"): its own key "go" (the script keyed its button "go" too, but the tree already
    // has the game's key `go`, so set_key failed at apply time) -> the script's own table has it
    let r = run_cmd(&mut t, &probe, 8, 0);
    assert!(r[0] > 0, "{r:?}");
    // the script's nodes belong to the script, not to Owner::Game
    let ui = t.ui();
    let layer = ui.find("hud").expect("plain keys for scripts");
    assert_eq!(ui.owner_of(layer), Some(&Owner::plugin("script:gamescript")));
    assert!(errors(&t).is_empty(), "{:?}", errors(&t));
}

#[test]
fn the_ui_api_is_in_the_index() {
    let (t, _) = setup();
    let index = t.engine.api_index();
    for f in ["layer", "column", "row", "text", "button", "checkbox", "slider", "text_input", "image", "remove", "find", "set_text", "set_value", "set_checked", "set_visible", "set_enabled", "set_class", "set_key"] {
        assert!(index.function("ui", f).is_some(), "ui.{f} is registered");
    }
    assert!(index.namespace("ui").is_some());
    let _ = UiTree::new();
}
