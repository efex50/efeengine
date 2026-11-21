//! The Editor tab body: toolbar, scene row, hierarchy (left) and inspector (right).
//!
//! The same body is shown in the Debug menu's tab (keys under `debug_menu/editor`) and, in layout
//! B (separate editor window), by the `editor_panel` view (keys under `editor_panel/editor`); while
//! detached the tab only says so and offers Attach (`debug_menu/editor/detached/row/attach`).
//!
//! Keys under `debug_menu/editor`: `toolbar/{edit,play,pause,step,stop,undo,redo}`,
//! `viewport/{tool_move,tool_rotate,tool_scale,grid,grid_step,detach}`,
//! `options/{status,keep}`, `scene/{label,save,save_as,load,new,path,list/f<N>}`, `message`,
//! `left/{hierarchy,prefabs,systems}` (the tabs of the left column),
//! `hierarchy/filter`, `hierarchy/actions/{new,duplicate,delete,unparent}`, `hierarchy/list/e<bits>/row`
//! (the clickable row), `inspector/entity/name` (the name input), `inspector/c<N>` (one per
//! component: `bar/head`, `bar/remove`, and `f<M>` rows with `name` and `value`; `value` is a text
//! input, a checkbox, a read-only text, or a row of text inputs `e<K>` for a list),
//! `prefabs/{name,save,list/p<N>}` (the Prefabs panel: name input, Save as prefab, one place button per prefab),
//! `systems/list/s<N>/{check,info}` (the Systems panel), `inspector/prefab/{info,apply,unpack}` and
//! `inspector/c<N>/f<M>/revert` (an overridden field),
//! `inspector/parent/row/{label,toggle}` and `inspector/parent/picker/{none,p<N>}` (the N-th candidate), `inspector/add/row/toggle` (under the name, so it stays in view) and `inspector/add/picker/a<N>` (the N-th of `EditorPanel::addable`).

use std::collections::BTreeSet;

use efeengine_core::ecs::{SceneValue, entity_to_bits};
use efeengine_core::engine::inspect::UNMANAGED_COMPONENTS;
use efeengine_core::prelude::*;

use super::{EditorMode, EditorMsg, EditorPanel, LeftTab, EditorRequest, EditorSelection, EditorState, EditorUi, EditorView, FieldKey, GizmoTool, TAB, ext};
use crate::debug_menu::DebugMsg;
use crate::style::Edges;
use crate::view::*;
use crate::{Align, Size, TextAlign, column, row};

