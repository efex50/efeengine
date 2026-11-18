//! coin_dash_mods without a window: the mod folders are packed into `.efmod` files (the way
//! `cargo xtask mods` does, with the efa and efemod libraries), loaded as packages under the game's
//! policy and asset handler, run, reloaded and unloaded.
//! Run on its own: `cargo test -p coin_dash_mods`.

use std::path::{Path, PathBuf};

use coin_dash_mods::*;
use efeengine_core::efa::asm::FsSource;
use efeengine_core::efa::mod_folder::{BuildLimits, build_mod, list_assets};
use efeengine_core::efemod::{Package, ReadLimits};
use efeengine_core::efevm::ApiIndex;
use efeengine_core::prelude::*;
use efeengine_core::cache::{CacheConfig, CacheDir};
use efeengine_core::mods::{LoadError, LoadInput, LoadReport, ModError};
use efeengine_core::scripting::{LogLevel, ModuleId};

const DT: f64 = 1.0 / 60.0;

fn mods_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("mods")
}

fn engine() -> (Engine, Game) {
    // never the platform cache directory
    let cfg = EngineConfig {
        fixed_dt: DT,
        script_cache: CacheConfig { disk: CacheDir::Off, ..Default::default() },
        ..EngineConfig::headless()
    };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    let game = setup(&mut e);
    game.set_autoplay(false); // whatever COIN_DASH_AUTOPLAY says
    (e, game)
}

/// Pack every folder of `mods/` against the game's index and the packages packed before it (a
/// mod is packed after the mods it depends on), like `cargo xtask mods`. Returns `(file name, bytes)`.
fn pack_all(game_index: &ApiIndex) -> Vec<(String, Vec<u8>)> {
    let mut todo: Vec<PathBuf> = std::fs::read_dir(mods_src())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("mod.toml").is_file())
        .collect();
    todo.sort();
    let mut indices = vec![game_index.clone()];
    let mut out = vec![];
    while !todo.is_empty() {
        let mut failed = vec![];
        let mut errs = vec![];
        let mut progress = false;
        for dir in todo {
            let refs: Vec<&ApiIndex> = indices.iter().collect();
            let merged = ApiIndex::merge(&refs).expect("merge indices");
            let assets = list_assets(&dir).expect("assets");
            match build_mod(&FsSource::new(&dir), Some(&merged), &assets, &BuildLimits::default()) {
                Ok(built) => {
                    let bytes = Package::write(&built.manifest, &built.module.encode(), &built.assets).expect("write package");
                    let name = format!("{}-{}.efmod", built.manifest.id, built.manifest.version);
                    indices.push(Package::read(&bytes, &ReadLimits::default()).expect("read back").index());
                    out.push((name, bytes));
                    progress = true;
                }
                Err(e) => {
                    errs.push(format!("{}: {e}", dir.display()));
                    failed.push(dir);
                }
            }
        }
        // a mod that failed may only be waiting for its dependency
        assert!(progress, "cannot pack:\n{}", errs.join("\n"));
        todo = failed;
    }
    out
}

fn packages() -> Vec<(String, Vec<u8>)> {
    // The game's index alone: what `EFE_API_EXPORT` writes. (Registering the game needs no mod.)
    let (e, _) = engine();
    let index = ApiIndex::from_json(&e.export_api_index()).expect("exported index parses");
    assert!(index.function("game", "texture").is_some());
    pack_all(&index)
}

/// An engine with the packages loaded through `load_mods`.
fn running() -> (Engine, Game, LoadReport) {
    let (mut e, game) = engine();
    let items = packages();
    assert_eq!(items.len(), 5, "{:?}", items.iter().map(|i| &i.0).collect::<Vec<_>>());
    e.add_mod_source(BytesSource { items });
    let report = e.load_mods();
    (e, game, report)
}

fn errors(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect()
}

fn lines(e: &Engine) -> Vec<String> {
    e.scripting().log_lines().map(|l| l.text.clone()).collect()
}

fn sprites(e: &Engine) -> Vec<(Vec2, Sprite)> {
    e.world().query::<(&Transform, &Sprite)>().iter().map(|(t, s)| (t.position, *s)).collect()
}

fn with_texture(e: &Engine, tex: TextureHandle) -> Vec<(Vec2, Sprite)> {
    sprites(e).into_iter().filter(|(_, s)| s.texture == tex).collect()
}

