//! `ApiIndex`: a snapshot of a `HostRegistry` as plain data, and the `.efapi` file
//! (crates/efevm/API_INDEX.md). Writing and reading live in `json.rs`.

use crate::api::types::{ApiType, HostFn, NamespaceInfo, StructDesc, fnv1a64, is_ident, is_namespace};
use std::collections::HashSet;
use std::fmt;

/// Everything a script can call or see: namespaces, struct layouts and host functions.
///
/// [`HostRegistry::index`](crate::api::HostRegistry::index) produces it sorted (namespaces
/// and types by name, functions by `(namespace, name)`), and [`ApiIndex::from_json`] sorts
/// what it reads the same way, so `from_json(&x.to_json()) == x` for every registry index.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ApiIndex {
    /// Informational; not part of the hash.
    pub game: String,
    pub namespaces: Vec<NamespaceInfo>,
    pub types: Vec<StructDesc>,
    pub functions: Vec<HostFn>,
}

/// Why an index file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// Not well-formed JSON. `offset` is a byte offset into the text.
    Syntax { offset: usize, message: String },
    /// A required key is missing or has the wrong JSON type (`path` like `functions[3].name`).
    Schema { path: String, message: String },
    /// `format` is not `"efapi"`.
    Format(String),
    /// `version` is not a version this reader supports (only `1`).
    Version(String),
    /// The `hash` key does not match the content.
    Hash { file: String, computed: u64 },
    /// A function's `signature` does not match its `params`/`results`.
    Signature { function: String, file: String, computed: String },
    /// A name, type or layout rule is broken (API_INDEX.md).
    Invalid(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Syntax { offset, message } => write!(f, "JSON syntax error at byte {offset}: {message}"),
            ApiError::Schema { path, message } => write!(f, "{path}: {message}"),
            ApiError::Format(s) => write!(f, "not an API index (format {s:?}, expected \"efapi\")"),
            ApiError::Version(v) => write!(f, "unsupported API index version {v} (this reader supports 1)"),
            ApiError::Hash { file, computed } => {
                write!(f, "hash mismatch: file says {file}, content is fnv1a64:{computed:016x} (edited or truncated?)")
            }
            ApiError::Signature { function, file, computed } => {
                write!(f, "{function}: signature {file:?} does not match the parameters ({computed})")
            }
            ApiError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ApiError {}

impl ApiIndex {
    /// The `.efapi` text: keys in the documented order, sorted entries, two-space
    /// indentation, `\n` line ends and a final newline.
    pub fn to_json(&self) -> String {
        crate::api::json::write(self, Some(self.hash()))
    }

    /// 64-bit FNV-1a over the canonical body (the file without its `game` and `hash` lines).
    pub fn hash(&self) -> u64 {
        fnv1a64(crate::api::json::write(self, None).as_bytes())
    }

    /// Parses and checks an index file: `format`, `version`, `hash`, every `signature`, and
    /// the rules of [`ApiIndex::validate`]. Unknown keys are ignored. Never panics.
    pub fn from_json(text: &str) -> Result<ApiIndex, ApiError> {
        crate::api::json::read(text)
    }

    /// Sorts namespaces and types by name and functions by `(namespace, name)`.
    pub fn sort(&mut self) {
        self.namespaces.sort_by(|a, b| a.name.cmp(&b.name));
        self.types.sort_by(|a, b| a.name.cmp(&b.name));
        self.functions.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    }

    /// Checks the API_INDEX.md rules: valid and unique namespace, struct, field, function and
    /// parameter names; valid struct layouts with unique component ids; every function in a
    /// listed namespace; `str`/`bytes`/`&S` only as parameters; every `&S` a listed struct.
    pub fn validate(&self) -> Result<(), ApiError> {
        let bad = |m: String| Err(ApiError::Invalid(m));
        let mut seen = HashSet::new();
        for n in &self.namespaces {
            if !is_namespace(&n.name) {
                return bad(format!("namespace `{}` is not a valid name", n.name));
            }
            if !seen.insert(n.name.as_str()) {
                return bad(format!("duplicate namespace `{}`", n.name));
            }
        }
        let mut names = HashSet::new();
        let mut ids = HashSet::new();
        for t in &self.types {
            t.validate().map_err(ApiError::Invalid)?;
            if !names.insert(t.name.as_str()) {
                return bad(format!("duplicate struct `{}`", t.name));
            }
            if let Some(id) = t.component
                && !ids.insert(id)
            {
                return bad(format!("struct `{}`: duplicate component id {id}", t.name));
            }
        }
        let mut fns = HashSet::new();
        for f in &self.functions {
            let q = f.qualified();
            if !seen.contains(f.namespace.as_str()) {
                return bad(format!("{q}: namespace `{}` is not listed", f.namespace));
            }
            if !is_ident(&f.name) {
                return bad(format!("{q}: function name is not an identifier"));
            }
            if !fns.insert((f.namespace.as_str(), f.name.as_str())) {
                return bad(format!("duplicate function {q}"));
            }
            let all: Vec<_> = f.params.iter().chain(&f.results).collect();
            for (i, p) in all.iter().enumerate() {
                if !is_ident(&p.name) {
                    return bad(format!("{q}: `{}` is not an identifier", p.name));
                }
                if all[..i].iter().any(|o| o.name == p.name) {
                    return bad(format!("{q}: duplicate parameter or result `{}`", p.name));
                }
                if let ApiType::Struct(s) = &p.ty
                    && !names.contains(s.as_str())
                {
                    return bad(format!("{q}: struct `{s}` is not listed"));
                }
            }
            if let Some(r) = f.results.iter().find(|r| !r.ty.is_scalar()) {
                return bad(format!("{q}: result `{}` must be a scalar type", r.name));
            }
        }
        Ok(())
    }

    pub fn function(&self, namespace: &str, name: &str) -> Option<&HostFn> {
        self.functions.iter().find(|f| f.namespace == namespace && f.name == name)
    }

    /// `namespace.name` of every function, in index order.
    pub fn qualified_names(&self) -> Vec<String> {
        self.functions.iter().map(HostFn::qualified).collect()
    }

    pub fn struct_type(&self, name: &str) -> Option<&StructDesc> {
        self.types.iter().find(|t| t.name == name)
    }

    pub fn namespace(&self, name: &str) -> Option<&NamespaceInfo> {
        self.namespaces.iter().find(|n| n.name == name)
    }
}