/// A scene value as short text: numbers trimmed, lists in brackets.
pub fn format_value(v: &SceneValue) -> String {
    match v {
        SceneValue::Bool(b) => b.to_string(),
        SceneValue::Int(i) => i.to_string(),
        SceneValue::UInt(u) => u.to_string(),
        SceneValue::Float(f) => {
            let s = format!("{f:.3}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            if s.is_empty() || s == "-" || s == "-0" { "0".into() } else { s.to_string() }
        }
        SceneValue::List(l) => format!("[{}]", l.iter().map(format_value).collect::<Vec<_>>().join(", ")),
    }
}

fn mode_button(label: &str, key: &str, req: EditorRequest, on: bool) -> El<DebugMsg> {
    let b = button(label).key(key).class("tab").on_click(ext(EditorMsg::Request(req)));
    if on { b.class("tab-active") } else { b }
}

/// A small button of the tab (compact padding).
fn small(label: &str, key: &str, msg: EditorMsg, enabled: bool) -> El<DebugMsg> {
    button(label).key(key).class("tab").padding(Edges::symmetric(10.0, 5.0)).enabled(enabled).on_click(ext(msg))
}

fn toolbar(state: &EditorState, ui: &EditorUi) -> El<DebugMsg> {
    let m = state.mode;
    row![
        mode_button("Edit", "edit", EditorRequest::Edit, m == EditorMode::Edit),
        mode_button("Play", "play", EditorRequest::Play, m == EditorMode::Play),
        mode_button("Pause", "pause", EditorRequest::Pause, m == EditorMode::Paused),
        mode_button("Step", "step", EditorRequest::Step, false),
        mode_button("Stop", "stop", EditorRequest::Stop, false),
        container().class("separator-v").key("sep_undo"),
        small("Undo", "undo", EditorMsg::Undo, ui.undo_len > 0),
        small("Redo", "redo", EditorMsg::Redo, ui.redo_len > 0),
    ]
    .key("toolbar")
    .align(Align::Center)
    .width(Size::Fill)
}

/// The viewport row: gizmo tool, grid, grid step, Detach / Attach.
fn viewport_bar(view: &EditorView, ui: &EditorUi) -> El<DebugMsg> {
    let tool = |label: &str, key: &str, t: GizmoTool| {
        let b = button(label).key(key).class("tab").padding(Edges::symmetric(10.0, 5.0)).on_click(ext(EditorMsg::Tool(t)));
        if view.tool == t { b.class("tab-active") } else { b }
    };
    let grid_key = FieldKey::grid();
    let step_text = ui.drafts.get(&grid_key).cloned().unwrap_or_else(|| edit_text(&SceneValue::Float(view.grid_step as f64)));
    let (k_in, k_sub, k_focus) = (grid_key.clone(), grid_key.clone(), grid_key.clone());
    let mut step = text_input(step_text)
        .class("wire-field")
        .key("grid_step")
        .width(Size::Px(56.0))
        .on_input(move |s| ext(EditorMsg::Draft(k_in.clone(), s)))
        .on_submit(move |s| ext(EditorMsg::Commit(k_sub.clone(), s)))
        .on_focus(move |on| ext(EditorMsg::Focus(k_focus.tag(), on)));
    if ui.errors.contains_key(&grid_key) {
        step = step.class("wire-error");
    }
    row![
        tool("Move (W)", "tool_move", GizmoTool::Move),
        tool("Rotate (E)", "tool_rotate", GizmoTool::Rotate),
        tool("Scale (R)", "tool_scale", GizmoTool::Scale),
        container().class("separator-v").key("sep_grid"),
        checkbox("Grid (G)", view.grid).key("grid").on_toggle(|on| ext(EditorMsg::Grid(on))),
        text("step").class("muted").key("step_label"),
        step,
        container().width(Size::Fill),
        small(if view.detached { "Attach" } else { "Detach" }, "detach", EditorMsg::Detach(!view.detached), true),
    ]
    .class("wire-row")
    .align(Align::Center)
    .gap(4.0)
    .key("viewport")
    .width(Size::Fill)
}

/// The scene row: name with the dirty marker, the file buttons, the path field and the files of `scenes/`.
fn scene_bar(state: &EditorState, panel: &EditorPanel, ui: &EditorUi) -> El<DebugMsg> {
    let name = ui
        .path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_stem().and_then(|s| s.to_str()))
        .map(str::to_string)
        .or_else(|| panel.scene.clone())
        .unwrap_or_else(|| "(none)".to_string());
    let label = format!("Scene: {name}{}", if ui.dirty { " *" } else { "" });
    let edit = state.mode == EditorMode::Edit;
    let path = ui.path_text.clone();
    let mut col = column![
        row![
            text(label).key("label").width(Size::Fill),
            small("Save", "save", EditorMsg::Save, edit && ui.path.is_some()),
            small("Save As", "save_as", EditorMsg::SaveAs, edit),
            small("Load", "load", EditorMsg::Load(path.clone()), edit),
            small("New", "new", EditorMsg::NewScene, edit),
        ]
        .class("wire-row")
        .align(Align::Center)
        .key("bar")
        .width(Size::Fill),
        row![
            text_input(ui.path_text.clone())
                .placeholder("scenes/name.ron")
                .class("wire-field")
                .key("path")
                .width(Size::Fill)
                .on_input(|s| ext(EditorMsg::PathText(s)))
                .on_submit(move |s| ext(EditorMsg::Load(s)))
                .on_focus(|on| ext(EditorMsg::Focus("@path".into(), on))),
        ]
        .class("wire-row")
        .key("path_row")
        .width(Size::Fill),
    ]
    .key("scene")
    .width(Size::Fill);
    if !ui.scene_files.is_empty() {
        let mut list = column![].width(Size::Fill).key("list");
        for (i, f) in ui.scene_files.iter().take(4).enumerate() {
            list = list.child(button(f.clone()).key(format!("f{i}")).class("tab").text_align(TextAlign::Start).width(Size::Fill).padding(Edges::symmetric(10.0, 3.0)).enabled(edit).on_click(ext(EditorMsg::Load(f.clone()))));
        }
        if ui.scene_files.len() > 4 {
            list = list.child(text(format!("... and {} more", ui.scene_files.len() - 4)).class("muted").key("more"));
        }
        col = col.child(list);
    }
    col
}

