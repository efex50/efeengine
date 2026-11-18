//! The `.efproj` project file (docs/PROJECT_FILE.md): parse, write, find, check, scaffold.
//! No dependency on the engine or the editor.

mod check;
mod create;
mod format;

pub use check::{CheckIssue, check, check_project};
pub use create::{ENGINE_GIT_URL, create};
pub use format::{parse, write};

use std::fmt;
use std::path::{Path, PathBuf};

/// The `Sdk` value: the format version of the file.
pub const SDK: &str = "efeengine/1";
/// The engine format version this crate accepts for `EngineVersion`.
pub const ENGINE_VERSION: u32 = 1;

/// A parsed project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub package: String,
    /// When set, the project builds `examples/<example>` and not the package root.
    pub example: Option<String>,
    pub start_scene: String,
    pub engine_version: u32,
    pub scenes: Vec<String>,
    pub mods: Vec<String>,
}

/// Why a project file could not be read or created.
#[derive(Debug)]
pub enum ProjectError {
    /// The text is not a valid project file. `line` is 1-based.
    Parse { line: usize, msg: String },
    /// A name or path argument is not allowed.
    Invalid(String),
    /// The scaffold target exists and is not empty.
    NotEmpty(PathBuf),
    Io { path: PathBuf, source: std::io::Error },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Parse { line, msg } => write!(f, "line {line}: {msg}"),
            ProjectError::Invalid(m) => write!(f, "{m}"),
            ProjectError::NotEmpty(p) => write!(f, "folder `{}` exists and is not empty", p.display()),
            ProjectError::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProjectError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Walk up from `from` (a file or folder) and return the nearest `*.efproj` file. In one folder
/// the first by name wins.
pub fn find(from: &Path) -> Option<PathBuf> {
    let mut dir = if from.is_dir() { Some(from) } else { from.parent() };
    while let Some(d) = dir {
        if let Ok(rd) = std::fs::read_dir(d) {
            let mut hits: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "efproj"))
                .collect();
            hits.sort();
            if let Some(h) = hits.into_iter().next() {
                return Some(h);
            }
        }
        dir = d.parent();
    }
    None
}

/// True if `p` is a relative path that stays inside the project folder.
pub(crate) fn path_problem(p: &str) -> Option<&'static str> {
    if p.is_empty() {
        return Some("path is empty");
    }
    let b = p.as_bytes();
    if p.starts_with('/') || p.starts_with('\\') || (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()) {
        return Some("absolute paths are not allowed");
    }
    if p.split(['/', '\\']).any(|c| c == "..") {
        return Some("`..` is not allowed");
    }
    None
}
