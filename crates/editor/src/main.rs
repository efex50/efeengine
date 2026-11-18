//! The editor window (docs/EDITOR.md E1/E2). A standalone iced application: it owns no game
//! state and talks to the game only through the editor-link worker thread.

use std::collections::BTreeMap;

use efeengine_editor::build::{self, BuildState, LineKind};
use efeengine_editor::cli::{Cli, parse_cli};
use efeengine_editor::conn::ConnState;
use efeengine_editor::doctor;
use efeengine_editor::edit::{self, Entry, InputKind, Op, Schema, Target, UndoStack};
use efeengine_editor::config::{self, Config};
use efeengine_editor::projects::{self, OpenProject};
use efeengine_editor::route::{Nav, Route};
use efeengine_editor::{gizmo, place};
use std::path::{Path, PathBuf};
use efeengine_editor::model::*;
use efeengine_editor::restart::{self, Action, Restart};
use efeengine_editor::runner;
use efeengine_editor::worker::{self, Event, Handle};
use efeengine_editor_link::protocol::{ComponentInfo, EntityInfo, GizmoMode, LogLine, Mode, Reply, Request, StageInfo};
use iced::time::{self, milliseconds};
use iced::widget::{Widget, button, checkbox, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Fill, Font, Subscription, Theme, keyboard};
use serde_json::{Value, json};

fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let (config_dir, config, config_notice) = match config::config_dir_from_env() {
        Some(d) => {
            let (c, n) = config::load(&d);
            (Some(d), c, n)
        }
        None => (None, Config::default(), Some("no user config directory (set HOME or XDG_CONFIG_HOME): settings are not saved".to_string())),
    };
    let mut cli = match parse_cli(&args, std::env::var("EFE_EDITOR_PORT").ok().as_deref(), &cwd) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "efe-editor: {e}\nusage: efe-editor [--port N] [--package NAME] [--example DIR] [--project-dir DIR]\n  \
                 port: $EFE_EDITOR_PORT or 7878; without --package Build and Run are disabled"
            );
            std::process::exit(2);
        }
    };
    // The configured default port applies when neither --port nor $EFE_EDITOR_PORT is given.
    if !args.iter().any(|a| a == "--port" || a.starts_with("--port=")) && std::env::var("EFE_EDITOR_PORT").is_err() {
        cli.port = config.default_port;
    }
    iced::application(move || Editor::new(cli.clone(), config_dir.clone(), config.clone(), config_notice.clone()), Editor::update, Editor::view)
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
    KeepChanges(bool),
    PathChanged(String),
    Save,
    Load,
    WarningsOnly(bool),
    /// Component, field, input index, new text.
    EditInput(String, String, usize, String),
    EditSubmit(String, String),
    SetBool(String, String, bool),
    PickStart(String, String),
    PickClear(String, String),
    AddEntity,
    DeleteSelected,
    Undo,
    Redo,
    Build,
    Run,
    RebuildRestart,
    Stop,
    Doctor,
    Release(bool),
    /// Gizmo mode buttons (E7.2).
    Gizmo(GizmoMode),
    /// Axis index and step: one `set_field` on the selected entity's `Transform`.
    Nudge(usize, f64),
    /// Settings: place the game window next to the editor (E7.1).
    PlaceGame(bool),
    // Pages (E9).
    Navigate(Route),
    Back,
    Forward,
    OpenRecent(PathBuf),
    RemoveRecent(PathBuf),
    NewName(String),
    NewCreate,
    OpenListed(PathBuf),
    /// Show what is wrong with a listed project (by its file).
    ShowIssues(PathBuf),
    OpenRustup,
    SettingsPort(String),
    SettingsPlace(bool),
    SettingsRelease(bool),
    SettingsSave,
    DeleteName(String),
    DeleteConfirm,
}

/// One row of the recent list: the `.efproj` path and what is known about it.
#[derive(Debug, Clone)]
struct RecentRow {
    file: PathBuf,
    name: Option<String>,
    missing: bool,
}

/// Text fields and messages of the pages other than the editor.
#[derive(Debug, Default)]
struct Pages {
    recent: Vec<RecentRow>,
    home_notice: Option<String>,
    new_name: String,
    new_error: Option<String>,
    open_list: Vec<projects::Listed>,
    open_issues: Vec<String>,
    settings_port: String,
    settings_place: bool,
    settings_release: bool,
    settings_error: Option<String>,
    delete_dir: PathBuf,
    delete_name: String,
    delete_project_name: Option<String>,
    delete_size: (u64, bool),
    delete_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct SceneView {
    name: Option<String>,
    path: Option<String>,
}

struct Editor {
    nav: Nav,
    /// The project the editor works on (E9); `None` when started with only `--package`.
    project: Option<OpenProject>,
    config: Config,
    config_dir: Option<PathBuf>,
    pages: Pages,
    port: u16,
    cli: Cli,
    runner: runner::Handle,
    build: BuildState,
    restart: Restart,
    release: bool,
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
    keep_changes: bool,
    fidelity: Vec<String>,
    path_input: String,
    notice: Option<String>,
    schema: Schema,
    /// What each inspector input shows (typed text, else the current value); rebuilt after every update.
    shown: BTreeMap<(String, String), Vec<String>>,
    /// Text typed into inspector inputs and not yet sent, by (component, field).
    buffers: BTreeMap<(String, String), Vec<String>>,
    edit_error: Option<String>,
    /// The entity field waiting for a pick in the Hierarchy.
    picking: Option<(String, String)>,
    history: UndoStack,
    /// Undo and redo requests sent and not answered yet: (request, entry, was undo).
    replaying: Vec<(Request, Entry, bool)>,
    /// Edits sent and not answered yet; undo waits for them so the stack stays in order.
    pending_edits: usize,
    /// Gizmo mode the game draws and the Gizmo panel edits (E7.2).
    gizmo: GizmoMode,
    /// Settings (E7.1): move the game window next to the editor's after it starts.
    place_game: bool,
    /// Results of the background placement threads, shown as notices.
    placement: (std::sync::mpsc::Sender<String>, std::sync::mpsc::Receiver<String>),
}

impl Pages {
    /// Recent projects that still exist, as (name, file).
    fn recent_names(&self) -> Vec<(String, PathBuf)> {
        self.recent.iter().filter(|r| !r.missing).map(|r| (r.name.clone().unwrap_or_else(|| "(unreadable)".into()), r.file.clone())).collect()
    }
}

impl Editor {
    fn new(cli: Cli, config_dir: Option<PathBuf>, config: Config, config_notice: Option<String>) -> Self {
        let port = cli.port;
        // `--package` is a shortcut: the project of that package when there is one, else the
        // editor opens without a project, as before E9.
        let project = cli.package.as_deref().and_then(|pkg| {
            let file = efeengine_project::find(&cli.project_dir)?;
            projects::load_project(&file).ok().filter(|p| p.project.package == pkg)
        });
        let start = if cli.package.is_some() { Route::Editor } else { Route::Home };
        let pages = Pages::default();
        let mut me = Self {
            nav: Nav::new(start),
            project,
            release: config.cargo_release,
            place_game: config.place_game_window,
            config,
            config_dir,
            pages,
            port,
            cli,
            runner: runner::spawn(),
            build: BuildState::default(),
            restart: Restart::default(),
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
            keep_changes: false,
            fidelity: Vec::new(),
            path_input: String::new(),
            notice: None,
            schema: Schema::new(),
            shown: BTreeMap::new(),
            buffers: BTreeMap::new(),
            edit_error: None,
            picking: None,
            history: UndoStack::default(),
            replaying: Vec::new(),
            pending_edits: 0,
            gizmo: GizmoMode::Move,
            placement: std::sync::mpsc::channel(),
        };
        me.notice = config_notice;
        me.enter_page();
        me
    }

