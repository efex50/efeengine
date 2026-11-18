//! Gizmo editing (docs/EDITOR.md E7.2), editor side: which `Transform` field each gizmo mode
//! edits and the nudge arithmetic. The handles themselves are drawn by the game; the editor has
//! no mouse dragging in the game window (that needs input ownership, deferred), so it offers
//! numeric fields and nudge buttons. Every nudge is one `set_field`, so it is one undo step.

use efeengine_editor_link::protocol::GizmoMode;
use serde_json::{Value, json};

/// The nudge steps shown as buttons: each as a negative and a positive press.
pub const STEPS: [f64; 2] = [0.1, 1.0];

pub const MODES: [GizmoMode; 3] = [GizmoMode::Move, GizmoMode::Rotate, GizmoMode::Scale];

pub fn label(m: GizmoMode) -> &'static str {
    match m {
        GizmoMode::Move => "Move",
        GizmoMode::Rotate => "Rotate",
        GizmoMode::Scale => "Scale",
    }
}

/// The `Transform` field the mode edits.
pub fn field(m: GizmoMode) -> &'static str {
    match m {
        GizmoMode::Move => "position",
        GizmoMode::Rotate => "rotation",
        GizmoMode::Scale => "scale",
    }
}

/// Axis names of a field value: X, Y (and Z when the value has three components). Rotation is
/// one scalar (radians), so it has a single unnamed axis.
pub fn axes(value: &Value) -> Vec<&'static str> {
    match value {
        Value::Array(a) => ["X", "Y", "Z", "W"].into_iter().take(a.len()).collect(),
        _ => vec![""],
    }
}

/// `value` with `delta` added on `axis` (ignored for a scalar), as the JSON to send in
/// `set_field`. Rounded to 6 decimals so f32 noise (0.1 stored as 0.10000000149) does not pile up.
pub fn nudge(value: &Value, axis: usize, delta: f64) -> Result<Value, String> {
    let round = |x: f64| (x * 1e6).round() / 1e6;
    let num = |v: &Value| v.as_f64().ok_or_else(|| format!("{v} is not a number"));
    match value {
        Value::Array(a) => {
            if axis >= a.len() {
                return Err(format!("no axis {axis} in a value of {} components", a.len()));
            }
            let mut out = Vec::with_capacity(a.len());
            for (i, v) in a.iter().enumerate() {
                let x = num(v)?;
                out.push(json!(round(if i == axis { x + delta } else { x })));
            }
            Ok(Value::Array(out))
        }
        v => Ok(json!(round(num(v)? + delta))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{self, Op, Target, UndoStack};
    use efeengine_editor_link::protocol::{Reply, Request};

    #[test]
    fn modes_map_to_transform_fields() {
        assert_eq!(MODES.map(field), ["position", "rotation", "scale"]);
        assert_eq!(MODES.map(label), ["Move", "Rotate", "Scale"]);
    }

    #[test]
    fn nudges_one_axis_and_rounds() {
        assert_eq!(nudge(&json!([1.0, 2.0]), 0, 0.1), Ok(json!([1.1, 2.0])));
        assert_eq!(nudge(&json!([1.0, 2.0]), 1, -1.0), Ok(json!([1.0, 1.0])));
        assert_eq!(nudge(&json!([0.10000000149, 0.0, 5.0]), 2, 1.0), Ok(json!([0.1, 0.0, 6.0])), "z is nudged when present");
        assert_eq!(nudge(&json!(0.5), 0, -0.1), Ok(json!(0.4)));
        assert!(nudge(&json!([1.0, 2.0]), 2, 1.0).is_err());
        assert!(nudge(&json!("x"), 0, 1.0).is_err());
        assert_eq!(axes(&json!([1.0, 2.0])), ["X", "Y"]);
        assert_eq!(axes(&json!([1.0, 2.0, 3.0])), ["X", "Y", "Z"]);
        assert_eq!(axes(&json!(0.0)), [""]);
    }

    /// Run a nudge the way the editor does: build the `set_field`, take the game's reply, record
    /// the inverse.
    fn do_nudge(stack: &mut UndoStack, current: &mut Value, mode: GizmoMode, axis: usize, delta: f64) {
        let new = nudge(current, axis, delta).unwrap();
        let op = Op::SetField { target: Target::EditorId("7".into()), component: "Transform".into(), field: field(mode).into(), value: new.clone() };
        let req = op.to_request();
        assert!(matches!(req, Request::SetField { ref component, .. } if component == "Transform"));
        let reply = Reply::FieldSet { entity: 1, old: current.clone() };
        stack.push(edit::inverse(&req, &reply).expect("a changed value is undoable"));
        *current = new;
    }

    #[test]
    fn each_nudge_is_one_undo_step() {
        let mut stack = UndoStack::default();
        let mut pos = json!([0.0, 0.0]);
        do_nudge(&mut stack, &mut pos, GizmoMode::Move, 0, 1.0);
        do_nudge(&mut stack, &mut pos, GizmoMode::Move, 1, 0.1);
        do_nudge(&mut stack, &mut pos, GizmoMode::Move, 0, -0.1);
        assert_eq!(stack.undo_len(), 3, "one entry per press");
        assert_eq!(pos, json!([0.9, 0.1]));
        // One undo reverts exactly the last press.
        let e = stack.take_undo().unwrap();
        assert_eq!(stack.undo_len(), 2);
        match e.undo {
            Op::SetField { value, field, .. } => {
                assert_eq!(field, "position");
                assert_eq!(value, json!([1.0, 0.1]));
            }
            o => panic!("{o:?}"),
        }
        // Rotate and scale nudges are single set_field edits on their own fields too.
        let mut rot = json!(0.0);
        do_nudge(&mut stack, &mut rot, GizmoMode::Rotate, 0, 0.1);
        assert_eq!(stack.undo_len(), 3);
        let mut sc = json!([1.0, 1.0]);
        do_nudge(&mut stack, &mut sc, GizmoMode::Scale, 1, 1.0);
        assert_eq!(sc, json!([1.0, 2.0]));
        assert_eq!(stack.undo_len(), 4);
    }
}