fn module_of(e: &Engine, id: &str) -> ModuleId {
    e.scripting().module_id(id).unwrap_or_else(|| panic!("{id} is not loaded"))
}

#[test]
fn the_packages_load_under_the_policy_with_their_grants_and_meta() {
    let (e, game, report) = running();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.loaded.len(), 5);
    // dependency order: dash_core first
    assert!(report.loaded[0].0.starts_with("dash_core"), "{:?}", report.loaded);
    let grant = |id: &str| e.package(module_of(&e, id)).unwrap().grant.namespaces.clone();
    assert!(grant("dash_core").iter().any(|n| n == "engine.input"));
    // dash_tiles asked for no engine.world and got none: it draws through dash_core
    assert!(!grant("dash_tiles").iter().any(|n| n == "engine.world"));
    assert!(!grant("dash_enemies").iter().any(|n| n == "engine.input"));
    // [meta] is kept in the manifest
    let meta = e.package(module_of(&e, "dash_enemies")).unwrap().manifest.meta.clone().expect("meta");
    let diff = meta.get("difficulty").expect("difficulty");
    assert_eq!(diff.get("enemy_count").and_then(|v| v.as_i64()), Some(3));
    // the PNGs of the packages became textures, one handle per asset
    assert_eq!(game.mod_texture_count(), 4);
    for (id, png) in [("dash_core", "player.png"), ("dash_enemies", "enemy.png"), ("dash_coins", "coin_sheet.png"), ("dash_tiles", "tiles.png")] {
        let h = game.texture_of(module_of(&e, id), png).unwrap_or_else(|| panic!("{id}/{png}"));
        assert!(e.textures().get(h).is_some());
    }
    // the mods provide functions, structs and an event; the index says who owns them
    let idx = e.api_index();
    idx.validate().unwrap();
    for f in ["spawn_sprite", "pickup", "hit", "player_pos", "set_target", "round", "format_i64"] {
        assert_eq!(idx.function("dash_core", f).unwrap_or_else(|| panic!("dash_core.{f}")).owner, "dash_core");
    }
    assert!(idx.event("dash_core", "round_started").is_some());
    assert_eq!(idx.struct_type("Round").expect("Round").owner, "dash_core");
}

