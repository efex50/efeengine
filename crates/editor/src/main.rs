//! The editor window (docs/EDITOR.md E1/E2). A standalone iced application: it owns no game
//! state and talks to the game only through the editor-link worker thread.

use std::collections::BTreeMap;

use efeengine_editor::conn::ConnState;
use efeengine_editor::edit::{self, Entry, InputKind, Op, Schema, Target, UndoStack};
use efeengine_editor::model::*;
use efeengine_editor::worker::{self, Event, Handle};
use efeengine_editor_link::protocol::{ComponentInfo, EntityInfo, LogLine, Mode, Reply, Request, StageInfo};
use iced::time::{self, milliseconds};
use iced::widget::{Widget, button, checkbox, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Fill, Font, Subscription, Theme, keyboard};
use serde_json::{Value, json};

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
        }
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
            (_, Reply::Logs { lines }) => self.logs = lines,
            (_, Reply::Systems { stages }) => self.systems = stages,
            (_, Reply::Scene { name, path, mode, project_dir }) => {
                self.scene = SceneView { name, path, project_dir };
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
                            if !was && self.conn.is_connected() {
                                self.ticks = 0;
                                self.notice = None;
                                self.history.clear();
                                self.replaying.clear();
                                self.pending_edits = 0;
                                self.worker.send(Request::Schema);
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

    fn view(&self) -> impl Widget<Message> {
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
        let top = row![
            text(self.conn.label(self.port)),
            space().width(Fill),
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

        column![top, body, panel("Log", self.log_panel()).height(200)].spacing(8).padding(8)
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

    fn log_panel(&self) -> impl Widget<Message> {
        let lines = self.logs.iter().filter(|l| log_visible(l, self.warnings_only)).map(|l| text(log_label(l)).font(Font::MONOSPACE).size(13).boxed());
        let toggle = button(if self.warnings_only { "show all" } else { "warnings only" }).on_press(Message::WarningsOnly(!self.warnings_only));
        column![toggle, scrollable(column(lines)).width(Fill).height(Fill).anchor_bottom()].spacing(4)
    }
}

fn panel<'a>(title: &'static str, content: impl Widget<Message> + 'a) -> iced::widget::Container<'a, impl Widget<Message> + 'a, Theme> {
    container(column![text(title).size(15), content].spacing(6)).padding(8).style(container::bordered_box)
}