/// Rows to show: the ones whose name contains the filter plus their ancestors (the tree stays
/// readable); all of them for an empty filter.
fn visible(panel: &EditorPanel) -> BTreeSet<u64> {
    let f = panel.filter.trim().to_lowercase();
    let all = panel.rows.iter().map(|r| entity_to_bits(r.entity));
    if f.is_empty() {
        return all.collect();
    }
    let mut out = BTreeSet::new();
    for r in panel.rows.iter().filter(|r| r.name.to_lowercase().contains(&f)) {
        let mut cur = Some(r);
        while let Some(c) = cur {
            if !out.insert(entity_to_bits(c.entity)) {
                break;
            }
            cur = c.parent.and_then(|p| panel.rows.iter().find(|x| x.entity == p));
        }
    }
    out
}

fn hierarchy(panel: &EditorPanel, selected: Option<Entity>) -> El<DebugMsg> {
    let has = selected.is_some();
    let has_parent = selected.is_some_and(|s| panel.rows.iter().any(|r| r.entity == s && r.parent.is_some()));
    let shown = visible(panel);
    let mut list = column![].width(Size::Fill);
    for r in panel.rows.iter().filter(|r| shown.contains(&entity_to_bits(r.entity))) {
        let bits = entity_to_bits(r.entity);
        let mut label = match r.editor_id {
            Some(id) => format!("{}  #{id}", r.name),
            None => r.name.clone(),
        };
        if let Some(p) = &r.prefab {
            label.push_str(&format!("  [{p}]"));
        }
        let mut b = button(label).key("row").class("tab").text_align(TextAlign::Start).width(Size::Fill).on_click(ext(EditorMsg::Select(Some(r.entity))));
        if selected == Some(r.entity) {
            b = b.class("tab-active");
        }
        list = list.child(row![container().width(Size::Px(r.depth as f32 * 14.0)).key("indent"), b].key(format!("e{bits}")).width(Size::Fill));
    }
    if panel.rows.is_empty() {
        list = list.child(text("(no entities)").class("muted").class("wire-row").key("empty"));
    } else if shown.is_empty() {
        list = list.child(text("(no match)").class("muted").class("wire-row").key("no_match"));
    }
    column![
        text_input(panel.filter.clone())
            .placeholder("filter")
            .class("wire-field")
            .key("filter")
            .width(Size::Fill)
            .on_input(|s| ext(EditorMsg::Filter(s)))
            .on_focus(|on| ext(EditorMsg::Focus("@filter".into(), on))),
        row![
            small("New", "new", EditorMsg::NewEntity, true),
            small("Duplicate", "duplicate", EditorMsg::Duplicate, has),
            small("Delete", "delete", EditorMsg::Delete, has),
            small("Unparent", "unparent", EditorMsg::Reparent(None), has_parent),
        ]
        .key("actions")
        .width(Size::Fill),
        scroll().width(Size::Fill).height(Size::Fill).key("list").child(list),
    ]
    .key("hierarchy")
    .width(Size::Fill)
    .height(Size::Fill)
}