#[test]
fn the_mods_entities_exist_and_the_game_reacts() {
    let (mut e, game, report) = running();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let tex = |id: &str, png: &str| game.texture_of(module_of(&e, id), png).unwrap();
    let (player_tex, enemy_tex, coin_tex, tile_tex) =
        (tex("dash_core", "player.png"), tex("dash_enemies", "enemy.png"), tex("dash_coins", "coin_sheet.png"), tex("dash_tiles", "tiles.png"));
    // the tilemap needs a row per frame (30 rows); the round-1 event arrives one frame late
    e.run_frames(45, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(with_texture(&e, player_tex).len(), 1);
    assert_eq!(with_texture(&e, enemy_tex).len(), 3);
    assert_eq!(with_texture(&e, coin_tex).len(), 5, "round 1 has 5 coins");
    assert!(sprites(&e).iter().all(|(_, s)| s.size == Vec2::splat(16.0)), "spawn_sprite writes the size field");
    assert_eq!(with_texture(&e, tile_tex).len(), 1200, "the whole 40 x 30 map");
    // the mods' own log lines reached the log
    let l = lines(&e);
    assert!(l.iter().any(|t| t.contains("[dash_enemies] enemies spawned: 3")), "{l:?}");
    assert!(l.iter().any(|t| t.contains("[dash_enemies] enemy speed percent: 100")), "{l:?}");
    assert!(l.iter().any(|t| t.contains("[dash_coins] coins in the new round: 5")), "{l:?}");
    assert!(l.iter().any(|t| t.contains("[dash_tiles] tilemap rows drawn: 30")), "{l:?}");
    assert!(l.iter().any(|t| t.contains("[dash_core] t=") && t.contains("score=0 round=1")), "{l:?}");

    // enemies patrol (Velocity from the mod, moved by the engine), coins spin and bob
    let enemies_before = with_texture(&e, enemy_tex);
    let coins_before = with_texture(&e, coin_tex);
    e.run_frames(85, DT);
    let (enemies, coins) = (with_texture(&e, enemy_tex), with_texture(&e, coin_tex));
    assert!(enemies_before.iter().zip(&enemies).all(|(a, b)| a.0.distance(b.0) > 10.0), "{:?} -> {:?}", enemies_before.iter().map(|e| e.0).collect::<Vec<_>>(), enemies.iter().map(|e| e.0).collect::<Vec<_>>());
    assert!(coins_before.iter().zip(&coins).any(|(a, b)| a.1.uv_min.x != b.1.uv_min.x), "no coin spun");
    assert!(coins_before.iter().zip(&coins).filter(|(a, b)| (a.0.y - b.0.y).abs() > 1e-4).count() >= 3, "coins do not bob");
    assert_eq!(game.score(), 0);
}

#[test]
fn the_autoplay_bot_scores_and_rounds_advance_through_events() {
    let (mut e, game, report) = running();
    assert!(report.failed.is_empty());
    game.set_autoplay(true);
    let mut best = 0;
    for _ in 0..40 * 60 {
        e.run_frames(1, DT);
        best = best.max(game.score());
    }
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert!(best >= 3, "best score {best}");
    let l = lines(&e);
    assert!(l.iter().any(|t| t.contains("[dash_core] score 1")), "dash_core logs the pickups the coins mod reported");
    // five coins in a row can be collected between two hits: round 2 has 8 coins
    if l.iter().any(|t| t.contains("[dash_core] round 2")) {
        assert!(l.iter().any(|t| t.contains("[dash_coins] coins in the new round: 8")), "{l:?}");
    }
}

#[test]
fn an_enemy_hit_resets_the_score_through_dash_core() {
    let (mut e, game, _) = running();
    e.run_frames(5, DT);
    let core = module_of(&e, "dash_core");
    let player_tex = game.texture_of(core, "player.png").unwrap();
    let coin_tex = game.texture_of(module_of(&e, "dash_coins"), "coin_sheet.png").unwrap();
    let enemy_tex = game.texture_of(module_of(&e, "dash_enemies"), "enemy.png").unwrap();
    let ent = |e: &Engine, tex: TextureHandle| {
        e.world().query::<(Entity, &Sprite)>().iter().filter(|(_, s)| s.texture == tex).map(|(en, _)| en).collect::<Vec<_>>()
    };
    let player = ent(&e, player_tex)[0];
    let coin = ent(&e, coin_tex)[0];
    let coin_pos = e.world().get::<&Transform>(coin).unwrap().position;
    e.world_mut().get::<&mut Transform>(player).unwrap().position = coin_pos;
    e.run_frames(3, DT);
    assert_eq!(game.score(), 1, "the coin mod reported the pickup, dash_core scored it");
    // wait out nothing: the player has not been hit yet, so it is not immune
    let enemy = ent(&e, enemy_tex)[0];
    let enemy_pos = e.world().get::<&Transform>(enemy).unwrap().position;
    e.world_mut().get::<&mut Transform>(player).unwrap().position = enemy_pos;
    e.run_frames(3, DT);
    assert_eq!(game.score(), 0, "dash_enemies called dash_core.hit");
    assert!(e.world().get::<&Transform>(player).unwrap().position.length() < 8.0, "back at the start");
    assert!(lines(&e).iter().any(|t| t.contains("hit by enemy, score reset, lost 1")));
}

#[test]
fn reload_skips_unchanged_mods_then_unload_cleans_up() {
    let (mut e, game, report) = running();
    assert!(report.failed.is_empty());
    e.run_frames(45, DT);
    let before_ids: Vec<ModuleId> = report.loaded.iter().map(|(_, id)| *id).collect();
    let live_before = sprites(&e).len();

    // reload_mods with nothing changed: every mod is skipped, nothing is touched
    let r = e.reload_mods();
    assert!(r.loaded.is_empty() && r.reloaded.is_empty() && r.unloaded.is_empty() && r.failed.is_empty(), "{r:?}");
    assert_eq!(r.skipped.len(), 5);
    assert_eq!(r.skipped.iter().map(|(_, id)| *id).collect::<Vec<_>>().len(), before_ids.len());
    e.run_frames(5, DT);
    assert_eq!(sprites(&e).len(), live_before, "no entity was added or lost");
    assert_eq!(game.mod_texture_count(), 4);

    // unload_mods: dependents first, then dash_core; textures are freed, entities are swept
    let tex_ids: Vec<u32> = ["dash_core", "dash_enemies", "dash_coins", "dash_tiles"]
        .iter()
        .zip(["player.png", "enemy.png", "coin_sheet.png", "tiles.png"])
        .map(|(m, p)| game.texture_of(module_of(&e, m), p).unwrap().id)
        .collect();
    let u = e.unload_mods();
    assert!(u.failed.is_empty(), "{:?}", u.failed);
    assert_eq!(u.unloaded.len(), 5);
    assert_eq!(u.unloaded.last().map(String::as_str), Some("dash_core"), "{:?}", u.unloaded);
    assert_eq!(game.mod_texture_count(), 0);
    for id in tex_ids {
        assert!(e.textures().get(TextureHandle { id }).is_none(), "texture {id} still stored");
    }
    // the sweep runs in the next frame and removes everything the mods drew
    e.run_frames(3, DT);
    assert_eq!(sprites(&e).len(), 0, "{} sprites left", sprites(&e).len());
    // the game keeps running with its camera and an empty arena
    assert!(e.is_running());
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    assert_eq!(e.world().query::<&Camera>().iter().count(), 1);
    // and the mods can come back: load_mods loads them again
    let again = e.load_mods();
    assert_eq!(again.loaded.len(), 5, "{again:?}");
    assert!(again.failed.is_empty());
}

#[test]
fn a_corrupt_package_fails_alone_and_the_others_run() {
    let (mut e, game) = engine();
    let mut items = packages();
    // truncate dash_enemies: it fails, and nothing that depends on it exists
    let i = items.iter().position(|(n, _)| n.starts_with("dash_enemies")).unwrap();
    let half = items[i].1.len() / 2;
    items[i].1.truncate(half);
    e.add_mod_source(BytesSource { items });
    let report = e.load_mods();
    assert_eq!(report.loaded.len(), 4, "{report:?}");
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].0.starts_with("dash_enemies"));
    log_report(&e, &report);
    e.run_frames(45, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    let enemy_ids = e.scripting().module_id("dash_enemies");
    assert!(enemy_ids.is_none());
    let coin_tex = game.texture_of(module_of(&e, "dash_coins"), "coin_sheet.png").unwrap();
    assert_eq!(with_texture(&e, coin_tex).len(), 5, "the rest of the game runs");
}

