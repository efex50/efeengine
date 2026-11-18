//! Pure helpers behind the panels: labels, value formatting and input checks.

use std::path::Path;

use efeengine_editor_link::protocol::{ComponentInfo, EntityInfo, LogLine, Mode, StageInfo, SystemInfo};
use efeengine_editor_link::resolve_project_path;
use serde_json::Value;

/// Hierarchy row text: the name (or `<unnamed>`), the editor id when there is one, else the raw id.
pub fn entity_label(e: &EntityInfo) -> String {
    let name = e.name.as_deref().filter(|n| !n.is_empty()).unwrap_or("<unnamed>");
    match &e.editor_id {
        Some(id) => format!("{name}  [editor id {id}]"),
        None => format!("{name}  (#{})", e.entity),
    }
}

/// Hierarchy order: named entities first (by name), then unnamed ones, stable by raw id.
pub fn sort_entities(list: &mut [EntityInfo]) {
    list.sort_by(|a, b| {
        let key = |e: &EntityInfo| (e.name.is_none(), e.name.clone().unwrap_or_default(), e.entity);
        key(a).cmp(&key(b))
    });
}

/// A JSON field value as short text: numbers trimmed, arrays as `[a, b]`.
pub fn format_value(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64().filter(|_| n.is_f64()) {
            Some(f) => {
                let s = format!("{f:.4}");
                let s = s.trim_end_matches('0').trim_end_matches('.');
                if s.is_empty() || s == "-" || s == "-0" { "0".into() } else { s.to_string() }
            }
            None => n.to_string(),
        },
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(format_value).collect::<Vec<_>>().join(", ")),
        Value::Object(_) => v.to_string(),
    }
}

/// `(field, value)` rows of one component, in field-name order.
pub fn component_rows(c: &ComponentInfo) -> Vec<(String, String)> {
    c.fields.iter().map(|(k, v)| (k.clone(), format_value(v))).collect()
}

/// Systems panel rows: `name`, with ` (off)` when disabled and ` [global]` when global.
pub fn system_label(s: &SystemInfo) -> String {
    format!("{}{}{}", s.name, if s.enabled { "" } else { "  (off)" }, if s.global { "  [global]" } else { "" })
}

/// `(stage header, rows)` for stages that have systems.
pub fn system_sections(stages: &[StageInfo]) -> Vec<(String, Vec<String>)> {
    stages
        .iter()
        .filter(|s| !s.systems.is_empty())
        .map(|s| {
            let on = s.systems.iter().filter(|x| x.enabled).count();
            (format!("{} ({on}/{} on)", s.stage, s.systems.len()), s.systems.iter().map(system_label).collect())
        })
        .collect()
}

/// One log line as `LEVEL text`.
pub fn log_label(l: &LogLine) -> String {
    format!("{:<5} {}", l.level.to_uppercase(), l.text)
}

/// Whether a log line is shown at the filter's level: `all`, or at least `warn`.
pub fn log_visible(l: &LogLine, warnings_only: bool) -> bool {
    !warnings_only || matches!(l.level.as_str(), "warn" | "alert" | "error")
}

pub fn mode_label(m: Mode) -> &'static str {
    match m {
        Mode::Edit => "Edit",
        Mode::Play => "Play",
    }
}

/// Resources an editor user does not need in the fidelity list: the engine's own (core, window,
/// graphics, audio and link internals). What is left is mostly the game's own state.
pub fn game_gaps(gaps: &[String]) -> Vec<String> {
    const INTERNAL: [&str; 4] = ["efeengine_core::", "efeengine_editor_link::", "efeengine_ui::", "efeengine_audio::"];
    gaps.iter().filter(|g| !INTERNAL.iter().any(|p| g.starts_with(p)) && !g.starts_with("std::") && !g.starts_with("alloc::")).cloned().collect()
}

/// The Play panel text: the resources a stop will not rewind, then what no snapshot covers.
pub fn fidelity_lines(gaps: &[String], not_covered: &[String]) -> Vec<String> {
    let game = game_gaps(gaps);
    let mut out = Vec::new();
    if game.is_empty() {
        out.push("all game resources are registered for snapshots".to_string());
    } else {
        out.push(format!("stop will not rewind these resources ({}; register them for snapshots):", game.len()));
        out.extend(game.into_iter().map(|g| format!("  {g}")));
    }
    out.push("a snapshot never covers:".to_string());
    out.extend(not_covered.iter().map(|n| format!("  {n}")));
    out
}

/// Client-side check of a scene path before it is sent (the game checks again): the same rule
/// as the link's, so a typo shows up without a round trip.
pub fn validate_scene_path(path: &str) -> Result<(), String> {
    resolve_project_path(Path::new("."), path.trim()).map(|_| ())
}