/// The width of the left column.
const LEFT_W: f32 = 230.0;

fn left_tab(label: &str, key: &str, tab: LeftTab, cur: LeftTab) -> El<DebugMsg> {
    let b = button(label).key(key).class("tab").padding(Edges::symmetric(4.0, 5.0)).width(Size::Fill).on_click(ext(EditorMsg::Left(tab)));
    if cur == tab { b.class("tab-active") } else { b }
}

/// The Prefabs panel: save the selection as a prefab, and the list of prefabs to place.
fn prefabs_panel(state: &EditorState, panel: &EditorPanel, ui: &EditorUi, selected: Option<Entity>) -> El<DebugMsg> {
    let edit = state.mode == EditorMode::Edit;
    let name = ui.prefab_name.trim().to_string();
    let can_save = edit && selected.is_some() && !name.is_empty();
    let mut list = column![].width(Size::Fill);
    for (i, f) in ui.prefab_files.iter().enumerate() {
        list = list.child(
            button(format!("Place {f}")).key(format!("p{i}")).class("tab").text_align(TextAlign::Start).width(Size::Fill).padding(Edges::symmetric(10.0, 4.0)).on_click(ext(EditorMsg::PlacePrefab(f.clone()))),
        );
    }
    if ui.prefab_files.is_empty() {
        list = list.child(text("(no prefabs yet)").class("muted").class("wire-row").key("empty"));
    }
    let hint = match (&panel.prefab, selected) {
        (Some(p), Some(_)) => format!("The selection is part of prefab {}.", p.name),
        (None, Some(_)) => "Saves the selection and its children.".to_string(),
        _ => "Select an entity to save it as a prefab.".to_string(),
    };
    column![
        text("Save selection as prefab").class("muted").class("wire-row").key("title"),
        text_input(ui.prefab_name.clone())
            .placeholder("PrefabName")
            .class("wire-field")
            .key("name")
            .width(Size::Fill)
            .on_input(|s| ext(EditorMsg::PrefabName(s)))
            .on_submit(|_| ext(EditorMsg::SaveAsPrefab))
            .on_focus(|on| ext(EditorMsg::Focus("@prefab".into(), on))),
        row![small("Save as prefab", "save", EditorMsg::SaveAsPrefab, can_save)].key("actions").width(Size::Fill),
        text(hint).class("muted").class("wire-row").key("hint").width(Size::Fill),
        container().class("separator").key("sep"),
        text("Prefabs (a click places a copy at the editor camera)").class("muted").class("wire-row").key("list_title").width(Size::Fill),
        scroll().width(Size::Fill).height(Size::Fill).key("list").child(list),
    ]
    .key("prefabs")
    .width(Size::Fill)
    .height(Size::Fill)
}

/// Whether a system is the editor's or the engine's own (shown read-only, never listed by a scene).
fn engine_system(s: &efeengine_core::engine::scene::SystemInfo) -> bool {
    s.owner.as_deref().is_some_and(|o| o.starts_with("efeengine")) || matches!(s.name.as_str(), "propagate_transforms" | "render_extract")
}

