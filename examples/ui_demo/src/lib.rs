//! UI demo: a settings menu and a HUD built with the `efeengine-ui` view layer.
//!
//! On native, `UiPlugin::new()` uses the iced backend (default feature `iced`; without it the
//! headless backend, so nothing is drawn): the menu and HUD are drawn over the game and driven
//! by mouse and keyboard. `EFE_UI_STATS=1` logs the cost of the iced pass. On the web, the plugin
//! picks the DOM backend and the menu and HUD are visible in the page; the log lines go to the
//! browser console.
//!
//! Run natively with `cargo xtask run ui_demo native`, on the web with `cargo xtask run ui_demo web`.
use efeengine_core::prelude::*;
use efeengine_core::render::Image;
use efeengine_ui::view::*;
use efeengine_ui::{ImageFit, Layer, Size, Style, UiPlugin, column, row};

/// The game state, read by both views and changed by the update system.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub volume: f32,
    pub fullscreen: bool,
    pub name: String,
    pub plays: u32,
    pub show_menu: bool,
    /// The logo shown in the menu: a generated texture whose colours change on every Play, which
    /// exercises the texture refresh of the backends.
    pub logo: Option<TextureHandle>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { volume: 0.8, fullscreen: false, name: String::new(), plays: 0, show_menu: true, logo: None }
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

/// A 64x64 gradient; `hue` shifts the colours.
fn logo_image(hue: u32) -> Image {
    let mut px = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let t = (x + y) as f32 / 126.0;
            let r = (255.0 * (0.2 + 0.8 * t)) as u32;
            let g = (255.0 * (1.0 - t)) as u32;
            let b = (255.0 * (0.5 + 0.5 * ((x as f32 / 10.0).sin() * (y as f32 / 10.0).cos()))) as u32;
            let rot = [r, g, b];
            let k = hue as usize % 3;
            px.extend_from_slice(&[rot[k] as u8, rot[(k + 1) % 3] as u8, rot[(k + 2) % 3] as u8, 255]);
        }
    }
    Image::new(64, 64, px).expect("logo image")
}

fn menu_view(s: &Settings) -> El<Msg> {
    column![
        text("UI demo").class("title").key("title"),
        s.logo
            .map(|t| image(t, ImageFit::Contain).key("logo").style(Style::new().width(Size::Px(64.0)).height(Size::Px(64.0))))
            .unwrap_or_else(|| container().key("logo")),
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
                let (plays, logo) = (s.plays, s.logo);
                if let (Some(logo), Some(textures)) = (logo, cx.resources.get_mut::<Textures>()) {
                    let _ = textures.replace(logo, logo_image(plays));
                }
                continue;
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
    let logo = engine.textures_mut().add(logo_image(0));
    engine.insert_resource(Settings { logo: Some(logo), ..Settings::default() });
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
