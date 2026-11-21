//! Manual test bed for the in-game editor (docs/IN_GAME_EDITOR.md): a few flat-coloured sprites
//! with `Name` and `EditorId`, a parent and child, and a system that spins them.
//!
//! `cargo xtask run editor_demo native`, then F5 for the Debug menu and its "Editor" tab. The game
//! starts frozen (Edit): pick entities in the hierarchy, edit their fields, add and remove
//! components, create / duplicate / delete entities (Ctrl+Z undoes), press Play to run the
//! spinning, Stop to put everything back, Save to write `scenes/demo.ron`.
//!
//! The world comes from `scenes/demo.ron` when that file exists (so Save / Load round-trip), else
//! from [`spawn_demo_world`]. The file is what `spawn_demo_world` saves (a test keeps them equal).
use std::path::PathBuf;

use efeengine_core::prelude::*;
use efeengine_ui::{DebugMenuPlugin, EditorPlugin, UiPlugin};

/// The scene file the demo loads at startup, relative to the current directory.
pub const SCENE_FILE: &str = "scenes/demo.ron";

/// Turns the entity (radians per second).
#[script_component(name = "Spin")]
#[derive(Debug, Default, PartialEq)]
pub struct Spin {
    pub speed: f32,
}

fn spin(cx: &mut Ctx) {
    let dt = cx.time.delta();
    for (t, s) in cx.world.query_mut::<(&mut Transform, &Spin)>() {
        t.rotation += s.speed * dt;
    }
}

fn sprite(color: [f32; 4], size: f32) -> Sprite {
    Sprite { color, ..Sprite::default().with_size(Vec2::splat(size)) }
}

/// `scenes/demo.ron` in the current directory, else the one next to this crate.
pub fn scene_path() -> Option<PathBuf> {
    [PathBuf::from(SCENE_FILE), PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SCENE_FILE)].into_iter().find(|p| p.exists())
}

/// The component and the game system, without a world.
pub fn register_game(engine: &mut Engine) {
    engine.register::<Spin>().expect("Spin");
    engine.add_system(Stage::Update, spin).name("spin");
}

/// The world and the game system, without any UI (tests add their own plugins): the scene file
/// when there is one (a broken file is reported and the world is spawned from code instead).
pub fn build_game(engine: &mut Engine) {
    register_game(engine);
    if let Some(path) = scene_path() {
        match engine.load_scene_file(&path) {
            Ok(v) => return global_info(&format!("[editor_demo] {} entities from {}", v.len(), path.display())),
            Err(e) => global_warn(&format!("[editor_demo] {}: {e}; spawning the demo world from code", path.display())),
        }
    }
    spawn_demo_world(engine);
}

/// The demo entities, spawned from code (what `scenes/demo.ron` holds).
pub fn spawn_demo_world(engine: &mut Engine) {
    let world = engine.world_mut();
    world.spawn((Camera { zoom: 1.0, clear_color: [0.05, 0.06, 0.09, 1.0], ..Camera::orthographic_2d() }, Transform::default(), Name::new("camera"), EditorId { id: 1 }));
    let sun = world.spawn((
        Name::new("sun"),
        EditorId { id: 2 },
        Transform::from_position(Vec2::new(-120.0, 0.0)),
        sprite([1.0, 0.75, 0.2, 1.0], 80.0),
        Spin { speed: 0.8 },
    ));
    world.spawn((
        Name::new("planet"),
        EditorId { id: 3 },
        Transform::from_position(Vec2::new(90.0, 0.0)),
        Parent::new(sun),
        sprite([0.3, 0.6, 1.0, 1.0], 36.0),
        Spin { speed: -2.0 },
    ));
    world.spawn((Name::new("crate"), EditorId { id: 4 }, Transform::from_position(Vec2::new(120.0, -60.0)), sprite([0.7, 0.45, 0.25, 1.0], 64.0)));
    world.spawn((Name::new("gem"), EditorId { id: 5 }, Transform::from_position(Vec2::new(150.0, 70.0)), sprite([0.4, 1.0, 0.5, 1.0], 28.0), Spin { speed: 3.0 }));
}

pub fn run() {
    // Save / Load and the `scenes/` list are relative to the current directory: use the crate's.
    if !std::path::Path::new("scenes").exists() {
        let _ = std::env::set_current_dir(env!("CARGO_MANIFEST_DIR"));
    }
    let mut engine = Engine::new("editor demo").expect("engine");
    engine.add_plugin(UiPlugin::new()).expect("ui");
    engine.add_plugin(DebugMenuPlugin).expect("debug menu");
    engine.add_plugin(EditorPlugin::new()).expect("editor");
    build_game(&mut engine);
    engine.set_renderer_2d(SpriteRenderer::new());
    global_info("[editor_demo] F5: Debug menu, tab Editor");
    engine.game_loop();
}