/// The Systems panel: the active scene's `systems` list as checkboxes over every registered system.
fn systems_panel(panel: &EditorPanel) -> El<DebugMsg> {
    let has_scene = panel.scene.is_some();
    let title = match &panel.scene {
        Some(n) => format!("Systems of scene {n}"),
        None => "No scene is loaded (Load a scene to edit its systems)".to_string(),
    };
    let mut rows: Vec<&efeengine_core::engine::scene::SystemInfo> = panel.systems.iter().collect();
    rows.sort_by(|a, b| (engine_system(a) || a.global, a.owner.as_deref().unwrap_or(""), &a.name).cmp(&(engine_system(b) || b.global, b.owner.as_deref().unwrap_or(""), &b.name)));
    let mut list = column![].width(Size::Fill);
    for (i, s) in rows.iter().enumerate() {
        let read_only = engine_system(s) || s.global;
        let name = s.name.clone();
        let tag = if s.global { "global (never toggled)" } else if engine_system(s) { "engine" } else { "" };
        let owner = s.owner.as_deref().map(|o| format!("plugin {o}")).unwrap_or_else(|| "no plugin".to_string());
        let info = if tag.is_empty() { owner } else { format!("{owner}, {tag}") };
        list = list.child(
            column![
                checkbox(s.name.clone(), s.in_scene).key("check").enabled(has_scene && !read_only).on_toggle(move |on| ext(EditorMsg::SceneSystem(name.clone(), on))),
                text(info).class("muted").key("info").width(Size::Fill),
            ]
            .key(format!("s{i}"))
            .width(Size::Fill)
            .class("wire-row"),
        );
    }
    column![
        text(title).class("muted").class("wire-row").key("title").width(Size::Fill),
        scroll().width(Size::Fill).height(Size::Fill).key("list").child(list),
    ]
    .key("systems")
    .width(Size::Fill)
    .height(Size::Fill)
}

/// The left column: tabs and the panel they choose.
fn left_column(state: &EditorState, panel: &EditorPanel, ui: &EditorUi, selected: Option<Entity>) -> El<DebugMsg> {
    let body = match ui.left {
        LeftTab::Hierarchy => hierarchy(panel, selected),
        LeftTab::Prefabs => prefabs_panel(state, panel, ui, selected),
        LeftTab::Systems => systems_panel(panel),
    };
    column![
        row![
            left_tab("Hierarchy", "hierarchy", LeftTab::Hierarchy, ui.left),
            left_tab("Prefabs", "prefabs", LeftTab::Prefabs, ui.left),
            left_tab("Systems", "systems", LeftTab::Systems, ui.left),
        ]
        .key("left")
        .gap(2.0)
        .width(Size::Fill),
        body,
    ]
    .width(Size::Px(LEFT_W))
    .height(Size::Fill)
}

/// The text of an input for `v`: the shortest exact form (`format_value` rounds for reading).
pub fn edit_text(v: &SceneValue) -> String {
    match v {
        SceneValue::Float(f) => format!("{f}"),
        other => format_value(other),
    }
}

/// One text input of a field (a scalar, or element `index` of a list).
fn field_input(ui: &EditorUi, key: FieldKey, shown: &SceneValue, name: &str) -> El<DebugMsg> {
    let text_now = ui.drafts.get(&key).cloned().unwrap_or_else(|| edit_text(shown));
    let (k_in, k_sub, k_focus) = (key.clone(), key.clone(), key.clone());
    let mut input = text_input(text_now)
        .class("wire-field")
        .key(name)
        .width(Size::Fill)
        .on_input(move |s| ext(EditorMsg::Draft(k_in.clone(), s)))
        .on_submit(move |s| ext(EditorMsg::Commit(k_sub.clone(), s)))
        .on_focus(move |on| ext(EditorMsg::Focus(k_focus.tag(), on)));
    if ui.errors.contains_key(&key) {
        input = input.class("wire-error");
    }
    input
}

/// The widget that shows (and edits) one field. Read-only components and entity references get
/// plain text.
fn field_widget(ui: &EditorUi, component: &str, field: &str, v: &SceneValue, editable: bool) -> El<DebugMsg> {
    if !editable {
        return text(format_value(v)).key("value").width(Size::Fill);
    }
    match v {
        SceneValue::Bool(b) => {
            let key = FieldKey::new(component, field, 0);
            checkbox("", *b).key("value").on_toggle(move |on| ext(EditorMsg::Toggle(key.clone(), on)))
        }
        SceneValue::List(items) => {
            // The inspector column is about 300 px wide: the elements share what is left of it.
            let w = ((200.0 - 4.0 * (items.len().saturating_sub(1)) as f32) / items.len().max(1) as f32).clamp(36.0, 120.0);
            let mut r = row![].key("value").width(Size::Fill).gap(4.0);
            for (k, item) in items.iter().enumerate() {
                r = r.child(field_input(ui, FieldKey::new(component, field, k), item, &format!("e{k}")).width(Size::Px(w)));
            }
            r
        }
        scalar => field_input(ui, FieldKey::new(component, field, 0), scalar, "value"),
    }
}

