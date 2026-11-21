//! The Editor tab body: toolbar, scene row, hierarchy (left) and inspector (right).
//!
//! Keys under `debug_menu/editor`: `toolbar/{edit,play,pause,step,stop,undo,redo}`,
//! `options/{status,keep}`, `scene/{label,save,save_as,load,new,path,list/f<N>}`, `message`,
//! `hierarchy/filter`, `hierarchy/actions/{new,duplicate,delete}`, `hierarchy/list/e<bits>/row`
//! (the clickable row), `inspector/entity/name` (the name input), `inspector/c<N>` (one per
//! component: `bar/head`, `bar/remove`, and `f<M>` rows with `name` and `value`; `value` is a text
//! input, a checkbox, a read-only text, or a row of text inputs `e<K>` for a list),
//! `inspector/add/row/toggle` (under the name, so it stays in view) and `inspector/add/picker/a<N>` (the N-th of `EditorPanel::addable`).

use std::collections::BTreeSet;

use efeengine_core::ecs::{SceneValue, entity_to_bits};
use efeengine_core::engine::inspect::UNMANAGED_COMPONENTS;
use efeengine_core::prelude::*;

use super::{EditorMode, EditorMsg, EditorPanel, EditorRequest, EditorSelection, EditorState, EditorUi, FieldKey, TAB, ext};
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
    let shown = visible(panel);
    let mut list = column![].width(Size::Fill);
    for r in panel.rows.iter().filter(|r| shown.contains(&entity_to_bits(r.entity))) {
        let bits = entity_to_bits(r.entity);
        let label = match r.editor_id {
            Some(id) => format!("{}  #{id}", r.name),
            None => r.name.clone(),
        };
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
        ]
        .key("actions")
        .width(Size::Fill),
        scroll().width(Size::Fill).height(Size::Fill).key("list").child(list),
    ]
    .key("hierarchy")
    .width(Size::Px(210.0))
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

fn inspector(panel: &EditorPanel, ui: &EditorUi, selected: Option<Entity>) -> El<DebugMsg> {
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
            for (i, c) in comps.iter().enumerate() {
                let title = if c.readonly { format!("{} (read-only)", c.name) } else { c.name.clone() };
                let mut bar = row![text(title).bold(true).key("head").width(Size::Fill)].class("wire-row").align(Align::Center).key("bar").width(Size::Fill);
                if super::edit::removable(&c.name, c.readonly) {
                    bar = bar.child(small("x", "remove", EditorMsg::RemoveComponent(c.name.clone()), true));
                }
                let mut block = column![bar].width(Size::Fill);
                for (j, (name, v)) in c.fields.iter().enumerate() {
                    let editable = !c.readonly && !c.entity_fields.contains(name) && !UNMANAGED_COMPONENTS.contains(&c.name.as_str());
                    block = block.child(
                        row![text(name.clone()).class("muted").key("name").width(Size::Px(90.0)), field_widget(ui, &c.name, name, v, editable)]
                            .class("wire-row")
                            .align(Align::Center)
                            .key(format!("f{j}"))
                            .width(Size::Fill),
                    );
                }
                col = col.child(block.key(format!("c{i}"))).child(container().class("separator").key(format!("sep{i}")));
            }
        }
        _ => col = col.child(text("(select an entity)").class("muted").class("wire-row").key("none")),
    }
    scroll().width(Size::Fill).height(Size::Fill).key("inspector").child(col)
}

pub(super) fn view(cx: &ViewCx) -> El<DebugMsg> {
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
            checkbox("Keep changes on Stop", state.keep_changes).key("keep").on_toggle(|on| ext(EditorMsg::KeepChanges(on))),
        ]
        .class("wire-row")
        .align(Align::Center)
        .key("options")
        .width(Size::Fill),
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
        .child(row![hierarchy(&panel, selected), container().class("separator-v").key("sep_mid"), inspector(&panel, &ui, selected)].width(Size::Fill).height(Size::Fill))
        .key(TAB)
        .width(Size::Fill)
        .height(Size::Fill)
}
