//! UI demo: a settings menu and a HUD built with the `efeengine-ui` view layer.
//!
//! On native, `UiPlugin::new()` uses the headless backend, so nothing is drawn: the menu only
//! runs (messages, state and log lines in the terminal). On the web, the plugin picks the DOM
//! backend and the menu and HUD are visible in the page; the log lines go to the browser console.
//!
//! Run natively with `cargo xtask run ui_demo native`, on the web with `cargo xtask run ui_demo web`.
use efeengine_core::prelude::*;
use efeengine_ui::view::*;
use efeengine_ui::{Layer, UiPlugin, column, row};

/// The game state, read by both views and changed by the update system.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub volume: f32,
    pub fullscreen: bool,
    pub name: String,
    pub plays: u32,
    pub show_menu: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { volume: 0.8, fullscreen: false, name: String::new(), plays: 0, show_menu: true }
    }
}

/// What the widgets send; the update system turns them into `Settings` changes.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Play,
    Volume(f32),
    Fullscreen(bool),
    Name(String),
    Submit(String),
}

fn percent(volume: f32) -> u32 {
    (volume * 100.0).round() as u32
}

fn hud_line(s: &Settings) -> String {
    format!("Plays: {}  Volume: {}%", s.plays, percent(s.volume))
}

fn menu_view(s: &Settings) -> El<Msg> {
    column![
        text("UI demo").class("title").key("title"),
        row![
            button("Play").key("play").on_click(Msg::Play),
            checkbox("Fullscreen", s.fullscreen).key("fullscreen").on_toggle(Msg::Fullscreen),
        ]
        .gap(12.0),
        text(format!("Volume: {}%", percent(s.volume))).key("volume_text"),
        slider(0.0..=1.0, s.volume).step(0.05).key("volume").on_change(Msg::Volume),
        text_input(&s.name)
            .placeholder("Player name")
            .max_len(16)
            .key("name")
            .on_input(Msg::Name)
            .on_submit(Msg::Submit),
        text(format!("Plays: {}", s.plays)).key("plays"),
    ]
    .class("panel")
    .gap(12.0)
}

fn hud_view(s: &Settings) -> El<Msg> {
    column![text(hud_line(s)).key("text")].class("panel")
}

fn update(cx: &mut Ctx) {
    let msgs: Vec<Msg> = cx.events::<Msg>().iter().cloned().collect();
    for m in msgs {
        global_info(&format!("[ui_demo] {m:?}"));
        let s = cx.resources.get_mut::<Settings>().unwrap();
        match m {
            Msg::Play => {
                s.plays += 1;
                global_info(&format!("[ui_demo] play pressed {} times", s.plays));
            }
            Msg::Volume(v) => s.volume = v,
            Msg::Fullscreen(f) => s.fullscreen = f,
            Msg::Name(n) => s.name = n,
            Msg::Submit(n) => global_info(&format!("[ui_demo] hello {n}")),
        }
    }

    // The menu is shown while `show_menu` is set.
    let show = cx.resources.get_mut::<Settings>().unwrap().show_menu;
    if let Some(views) = cx.resources.get_mut::<UiViews>() {
        views.set_enabled("menu", show);
    }
}

/// The game's resources, views and systems. Expects `UiPlugin` to be added already (`run` adds
/// it; `UiTest::new` adds it before calling this).
pub fn setup(engine: &mut Engine) {
    engine.insert_resource(Settings::default());
    engine.add_plugin(UiView::new("menu", menu_view).layer(Layer::centered(10))).expect("menu view");
    engine.add_plugin(UiView::new("hud", hud_view).layer(Layer::top_left(20))).expect("hud view");
    engine.add_system(Stage::Update, update);
}

#[main]
fn main() {
    let mut engine = match Engine::new("ui demo") {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine init failed: {e}");
            return;
        }
    };
    engine.add_plugin(UiPlugin::new()).expect("UiPlugin");
    setup(&mut engine);

    engine.add_system(Stage::Startup, |_: &mut Ctx<'_>| {
        global_info("[ui_demo] started");
    });

    engine.game_loop();
}