/// "Parent: <name>" with a button that opens the list of possible parents (everything but the
/// entity and its descendants) and "None" to unparent.
fn parent_section(panel: &EditorPanel, ui: &EditorUi, sel: Entity) -> El<DebugMsg> {
    let row_of = |e: Entity| panel.rows.iter().find(|r| r.entity == e);
    let current = row_of(sel).and_then(|r| r.parent).and_then(row_of).map(|r| r.name.clone());
    let mut banned: BTreeSet<u64> = BTreeSet::new();
    let mut stack = vec![sel];
    while let Some(e) = stack.pop() {
        if banned.insert(entity_to_bits(e)) {
            stack.extend(row_of(e).map(|r| r.children.clone()).unwrap_or_default());
        }
    }
    let mut sec = column![
        row![
            text("Parent").class("muted").key("label").width(Size::Px(90.0)),
            text(current.unwrap_or_else(|| "(none)".to_string())).key("value").width(Size::Fill),
            small(if ui.parent_picker { "Close" } else { "Change" }, "toggle", EditorMsg::ParentPicker(!ui.parent_picker), true),
        ]
        .class("wire-row")
        .align(Align::Center)
        .key("row")
        .width(Size::Fill),
    ]
    .key("parent")
    .width(Size::Fill);
    if ui.parent_picker {
        let mut list = column![button("(none: unparent)").key("none").class("tab").text_align(TextAlign::Start).width(Size::Fill).padding(Edges::symmetric(14.0, 4.0)).on_click(ext(EditorMsg::Reparent(None)))]
            .key("picker")
            .width(Size::Fill);
        for (i, r) in panel.rows.iter().filter(|r| !banned.contains(&entity_to_bits(r.entity))).enumerate() {
            list = list.child(button(r.name.clone()).key(format!("p{i}")).class("tab").text_align(TextAlign::Start).width(Size::Fill).padding(Edges::symmetric(14.0, 4.0)).on_click(ext(EditorMsg::Reparent(Some(r.entity)))));
        }
        sec = sec.child(list);
    }
    sec
}

/// "Prefab: <name>" with Apply and Unpack. Overridden fields are marked in the component blocks.
fn prefab_section(state: &EditorState, p: &super::PrefabInfo) -> El<DebugMsg> {
    let role = if p.is_root { "root" } else if p.linked { "part of an instance" } else { "detached from its instance" };
    column![
        row![
            text("Prefab").class("muted").key("label").width(Size::Px(90.0)),
            text(format!("{} ({role})", p.name)).key("info").width(Size::Fill),
        ]
        .class("wire-row")
        .align(Align::Center)
        .key("row")
        .width(Size::Fill),
        row![
            small("Apply to prefab", "apply", EditorMsg::ApplyPrefab, p.linked && state.mode == EditorMode::Edit),
            small("Unpack", "unpack", EditorMsg::UnpackPrefab, true),
        ]
        .class("wire-row")
        .key("actions")
        .gap(4.0)
        .width(Size::Fill),
    ]
    .key("prefab")
    .width(Size::Fill)
}

