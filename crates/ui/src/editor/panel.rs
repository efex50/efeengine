//! The Editor tab body: toolbar, hierarchy (left) and inspector (right).
//!
//! Keys under `debug_menu/editor`: `toolbar/{edit,play,pause,step,stop}`, `options/{status,keep}`,
//! `hierarchy/filter`, `hierarchy/list/e<bits>/row` (the clickable row), `inspector/c<N>` (one per
//! component, `head` and `f<M>` rows with `name` and `value`).

use std::collections::BTreeSet;

use efeengine_core::ecs::{SceneValue, entity_to_bits};
use efeengine_core::prelude::*;

use super::{EditorMode, EditorMsg, EditorPanel, EditorRequest, EditorSelection, EditorState, TAB, ext};
use crate::debug_menu::DebugMsg;
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

fn toolbar(state: &EditorState) -> El<DebugMsg> {
    let m = state.mode;
    row![
        mode_button("Edit", "edit", EditorRequest::Edit, m == EditorMode::Edit),
        mode_button("Play", "play", EditorRequest::Play, m == EditorMode::Play),
        mode_button("Pause", "pause", EditorRequest::Pause, m == EditorMode::Paused),
        mode_button("Step", "step", EditorRequest::Step, false),
        mode_button("Stop", "stop", EditorRequest::Stop, false),
    ]
    .key("toolbar")
    .align(Align::Center)
    .width(Size::Fill)
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
        text_input(panel.filter.clone()).placeholder("filter").class("wire-field").key("filter").width(Size::Fill).on_input(|s| ext(EditorMsg::Filter(s))),
        scroll().width(Size::Fill).height(Size::Fill).key("list").child(list),
    ]
    .key("hierarchy")
    .width(Size::Px(210.0))
    .height(Size::Fill)
}

fn inspector(panel: &EditorPanel, selected: Option<Entity>) -> El<DebugMsg> {
    let mut col = column![].width(Size::Fill);
    match (&panel.inspected, selected) {
        (Some(comps), Some(_)) => {
            for (i, c) in comps.iter().enumerate() {
                let title = if c.readonly { format!("{} (read-only)", c.name) } else { c.name.clone() };
                let mut block = column![text(title).bold(true).class("wire-row").key("head")].width(Size::Fill);
                for (j, (name, v)) in c.fields.iter().enumerate() {
                    block = block.child(
                        row![text(name.clone()).class("muted").key("name").width(Size::Px(90.0)), text(format_value(v)).key("value").width(Size::Fill)]
                            .class("wire-row")
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
    let selected = cx.res::<EditorSelection>().and_then(|s| s.entity);
    let status = match state.mode {
        EditorMode::Edit => format!("Edit: {} game systems frozen", state.frozen),
        EditorMode::Play => "Play: the game runs (Stop restores the world)".to_string(),
        EditorMode::Paused => format!("Paused: {} game systems frozen", state.frozen),
    };
    column![
        toolbar(&state),
        row![
            text(status).class("muted").key("status").width(Size::Fill),
            checkbox("Keep changes on Stop", state.keep_changes).key("keep").on_toggle(|on| ext(EditorMsg::KeepChanges(on))),
        ]
        .class("wire-row")
        .align(Align::Center)
        .key("options")
        .width(Size::Fill),
        container().class("separator").key("sep_toolbar"),
        row![hierarchy(&panel, selected), container().class("separator-v").key("sep_mid"), inspector(&panel, selected)].width(Size::Fill).height(Size::Fill),
    ]
    .key(TAB)
    .width(Size::Fill)
    .height(Size::Fill)
}