/// The port from `--port N` or `EFE_EDITOR_PORT`, default 7878.
pub fn parse_port(args: &[String], env: Option<&str>) -> Result<u16, String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--port" {
            let v = it.next().ok_or("--port needs a value")?;
            return v.parse().map_err(|_| format!("bad port `{v}`"));
        }
        if let Some(v) = a.strip_prefix("--port=") {
            return v.parse().map_err(|_| format!("bad port `{v}`"));
        }
    }
    match env {
        Some(v) => v.parse().map_err(|_| format!("bad EFE_EDITOR_PORT `{v}`")),
        None => Ok(7878),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ent(name: Option<&str>, id: Option<&str>, bits: u64) -> EntityInfo {
        EntityInfo { editor_id: id.map(Into::into), entity: bits, name: name.map(Into::into) }
    }

    #[test]
    fn labels_and_order() {
        assert_eq!(entity_label(&ent(Some("hero"), None, 5)), "hero  (#5)");
        assert_eq!(entity_label(&ent(Some("hero"), Some("9"), 5)), "hero  [editor id 9]");
        assert_eq!(entity_label(&ent(None, None, 7)), "<unnamed>  (#7)");
        let mut v = vec![ent(None, None, 1), ent(Some("b"), None, 3), ent(Some("a"), None, 4), ent(None, None, 0)];
        sort_entities(&mut v);
        assert_eq!(v.iter().map(|e| e.entity).collect::<Vec<_>>(), [4, 3, 0, 1]);
    }

    #[test]
    fn values() {
        assert_eq!(format_value(&json!(2.5)), "2.5");
        assert_eq!(format_value(&json!(3.0)), "3");
        assert_eq!(format_value(&json!(0.0)), "0");
        assert_eq!(format_value(&json!(-0.00001)), "0");
        assert_eq!(format_value(&json!(7)), "7");
        assert_eq!(format_value(&json!(true)), "true");
        assert_eq!(format_value(&json!([1.5, 2, 0.25])), "[1.5, 2, 0.25]");
        assert_eq!(format_value(&Value::Null), "-");
    }

    #[test]
    fn rows_are_in_field_order() {
        let mut fields = serde_json::Map::new();
        fields.insert("y".into(), json!(2.0));
        fields.insert("x".into(), json!(1.0));
        let c = ComponentInfo { name: "P".into(), id: 1, fields };
        assert_eq!(component_rows(&c), vec![("x".into(), "1".into()), ("y".into(), "2".into())]);
    }

    #[test]
    fn systems() {
        let sys = |n: &str, en, g| SystemInfo { name: n.into(), enabled: en, global: g };
        assert_eq!(system_label(&sys("a", true, false)), "a");
        assert_eq!(system_label(&sys("a", false, true)), "a  (off)  [global]");
        let stages = vec![
            StageInfo { stage: "Startup".into(), systems: vec![] },
            StageInfo { stage: "Update".into(), systems: vec![sys("a", true, false), sys("b", false, false)] },
        ];
        let s = system_sections(&stages);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "Update (1/2 on)");
        assert_eq!(s[0].1, vec!["a".to_string(), "b  (off)".to_string()]);
    }

    #[test]
    fn fidelity() {
        let gaps = vec!["efeengine_core::engine::Time".to_string(), "mygame::Score".to_string()];
        assert_eq!(game_gaps(&gaps), vec!["mygame::Score".to_string()]);
        let l = fidelity_lines(&gaps, &["events in flight".to_string()]);
        assert!(l[0].contains("(1;") && l.iter().any(|x| x == "  mygame::Score") && l.iter().any(|x| x == "  events in flight"));
        assert!(fidelity_lines(&[], &[])[0].starts_with("all game"));
    }

    #[test]
    fn logs() {
        let l = |lv: &str| LogLine { level: lv.into(), text: "x".into() };
        assert_eq!(log_label(&l("info")), "INFO  x");
        assert!(log_visible(&l("info"), false));
        assert!(!log_visible(&l("info"), true));
        assert!(log_visible(&l("error"), true) && log_visible(&l("warn"), true));
    }

    #[test]
    fn scene_paths() {
        assert!(validate_scene_path("levels/a.ron").is_ok());
        assert!(validate_scene_path("  a.ron  ").is_ok());
        for bad in ["", "   ", "/etc/x", "../x", "a/../../b"] {
            assert!(validate_scene_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ports() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_port(&a(&[]), None), Ok(7878));
        assert_eq!(parse_port(&a(&[]), Some("9000")), Ok(9000));
        assert_eq!(parse_port(&a(&["--port", "1234"]), Some("9000")), Ok(1234));
        assert_eq!(parse_port(&a(&["--port=55"]), None), Ok(55));
        assert!(parse_port(&a(&["--port"]), None).is_err());
        assert!(parse_port(&a(&["--port", "x"]), None).is_err());
        assert!(parse_port(&a(&[]), Some("nope")).is_err());
    }
}