fn inspector(state: &EditorState, panel: &EditorPanel, ui: &EditorUi, selected: Option<Entity>) -> El<DebugMsg> {
    let mut col = column![].width(Size::Fill);
    match (&panel.inspected, selected) {
        (Some(comps), Some(_)) => {
            let name_key = FieldKey::name();
            let (k_in, k_sub, k_focus) = (name_key.clone(), name_key.clone(), name_key.clone());
            let name_text = ui.drafts.get(&name_key).cloned().unwrap_or_else(|| panel.selected_name.clone().unwrap_or_default());
            let mut name_input = text_input(name_text)
                .placeholder("(no name)")
                .class("wire-field")
                .key("name")
                .width(Size::Fill)
                .on_input(move |s| ext(EditorMsg::Draft(k_in.clone(), s)))
                .on_submit(move |s| ext(EditorMsg::Commit(k_sub.clone(), s)))
                .on_focus(move |on| ext(EditorMsg::Focus(k_focus.tag(), on)));
            if ui.errors.contains_key(&name_key) {
                name_input = name_input.class("wire-error");
            }
            col = col
                .child(row![text("Name").class("muted").key("label").width(Size::Px(90.0)), name_input].class("wire-row").align(Align::Center).key("entity").width(Size::Fill))
                .child(container().class("separator").key("sep_name"));
            let mut add = column![
                row![small(if ui.picker_open { "Close" } else { "Add component" }, "toggle", EditorMsg::Picker(!ui.picker_open), true)].class("wire-row").key("row").width(Size::Fill),
            ]
            .key("add")
            .width(Size::Fill);
            if ui.picker_open {
                let mut list = column![].key("picker").width(Size::Fill);
                for (i, n) in panel.addable.iter().enumerate() {
                    list = list.child(button(n.clone()).key(format!("a{i}")).class("tab").text_align(TextAlign::Start).width(Size::Fill).padding(Edges::symmetric(14.0, 4.0)).on_click(ext(EditorMsg::AddComponent(n.clone()))));
                }
                if panel.addable.is_empty() {
                    list = list.child(text("(nothing left to add)").class("muted").class("wire-row").key("none"));
                }
                add = add.child(list);
            }
            col = col.child(add).child(container().class("separator").key("sep_add"));
            if let Some(sel) = selected {
                col = col.child(parent_section(panel, ui, sel)).child(container().class("separator").key("sep_parent"));
                if let Some(p) = &panel.prefab {
                    col = col.child(prefab_section(state, p)).child(container().class("separator").key("sep_prefab"));
                }
            }
            for (i, c) in comps.iter().enumerate() {
                let title = if c.readonly { format!("{} (read-only)", c.name) } else { c.name.clone() };
                let mut bar = row![text(title).bold(true).key("head").width(Size::Fill)].class("wire-row").align(Align::Center).key("bar").width(Size::Fill);
                if super::edit::removable(&c.name, c.readonly) {
                    bar = bar.child(small("x", "remove", EditorMsg::RemoveComponent(c.name.clone()), true));
                }
                let mut block = column![bar].width(Size::Fill);
                for (j, (name, v)) in c.fields.iter().enumerate() {
                    let editable = !c.readonly && !c.entity_fields.contains(name) && !UNMANAGED_COMPONENTS.contains(&c.name.as_str());
                    let overridden = panel.overridden.iter().any(|(pc, pf)| *pc == c.name && pf == name);
                    let label = if overridden { text(format!("{name} *")).bold(true).key("name").width(Size::Px(90.0)) } else { text(name.clone()).class("muted").key("name").width(Size::Px(90.0)) };
                    let mut r = row![label, field_widget(ui, &c.name, name, v, editable)].class("wire-row").align(Align::Center).key(format!("f{j}")).width(Size::Fill);
                    if overridden {
                        r = r.child(small("revert", "revert", EditorMsg::RevertField(FieldKey::new(&c.name, name, 0)), true));
                    }
                    block = block.child(r);
                }
                col = col.child(block.key(format!("c{i}"))).child(container().class("separator").key(format!("sep{i}")));
            }
        }
        _ => col = col.child(text("(select an entity)").class("muted").class("wire-row").key("none")),
    }
    scroll().width(Size::Fill).height(Size::Fill).key("inspector").child(col)
}

