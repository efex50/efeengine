//! Component schema export (ROADMAP 7.2, docs/EDITOR.md "Schema and inspectors").
//!
//! The editor builds inspectors from this file without linking game code. `EFE_API_EXPORT=<path>`
//! writes the script API index to `<path>` and the schema next to it, in `<path>` with the
//! extension `efschema.json` (`game.efapi` gives `game.efschema.json`).
//!
//! Format (JSON, `"format": "efe-schema"`, `"version": 1`):
//!
//! ```json
//! {
//!   "format": "efe-schema",
//!   "version": 1,
//!   "game": "my_game",
//!   "components": [
//!     {
//!       "name": "Transform", "id": 123456, "owner": "engine",
//!       "size": 40, "align": 4, "readonly": false, "dynamic": false, "doc": "...",
//!       "fields": [
//!         { "name": "position", "type": "f32", "count": 2, "offset": 0, "size": 8, "doc": "..." }
//!       ]
//!     }
//!   ],
//!   "structs": [ ]
//! }
//! ```
//!
//! - `components` are the script-visible components in registration order; `structs` are the
//!   layout-only types (used in signatures, no component behind them), same shape.
//! - `type` is a primitive name: `u8 i8 u16 i16 u32 i32 u64 i64 f32 f64 bool entity`.
//!   `count` is 1 for a scalar and N for an array or vector (`Vec3` is `f32` x 3). `offset` and
//!   `size` are in bytes inside the component; `size` of the component includes padding.
//! - `owner` is `engine`, the game name or `mod:<mod id>` for a component a mod defines
//!   (`dynamic` is `true` for those).
//! - Rust-only components (no `ScriptType`) are not listed, and neither are resources: resources
//!   have no byte layout.

use serde_json::{json, Value};

use super::Engine;
use super::types::TypeEntry;

/// The file name of the schema written next to the API index at `api_path`.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn schema_path(api_path: &std::path::Path) -> std::path::PathBuf {
    api_path.with_extension("efschema.json")
}

fn type_json(t: &TypeEntry) -> Value {
    let d = t.desc();
    let fields: Vec<Value> = d
        .fields
        .iter()
        .map(|f| json!({
            "name": f.name,
            "type": f.ty.name(),
            "count": f.count,
            "offset": f.offset,
            "size": f.size(),
            "doc": f.doc,
        }))
        .collect();
    json!({
        "name": d.name,
        "id": t.id(),
        "owner": d.owner,
        "size": d.size,
        "align": d.align,
        "readonly": d.readonly,
        "dynamic": t.is_dynamic(),
        "doc": d.doc,
        "fields": fields,
    })
}

impl Engine {
    /// The component schema as a JSON value (see the module docs for the format).
    pub fn schema(&self) -> Value {
        let (mut components, mut structs) = (Vec::new(), Vec::new());
        for t in self.state.types.entries() {
            if t.is_component() {
                components.push(type_json(t));
            } else {
                structs.push(type_json(t));
            }
        }
        json!({
            "format": "efe-schema",
            "version": 1,
            "game": self.api_index().game,
            "components": components,
            "structs": structs,
        })
    }

    /// [`schema`](Self::schema) as pretty-printed JSON text.
    pub fn export_schema(&self) -> String {
        serde_json::to_string_pretty(&self.schema()).expect("a json value serializes")
    }
}
