//! Manual test bed for the in-game editor (docs/IN_GAME_EDITOR.md): a few flat-coloured sprites
//! with `Name` and `EditorId`, a parent and child, and a system that spins them.
//!
//! `cargo xtask run editor_demo native`, then F5 for the Debug menu and its "Editor" tab. The game
//! starts frozen (Edit): pick entities in the hierarchy, see their components, press Play to run
//! the spinning, Stop to put everything back.
use efeengine_core::prelude::*;
use efeengine_ui::{DebugMenuPlugin, EditorPlugin, UiPlugin};

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

/// The world and the game system, without any UI (tests add their own plugins).
pub fn build_game(engine: &mut Engine) {
    engine.register::<Spin>().expect("Spin");
    engine.add_system(Stage::Update, spin).name("spin");
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
    let mut engine = Engine::new("editor demo").expect("engine");
    engine.add_plugin(UiPlugin::new()).expect("ui");
    engine.add_plugin(DebugMenuPlugin).expect("debug menu");
    engine.add_plugin(EditorPlugin::new()).expect("editor");
    build_game(&mut engine);
    engine.set_renderer_2d(SpriteRenderer::new());
    global_info("[editor_demo] F5: Debug menu, tab Editor");
    engine.game_loop();
}