/// The Debug menu tab: the whole editor, or a note while the editor has its own window.
pub(super) fn view(cx: &ViewCx) -> El<DebugMsg> {
    let detached = cx.res::<EditorView>().is_some_and(|v| v.detached);
    if detached {
        return column![
            column![
                text("The editor panels are in the Editor window.").class("muted").class("wire-row").key("note"),
                row![small("Attach", "attach", EditorMsg::Detach(false), true)].class("wire-row").key("row").width(Size::Fill),
            ]
            .key("detached")
            .width(Size::Fill),
        ]
        .key(TAB)
        .width(Size::Fill)
        .height(Size::Fill);
    }
    body(cx)
}

/// The size of the panel in the editor window for a window of `window` pixels: 45% of the width
/// (between 420 and 760, never wider than the window) and the whole height. Before the window's
/// size is known (0) the old fixed 540x660.
pub(super) fn panel_size(window: (u32, u32)) -> (f32, f32) {
    let (w, h) = window;
    if w == 0 || h == 0 {
        return (540.0, 660.0);
    }
    ((w as f32 * 0.45).clamp(420.0, 760.0).min(w as f32), h as f32)
}

/// The panel view of the editor window (layout B).
pub(super) fn window_view(cx: &ViewCx) -> El<DebugMsg> {
    // Docked to the left of the editor window, as tall as the window and a share of its width,
    // so the viewport stays visible beside it; a resize of the window resizes the panel.
    let (pw, ph) = panel_size(cx.res::<EditorView>().map_or((0, 0), |v| v.window_size));
    column![body(cx)].class("wire-panel").width(Size::Px(pw)).height(Size::Px(ph))
}

fn body(cx: &ViewCx) -> El<DebugMsg> {
    let view = cx.res::<EditorView>().map(|v| (*v).clone()).unwrap_or_default();
    let state = cx.res::<EditorState>().map(|s| (*s).clone()).unwrap_or(EditorState { mode: EditorMode::Edit, keep_changes: false, frozen: 0 });
    let panel = cx.res::<EditorPanel>().map(|p| (*p).clone()).unwrap_or_default();
    let ui = cx.res::<EditorUi>().map(|u| (*u).clone()).unwrap_or_default();
    let selected = cx.res::<EditorSelection>().and_then(|s| s.entity);
    let status = match state.mode {
        EditorMode::Edit => format!("Edit: {} game systems frozen", state.frozen),
        EditorMode::Play => "Play: the game runs (Stop restores the world)".to_string(),
        EditorMode::Paused => format!("Paused: {} game systems frozen", state.frozen),
    };
    let mut col = column![
        toolbar(&state, &ui),
        row![
            text(status).class("muted").key("status").width(Size::Fill),
            checkbox("Handles (H)", view.handles).key("handles").on_toggle(|on| ext(EditorMsg::Handles(on))),
            checkbox("Keep changes on Stop", state.keep_changes).key("keep").on_toggle(|on| ext(EditorMsg::KeepChanges(on))),
        ]
        .class("wire-row")
        .align(Align::Center)
        .key("options")
        .width(Size::Fill),
        viewport_bar(&view, &ui),
        container().class("separator").key("sep_toolbar"),
        scene_bar(&state, &panel, &ui),
    ];
    // One line for the last result, or the reminder that scene files wait for Edit mode.
    if !ui.status.is_empty() {
        col = col.child(text(ui.status.clone()).class(if ui.status_error { "danger" } else { "muted" }).class("wire-row").key("message").width(Size::Fill));
    } else if state.mode != EditorMode::Edit {
        col = col.child(text("Save, Load and New are available in Edit mode (press Stop)").class("muted").class("wire-row").key("message").width(Size::Fill));
    }
    col.child(container().class("separator").key("sep_scene"))
        .child(row![left_column(&state, &panel, &ui, selected), container().class("separator-v").key("sep_mid"), inspector(&state, &panel, &ui, selected)].width(Size::Fill).height(Size::Fill))
        .key(TAB)
        .width(Size::Fill)
        .height(Size::Fill)
}
