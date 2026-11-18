//! multi_window: one world, two windows.
//!
//! The main window follows the player (WASD / arrows). A second window, "Map", shows the whole
//! playfield from a second camera (`Camera::target`), with its own UI label layer
//! (`ui.layer_in(map, ...)`). On the web the second window is a canvas mounted in the page's
//! `#map-box` element (see `web/index.html`); natively it is a second OS window. Closing "Map"
//! only closes that window; closing the main window quits. In the main window, C closes the map
//! and M reopens it.
//!
//! Everything is built from untextured sprites (flat colours), so there are no assets.

#![allow(dead_code)]

use efeengine_core::input::{InputEventKind, KeyCode, WindowEvent, WindowId};
use efeengine_core::prelude::*;
use efeengine_core::window::WindowDesc;
use efeengine_ui::{Layer, UiPlugin, UiTree};

const HALF: Vec2 = Vec2::new(320.0, 240.0);
const PLAYER_SPEED: f32 = 140.0;
const COINS: usize = 14;

struct Player;

struct Coin {
    base_y: f32,
    phase: f32,
}

struct Patrol {
    origin: Vec2,
    range: f32,
    speed: f32,
    dir: f32,
}

/// The game state (a resource): the map window while it is open.
struct Game {
    player: Entity,
    camera: Entity,
    map: Option<WindowId>,
    map_camera: Option<Entity>,
    /// Text nodes of the two HUD labels.
    main_label: efeengine_ui::WidgetId,
    map_label: efeengine_ui::WidgetId,
    score: u32,
    coins_left: usize,
}

fn flat(color: [f32; 4], size: Vec2, layer: i32) -> Sprite {
    Sprite { color, layer, ..Sprite::default() }.with_size(size)
}

fn spawn_world(world: &mut World) -> (Entity, Entity) {
    // checkerboard ground
    for y in 0..12 {
        for x in 0..16 {
            let dark = (x + y) % 2 == 0;
            let c = if dark { [0.10, 0.16, 0.12, 1.0] } else { [0.13, 0.2, 0.15, 1.0] };
            let pos = Vec2::new(x as f32 * 40.0 + 20.0, y as f32 * 40.0 + 20.0) - HALF;
            world.spawn((Transform::from_position(pos), flat(c, Vec2::splat(40.0), -10)));
        }
    }
    for i in 0..COINS {
        let a = i as f32 / COINS as f32 * std::f32::consts::TAU;
        let pos = Vec2::new(a.cos() * 200.0, a.sin() * 140.0);
        world.spawn((Transform::from_position(pos), flat([1.0, 0.8, 0.1, 1.0], Vec2::splat(14.0), 1), Coin { base_y: pos.y, phase: a }));
    }
    for (origin, range, speed) in [(Vec2::new(0.0, 100.0), 200.0, 70.0), (Vec2::new(-80.0, -120.0), 150.0, 90.0)] {
        world.spawn((
            Transform::from_position(origin),
            Velocity::default(),
            Patrol { origin, range, speed, dir: 1.0 },
            flat([0.9, 0.2, 0.2, 1.0], Vec2::splat(18.0), 2),
        ));
    }
    let player = world.spawn((Player, Transform::default(), Velocity::default(), flat([0.3, 0.6, 1.0, 1.0], Vec2::splat(18.0), 4)));
    let camera = world.spawn((Camera::new_2d().zoom(1.5).clear_color([0.02, 0.02, 0.04, 1.0]), Transform::default()));
    (player, camera)
}

/// Open the map window and its camera.
fn open_map(cx: &mut Ctx<'_>, game: &mut Game) {
    let map = cx.commands.create_window(WindowDesc::new("Map").web_element("#map-box").size(320, 240));
    // 320x240 shows the 640x480 playfield at zoom 0.5.
    let cam = cx.commands.spawn(cx.world, (Camera::new_2d().target(map).zoom(0.5).clear_color([0.0, 0.05, 0.1, 1.0]), Transform::default()));
    game.map = Some(map);
    game.map_camera = Some(cam);
    // The HUD layer is the same one: giving it the new window moves it there.
    if let Some(ui) = cx.resources.get_mut::<UiTree>() {
        ui.layer_in(map, "map-hud", Layer::top_left(0).offset(6.0, 6.0));
    }
}

fn player_input(cx: &mut Ctx<'_>) {
    let Some(game) = cx.resources.get::<Game>() else { return };
    let player = game.player;
    drop(game);
    let input = cx.input.game_input();
    let mut dir = Vec2::ZERO;
    if input.pressed(KeyCode::A) || input.pressed(KeyCode::Left) { dir.x -= 1.0 }
    if input.pressed(KeyCode::D) || input.pressed(KeyCode::Right) { dir.x += 1.0 }
    if input.pressed(KeyCode::W) || input.pressed(KeyCode::Up) { dir.y += 1.0 }
    if input.pressed(KeyCode::S) || input.pressed(KeyCode::Down) { dir.y -= 1.0 }
    if let Ok(mut v) = cx.world.get::<&mut Velocity>(player) {
        v.linear = dir.normalize_or_zero() * PLAYER_SPEED;
    }
}