    /// The game package Build and Run use: the open project's, else `--package`.
    fn package(&self) -> Option<String> {
        self.project.as_ref().map(|p| p.project.package.clone()).or_else(|| self.cli.package.clone())
    }

    /// The cargo project folder: the open project's folder, else `--project-dir`.
    fn project_dir(&self) -> PathBuf {
        self.project.as_ref().map(|p| p.dir.clone()).unwrap_or_else(|| self.cli.project_dir.clone())
    }

    fn has_editor_target(&self) -> bool {
        self.project.is_some() || self.cli.package.is_some()
    }

    fn navigate(&mut self, route: Route) {
        self.nav.navigate(route, self.has_editor_target());
        self.enter_page();
    }

    /// Prepare the state of the page just reached.
    fn enter_page(&mut self) {
        match self.nav.current().clone() {
            Route::Home => {
                self.pages.recent = self
                    .config
                    .recent
                    .iter()
                    .map(|f| RecentRow { file: f.clone(), name: projects::project_name(f), missing: !f.is_file() })
                    .collect();
                self.pages.home_notice = None;
            }
            Route::NewProject => self.pages.new_error = None,
            Route::OpenProject => {
                self.pages.open_issues.clear();
                self.pages.open_list = self.config.projects_dir().map(|d| projects::list_projects(&d)).unwrap_or_default();
            }
            Route::Settings => {
                self.pages.settings_port = self.config.default_port.to_string();
                self.pages.settings_place = self.config.place_game_window;
                self.pages.settings_release = self.config.cargo_release;
                self.pages.settings_error = None;
            }
            Route::DeleteProject(dir) => {
                self.pages.delete_name.clear();
                self.pages.delete_error = None;
                self.pages.delete_project_name = efeengine_project::find(&dir).filter(|f| f.parent() == Some(dir.as_path())).and_then(|f| projects::project_name(&f));
                self.pages.delete_size = projects::dir_size(&dir, projects::SIZE_WALK_CAP);
                self.pages.delete_dir = dir;
            }
            Route::Editor => {}
        }
    }

    fn save_config(&mut self) -> Result<(), String> {
        match &self.config_dir {
            Some(d) => config::save(d, &self.config),
            None => Err("no user config directory: settings are not saved".into()),
        }
    }

    /// Make `p` the open project, remember it in the recent list and go to the editor.
    /// Refused while a game or build of another project is running.
    fn open_project(&mut self, p: OpenProject) -> Result<(), String> {
        if (self.build.running() || self.build.building()) && self.project.as_ref().map(|o| &o.dir) != Some(&p.dir) {
            return Err("stop the running game or build before opening another project".into());
        }
        self.config.add_recent(&p.file);
        if let Err(e) = self.save_config() {
            self.notice = Some(format!("recent list not saved: {e}"));
        }
        self.project = Some(p);
        self.navigate(Route::Editor);
        Ok(())
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.history.on_mode_change(mode);
            if mode == Mode::Play {
                self.replaying.clear();
            }
        }
    }

    /// Keep the undo history: a replayed undo or redo moves its entry across, any other
    /// successful edit made in edit mode is recorded with its inverse.
    fn record(&mut self, request: &Request, reply: &Reply) {
        let is_edit = matches!(request, Request::SetField { .. } | Request::Spawn { .. } | Request::Despawn { .. });
        if is_edit {
            self.pending_edits = self.pending_edits.saturating_sub(1);
        }
        let failed = matches!(reply, Reply::Error { .. });
        if let Some(i) = self.replaying.iter().position(|(r, _, _)| r == request) {
            let (_, mut entry, undo) = self.replaying.remove(i);
            if !failed {
                // A respawn may get a new editor id (the entity had none): the other direction follows it.
                if let Reply::Spawned { editor_id, .. } = reply {
                    let despawn = Op::Despawn { target: Target::EditorId(editor_id.clone()) };
                    if undo { entry.redo = despawn } else { entry.undo = despawn }
                }
                if undo { self.history.finish_undo(entry) } else { self.history.finish_redo(entry) }
                self.notice = Some(if undo { "undone" } else { "redone" }.into());
            }
        } else if let (true, Some(entry)) = (is_edit && !failed && self.mode == Mode::Edit, edit::inverse(request, reply)) {
            self.history.push(entry);
        }
    }

    /// Tell the game which entity to draw the gizmo over (it draws in edit mode only).
    fn sync_selection(&self) {
        match &self.selected {
            Some(s) if s.editor_id.is_some() => {
                self.worker.send(Request::Select { entity: None, editor_id: s.editor_id.clone(), gizmo: self.gizmo })
            }
            Some(s) => self.worker.send(Request::Select { entity: Some(s.entity), editor_id: None, gizmo: self.gizmo }),
            None => self.worker.send(Request::ClearSelection),
        }
    }

