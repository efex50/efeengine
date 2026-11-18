//! The editor window (docs/EDITOR.md E1/E2). A standalone iced application: it owns no game
//! state and talks to the game only through the editor-link worker thread.

use efeengine_editor::conn::ConnState;
use efeengine_editor::model::*;
use efeengine_editor::worker::{self, Event, Handle};
use efeengine_editor_link::protocol::{ComponentInfo, EntityInfo, LogLine, Mode, Reply, Request, StageInfo};
use iced::time::{self, milliseconds};
use iced::widget::{Widget, button, column, container, row, scrollable, space, text, text_input};
use iced::{Fill, Font, Subscription, Theme};

fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port = match parse_port(&args, std::env::var("EFE_EDITOR_PORT").ok().as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("efe-editor: {e}\nusage: efe-editor [--port N]   (default: $EFE_EDITOR_PORT or 7878)");
            std::process::exit(2);
        }
    };
    iced::application(move || Editor::new(port), Editor::update, Editor::view)
        .title("efeengine editor")
        .subscription(Editor::subscription)
        .theme(|_: &Editor| Theme::Dark)
        .window_size((1280.0, 800.0))
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    Select(EntityInfo),
    SetMode(Mode),
    PathChanged(String),
    Save,
    Load,
    WarningsOnly(bool),
}

#[derive(Debug, Clone, Default)]
struct SceneView {
    name: Option<String>,
    path: Option<String>,
    project_dir: String,
}

struct Editor {
    port: u16,
    worker: Handle,
    conn: ConnState,
    ticks: u32,
    entities: Vec<EntityInfo>,
    selected: Option<EntityInfo>,
    components: Vec<ComponentInfo>,
    inspector_error: Option<String>,
    stats: Option<(u64, f64, u32)>,
    logs: Vec<LogLine>,
    warnings_only: bool,
    systems: Vec<StageInfo>,
    scene: SceneView,
    mode: Mode,
    path_input: String,
    notice: Option<String>,
}

impl Editor {
    fn new(port: u16) -> Self {
        Self {
            port,
            worker: worker::spawn(port),
            conn: ConnState::Connecting,
            ticks: 0,
            entities: Vec::new(),
            selected: None,
            components: Vec::new(),
            inspector_error: None,
            stats: None,
            logs: Vec::new(),
            warnings_only: false,
            systems: Vec::new(),
            scene: SceneView::default(),
            mode: Mode::Play,
            path_input: String::new(),
            notice: None,
        }
    }

    fn components_request(sel: &EntityInfo) -> Request {
        match &sel.editor_id {
            Some(id) => Request::Components { entity: None, editor_id: Some(id.clone()) },
            None => Request::Components { entity: Some(sel.entity), editor_id: None },
        }
    }

    fn apply(&mut self, request: Request, reply: Reply) {
        match (request, reply) {
            (_, Reply::Stats { frame, frame_time, entity_count }) => self.stats = Some((frame, frame_time, entity_count)),
            (_, Reply::Entities { mut entities }) => {
                sort_entities(&mut entities);
                self.entities = entities;
            }
            (_, Reply::Components { entity, components }) => {
                if self.selected.as_ref().is_some_and(|s| s.entity == entity || s.editor_id.is_some()) {
                    self.components = components;
                    self.inspector_error = None;
                }
            }
            (Request::Components { .. }, Reply::Error { message, .. }) => {
                self.components.clear();
                self.inspector_error = Some(message);
            }
            (_, Reply::Logs { lines }) => self.logs = lines,
            (_, Reply::Systems { stages }) => self.systems = stages,
            (_, Reply::Scene { name, path, mode, project_dir }) => {
                self.scene = SceneView { name, path, project_dir };
                self.mode = mode;
            }
            (_, Reply::Mode { mode }) => {
                self.mode = mode;
                self.notice = Some(format!("{} mode", mode_label(mode)));
                self.refresh_lists();
            }
            (_, Reply::Saved { path }) => self.notice = Some(format!("saved {path}")),
            (_, Reply::Loaded { path, entities }) => {
                self.notice = Some(format!("loaded {path} ({entities} entities)"));
                self.refresh_lists();
            }
            (_, Reply::Error { code, message }) => self.notice = Some(format!("error ({code}): {message}")),
            (_, Reply::Schema { .. }) => {}
        }
    }

    fn refresh_lists(&self) {
        for r in [Request::Entities, Request::Systems, Request::Scene] {
            self.worker.send(r);
        }
    }

    /// Poll cadence on a 100 ms tick: stats about 3 times a second, the open entity every
    /// half second, the lists every second. Nothing is sent per frame.
    fn poll(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if !self.conn.is_connected() {
            return;
        }
        if self.ticks % 3 == 0 {
            self.worker.send(Request::Stats);
        }
        if let (0, Some(sel)) = (self.ticks % 5, &self.selected) {
            self.worker.send(Self::components_request(sel));
        }
        if self.ticks % 10 == 1 {
            self.refresh_lists();
            self.worker.send(Request::Logs { count: 300 });
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Tick => {
                for ev in self.worker.poll() {
                    match ev {
                        Event::State(s) => {
                            let was = self.conn.is_connected();
                            self.conn = s;
                            if !was && self.conn.is_connected() {
                                self.ticks = 0;
                                self.notice = None;
                                self.refresh_lists();
                                self.worker.send(Request::Logs { count: 300 });
                                self.worker.send(Request::Stats);
                            }
                        }
                        Event::Reply { request, reply } => self.apply(request, reply),
                    }
                }
                self.poll();
            }
            Message::Select(e) => {
                self.worker.send(Self::components_request(&e));
                self.selected = Some(e);
                self.components.clear();
                self.inspector_error = None;
            }
            Message::SetMode(m) => self.worker.send(Request::SetMode { mode: m }),
            Message::PathChanged(p) => self.path_input = p,
            Message::Save => self.file_action(true),
            Message::Load => self.file_action(false),
            Message::WarningsOnly(on) => self.warnings_only = on,
        }
    }