#[test]
fn a_mod_with_a_broken_png_is_refused_by_the_asset_handler() {
    let (mut e, game) = engine();
    let files = efeengine_core::efa::asm::MemSource::new()
        .with("mod.toml", "[mod]\nid = \"badart\"\nversion = \"1.0.0\"\ngame = \"coin_dash_mods\"\n")
        .with("src/main.efa", "export fn frame(dt: f32) {\n    ret\n}\n")
        .with_bytes("assets/oops.png", b"not a png");
    let err = e
        .load_input(LoadInput::Folder { label: "badart", files: &files, assets: &["oops.png".to_string()] })
        .expect_err("refused");
    match err {
        LoadError::Mod(ModError::Asset { .. }) => {}
        other => panic!("{other}"),
    }
    assert_eq!(game.mod_texture_count(), 0);
    assert!(e.scripting().module_id("badart").is_none());
}

#[test]
fn an_unknown_mod_gets_only_the_default_grant() {
    let (mut e, _game) = engine();
    let files = efeengine_core::efa::asm::MemSource::new()
        .with("mod.toml", "[mod]\nid = \"stranger\"\nversion = \"1.0.0\"\ngame = \"coin_dash_mods\"\n[permissions]\nnamespaces = [\"engine.world\", \"engine.log\"]\n")
        .with("src/main.efa", "import engine.world.spawn\nexport fn frame(dt: f32) {\n    reg e\n    call engine.world.spawn() -> e\n    ret\n}\n");
    match e.load_input(LoadInput::Folder { label: "stranger", files: &files, assets: &[] }) {
        Err(LoadError::Mod(ModError::NotGranted { id, namespaces })) => {
            assert_eq!(id, "stranger");
            assert_eq!(namespaces, ["engine.world"]);
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
}

// ---- the HUD mod builds UI through the `ui` host functions ----

use efeengine_ui::{Owner, UiScripts, UiTree};

fn ui_text(e: &Engine, key: &str) -> Option<String> {
    let tree = e.resources().get::<UiTree>()?;
    let id = tree.find(key)?;
    tree.text_of(id).map(str::to_string)
}

#[test]
fn the_hud_mod_builds_its_own_panel_and_follows_the_score() {
    let (mut e, game, report) = running();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    e.run_frames(5, DT);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
    {
        let ui = e.resources().get::<UiTree>().expect("UiTree");
        let layer = ui.find("mod:dash_hud/hud").expect("the mod's layer, under its own prefix");
        assert_eq!(ui.owner_of(layer), Some(&Owner::module("dash_hud")));
        // layer, column, row, two texts, button
        assert_eq!(ui.nodes_of(&Owner::module("dash_hud")).len(), 6);
        assert!(ui.nodes_of(&Owner::Game).is_empty(), "the game built nothing");
    }
    assert!(lines(&e).iter().any(|t| t.contains("[dash_hud] hud panel built")));
    // the digits follow game.score
    game.set_score(42);
    e.run_frames(3, DT);
    let tree = e.resources().get::<UiTree>().unwrap();
    let layer = tree.find("mod:dash_hud/hud").unwrap();
    let texts: Vec<String> = tree.subtree(layer).into_iter().filter_map(|id| tree.text_of(id).map(str::to_string)).collect();
    assert!(texts.contains(&"42".to_string()), "{texts:?}");
    assert!(texts.contains(&"Score:".to_string()));
}

#[test]
fn a_click_on_the_hud_button_reaches_the_mod_and_resets_the_score() {
    let (mut e, game, _) = running();
    e.run_frames(5, DT);
    game.set_score(7);
    e.run_frames(2, DT);
    let button = {
        let ui = e.resources().get::<UiTree>().unwrap();
        let layer = ui.find("mod:dash_hud/hud").unwrap();
        ui.subtree(layer).into_iter().find(|id| matches!(ui.kind(*id), Some(efeengine_ui::NodeKind::Button { .. }))).unwrap()
    };
    // the headless backend queues the click like a real one; the plugin delivers it as ui.signal
    let handle = e.resources().get::<efeengine_ui::HeadlessHandle>().unwrap().clone();
    handle.inject(efeengine_ui::UiSignal { widget: button, key: None, owner: Owner::Game, kind: efeengine_ui::UiSignalKind::Click });
    e.run_frames(2, DT);
    assert_eq!(game.score(), 0, "the mod got the click and called game.score_reset");
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
}

#[test]
fn the_hud_mod_is_limited_to_its_grant_and_its_own_widgets() {
    let (e, _game, _) = running();
    let grant = e.package(module_of(&e, "dash_hud")).unwrap().grant.namespaces.clone();
    assert!(grant.iter().any(|n| n == "ui"));
    assert!(!grant.iter().any(|n| n == "engine.world"), "{grant:?}");
    // a game widget is out of reach: the mod only finds its own keys
    let scripts = e.resources().get::<UiScripts>().unwrap().clone();
    assert!(scripts.violations().is_empty());
    assert_eq!(scripts.nodes_of("dash_hud"), 6);
    assert_eq!(ui_text(&e, "mod:dash_hud/nothing"), None);
}

#[test]
fn an_unknown_mod_cannot_use_the_ui_namespace() {
    let (mut e, _game) = engine();
    let files = efeengine_core::efa::asm::MemSource::new()
        .with("mod.toml", "[mod]\nid = \"intruder\"\nversion = \"1.0.0\"\ngame = \"coin_dash_mods\"\n[permissions]\nnamespaces = [\"ui\", \"engine.log\"]\n")
        .with("src/main.efa", "import ui.layer\nx db \"x\"\nexport fn init() {\n    reg l, f\n    const.i32 f, 0\n    call ui.layer(x, x.len, 1, 1, f) -> l\n    ret\n}\n");
    match e.load_input(LoadInput::Folder { label: "intruder", files: &files, assets: &[] }) {
        Err(LoadError::Mod(ModError::NotGranted { id, namespaces })) => {
            assert_eq!(id, "intruder");
            assert_eq!(namespaces, ["ui"]);
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert_eq!(e.resources().get::<UiTree>().unwrap().len(), 0);
}

#[test]
fn unloading_the_mods_removes_the_hud() {
    let (mut e, _game, _) = running();
    e.run_frames(5, DT);
    assert_eq!(e.resources().get::<UiTree>().unwrap().nodes_of(&Owner::module("dash_hud")).len(), 6);
    let u = e.unload_mods();
    assert!(u.failed.is_empty(), "{:?}", u.failed);
    e.run_frames(3, DT);
    assert_eq!(e.resources().get::<UiTree>().unwrap().nodes_of(&Owner::module("dash_hud")).len(), 0);
    assert!(e.resources().get::<UiTree>().unwrap().find("mod:dash_hud/hud").is_none());
}
