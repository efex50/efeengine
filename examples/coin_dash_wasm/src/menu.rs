//! coin_dash_wasm menus: main menu, settings, pause menu, game-over screen and HUD.
//!
//! Mirrors `examples/coin_dash/src/menu.rs` and adds:
//! - `Screen::GameOver` when the player loses all lives (with Play again / Main menu / Quit),
//! - `Lives` in the HUD alongside Score, Round and Coins left,
//! - a warning banner in the main menu and HUD if the wasm mods were not built yet
//!   ("mods not built: run build_mods.py") or on unsupported web targets.

use efeengine_core::prelude::*;
use efeengine_ui::view::*;
use efeengine_ui::{Align, Color, Justify, Layer, Size, Style, TextAlign, UiPlugin, UiTree, column};

use crate::Game;

/// Opacity of the whole HUD (0 = invisible, 1 = opaque).
const HUD_OPACITY: f32 = 0.6;
/// Size (logical px) of the area of the HUD you can grab with the mouse.
const HUD_GRAB: Vec2 = Vec2::new(320.0, 70.0);
/// Gap kept between the HUD and the window edge while dragging (logical px).
const HUD_MARGIN: f32 = 4.0;

/// Where the HUD is (offset of its top-left layer, logical px) and the drag in progress.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudDrag {
    pub offset: Vec2,
    pub dragging: bool,
    grab: Vec2,
}

/// Which screen is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Main,
    Playing,
    Paused,
    GameOver,
}

/// Menu state and settings. A resource.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub screen: Screen,
    pub settings_open: bool,
    pub volume: f32,
    pub fullscreen: bool,
}

impl Menu {
    pub fn new(start_playing: bool) -> Self {
        Menu {
            screen: if start_playing { Screen::Playing } else { Screen::Main },
            settings_open: false,
            volume: 0.8,
            fullscreen: false,
        }
    }

    /// True while gameplay should advance (no modal menu is open).
    pub fn is_running(&self) -> bool {
        self.screen == Screen::Playing
    }
}

/// UI button / control messages.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuMsg {
    Play,
    Resume,
    Restart,
    OpenSettings,
    CloseSettings,
    MainMenu,
    Quit,
    Volume(f32),
    Fullscreen(bool),
}

/// Run condition for game systems: true while playing.
pub fn running(cx: &RunCtx<'_>) -> bool {
    cx.res::<Menu>().is_none_or(|m| m.is_running())
}

fn percent(v: f32) -> u32 {
    (v * 100.0).round() as u32
}

/// A centred column on a dark backdrop that fills the window.
fn screen(title: &str, body: El<MenuMsg>) -> El<MenuMsg> {
    container()
        .width(Size::Fill)
        .height(Size::Fill)
        .align(Align::Center)
        .justify(Justify::Center)
        .background(Color::rgba(0.0, 0.0, 0.0, 0.6))
        .child(
            column![text(title).class("title").key("title").width(Size::Fill).text_align(TextAlign::Center), body]
                .class("panel")
                .gap(12.0)
                .align(Align::Center)
                .style(Style::new().width(Size::Px(320.0))),
        )
}

fn wide(b: El<MenuMsg>) -> El<MenuMsg> {
    b.width(Size::Fill)
}

fn mod_status_banner(cx: &ViewCx) -> Option<El<MenuMsg>> {
    let status = cx.res::<Game>().and_then(|g| g.mod_status())?;
    Some(
        text(status)
            .key("mod_status")
            .width(Size::Fill)
            .text_align(TextAlign::Center)
            .style(Style::new().text_color(Color::rgba(1.0, 0.45, 0.35, 1.0))),
    )
}

fn main_menu(cx: &ViewCx) -> El<MenuMsg> {
    let mut items = column![
        wide(button("Play").key("play").on_click(MenuMsg::Play)),
        wide(button("Settings").key("settings").on_click(MenuMsg::OpenSettings)),
        wide(button("Quit").key("quit").on_click(MenuMsg::Quit)),
    ]
    .gap(8.0)
    .width(Size::Fill);
    if let Some(banner) = mod_status_banner(cx) {
        items = items.child(banner);
    }
    screen("Coin Dash", items)
}