    /// The game's environment for the editor's own settings (the window title it is found by).
    fn game_env(&self, mut spec: build::CommandSpec) -> build::CommandSpec {
        if self.place_game {
            spec.env.push(("EFE_WINDOW_TITLE".into(), place::game_window_title(std::process::id())));
        }
        // The game side does not read this yet (E9.7).
        if let Some(p) = &self.project {
            spec.env.push(("EFE_START_SCENE".into(), p.project.start_scene.clone()));
        }
        spec
    }

    /// After the game starts: find its window and move it next to ours, off the UI thread.
    fn place_game_window(&self) {
        if !self.place_game {
            return;
        }
        let tx = self.placement.0.clone();
        std::thread::spawn(move || {
            let title = place::game_window_title(std::process::id());
            let msg = match place::place_with_retry(std::process::id(), &title, 40, std::time::Duration::from_millis(250)) {
                Ok(r) => format!("game window placed at {},{} ({}x{})", r.x, r.y, r.w, r.h),
                Err(e) => format!("game window not placed: {e}"),
            };
            let _ = tx.send(msg);
        });
    }

    fn selected_target(&self) -> Option<Target> {
        self.selected.as_ref().and_then(|s| Target::of(Some(s.entity), s.editor_id.as_deref()))
    }

    fn send_edit(&mut self, op: Op) {
        self.pending_edits += 1;
        self.worker.send(op.to_request());
    }

    fn send_set(&mut self, component: &str, field: &str, value: Value) {
        let Some(target) = self.selected_target() else { return };
        self.send_edit(Op::SetField { target, component: component.into(), field: field.into(), value });
        if let Some(sel) = &self.selected {
            self.worker.send(Self::components_request(sel));
        }
    }

    /// The text the inputs of a field show when nothing is typed: its current value.
    fn current_texts(&self, component: &str, field: &str) -> Vec<String> {
        let ty = self.field_spec(component, field).map(|f| f.ty.clone()).unwrap_or_default();
        self.components
            .iter()
            .find(|c| c.name == component)
            .and_then(|c| c.fields.get(field))
            .map(|v| edit::edit_texts(&ty, v))
            .unwrap_or_default()
    }

    fn field_spec(&self, component: &str, field: &str) -> Option<&edit::FieldSpec> {
        self.schema.get(component)?.fields.iter().find(|f| f.name == field)
    }

    /// Send the typed text of one field (Enter, or another field taking over the keyboard).
    fn commit(&mut self, key: (String, String)) {
        let Some(parts) = self.buffers.remove(&key) else { return };
        let (comp, field) = &key;
        if parts == self.current_texts(comp, field) {
            return;
        }
        let (Some(spec), Some(cs)) = (self.field_spec(comp, field).cloned(), self.schema.get(comp).cloned()) else { return };
        let parsed = match edit::input_kind(&cs, &spec) {
            InputKind::Number { ty } => edit::parse_scalar(&ty, parts.first().map(String::as_str).unwrap_or("")),
            InputKind::Vector { ty, count } => edit::parse_vector(&ty, count, &parts),
            InputKind::Entity => edit::parse_scalar("entity", parts.first().map(String::as_str).unwrap_or("")),
            _ => return,
        };
        match parsed {
            Ok(v) => {
                self.edit_error = None;
                self.send_set(comp, field, v);
            }
            Err(e) => {
                self.edit_error = Some(format!("{comp}.{field}: {e}"));
                self.buffers.insert(key, parts);
            }
        }
    }

    /// Typed-but-unsent inputs count as submitted when the user moves on (iced's text input
    /// has no blur event): called when another input is edited, the selection or mode changes.
    fn commit_others(&mut self, keep: Option<&(String, String)>) {
        let keys: Vec<_> = self.buffers.keys().filter(|k| Some(*k) != keep).cloned().collect();
        for k in keys {
            self.commit(k);
        }
    }

    fn history_step(&mut self, undo: bool) {
        self.commit_others(None);
        if let Some(why) = edit::history_blocked(self.mode, self.conn.is_connected()) {
            self.notice = Some(why.into());
            return;
        }
        if self.pending_edits > 0 || !self.replaying.is_empty() {
            self.notice = Some("waiting for the previous edit to finish".into());
            return;
        }
        let Some(entry) = (if undo { self.history.take_undo() } else { self.history.take_redo() }) else {
            self.notice = Some(if undo { "nothing to undo" } else { "nothing to redo" }.into());
            return;
        };
        let req = (if undo { &entry.undo } else { &entry.redo }).to_request();
        self.replaying.push((req.clone(), entry, undo));
        self.worker.send(req);
    }

    fn components_request(sel: &EntityInfo) -> Request {
        match &sel.editor_id {
            Some(id) => Request::Components { entity: None, editor_id: Some(id.clone()) },
            None => Request::Components { entity: Some(sel.entity), editor_id: None },
        }
    }

