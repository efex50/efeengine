//! Where the files of a multi-file program come from (efa-syntax 16.1). The assembler never
//! touches the file system itself: it reads through a [`FileSource`].

use std::collections::BTreeMap;

/// Reads the files of a program by their path relative to the source's root, with `/` as the
/// separator and no `.` or `..` segments (the assembler normalises `use` paths first).
pub trait FileSource {
    /// The text of the file at `path`, or a message saying why it cannot be read.
    fn read(&self, path: &str) -> Result<String, String>;
    /// The name of the file at `path` in diagnostics and in the lines section. The default
    /// is the path itself.
    fn display_name(&self, path: &str) -> String {
        path.to_string()
    }
}

/// Files held in memory, keyed by path (tests, language-server buffers, web tools).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemSource(pub BTreeMap<String, String>);

impl MemSource {
    pub fn new() -> MemSource {
        MemSource(BTreeMap::new())
    }

    /// Adds (or replaces) a file; returns `self` for chaining.
    pub fn with(mut self, path: &str, text: &str) -> MemSource {
        self.0.insert(path.to_string(), text.to_string());
        self
    }
}

impl FileSource for MemSource {
    fn read(&self, path: &str) -> Result<String, String> {
        self.0.get(path).cloned().ok_or_else(|| "no such file".to_string())
    }
}

/// The largest source file [`FsSource`] reads.
#[cfg(not(target_family = "wasm"))]
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Files below a directory of the native file system. Paths are joined to the root,
/// canonicalised, and refused when they lead outside the root (also through a symbolic
/// link); only regular files up to [`MAX_FILE_BYTES`] are read.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Debug)]
pub struct FsSource {
    root: std::path::PathBuf,
    /// Prefix of the display names (the root directory as the user wrote it); empty for none.
    prefix: String,
}

#[cfg(not(target_family = "wasm"))]
impl FsSource {
    /// Files below `root`; display names are the paths relative to it.
    pub fn new(root: impl Into<std::path::PathBuf>) -> FsSource {
        FsSource { root: root.into(), prefix: String::new() }
    }

    /// The source rooted at the directory of `file` (a path as the user typed it), and the
    /// file's name relative to it. Display names keep the directory as typed, so the root
    /// file is shown (and recorded in the lines section) exactly as `file`.
    pub fn for_file(file: &str) -> (FsSource, String) {
        match file.rsplit_once('/') {
            Some((dir, name)) => {
                let dir = if dir.is_empty() { "/" } else { dir };
                (FsSource { root: dir.into(), prefix: dir.to_string() }, name.to_string())
            }
            None => (FsSource { root: ".".into(), prefix: String::new() }, file.to_string()),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl FileSource for FsSource {
    fn read(&self, path: &str) -> Result<String, String> {
        use std::path::{Component, Path};
        let rel = Path::new(path);
        if path.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err("the path must be relative and must not contain `.` or `..`".to_string());
        }
        let root = self.root.canonicalize().map_err(|e| format!("cannot open the root directory: {e}"))?;
        let full = root.join(rel).canonicalize().map_err(|e| e.to_string())?;
        if !full.starts_with(&root) {
            return Err("it leads outside the root directory (through a symbolic link)".to_string());
        }
        let md = std::fs::metadata(&full).map_err(|e| e.to_string())?;
        if !md.is_file() {
            return Err("not a regular file".to_string());
        }
        if md.len() > MAX_FILE_BYTES {
            return Err(format!("the file is larger than {MAX_FILE_BYTES} bytes"));
        }
        let bytes = std::fs::read(&full).map_err(|e| e.to_string())?;
        String::from_utf8(bytes).map_err(|_| "not valid UTF-8".to_string())
    }

    fn display_name(&self, path: &str) -> String {
        if self.prefix.is_empty() {
            path.to_string()
        } else if self.prefix.ends_with('/') {
            format!("{}{path}", self.prefix)
        } else {
            format!("{}/{path}", self.prefix)
        }
    }
}
