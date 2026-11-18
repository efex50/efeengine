//! coin_dash menus: main menu, settings and pause menu, built with the UI view layer.
//!
//! Three full-window modal layers (`main_menu`, `settings`, `pause`) and a HUD. Only one menu
//! is shown at a time; a visible modal layer captures pointer and keyboard (`InputState::capture`),
//! and the game systems also stop (`running` is their run condition) and the built-in velocity
//! system is switched off, so nothing moves while a menu is open.
//!
//! * Main menu: Play, Settings, Quit. The game starts here (unless `COIN_DASH_AUTOPLAY` is set).
//! * Esc while playing opens the pause menu (Resume, Settings, Main menu, Quit); Esc again resumes.
//!   Esc inside the settings goes back. The Esc check reads the raw input (`ctx.input`), because
//!   the game view of the input is empty while a modal layer captures the keyboard.
//! * Settings: volume slider and a fullscreen checkbox. The volume is stored in [`Menu`] (the game
//!   has no audio yet, the value is shown and kept for when it does); fullscreen goes to the
//!   window through `Commands::set_fullscreen`.
//!
//! The same code runs on both backends (iced on native, DOM on the web); it only uses
//! `efeengine-ui` types.

use efeengine_core::prelude::*;
use efeengine_ui::view::*;
use efeengine_ui::{Align, Color, Justify, Layer, Size, Style, TextAlign, UiPlugin, column};

/// Where the player is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Main,
    Playing,
    Paused,
}

/// Menu state and the settings. A resource.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub screen: Screen,
    /// The settings page is open on top of `screen` (Main or Paused).
    pub settings_open: bool,
    /// 0.0 to 1.0.
    pub volume: f32,
    pub fullscreen: bool,
}

impl Menu {
    pub fn new(start_playing: bool) -> Self {
        Menu { screen: if start_playing { Screen::Playing } else { Screen::Main }, settings_open: false, volume: 0.8, fullscreen: false }
    }

    /// The game runs (no menu is open).
    pub fn is_running(&self) -> bool {
        self.screen == Screen::Playing
    }
}

/// What the buttons send.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuMsg {
    Play,
    Resume,
    OpenSettings,
    CloseSettings,
    MainMenu,
    Quit,
    Volume(f32),
    Fullscreen(bool),
}

/// Run condition of the game systems: true while no menu is open (and when there is no menu).
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
                .style(Style::new().width(Size::Px(300.0))),
        )
}

fn wide(b: El<MenuMsg>) -> El<MenuMsg> {
    b.width(Size::Fill)
}

fn main_menu(_: &Menu) -> El<MenuMsg> {
    screen(
        "Coin Dash",
        column![
            wide(button("Play").key("play").on_click(MenuMsg::Play)),
            wide(button("Settings").key("settings").on_click(MenuMsg::OpenSettings)),
            wide(button("Quit").key("quit").on_click(MenuMsg::Quit)),
        ]
        .gap(8.0)
        .width(Size::Fill),
    )
}

fn pause_menu(_: &Menu) -> El<MenuMsg> {
    screen(
        "Paused",
        column![
            wide(button("Resume").key("resume").on_click(MenuMsg::Resume)),
            wide(button("Settings").key("settings").on_click(MenuMsg::OpenSettings)),
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
    let line = match cx.res::<crate::Game>() {
        Some(g) => format!("Score {}   Round {}   Coins left {}", g.score, g.round, g.coins_left),
        None => String::new(),
    };
    column![text(line).key("stats"), text("Esc: pause").class("muted").key("hint")].class("panel").gap(2.0)
}

/// Apply the messages and the Esc key, then show the right menu and (un)freeze the game.
fn menu_update(cx: &mut Ctx) {
    let msgs: Vec<MenuMsg> = cx.events::<MenuMsg>().iter().cloned().collect();
    let esc = cx.input.just_pressed(KeyCode::Escape);
    let Some(m) = cx.resources.get_mut::<Menu>() else { return };
    let before = m.clone();
    for msg in msgs {
        match msg {
            MenuMsg::Play | MenuMsg::Resume => {
                m.screen = Screen::Playing;
                m.settings_open = false;
            }
            MenuMsg::OpenSettings => m.settings_open = true,
            MenuMsg::CloseSettings => m.settings_open = false,
            MenuMsg::MainMenu => {
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
                Screen::Main => {}
            }
        }
    }
    let now = m.clone();
    if now.fullscreen != before.fullscreen {
        cx.commands.set_fullscreen(now.fullscreen);
    }
    if now.is_running() != before.is_running() {
        // the built-in velocity system would keep sliding the player and the enemies
        cx.schedule().set_enabled("velocity", now.is_running());
    }
    if let Some(views) = cx.resources.get_mut::<UiViews>() {
        views.set_enabled("main_menu", now.screen == Screen::Main && !now.settings_open);
        views.set_enabled("pause", now.screen == Screen::Paused && !now.settings_open);
        views.set_enabled("settings", now.settings_open);
    }
}

/// Add the menus and the HUD. Expects `UiPlugin` to be added already (`UiTest::new` does it, and
/// `main` adds it before calling this).
pub fn setup(engine: &mut Engine, start_playing: bool) {
    engine.insert_resource(Menu::new(start_playing));
    // the built-in velocity system would keep sliding the player and the enemies behind a menu
    let _ = engine.set_system_enabled("velocity", start_playing);
    let modal = |z| Layer::fill(z).modal();
    engine.add_plugin(UiView::new("main_menu", main_menu).layer(modal(30)).enabled(!start_playing)).expect("main menu view");
    engine.add_plugin(UiView::new("pause", pause_menu).layer(modal(30)).enabled(false)).expect("pause view");
    engine.add_plugin(UiView::new("settings", settings_menu).layer(modal(31)).enabled(false)).expect("settings view");
    engine.add_plugin(UiView::with("hud", hud).layer(Layer::top_left(10))).expect("hud view");
    engine.add_system(Stage::Update, menu_update).order(order::EARLY).name("coin_dash_menu");
}

/// The UI plugin for the game: the default one (iced on native, DOM on the web).
pub fn plugin() -> UiPlugin {
    UiPlugin::new()
}