    fn apply(&mut self, request: Request, reply: Reply) {
        self.record(&request, &reply);
        if matches!(request, Request::RestartSnapshot { .. }) {
            let actions = self.restart.on_snapshot_reply(&reply);
            return self.do_restart_actions(actions);
        }
        match (request, reply) {
            (Request::SetField { .. }, Reply::Error { code, message }) => {
                self.edit_error = Some(format!("{code}: {message}"));
                self.notice = Some(format!("edit failed ({code})"));
            }
            (_, Reply::FieldSet { .. }) => self.edit_error = None,
            (_, Reply::Spawned { entity, editor_id }) => {
                self.notice = Some(format!("spawned entity with editor id {editor_id}"));
                self.selected = Some(EntityInfo { editor_id: Some(editor_id), entity, name: None });
                self.components.clear();
                self.buffers.clear();
                if let Some(sel) = &self.selected {
                    self.worker.send(Self::components_request(sel));
                }
                self.sync_selection();
                self.refresh_lists();
            }
            (request, Reply::Despawned { owned, editor_id, .. }) => {
                let gone = match &request {
                    Request::Despawn { entity, editor_id: id } => self
                        .selected
                        .as_ref()
                        .is_some_and(|s| Some(s.entity) == *entity || (s.editor_id.is_some() && s.editor_id == *id) || (s.editor_id.is_some() && s.editor_id == editor_id)),
                    _ => false,
                };
                if gone {
                    self.selected = None;
                    self.components.clear();
                    self.buffers.clear();
                    self.sync_selection();
                }
                self.notice = Some(if owned { "despawned".into() } else { "despawned (it was not owned by the scene, so it was never saved)".to_string() });
                self.refresh_lists();
            }
            (_, Reply::Schema { schema }) => self.schema = edit::parse_schema(&schema),
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
            (_, Reply::Selected { .. }) => {}
            (_, Reply::Logs { lines }) => self.logs = lines,
            (_, Reply::Systems { stages }) => self.systems = stages,
            (_, Reply::Scene { name, path, mode, .. }) => {
                self.scene = SceneView { name, path };
                self.set_mode(mode);
            }
            (_, Reply::Fidelity { gaps, not_covered, .. }) => self.fidelity = fidelity_lines(&gaps, &not_covered),
            (_, Reply::Mode { mode }) => {
                self.set_mode(mode);
                if mode == Mode::Play {
                    self.worker.send(Request::Fidelity);
                } else {
                    self.fidelity.clear();
                }
                self.notice = Some(format!("{} mode", mode_label(mode)));
                self.refresh_lists();
            }
            (_, Reply::Saved { path }) => self.notice = Some(format!("saved {path}")),
            (_, Reply::Loaded { path, entities }) => {
                self.notice = Some(format!("loaded {path} ({entities} entities)"));
                self.history.clear();
                self.replaying.clear();
                self.buffers.clear();
                self.refresh_lists();
            }
            (_, Reply::Error { code, message }) => self.notice = Some(format!("error ({code}): {message}")),
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

    fn refresh_shown(&mut self) {
        let mut shown = BTreeMap::new();
        for c in &self.components {
            for (fname, v) in &c.fields {
                let key = (c.name.clone(), fname.clone());
                let ty = self.field_spec(&c.name, fname).map(|f| f.ty.as_str()).unwrap_or("");
                let texts = self.buffers.get(&key).cloned().unwrap_or_else(|| edit::edit_texts(ty, v));
                shown.insert(key, texts);
            }
        }
        self.shown = shown;
    }

    fn update(&mut self, message: Message) {
        self.handle(message);
        self.refresh_shown();
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Tick => {
                for ev in self.worker.poll() {
                    match ev {
                        Event::State(s) => {
                            let was = self.conn.is_connected();
                            self.conn = s;
                            if was && !self.conn.is_connected() {
                                let actions = self.restart.on_disconnect();
                                self.do_restart_actions(actions);
                            }
                            if !was && self.conn.is_connected() {
                                self.ticks = 0;
                                self.notice = None;
                                self.history.clear();
                                self.replaying.clear();
                                self.pending_edits = 0;
                                self.worker.send(Request::Schema);
                                self.sync_selection();
                                self.refresh_lists();
                                self.worker.send(Request::Logs { count: 300 });
                                self.worker.send(Request::Stats);
                            }
                        }
                        Event::Reply { request, reply } => self.apply(request, reply),
                    }
                }
                for ev in self.runner.poll() {
                    if matches!(ev, build::RunnerEvent::GameStarted { .. }) {
                        self.place_game_window();
                    }
                    self.restart.on_runner_event(&ev);
                    self.build.apply(ev);
                }
                while let Ok(m) = self.placement.1.try_recv() {
                    self.notice = Some(m);
                }
                self.poll();
            }
            Message::Navigate(r) => self.navigate(r),
            Message::Back => {
                self.nav.back(self.has_editor_target());
                self.enter_page();
            }
            Message::Forward => {
                self.nav.forward(self.has_editor_target());
                self.enter_page();
            }
            Message::OpenRecent(file) => match projects::load_project(&file) {
                Ok(p) => {
                    if let Err(e) = self.open_project(p) {
                        self.pages.home_notice = Some(e);
                    }
                }
                Err(issues) => self.pages.home_notice = Some(issues.join("; ")),
            },
            Message::RemoveRecent(file) => {
                self.config.remove_recent(&file);
                if let Err(e) = self.save_config() {
                    self.pages.home_notice = Some(format!("recent list not saved: {e}"));
                }
                self.pages.recent.retain(|r| r.file != file);
            }
            Message::NewName(n) => self.pages.new_name = n,
            Message::NewCreate => {
                let result = match self.config.projects_dir() {
                    None => Err("no home folder was found to keep projects in".to_string()),
                    Some(root) => std::fs::create_dir_all(&root)
                        .map_err(|e| format!("could not create the projects folder: {e}"))
                        .and_then(|()| {
                            efeengine_project::create(&root, &self.pages.new_name).map_err(|e| match e {
                                efeengine_project::ProjectError::NotEmpty(_) => "a project with this name already exists".to_string(),
                                efeengine_project::ProjectError::Io { source, .. } => source.to_string(),
                                other => other.to_string(),
                            })
                        })
                        .and_then(|file| projects::load_project(&file).map_err(|i| i.join("; "))),
                };
                match result.and_then(|p| self.open_project(p)) {
                    Ok(()) => self.pages.new_error = None,
                    Err(e) => self.pages.new_error = Some(e),
                }
            }
            Message::OpenListed(file) => match projects::load_project(&file) {
                Ok(p) => {
                    if let Err(e) = self.open_project(p) {
                        self.pages.open_issues = vec![e];
                    }
                }
                Err(issues) => self.pages.open_issues = issues,
            },
            Message::ShowIssues(file) => {
                self.pages.open_issues = self.pages.open_list.iter().find(|l| l.file == file).map(|l| l.issues.clone()).unwrap_or_default();
            }
            Message::OpenRustup => {
                if let Err(e) = doctor::open_url(doctor::RUSTUP_URL) {
                    self.notice = Some(e);
                }
            }
            Message::SettingsPort(t) => self.pages.settings_port = t,
            Message::SettingsPlace(b) => self.pages.settings_place = b,
            Message::SettingsRelease(b) => self.pages.settings_release = b,
            Message::SettingsSave => match config::validate_port(&self.pages.settings_port) {
                Err(e) => self.pages.settings_error = Some(e),
                Ok(port) => {
                    self.config.default_port = port;
                    self.config.place_game_window = self.pages.settings_place;
                    self.config.cargo_release = self.pages.settings_release;
                    self.place_game = self.config.place_game_window;
                    self.release = self.config.cargo_release;
                    match self.save_config() {
                        Ok(()) => {
                            self.pages.settings_error = None;
                            self.notice = Some("settings saved (the port applies at the next start)".into());
                        }
                        Err(e) => {
                            self.pages.settings_error = Some(e.clone());
                            self.notice = Some(format!("settings not saved: {e}"));
                        }
                    }
                }
            },
            Message::DeleteName(t) => self.pages.delete_name = t,
            Message::DeleteConfirm => {
                let dir = self.pages.delete_dir.clone();
                let running = self.build.running() || self.build.building();
                let name = self.pages.delete_project_name.clone();
                match projects::delete_project(&dir, &self.pages.delete_name, running) {
                    Err(e) => self.pages.delete_error = Some(e),
                    Ok(()) => {
                        let file = self.config.recent.iter().find(|f| f.parent() == Some(dir.as_path())).cloned();
                        if let Some(f) = file {
                            self.config.remove_recent(&f);
                        }
                        if self.project.as_ref().is_some_and(|p| p.dir == dir) {
                            self.project = None;
                        }
                        if let Err(e) = self.save_config() {
                            self.notice = Some(format!("recent list not saved: {e}"));
                        }
                        self.navigate(Route::Home);
                        self.pages.home_notice = Some(format!("deleted project {}", name.unwrap_or_default()));
                    }
                }
            }
            Message::Build => {
                if let Some(pkg) = self.package().filter(|_| self.build.can_build(true)) {
                    self.runner.build(build::build_command(&self.project_dir(), &pkg, self.release));
                }
            }
            Message::Run => {
                if let Some(pkg) = self.package().filter(|_| self.build.can_run(true)) {
                    let dir = &self.project_dir();
                    let target = build::target_dir(dir, std::env::var("CARGO_TARGET_DIR").ok().as_deref());
                    let game = self.game_env(build::run_command(dir, &target, &pkg, self.release, self.port));
                    self.runner.run(build::build_command(dir, &pkg, self.release), game);
                }
            }
            Message::RebuildRestart => {
                let actions = self.restart.begin(self.package().is_some(), self.build.building(), self.conn.is_connected());
                self.do_restart_actions(actions);
            }
            Message::Stop => self.runner.stop_game(),
            Message::Doctor => self.runner.doctor(self.project_dir()),
            Message::Release(on) => self.release = on,
            Message::Gizmo(m) => {
                self.gizmo = m;
                self.sync_selection();
            }
            Message::PlaceGame(on) => self.place_game = on,
            Message::Nudge(axis, delta) => {
                self.commit_others(None);
                let f = gizmo::field(self.gizmo);
                let current = self.components.iter().find(|c| c.name == "Transform").and_then(|c| c.fields.get(f)).cloned();
                match current.map(|v| gizmo::nudge(&v, axis, delta)) {
                    Some(Ok(v)) => {
                        self.edit_error = None;
                        self.send_set("Transform", f, v);
                    }
                    Some(Err(e)) => self.edit_error = Some(format!("Transform.{f}: {e}")),
                    None => self.notice = Some("the selected entity has no Transform".into()),
                }
            }
            Message::Select(e) if self.picking.is_some() => {
                let (comp, field) = self.picking.take().unwrap_or_default();
                let value = match &e.editor_id {
                    Some(id) => json!(id),
                    None => json!(e.entity),
                };
                self.send_set(&comp, &field, value);
            }
            Message::Select(e) => {
                self.commit_others(None);
                self.buffers.clear();
                self.edit_error = None;
                self.worker.send(Self::components_request(&e));
                self.selected = Some(e);
                self.components.clear();
                self.inspector_error = None;
                self.sync_selection();
            }
            Message::EditInput(c, f, i, t) => {
                let key = (c, f);
                self.commit_others(Some(&key));
                let mut parts = self.buffers.get(&key).cloned().unwrap_or_else(|| self.current_texts(&key.0, &key.1));
                if parts.len() <= i {
                    parts.resize(i + 1, String::new());
                }
                parts[i] = t;
                self.buffers.insert(key, parts);
            }
            Message::EditSubmit(c, f) => self.commit((c, f)),
            Message::SetBool(c, f, b) => {
                self.commit_others(None);
                self.edit_error = None;
                self.send_set(&c, &f, json!(b));
            }
            Message::PickStart(c, f) => {
                self.picking = if self.picking.as_ref() == Some(&(c.clone(), f.clone())) { None } else { Some((c, f)) };
            }
            Message::PickClear(c, f) => self.send_set(&c, &f, json!(0)),
            Message::AddEntity => {
                self.commit_others(None);
                self.send_edit(Op::Spawn { components: Default::default(), editor_id: None });
            }
            Message::DeleteSelected => {
                if let Some(target) = self.selected_target() {
                    self.send_edit(Op::Despawn { target });
                }
            }
            Message::Undo => self.history_step(true),
            Message::Redo => self.history_step(false),
            Message::SetMode(m) => {
                self.commit_others(None);
                self.worker.send(Request::SetMode { mode: m, keep_changes: self.keep_changes });
            }
            Message::KeepChanges(on) => self.keep_changes = on,
            Message::PathChanged(p) => self.path_input = p,
            Message::Save => self.file_action(true),
            Message::Load => self.file_action(false),
            Message::WarningsOnly(on) => self.warnings_only = on,
        }
    }

    fn do_restart_actions(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::RequestSnapshot => self.worker.send(Request::RestartSnapshot { path: restart::SNAPSHOT_PATH.into() }),
                Action::Notice(n) => self.notice = Some(n),
                Action::Restart { restore } => {
                    let Some(pkg) = self.package() else { continue };
                    let dir = self.project_dir();
                    let target = build::target_dir(&dir, std::env::var("CARGO_TARGET_DIR").ok().as_deref());
                    let mut game = build::run_command(&dir, &target, &pkg, self.release, self.port);
                    if restore {
                        game = build::with_restore(game, &restart::snapshot_file(&dir));
                    }
                    let game = self.game_env(game);
                    self.runner.restart(build::build_command(&dir, &pkg, self.release), game);
                }
            }
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
        Subscription::batch([
            time::every(milliseconds(100)).map(|_| Message::Tick),
            keyboard::listen().filter_map(|event| {
                let keyboard::Event::KeyPressed { key, modifiers, .. } = event else { return None };
                if !modifiers.command() {
                    return None;
                }
                match key.as_ref() {
                    keyboard::Key::Character("z") if modifiers.shift() => Some(Message::Redo),
                    keyboard::Key::Character("z") => Some(Message::Undo),
                    keyboard::Key::Character("y") => Some(Message::Redo),
                    _ => None,
                }
            }),
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let page: Element<'_, Message> = match self.nav.current() {
            Route::Home => self.home_page(),
            Route::NewProject => self.new_page(),
            Route::OpenProject => self.open_page(),
            Route::DeleteProject(_) => self.delete_page(),
            Route::Settings => self.settings_page(),
            Route::Editor => self.editor_view().boxed(),
        };
        let header = row![
            button("Home").on_press(Message::Navigate(Route::Home)),
            button("<").on_press_maybe(self.nav.can_back().then_some(Message::Back)),
            button(">").on_press_maybe(self.nav.can_forward().then_some(Message::Forward)),
            text(self.nav.current().path()).font(Font::MONOSPACE),
            text(self.project.as_ref().map(|p| format!("project: {}", p.project.name)).unwrap_or_default()).size(13),
        ]
        .spacing(10)
        .align_y(iced::Center);
        column![container(header).padding([6, 8]).width(Fill).style(container::bordered_box), page].spacing(0).boxed()
    }

    fn page_box<'a>(&self, title: &'static str, items: Vec<Element<'a, Message>>) -> Element<'a, Message> {
        let mut all = vec![text(title).size(24).boxed()];
        all.extend(items);
        scrollable(container(column(all).spacing(10).width(720)).padding(16)).width(Fill).height(Fill).boxed()
    }

    fn home_page(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = vec![
            row![
                button("New project").on_press(Message::Navigate(Route::NewProject)),
                button("Open project").on_press(Message::Navigate(Route::OpenProject)),
                button("Settings").on_press(Message::Navigate(Route::Settings)),
            ]
            .spacing(8)
            .boxed(),
            text("Recent projects").size(16).boxed(),
        ];
        if let Some(n) = &self.pages.home_notice {
            items.push(text(n.clone()).size(13).boxed());
        }
        if let Some(n) = &self.notice {
            items.push(text(n.clone()).size(13).boxed());
        }
        if self.pages.recent.is_empty() {
            items.push(text("no recent projects").size(13).boxed());
        }
        for r in &self.pages.recent {
            let name = r.name.clone().unwrap_or_else(|| "(unreadable)".into());
            let dir = r.file.parent().map(Path::to_path_buf).unwrap_or_default();
            items.push(
                row![
                    text(if r.missing { format!("{name}  (missing)") } else { name }).width(Fill),
                    button("Open").on_press_maybe((!r.missing).then(|| Message::OpenRecent(r.file.clone()))),
                    button("Remove").on_press(Message::RemoveRecent(r.file.clone())),
                    button("Delete").on_press_maybe((!r.missing).then_some(Message::Navigate(Route::DeleteProject(dir)))),
                ]
                .spacing(8)
                .align_y(iced::Center)
                .boxed(),
            );
        }
        self.page_box("Home", items)
    }

    fn new_page(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = vec![
            text("Name (a-z, 0-9, _; starts with a letter)").size(13).boxed(),
            text_input("my_game", &self.pages.new_name).on_input(Message::NewName).on_submit(Message::NewCreate).boxed(),
            text(doctor::FIRST_BUILD_NOTE).size(12).boxed(),
        ];
        if let Some(e) = &self.pages.new_error {
            items.push(text(format!("cannot create: {e}")).color(iced::Color::from_rgb(0.95, 0.35, 0.35)).boxed());
        }
        items.push(row![button("Create").on_press(Message::NewCreate), button("Cancel").on_press(Message::Back)].spacing(8).boxed());
        self.page_box("New project", items)
    }

    fn open_page(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = vec![text("Your projects").size(16).boxed()];
        if self.pages.open_list.is_empty() {
            items.push(text("no projects yet: create one with New project").size(13).boxed());
        }
        for l in &self.pages.open_list {
            let ok = l.issues.is_empty();
            items.push(
                row![
                    text(if ok { l.name.clone() } else { format!("{}  (needs attention)", l.name) }).width(Fill),
                    button("Open").on_press_maybe(ok.then(|| Message::OpenListed(l.file.clone()))),
                    button("Details").on_press_maybe((!ok).then(|| Message::ShowIssues(l.file.clone()))),
                ]
                .spacing(8)
                .align_y(iced::Center)
                .boxed(),
            );
        }
        if !self.pages.open_issues.is_empty() {
            items.push(text("the project was not opened:").color(iced::Color::from_rgb(0.95, 0.35, 0.35)).boxed());
            for i in &self.pages.open_issues {
                items.push(text(format!("  {i}")).size(13).font(Font::MONOSPACE).boxed());
            }
        }
        items.push(text("Recent").size(16).boxed());
        if self.pages.recent_names().is_empty() {
            items.push(text("no recent projects").size(13).boxed());
        }
        for (name, file) in self.pages.recent_names() {
            items.push(button(text(name).size(13)).style(button::secondary).on_press(Message::OpenListed(file)).boxed());
        }
        items.push(button("Back").on_press(Message::Back).boxed());
        self.page_box("Open project", items)
    }

    fn settings_page(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = vec![
            text("Default port (1 to 65535; applies at the next start)").size(13).boxed(),
            text_input("7878", &self.pages.settings_port).on_input(Message::SettingsPort).boxed(),
            checkbox(self.pages.settings_place).label("Place game window next to editor").on_toggle(Message::SettingsPlace).boxed(),
            checkbox(self.pages.settings_release).label("Build in release mode by default").on_toggle(Message::SettingsRelease).boxed(),
        ];
        if let Some(e) = &self.pages.settings_error {
            items.push(text(e.clone()).color(iced::Color::from_rgb(0.95, 0.35, 0.35)).boxed());
        } else if let Some(n) = &self.notice {
            items.push(text(n.clone()).size(13).boxed());
        }
        items.push(row![button("Save").on_press(Message::SettingsSave), button("Back").on_press(Message::Back)].spacing(8).boxed());
        self.page_box("Settings", items)
    }

    fn delete_page(&self) -> Element<'_, Message> {
        let p = &self.pages;
        let size = format!("{}{}", projects::human_size(p.delete_size.0), if p.delete_size.1 { " or more (not everything was counted)" } else { "" });
        let matches = p.delete_project_name.as_deref() == Some(p.delete_name.as_str());
        let mut items: Vec<Element<'_, Message>> = vec![
            text(format!("Project: {}", p.delete_project_name.as_deref().unwrap_or("(unreadable)"))).boxed(),
            text(format!("Size: {size}")).boxed(),
            text(match &p.delete_project_name {
                Some(n) => format!("This deletes the whole folder. Type the project name `{n}` to confirm."),
                None => "This folder has no valid .efproj file directly in it: it cannot be deleted here.".into(),
            })
            .boxed(),
            text_input("project name", &p.delete_name).on_input(Message::DeleteName).boxed(),
        ];
        if self.build.running() || self.build.building() {
            items.push(text("a game or build is running in this editor session: stop it first").size(13).boxed());
        }
        if let Some(e) = &p.delete_error {
            items.push(text(e.clone()).color(iced::Color::from_rgb(0.95, 0.35, 0.35)).boxed());
        }
        items.push(
            row![
                button("Delete").style(button::danger).on_press_maybe(matches.then_some(Message::DeleteConfirm)),
                button("Cancel").on_press(Message::Back),
            ]
            .spacing(8)
            .boxed(),
        );
        self.page_box("Delete project", items)
    }

    fn editor_view(&self) -> impl Widget<Message> {
        let connected = self.conn.is_connected();
        let mode_button = |label: &'static str, m: Mode| {
            let b = button(label);
            if connected && self.mode != m { b.on_press(Message::SetMode(m)) } else { b }
        };
        let blocked = edit::history_blocked(self.mode, connected);
        let hist_button = |label: String, m: Message, n: usize| {
            let b = button(text(label));
            if blocked.is_none() && n > 0 { b.on_press(m) } else { b }
        };
        let has_pkg = self.package().is_some();
        let gate = |label: &'static str, m: Message, allowed: bool| {
            let b = button(label);
            if allowed { b.on_press(m) } else { b }
        };
        let top = row![
            text(self.conn.label(self.port)),
            space().width(Fill),
            text(self.build.header()).size(13),
            checkbox(self.release).label("release").on_toggle(Message::Release),
            gate("Build", Message::Build, self.build.can_build(has_pkg)),
            gate("Run", Message::Run, self.build.can_run(has_pkg)),
            gate("Rebuild and restart", Message::RebuildRestart, self.restart.can_begin(has_pkg, self.build.building(), self.conn.is_connected())),
            gate("Stop", Message::Stop, self.build.can_stop()),
            button("Doctor").on_press(Message::Doctor),
            text(if has_pkg { "" } else { "start with --package <name> to build and run" }).size(12),
            text(blocked.unwrap_or("")).size(12),
            hist_button(format!("Undo ({})", self.history.undo_len()), Message::Undo, self.history.undo_len()),
            hist_button(format!("Redo ({})", self.history.redo_len()), Message::Redo, self.history.redo_len()),
            text(format!("mode: {}", mode_label(self.mode))),
            checkbox(self.keep_changes).label("keep changes on stop").on_toggle(Message::KeepChanges),
            mode_button("Edit", Mode::Edit),
            mode_button("Play", Mode::Play),
        ]
        .spacing(10)
        .align_y(iced::Center);

        let body = row![
            panel("Hierarchy", self.hierarchy()).width(Fill),
            panel("Inspector", self.inspector()).width(Fill),
            column![
                panel("Gizmo", self.gizmo_panel()),
                panel("Settings", self.settings_panel()),
                panel("Stats", self.stats_panel()),
                panel("Scene", self.scene_panel()),
                panel("Play", self.play_panel()),
                panel("Systems", self.systems_panel()).height(Fill),
            ]
            .spacing(8)
            .width(Fill),
        ]
        .spacing(8)
        .height(Fill);

        let bottom = row![panel("Log", self.log_panel()).width(Fill), panel("Errors", self.errors_panel()).width(Fill)].spacing(8).height(220);
        let banner = self.build.toolchain_missing.then(|| {
            row![text(self.build.notice.clone().unwrap_or_default()), button("Open rustup.rs").on_press(Message::OpenRustup)].spacing(10).align_y(iced::Center)
        });
        let mut all = vec![top.boxed()];
        all.extend(banner.map(|b| b.boxed()));
        all.push(body.boxed());
        all.push(bottom.boxed());
        column(all).spacing(8).padding(8)
    }

    fn hierarchy(&self) -> impl Widget<Message> {
        let add = if self.conn.is_connected() { button("Add entity").on_press(Message::AddEntity) } else { button("Add entity") };
        let mut items = vec![add.boxed()];
        if self.picking.is_some() {
            items.push(text("pick an entity for the field").size(12).boxed());
        }
        if self.entities.is_empty() {
            items.push(text("no entities (or not connected)").boxed());
            return column(items).spacing(4).boxed();
        }
        let rows = self.entities.iter().map(|e| {
            let selected = self.selected.as_ref().is_some_and(|s| s.entity == e.entity);
            button(text(entity_label(e)))
                .width(Fill)
                .style(if selected { button::primary } else { button::text })
                .on_press(Message::Select(e.clone()))
                .boxed()
        });
        items.push(scrollable(column(rows)).width(Fill).height(Fill).boxed());
        column(items).spacing(4).height(Fill).boxed()
    }

    fn inspector(&self) -> impl Widget<Message> {
        let Some(sel) = &self.selected else {
            return text("select an entity").boxed();
        };
        let mut items = vec![text(entity_label(sel)).size(18).boxed()];
        if let Some(err) = &self.inspector_error {
            items.push(text(format!("cannot read components: {err}")).boxed());
        }
        items.push(button("Delete entity").on_press_maybe(self.conn.is_connected().then_some(Message::DeleteSelected)).boxed());
        if let Some(err) = &self.edit_error {
            items.push(text(format!("edit failed: {err}")).size(13).boxed());
        }
        for c in &self.components {
            items.push(text(c.name.clone()).size(16).boxed());
            let comp = self.schema.get(&c.name);
            for (fname, value) in &c.fields {
                let kind = match comp.and_then(|cs| cs.fields.iter().find(|f| &f.name == fname).map(|f| (cs, f))) {
                    Some((cs, f)) => edit::input_kind(cs, f),
                    None => InputKind::ReadOnly("not in the schema".into()),
                };
                items.push(self.field_row(&c.name, fname, value, kind));
            }
        }
        scrollable(column(items).spacing(4)).width(Fill).height(Fill).boxed()
    }

    fn field_row<'a>(&'a self, comp: &str, field: &str, value: &Value, kind: InputKind) -> Element<'a, Message> {
        let label = text(field.to_string()).width(120).font(Font::MONOSPACE);
        let key = (comp.to_string(), field.to_string());
        let texts = self.shown.get(&key);
        let input = |i: usize, width: iced::Length| {
            let (c, f) = key.clone();
            let (c2, f2) = key.clone();
            text_input("", texts.and_then(|t| t.get(i)).map(String::as_str).unwrap_or(""))
                .font(Font::MONOSPACE)
                .width(width)
                .on_input(move |t| Message::EditInput(c.clone(), f.clone(), i, t))
                .on_submit(Message::EditSubmit(c2, f2))
        };
        match kind {
            InputKind::Number { .. } => row![label, input(0, Fill)].spacing(8).boxed(),
            InputKind::Vector { count, .. } => {
                let inputs = (0..count as usize).map(|i| input(i, Fill).boxed());
                row![label, row(inputs).spacing(4)].spacing(8).boxed()
            }
            InputKind::Bool => {
                let (c, f) = key.clone();
                let on = value.as_bool().unwrap_or(false);
                row![label, checkbox(on).on_toggle(move |b| Message::SetBool(c.clone(), f.clone(), b))].spacing(8).boxed()
            }
            InputKind::Entity => {
                let bits = value.as_u64().unwrap_or(0);
                let name = if bits == 0 {
                    "none".to_string()
                } else {
                    self.entities.iter().find(|e| e.entity == bits).map(entity_label).unwrap_or_else(|| format!("#{bits} (not in the list)"))
                };
                let picking = self.picking.as_ref() == Some(&key);
                let (c, f) = key.clone();
                row![
                    label,
                    text(name).width(Fill),
                    button(if picking { "cancel" } else { "pick" }).on_press(Message::PickStart(c.clone(), f.clone())),
                    button("clear").on_press(Message::PickClear(c, f)),
                ]
                .spacing(8)
                .boxed()
            }
            InputKind::ReadOnly(note) => row![label, text(format_value(value)).font(Font::MONOSPACE), text(format!("({note})")).size(12)].spacing(8).boxed(),
        }
    }

    fn settings_panel(&self) -> impl Widget<Message> {
        column![
            checkbox(self.place_game).label("Place game window next to editor").on_toggle(Message::PlaceGame),
            text(if self.place_game { "applies the next time the game starts; Wayland cannot place windows" } else { "" }).size(12),
        ]
        .spacing(4)
    }

    /// Gizmo mode, the numeric fields of the mode's `Transform` field, and nudge buttons. The
    /// handles are drawn by the game in edit mode; there is no mouse dragging in its window yet.
    fn gizmo_panel(&self) -> impl Widget<Message> {
        let modes = row(gizmo::MODES.iter().map(|&m| {
            button(gizmo::label(m)).style(if self.gizmo == m { button::primary } else { button::secondary }).on_press(Message::Gizmo(m)).boxed()
        }))
        .spacing(6);
        let mut items = vec![modes.boxed()];
        let f = gizmo::field(self.gizmo);
        let value = self.components.iter().find(|c| c.name == "Transform").and_then(|c| c.fields.get(f)).cloned();
        match (&self.selected, value) {
            (None, _) => items.push(text("select an entity").size(13).boxed()),
            (Some(_), None) => items.push(text("the selected entity has no Transform").size(13).boxed()),
            (Some(_), Some(v)) => {
                if let Some((cs, fs)) = self.schema.get("Transform").and_then(|cs| cs.fields.iter().find(|x| x.name == f).map(|x| (cs, x))) {
                    items.push(self.field_row("Transform", f, &v, edit::input_kind(cs, fs)));
                }
                for (i, axis) in gizmo::axes(&v).into_iter().enumerate() {
                    let mut r = vec![text(if axis.is_empty() { f.to_string() } else { axis.to_string() }).width(70).font(Font::MONOSPACE).boxed()];
                    for step in gizmo::STEPS.iter().rev().map(|s| -s).chain(gizmo::STEPS) {
                        r.push(button(text(format!("{step:+}")).size(13)).on_press(Message::Nudge(i, step)).boxed());
                    }
                    items.push(row(r).spacing(4).boxed());
                }
            }
        }
        items.push(
            text("handles are drawn in the game window in edit mode; dragging them with the mouse is not available yet (input ownership is deferred), so use these fields and buttons. Each press is one undo step.")
                .size(12)
                .boxed(),
        );
        column(items).spacing(6)
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

    fn play_panel(&self) -> impl Widget<Message> {
        if self.fidelity.is_empty() {
            return text("press Play to see what Stop will not restore").boxed();
        }
        let lines = self.fidelity.iter().map(|l| text(l.clone()).font(Font::MONOSPACE).size(13).boxed());
        scrollable(column(lines)).width(Fill).height(120).boxed()
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

    fn errors_panel(&self) -> impl Widget<Message> {
        let header = match (self.package(), &self.cli.example) {
            (Some(p), Some(e)) if self.project.is_none() => format!("{p} (examples/{e})"),
            (Some(p), _) => p,
            _ => "no game: pass --package <name> (and --example <dir>)".to_string(),
        };
        let lines = self.build.output.iter().map(|l| {
            let color = match l.kind {
                LineKind::Error => iced::Color::from_rgb(0.95, 0.35, 0.35),
                LineKind::Warning => iced::Color::from_rgb(0.95, 0.8, 0.3),
                LineKind::Other => iced::Color::from_rgb(0.6, 0.6, 0.6),
            };
            text(l.text.clone()).font(Font::MONOSPACE).size(13).color(color).boxed()
        });
        column![text(header).size(12), scrollable(column(lines)).width(Fill).height(Fill).anchor_bottom()].spacing(4)
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
