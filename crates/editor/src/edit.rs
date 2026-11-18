//! Pure logic behind inspector editing and undo (docs/EDITOR.md E4): input text to JSON per schema
//! type, the inverse of each edit, and the undo stack. No window, no socket.

use std::collections::BTreeMap;

use efeengine_editor_link::protocol::{ComponentInfo, Mode, Reply, Request};
use serde_json::{Map, Value, json};

// ---- schema ----

/// One field of a component, from the exported schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: String,
    /// A primitive name: `u8 i8 u16 i16 u32 i32 u64 i64 f32 f64 bool entity`.
    pub ty: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompSpec {
    pub readonly: bool,
    pub fields: Vec<FieldSpec>,
}

/// Component name to its fields.
pub type Schema = BTreeMap<String, CompSpec>;

/// Read the `components` of an `efe-schema` document (anything malformed is skipped).
pub fn parse_schema(doc: &Value) -> Schema {
    let mut out = Schema::new();
    let Some(list) = doc.get("components").and_then(Value::as_array) else { return out };
    for c in list {
        let Some(name) = c.get("name").and_then(Value::as_str) else { continue };
        let fields = c
            .get("fields")
            .and_then(Value::as_array)
            .map(|fs| {
                fs.iter()
                    .filter_map(|f| {
                        Some(FieldSpec {
                            name: f.get("name")?.as_str()?.to_string(),
                            ty: f.get("type")?.as_str()?.to_string(),
                            count: f.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(name.to_string(), CompSpec { readonly: c.get("readonly").and_then(Value::as_bool).unwrap_or(false), fields });
    }
    out
}

/// What widget a field gets in the inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    Number { ty: String },
    Bool,
    /// `count` numeric inputs.
    Vector { ty: String, count: u32 },
    /// A picker over the Hierarchy.
    Entity,
    /// Shown, not editable; the text says why.
    ReadOnly(String),
}

const NUMBERS: [&str; 10] = ["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "f32", "f64"];

pub fn input_kind(comp: &CompSpec, f: &FieldSpec) -> InputKind {
    if comp.readonly {
        return InputKind::ReadOnly("read-only component".into());
    }
    match (f.ty.as_str(), f.count) {
        (_, 0) => InputKind::ReadOnly("empty field".into()),
        ("bool", 1) => InputKind::Bool,
        ("entity", 1) => InputKind::Entity,
        (t, 1) if NUMBERS.contains(&t) => InputKind::Number { ty: t.into() },
        (t, n) if NUMBERS.contains(&t) && n <= 16 => InputKind::Vector { ty: t.into(), count: n },
        ("bool" | "entity", n) => InputKind::ReadOnly(format!("arrays of {} ({n}) are not editable", f.ty)),
        (t, n) if NUMBERS.contains(&t) => InputKind::ReadOnly(format!("arrays of {n} values are not editable")),
        (t, _) => InputKind::ReadOnly(format!("unsupported type `{t}`")),
    }
}

// ---- input text to JSON ----

fn range_of(ty: &str) -> Option<(i128, i128)> {
    Some(match ty {
        "u8" => (0, u8::MAX as i128),
        "i8" => (i8::MIN as i128, i8::MAX as i128),
        "u16" => (0, u16::MAX as i128),
        "i16" => (i16::MIN as i128, i16::MAX as i128),
        "u32" => (0, u32::MAX as i128),
        "i32" => (i32::MIN as i128, i32::MAX as i128),
        "u64" => (0, u64::MAX as i128),
        "i64" => (i64::MIN as i128, i64::MAX as i128),
        _ => return None,
    })
}

/// Convert the text of one input to the JSON value `set_field` takes, by primitive type.
/// Entity text is `none` or raw entity bits (the picker sends editor ids itself).
pub fn parse_scalar(ty: &str, text: &str) -> Result<Value, String> {
    let s = text.trim();
    if let Some((lo, hi)) = range_of(ty) {
        let n: i128 = s.parse().map_err(|_| format!("`{s}` is not a whole number for {ty}"))?;
        if n < lo || n > hi {
            return Err(format!("{n} is out of range for {ty} ({lo}..={hi})"));
        }
        return Ok(if n < 0 { json!(n as i64) } else { json!(n as u64) });
    }
    match ty {
        "f32" | "f64" => {
            let x: f64 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
            if !x.is_finite() || (ty == "f32" && !(x as f32).is_finite()) {
                return Err(format!("`{s}` is out of range for {ty}"));
            }
            Ok(json!(x))
        }
        "bool" => match s.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(json!(true)),
            "false" | "0" => Ok(json!(false)),
            _ => Err(format!("`{s}` is not a bool (true or false)")),
        },
        "entity" => match s.to_ascii_lowercase().as_str() {
            "" | "none" => Ok(json!(0u64)),
            d => d.parse::<u64>().map(|n| json!(n)).map_err(|_| format!("`{s}` is not entity bits (use the picker, or `none`)")),
        },
        other => Err(format!("unsupported type `{other}`")),
    }
}

/// `count` inputs to a JSON array; the error names the failing component.
pub fn parse_vector(ty: &str, count: u32, parts: &[String]) -> Result<Value, String> {
    if parts.len() != count as usize {
        return Err(format!("expected {count} values, got {}", parts.len()));
    }
    let mut out = Vec::with_capacity(parts.len());
    for (i, p) in parts.iter().enumerate() {
        out.push(parse_scalar(ty, p).map_err(|e| format!("[{i}]: {e}"))?);
    }
    Ok(Value::Array(out))
}

/// The inputs of a field (one per value) as editable text, from its current JSON value.
pub fn edit_texts(ty: &str, v: &Value) -> Vec<String> {
    let one = |v: &Value| match v {
        Value::Number(n) if ty == "f32" => n.as_f64().map(|f| (f as f32).to_string()).unwrap_or_else(|| n.to_string()),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    match v {
        Value::Array(a) => a.iter().map(one).collect(),
        other => vec![one(other)],
    }
}

// ---- operations and their inverses ----

/// An entity named in an edit: by editor id when it has one (stable), else raw bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    EditorId(String),
    Bits(u64),
}

impl Target {
    pub fn of(entity: Option<u64>, editor_id: Option<&str>) -> Option<Target> {
        match (editor_id, entity) {
            (Some(id), _) => Some(Target::EditorId(id.to_string())),
            (None, Some(b)) => Some(Target::Bits(b)),
            (None, None) => None,
        }
    }
    fn parts(&self) -> (Option<u64>, Option<String>) {
        match self {
            Target::EditorId(id) => (None, Some(id.clone())),
            Target::Bits(b) => (Some(*b), None),
        }
    }
}

/// One edit the editor can send (and so undo or redo).
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    SetField { target: Target, component: String, field: String, value: Value },
    Spawn { components: Map<String, Value>, editor_id: Option<String> },
    Despawn { target: Target },
}

impl Op {
    pub fn to_request(&self) -> Request {
        match self {
            Op::SetField { target, component, field, value } => {
                let (entity, editor_id) = target.parts();
                Request::SetField { entity, editor_id, component: component.clone(), field: field.clone(), value: value.clone() }
            }
            Op::Spawn { components, editor_id } => Request::Spawn { components: components.clone(), editor_id: editor_id.clone() },
            Op::Despawn { target } => {
                let (entity, editor_id) = target.parts();
                Request::Despawn { entity, editor_id }
            }
        }
    }
}

/// A recorded edit: how to undo it and how to do it again.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub undo: Op,
    pub redo: Op,
}

/// The components a `despawn` reply recorded, as the `components` map of a `spawn`.
pub fn components_to_spawn(infos: &[ComponentInfo]) -> Map<String, Value> {
    infos.iter().map(|c| (c.name.clone(), Value::Object(c.fields.clone()))).collect()
}

/// The inverse of a request that succeeded with `reply`, or `None` when there is nothing to
/// undo (a failed request, a read, or a `set_field` that changed nothing).
pub fn inverse(request: &Request, reply: &Reply) -> Option<Entry> {
    match (request, reply) {
        (Request::SetField { entity, editor_id, component, field, value }, Reply::FieldSet { entity: bits, old }) => {
            if old == value {
                return None;
            }
            let target = Target::of(Some(*bits).or(*entity), editor_id.as_deref())?;
            Some(Entry {
                undo: Op::SetField { target: target.clone(), component: component.clone(), field: field.clone(), value: old.clone() },
                redo: Op::SetField { target, component: component.clone(), field: field.clone(), value: value.clone() },
            })
        }
        (Request::Spawn { components, .. }, Reply::Spawned { editor_id, .. }) => Some(Entry {
            undo: Op::Despawn { target: Target::EditorId(editor_id.clone()) },
            redo: Op::Spawn { components: components.clone(), editor_id: Some(editor_id.clone()) },
        }),
        (Request::Despawn { entity, editor_id }, Reply::Despawned { editor_id: had, components, .. }) => {
            let target = match had {
                Some(id) => Target::EditorId(id.clone()),
                None => Target::of(*entity, editor_id.as_deref())?,
            };
            Some(Entry {
                undo: Op::Spawn { components: components_to_spawn(components), editor_id: had.clone() },
                redo: Op::Despawn { target },
            })
        }
        _ => None,
    }
}

// ---- the undo stack ----

const MAX_HISTORY: usize = 200;

#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

impl UndoStack {
    /// Record a new edit; a new edit ends the redo history.
    pub fn push(&mut self, e: Entry) {
        self.redo.clear();
        self.undo.push(e);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
    }
    pub fn take_undo(&mut self) -> Option<Entry> {
        self.undo.pop()
    }
    pub fn take_redo(&mut self) -> Option<Entry> {
        self.redo.pop()
    }
    /// An entry whose undo succeeded becomes redoable.
    pub fn finish_undo(&mut self, e: Entry) {
        self.redo.push(e);
    }
    /// An entry whose redo succeeded becomes undoable again (the rest of the redo history stays).
    pub fn finish_redo(&mut self, e: Entry) {
        self.undo.push(e);
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
    /// The stack is cleared when play starts: play changes are rewound by Stop, not undone.
    pub fn on_mode_change(&mut self, mode: Mode) {
        if mode == Mode::Play {
            self.clear();
        }
    }
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

/// Why undo and redo are unavailable right now, if they are.
pub fn history_blocked(mode: Mode, connected: bool) -> Option<&'static str> {
    if !connected {
        Some("not connected")
    } else if mode == Mode::Play {
        Some("undo is disabled in play mode: Stop rewinds play changes")
    } else {
        None
    }
}

/// Whether a request changes the game (the worker must never merge or drop these).
pub fn is_mutation(r: &Request) -> bool {
    matches!(
        r,
        Request::SetField { .. } | Request::Spawn { .. } | Request::Despawn { .. } | Request::SetMode { .. } | Request::Stop { .. } | Request::SaveScene { .. } | Request::LoadScene { .. } | Request::RestartSnapshot { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(ty: &str, count: u32) -> FieldSpec {
        FieldSpec { name: "f".into(), ty: ty.into(), count }
    }

    #[test]
    fn schema_parses_and_kinds() {
        let doc = json!({"components": [
            {"name": "Door", "readonly": false, "fields": [
                {"name": "open", "type": "bool", "count": 1},
                {"name": "pos", "type": "f32", "count": 3}]},
            {"name": "GlobalTransform", "readonly": true, "fields": [{"name": "m", "type": "f32", "count": 1}]},
            {"bad": 1}]});
        let s = parse_schema(&doc);
        assert_eq!(s.len(), 2);
        let door = &s["Door"];
        assert_eq!(input_kind(door, &door.fields[0]), InputKind::Bool);
        assert_eq!(input_kind(door, &door.fields[1]), InputKind::Vector { ty: "f32".into(), count: 3 });
        assert!(matches!(input_kind(&s["GlobalTransform"], &s["GlobalTransform"].fields[0]), InputKind::ReadOnly(_)));
        let c = CompSpec::default();
        assert_eq!(input_kind(&c, &spec("entity", 1)), InputKind::Entity);
        assert_eq!(input_kind(&c, &spec("u8", 1)), InputKind::Number { ty: "u8".into() });
        assert!(matches!(input_kind(&c, &spec("bool", 2)), InputKind::ReadOnly(_)));
        assert!(matches!(input_kind(&c, &spec("string", 1)), InputKind::ReadOnly(n) if n.contains("unsupported")));
        assert!(matches!(input_kind(&c, &spec("f32", 99)), InputKind::ReadOnly(_)));
    }

    #[test]
    fn scalar_inputs_by_type() {
        assert_eq!(parse_scalar("f32", " 2.5 "), Ok(json!(2.5)));
        assert_eq!(parse_scalar("f64", "-1e3"), Ok(json!(-1000.0)));
        assert!(parse_scalar("f32", "abc").is_err());
        assert!(parse_scalar("f32", "NaN").is_err());
        assert!(parse_scalar("f32", "inf").is_err());
        assert!(parse_scalar("f32", "1e60").is_err());
        assert_eq!(parse_scalar("u8", "255"), Ok(json!(255)));
        assert!(parse_scalar("u8", "256").is_err());
        assert!(parse_scalar("u8", "-1").is_err());
        assert!(parse_scalar("u8", "1.5").is_err());
        assert_eq!(parse_scalar("i8", "-128"), Ok(json!(-128)));
        assert!(parse_scalar("i8", "128").is_err());
        assert_eq!(parse_scalar("i32", "-7"), Ok(json!(-7)));
        assert_eq!(parse_scalar("u64", "18446744073709551615"), Ok(json!(u64::MAX)));
        assert_eq!(parse_scalar("i64", "-9223372036854775808"), Ok(json!(i64::MIN)));
        assert!(parse_scalar("i64", "9223372036854775808").is_err());
        assert_eq!(parse_scalar("bool", "TRUE"), Ok(json!(true)));
        assert_eq!(parse_scalar("bool", "0"), Ok(json!(false)));
        assert!(parse_scalar("bool", "maybe").is_err());
        assert_eq!(parse_scalar("entity", "none"), Ok(json!(0)));
        assert_eq!(parse_scalar("entity", "123"), Ok(json!(123)));
        assert!(parse_scalar("entity", "x").is_err());
        assert!(parse_scalar("string", "x").is_err());
    }

    #[test]
    fn vectors_and_texts() {
        let p = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_vector("f32", 3, &p(&["1", "2.5", "-3"])), Ok(json!([1.0, 2.5, -3.0])));
        assert!(parse_vector("f32", 3, &p(&["1", "2"])).is_err());
        let e = parse_vector("u8", 2, &p(&["1", "300"])).unwrap_err();
        assert!(e.starts_with("[1]:"), "{e}");
        assert_eq!(edit_texts("f32", &json!(0.1f32 as f64)), vec!["0.1"]);
        assert_eq!(edit_texts("f32", &json!([1.0, 2.5])), vec!["1".to_string(), "2.5".to_string()]);
        assert_eq!(edit_texts("bool", &json!(true)), vec!["true"]);
        assert_eq!(edit_texts("u8", &json!(7)), vec!["7"]);
    }

    fn set_req(value: Value) -> Request {
        Request::SetField { entity: Some(4), editor_id: Some("9".into()), component: "Door".into(), field: "speed".into(), value }
    }

    #[test]
    fn inverse_of_set_field() {
        let e = inverse(&set_req(json!(2.0)), &Reply::FieldSet { entity: 4, old: json!(1.0) }).unwrap();
        assert_eq!(e.undo, Op::SetField { target: Target::EditorId("9".into()), component: "Door".into(), field: "speed".into(), value: json!(1.0) });
        assert_eq!(e.redo, Op::SetField { target: Target::EditorId("9".into()), component: "Door".into(), field: "speed".into(), value: json!(2.0) });
        // Without an editor id the raw bits are used.
        let r = Request::SetField { entity: Some(4), editor_id: None, component: "Door".into(), field: "speed".into(), value: json!(2.0) };
        let e = inverse(&r, &Reply::FieldSet { entity: 4, old: json!(1.0) }).unwrap();
        assert!(matches!(e.undo, Op::SetField { target: Target::Bits(4), .. }));
        // Nothing changed, or the request failed: nothing to record.
        assert!(inverse(&set_req(json!(1.0)), &Reply::FieldSet { entity: 4, old: json!(1.0) }).is_none());
        assert!(inverse(&set_req(json!(1.0)), &Reply::error("bad_value", "x")).is_none());
        assert!(inverse(&Request::Stats, &Reply::Stats { frame: 0, frame_time: 0.0, entity_count: 0 }).is_none());
    }

    #[test]
    fn inverse_of_spawn_and_despawn() {
        let comps = json!({"Door": {"speed": 1.0}}).as_object().unwrap().clone();
        let e = inverse(&Request::Spawn { components: comps.clone(), editor_id: None }, &Reply::Spawned { entity: 1, editor_id: "7".into() }).unwrap();
        assert_eq!(e.undo, Op::Despawn { target: Target::EditorId("7".into()) });
        assert_eq!(e.redo, Op::Spawn { components: comps, editor_id: Some("7".into()) });

        let mut fields = Map::new();
        fields.insert("speed".into(), json!(3.0));
        let infos = vec![ComponentInfo { name: "Door".into(), id: 1, fields }];
        let r = Request::Despawn { entity: Some(1), editor_id: None };
        let e = inverse(&r, &Reply::Despawned { owned: true, editor_id: Some("7".into()), components: infos.clone() }).unwrap();
        assert_eq!(e.undo, Op::Spawn { components: components_to_spawn(&infos), editor_id: Some("7".into()) });
        assert_eq!(e.redo, Op::Despawn { target: Target::EditorId("7".into()) });
        assert_eq!(components_to_spawn(&infos)["Door"], json!({"speed": 3.0}));
        let e = inverse(&r, &Reply::Despawned { owned: false, editor_id: None, components: vec![] }).unwrap();
        assert_eq!(e.redo, Op::Despawn { target: Target::Bits(1) });
    }

    #[test]
    fn ops_become_requests() {
        let op = Op::Despawn { target: Target::EditorId("3".into()) };
        assert_eq!(op.to_request(), Request::Despawn { entity: None, editor_id: Some("3".into()) });
        let op = Op::SetField { target: Target::Bits(8), component: "A".into(), field: "b".into(), value: json!(1) };
        assert_eq!(op.to_request(), Request::SetField { entity: Some(8), editor_id: None, component: "A".into(), field: "b".into(), value: json!(1) });
    }

    fn entry(n: u64) -> Entry {
        let op = |v| Op::SetField { target: Target::Bits(1), component: "A".into(), field: "f".into(), value: json!(v) };
        Entry { undo: op(n), redo: op(n + 1) }
    }

    #[test]
    fn stack_push_undo_redo_clear() {
        let mut s = UndoStack::default();
        assert!(s.take_undo().is_none() && s.take_redo().is_none());
        s.push(entry(1));
        s.push(entry(2));
        assert_eq!(s.undo_len(), 2);
        let e = s.take_undo().unwrap();
        assert_eq!(e, entry(2));
        s.finish_undo(e);
        assert_eq!((s.undo_len(), s.redo_len()), (1, 1));
        let e = s.take_redo().unwrap();
        s.finish_redo(e);
        assert_eq!((s.undo_len(), s.redo_len()), (2, 0));
        // A new edit ends the redo history.
        let e = s.take_undo().unwrap();
        s.finish_undo(e);
        s.push(entry(3));
        assert_eq!(s.redo_len(), 0);
        s.clear();
        assert_eq!((s.undo_len(), s.redo_len()), (0, 0));
    }

    #[test]
    fn stack_clears_on_play_only_and_is_bounded() {
        let mut s = UndoStack::default();
        s.push(entry(1));
        s.on_mode_change(Mode::Edit);
        assert_eq!(s.undo_len(), 1);
        s.on_mode_change(Mode::Play);
        assert_eq!(s.undo_len(), 0);
        for i in 0..(MAX_HISTORY as u64 + 20) {
            s.push(entry(i));
        }
        assert_eq!(s.undo_len(), MAX_HISTORY);
    }

    #[test]
    fn blocked_reasons() {
        assert_eq!(history_blocked(Mode::Edit, true), None);
        assert!(history_blocked(Mode::Play, true).unwrap().contains("play mode"));
        assert_eq!(history_blocked(Mode::Edit, false), Some("not connected"));
    }

    #[test]
    fn mutations_are_never_merged() {
        assert!(is_mutation(&set_req(json!(1))));
        assert!(!is_mutation(&Request::Stats));
    }
}