fn pause_menu(_: &Menu) -> El<MenuMsg> {
    screen(
        "Paused",
        column![
            wide(button("Resume").key("resume").on_click(MenuMsg::Resume)),
            wide(button("Restart").key("restart").on_click(MenuMsg::Restart)),
            wide(button("Settings").key("settings").on_click(MenuMsg::OpenSettings)),
            wide(button("Main menu").key("main").on_click(MenuMsg::MainMenu)),
            wide(button("Quit").key("quit").on_click(MenuMsg::Quit)),
        ]
        .gap(8.0)
        .width(Size::Fill),
    )
}

fn game_over_menu(cx: &ViewCx) -> El<MenuMsg> {
    let summary = match cx.res::<Game>() {
        Some(g) => format!("Final score {}   Round {}", g.score(), g.round()),
        None => String::new(),
    };
    screen(
        "Game Over",
        column![
            text(summary).key("final_stats").width(Size::Fill).text_align(TextAlign::Center),
            wide(button("Play again").key("restart").on_click(MenuMsg::Restart)),
            wide(button("Main menu").key("main").on_click(MenuMsg::MainMenu)),
            wide(button("Quit").key("quit").on_click(MenuMsg::Quit)),
        ]
        .gap(8.0)
        .width(Size::Fill),
    )
}

fn settings_menu(m: &Menu) -> El<MenuMsg> {
    screen(
        "Settings",
        column![
            text(format!("Volume: {}%", percent(m.volume))).key("volume_text"),
            wide(slider(0.0..=1.0, m.volume).step(0.05).key("volume").on_change(MenuMsg::Volume)),
            checkbox("Fullscreen", m.fullscreen).key("fullscreen").on_toggle(MenuMsg::Fullscreen),
            wide(button("Back").key("back").on_click(MenuMsg::CloseSettings)),
        ]
        .gap(10.0)
        .width(Size::Fill),
    )
}

fn hud(cx: &ViewCx) -> El<MenuMsg> {
    let line = match cx.res::<Game>() {
        Some(g) => format!(
            "Score {}   Round {}   Coins left {}   Lives {}",
            g.score(),
            g.round(),
            g.coins_left(),
            g.lives()
        ),
        None => String::new(),
    };
    let mut col = column![text(line).key("stats"), text("Esc: pause").class("muted").key("hint")]
        .class("panel")
        .gap(2.0)
        .style(Style::new().opacity(HUD_OPACITY));
    if let Some(status) = cx.res::<Game>().and_then(|g| g.mod_status()) {
        col = col.child(text(status).key("mod_status").style(Style::new().text_color(Color::rgba(1.0, 0.45, 0.35, 1.0))));
    }
    col
}

fn hud_drag(cx: &mut Ctx) {
    let scale = cx.window.scale_factor as f32;
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (w, h) = cx.window.logical_size();
    let pos = cx.input.mouse_pos().map(|(x, y)| Vec2::new(x / scale, y / scale));
    let pressed = cx.input.mouse_pressed(MouseButton::Left);
    let just = cx.input.mouse_just_pressed(MouseButton::Left);
    let Some(d) = cx.resources.get_mut::<HudDrag>() else { return };
    if just
        && let Some(p) = pos
    {
        let rel = p - d.offset;
        if rel.x >= 0.0 && rel.y >= 0.0 && rel.x <= HUD_GRAB.x && rel.y <= HUD_GRAB.y {
            d.dragging = true;
            d.grab = rel;
        }
    }
    if !pressed {
        d.dragging = false;
    }
    if d.dragging
        && let Some(p) = pos
    {
        let max = Vec2::new((w - HUD_GRAB.x - HUD_MARGIN).max(HUD_MARGIN), (h - HUD_GRAB.y - HUD_MARGIN).max(HUD_MARGIN));
        d.offset = (p - d.grab).clamp(Vec2::splat(HUD_MARGIN), max);
    }
}

fn hud_place(cx: &mut Ctx) {
    let Some(off) = cx.resources.get::<HudDrag>().map(|d| d.offset) else { return };
    if let Some(tree) = cx.resources.get_mut::<UiTree>() {
        tree.layer("hud", hud_layer().offset(off.x, off.y));
    }
}

fn hud_layer() -> Layer {
    Layer::top_left(10)
}