fn patrol_and_bob(cx: &mut Ctx<'_>) {
    let t = cx.time.elapsed() as f32;
    for (p, v, tr) in cx.world.query::<(&mut Patrol, &mut Velocity, &Transform)>().iter() {
        let off = tr.position.x - p.origin.x;
        if off > p.range / 2.0 { p.dir = -1.0 } else if off < -p.range / 2.0 { p.dir = 1.0 }
        v.linear = Vec2::new(p.dir * p.speed, 0.0);
    }
    for (c, tr) in cx.world.query::<(&Coin, &mut Transform)>().iter() {
        tr.position.y = c.base_y + (t * 3.0 + c.phase).sin() * 4.0;
    }
}

fn collect(cx: &mut Ctx<'_>) {
    let Some(mut game) = cx.resources.remove::<Game>() else { return };
    let player_pos = cx.world.get::<&Transform>(game.player).map(|t| t.position).unwrap_or_default();
    let mut taken = Vec::new();
    for (e, (_, tr)) in cx.world.query::<(Entity, (&Coin, &Transform))>().iter() {
        if tr.position.distance(player_pos) < 16.0 {
            taken.push(e);
        }
    }
    for e in taken {
        cx.commands.despawn(e);
        game.score += 1;
        game.coins_left -= 1;
    }
    cx.resources.insert(game);
}

fn follow_camera(cx: &mut Ctx<'_>) {
    let Some(game) = cx.resources.get::<Game>() else { return };
    let (player, camera) = (game.player, game.camera);
    drop(game);
    let p = cx.world.get::<&Transform>(player).map(|t| t.position).unwrap_or_default();
    if let Ok(mut c) = cx.world.get::<&mut Transform>(camera) {
        c.position = p;
    }
}

/// Window events: `M` reopens the map; a `Closed` event for the map forgets it.
fn windows(cx: &mut Ctx<'_>) {
    let Some(mut game) = cx.resources.remove::<Game>() else { return };
    for ev in cx.input.events() {
        if let InputEventKind::Window(WindowEvent::Closed) = ev.kind
            && game.map == Some(ev.window)
        {
            game.map = None;
            if let Some(c) = game.map_camera.take() {
                cx.commands.despawn(c);
            }
            global_info("[multi_window] map window closed (press M to reopen it)");
        }
    }
    if game.map.is_none() && cx.input.just_pressed(KeyCode::M) {
        open_map(cx, &mut game);
    } else if let Some(map) = game.map
        && cx.input.just_pressed(KeyCode::C)
    {
        // (the web has no close button on the canvas)
        cx.commands.close_window(map);
    }
    cx.resources.insert(game);
}

/// Keep the two labels current.
fn labels(cx: &mut Ctx<'_>) {
    let Some(game) = cx.resources.get::<Game>() else { return };
    let (score, left, main_label, map_label, has_map) = (game.score, game.coins_left, game.main_label, game.map_label, game.map.is_some());
    drop(game);
    let Some(ui) = cx.resources.get_mut::<UiTree>() else { return };
    ui.set_text(main_label, format!("Coins {score}/{}  (M: map window)", score as usize + left));
    ui.set_text(map_label, if has_map { format!("Map - {left} coins left") } else { "Map".to_string() });
}

fn setup(engine: &mut Engine) {
    let (player, camera) = spawn_world(engine.world_mut());
    let map = match engine.create_window(WindowDesc::new("Map").web_element("#map-box").size(320, 240)) {
        Ok(w) => Some(w),
        Err(e) => {
            global_warn(&format!("[multi_window] no map window: {e}"));
            None
        }
    };
    let map_camera = map.map(|w| {
        engine.world_mut().spawn((Camera::new_2d().target(w).zoom(0.5).clear_color([0.0, 0.05, 0.1, 1.0]), Transform::default()))
    });
    if let Err(e) = engine.add_plugin(UiPlugin::new()) {
        global_warn(&format!("[multi_window] ui plugin failed: {e}"));
    }
    // One HUD layer per window: the main one, and the map's with its own label.
    let (main_label, map_label) = {
        let ui = engine.resources_mut().get_mut::<UiTree>().expect("UiTree");
        let main = ui.layer("main-hud", Layer::top_left(0).offset(8.0, 8.0));
        let main_label = ui.text(main, "Coins").class("panel").key("main-label").id();
        let map_layer = match map {
            Some(w) => ui.layer_in(w, "map-hud", Layer::top_left(0).offset(6.0, 6.0)),
            None => ui.layer("map-hud", Layer::top_left(0).offset(6.0, 6.0).window(u32::MAX)),
        };
        let map_label = ui.text(map_layer, "Map").class("panel").key("map-label").id();
        (main_label, map_label)
    };
    engine.insert_resource(Game { player, camera, map, map_camera, main_label, map_label, score: 0, coins_left: COINS });
    engine.add_system(Stage::PreUpdate, windows);
    engine.add_system(Stage::Update, player_input);
    engine.add_system(Stage::Update, patrol_and_bob);
    engine.add_system(Stage::PostUpdate, collect);
    engine.add_system(Stage::PostUpdate, labels);
    engine.add_system(Stage::PostUpdate, follow_camera);
}

#[main]
fn main() {
    match Engine::new("multi window") {
        Ok(mut engine) => {
            setup(&mut engine);
            global_info("[multi_window] WASD / arrows to move; C closes the Map window, M reopens it");
            engine.game_loop();
        }
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