    fn file_action(&mut self, save: bool) {
        let path = self.path_input.trim().to_string();
        if let Err(e) = validate_scene_path(&path) {
            self.notice = Some(format!("bad path: {e}"));
        } else if save {
            self.worker.send(Request::SaveScene { path });
        } else {
            self.worker.send(Request::LoadScene { path });
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        time::every(milliseconds(100)).map(|_| Message::Tick)
    }

    fn view(&self) -> impl Widget<Message> {
        let connected = self.conn.is_connected();
        let mode_button = |label: &'static str, m: Mode| {
            let b = button(label);
            if connected && self.mode != m { b.on_press(Message::SetMode(m)) } else { b }
        };
        let top = row![
            text(self.conn.label(self.port)),
            space().width(Fill),
            text(format!("mode: {}", mode_label(self.mode))),
            mode_button("Edit", Mode::Edit),
            mode_button("Play", Mode::Play),
        ]
        .spacing(10)
        .align_y(iced::Center);

        let body = row![
            panel("Hierarchy", self.hierarchy()).width(Fill),
            panel("Inspector", self.inspector()).width(Fill),
            column![
                panel("Stats", self.stats_panel()),
                panel("Scene", self.scene_panel()),
                panel("Systems", self.systems_panel()).height(Fill),
            ]
            .spacing(8)
            .width(Fill),
        ]
        .spacing(8)
        .height(Fill);

        column![top, body, panel("Log", self.log_panel()).height(200)].spacing(8).padding(8)
    }

    fn hierarchy(&self) -> impl Widget<Message> {
        if self.entities.is_empty() {
            return text("no entities (or not connected)").boxed();
        }
        let rows = self.entities.iter().map(|e| {
            let selected = self.selected.as_ref().is_some_and(|s| s.entity == e.entity);
            button(text(entity_label(e)))
                .width(Fill)
                .style(if selected { button::primary } else { button::text })
                .on_press(Message::Select(e.clone()))
                .boxed()
        });
        scrollable(column(rows)).width(Fill).height(Fill).boxed()
    }

    fn inspector(&self) -> impl Widget<Message> {
        let Some(sel) = &self.selected else {
            return text("select an entity").boxed();
        };
        let mut items = vec![text(entity_label(sel)).size(18).boxed()];
        if let Some(err) = &self.inspector_error {
            items.push(text(format!("cannot read components: {err}")).boxed());
        }
        for c in &self.components {
            items.push(text(c.name.clone()).size(16).boxed());
            for (k, v) in component_rows(c) {
                items.push(row![text(k).width(120).font(Font::MONOSPACE), text(v).font(Font::MONOSPACE)].spacing(8).boxed());
            }
        }
        scrollable(column(items).spacing(4)).width(Fill).height(Fill).boxed()
    }

    fn stats_panel(&self) -> impl Widget<Message> {
        match self.stats {
            Some((frame, dt, n)) => {
                let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                column![
                    text(format!("frame {frame}")),
                    text(format!("frame time {:.2} ms ({fps:.0} fps)", dt * 1000.0)),
                    text(format!("entities {n}")),
                ]
                .boxed()
            }
            None => text("no data").boxed(),
        }
    }

    fn scene_panel(&self) -> impl Widget<Message> {
        let connected = self.conn.is_connected();
        let act = |label: &'static str, m: Message| {
            let b = button(label);
            if connected { b.on_press(m) } else { b }
        };
        let mut items = vec![
            text(format!("scene: {}", self.scene.name.as_deref().unwrap_or("(none)"))).boxed(),
            text(format!("file: {}", self.scene.path.as_deref().unwrap_or("(none)"))).boxed(),
            text(format!("project: {}", self.scene.project_dir)).size(12).boxed(),
            text_input("scene file, relative to the project", &self.path_input)
                .on_input(Message::PathChanged)
                .on_submit(Message::Load)
                .boxed(),
            row![act("Save", Message::Save), act("Load", Message::Load)].spacing(8).boxed(),
        ];
        if let Some(n) = &self.notice {
            items.push(text(n.clone()).boxed());
        }
        column(items).spacing(6)
    }

    fn systems_panel(&self) -> impl Widget<Message> {
        let mut items = Vec::new();
        for (head, rows) in system_sections(&self.systems) {
            items.push(text(head).size(14).boxed());
            for r in rows {
                items.push(text(r).font(Font::MONOSPACE).size(13).boxed());
            }
        }
        scrollable(column(items).spacing(2)).width(Fill).height(Fill)
    }

    fn log_panel(&self) -> impl Widget<Message> {
        let lines = self.logs.iter().filter(|l| log_visible(l, self.warnings_only)).map(|l| text(log_label(l)).font(Font::MONOSPACE).size(13).boxed());
        let toggle = button(if self.warnings_only { "show all" } else { "warnings only" }).on_press(Message::WarningsOnly(!self.warnings_only));
        column![toggle, scrollable(column(lines)).width(Fill).height(Fill).anchor_bottom()].spacing(4)
    }
}

fn panel<'a>(title: &'static str, content: impl Widget<Message> + 'a) -> iced::widget::Container<'a, impl Widget<Message> + 'a, Theme> {
    container(column![text(title).size(15), content].spacing(6)).padding(8).style(container::bordered_box)
}