fn menu_update(cx: &mut Ctx) {
    let msgs: Vec<MenuMsg> = cx.events::<MenuMsg>().iter().cloned().collect();
    let esc = cx.input.just_pressed(KeyCode::Escape);
    let fullscreen_now = cx
        .input
        .events()
        .iter()
        .filter(|e| e.window.is_main())
        .filter_map(|e| match e.kind {
            efeengine_core::input::InputEventKind::Window(efeengine_core::input::WindowEvent::FullscreenChanged(on)) => Some(on),
            _ => None,
        })
        .last();
    let game_over = cx.resources.get::<Game>().is_some_and(|g| g.is_game_over());
    let mut trigger_restart = false;

    let Some(m) = cx.resources.get_mut::<Menu>() else { return };
    if let Some(on) = fullscreen_now {
        m.fullscreen = on;
    }
    let before = m.clone();
    if game_over && m.screen == Screen::Playing {
        m.screen = Screen::GameOver;
        m.settings_open = false;
    }
    for msg in msgs {
        match msg {
            MenuMsg::Play => {
                if m.screen == Screen::GameOver {
                    trigger_restart = true;
                }
                m.screen = Screen::Playing;
                m.settings_open = false;
            }
            MenuMsg::Resume => {
                m.screen = Screen::Playing;
                m.settings_open = false;
            }
            MenuMsg::Restart => {
                trigger_restart = true;
                m.screen = Screen::Playing;
                m.settings_open = false;
            }
            MenuMsg::OpenSettings => m.settings_open = true,
            MenuMsg::CloseSettings => m.settings_open = false,
            MenuMsg::MainMenu => {
                if m.screen == Screen::GameOver {
                    trigger_restart = true;
                }
                m.screen = Screen::Main;
                m.settings_open = false;
            }
            MenuMsg::Quit => cx.commands.quit(),
            MenuMsg::Volume(v) => m.volume = v.clamp(0.0, 1.0),
            MenuMsg::Fullscreen(on) => m.fullscreen = on,
        }
    }
    if esc {
        if m.settings_open {
            m.settings_open = false;
        } else {
            match m.screen {
                Screen::Playing => m.screen = Screen::Paused,
                Screen::Paused => m.screen = Screen::Playing,
                Screen::Main | Screen::GameOver => {}
            }
        }
    }
    let now = m.clone();

    if trigger_restart && let Some(g) = cx.resources.get::<Game>() {
        g.request_restart();
    }
    if now.fullscreen != before.fullscreen {
        cx.commands.set_fullscreen(now.fullscreen);
    }
    if now.is_running() != before.is_running() {
        cx.schedule().set_enabled("velocity", now.is_running());
        cx.schedule().set_enabled("script_system", now.is_running());
    }
    if let Some(views) = cx.resources.get_mut::<UiViews>() {
        views.set_enabled("main_menu", now.screen == Screen::Main && !now.settings_open);
        views.set_enabled("pause", now.screen == Screen::Paused && !now.settings_open);
        views.set_enabled("game_over", now.screen == Screen::GameOver && !now.settings_open);
        views.set_enabled("settings", now.settings_open);
    }
}

/// Add the menus and the HUD. Expects `UiPlugin` to be added already.
pub fn setup(engine: &mut Engine, start_playing: bool) {
    engine.insert_resource(Menu::new(start_playing));
    let _ = engine.set_system_enabled("velocity", start_playing);
    let _ = engine.set_system_enabled("script_system", start_playing);
    let modal = |z| Layer::fill(z).modal();
    engine.add_plugin(UiView::with("main_menu", main_menu).layer(modal(30)).enabled(!start_playing)).expect("main menu view");
    engine.add_plugin(UiView::new("pause", pause_menu).layer(modal(30)).enabled(false)).expect("pause view");
    engine.add_plugin(UiView::with("game_over", game_over_menu).layer(modal(30)).enabled(false)).expect("game over view");
    engine.add_plugin(UiView::new("settings", settings_menu).layer(modal(31)).enabled(false)).expect("settings view");
    engine.add_plugin(UiView::with("hud", hud).layer(hud_layer())).expect("hud view");
    engine.insert_resource(HudDrag { offset: Vec2::splat(HUD_MARGIN), ..HudDrag::default() });
    engine.add_system(Stage::Update, hud_drag).order(order::EARLY + 1).name("coin_dash_hud_drag").run_if(running);
    engine.add_system(Stage::PostUpdate, hud_place).order(order::LATE + 12).name("coin_dash_hud_place");
    engine.add_system(Stage::Update, menu_update).order(order::EARLY).name("coin_dash_menu");
}

/// The UI plugin for the game.
pub fn plugin() -> UiPlugin {
    UiPlugin::new()
}
