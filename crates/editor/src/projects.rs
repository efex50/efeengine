//! Opening and deleting projects (docs/EDITOR.md E9.4, E9.5). Pure file logic over
//! `efeengine-project`; the pages in `main.rs` only show the results.

use std::path::{Path, PathBuf};

use efeengine_project::Project;

/// A project that passed the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenProject {
    pub file: PathBuf,
    pub dir: PathBuf,
    pub project: Project,
}

/// Read and check the project file. Any problem is a list of messages and nothing opens.
pub fn load_project(file: &Path) -> Result<OpenProject, Vec<String>> {
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let text = std::fs::read_to_string(file).map_err(|e| vec![format!("the project file cannot be read: {e}")])?;
    let project = efeengine_project::parse(&text).map_err(|e| vec![format!("the project file is not valid: {e}")])?;
    let issues = efeengine_project::check_project(&project, &dir);
    if issues.is_empty() { Ok(OpenProject { file: file.to_path_buf(), dir, project }) } else { Err(issues.iter().map(|i| i.to_string()).collect()) }
}

/// One project found in the projects folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub file: PathBuf,
    /// The project name, or the folder name when the project file cannot be read.
    pub name: String,
    /// Empty when the project is fine; else what needs attention.
    pub issues: Vec<String>,
}

/// The projects that are direct sub-folders of `root` with a `.efproj` file, sorted by name.
/// A missing `root` gives an empty list. Folders without a `.efproj` are not projects.
pub fn list_projects(root: &Path) -> Vec<Listed> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<Listed> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let dir = e.path();
            let file = efeengine_project::find(&dir).filter(|f| f.parent() == Some(dir.as_path()))?;
            let folder = e.file_name().to_string_lossy().into_owned();
            Some(match load_project(&file) {
                Ok(p) => Listed { file, name: p.project.name, issues: Vec::new() },
                Err(issues) => Listed { file, name: project_name_or(&folder, &dir), issues },
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn project_name_or(folder: &str, dir: &Path) -> String {
    efeengine_project::find(dir).and_then(|f| project_name(&f)).unwrap_or_else(|| folder.to_string())
}

/// `1.5 MB` style size for the delete page.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} bytes")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// The project name of a `.efproj` file, for lists; `None` when it is missing or unreadable.
pub fn project_name(file: &Path) -> Option<String> {
    efeengine_project::parse(&std::fs::read_to_string(file).ok()?).ok().map(|p| p.name)
}

pub const SIZE_WALK_CAP: usize = 100_000;

/// Total bytes of the files under `dir` (symlinks are counted as links, not followed) and
/// whether the walk stopped at `cap` entries.
pub fn dir_size(dir: &Path, cap: usize) -> (u64, bool) {
    let (mut bytes, mut seen) = (0u64, 0usize);
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            if seen >= cap {
                return (bytes, true);
            }
            seen += 1;
            let Ok(md) = e.path().symlink_metadata() else { continue };
            if md.is_dir() {
                todo.push(e.path());
            } else {
                bytes += md.len();
            }
        }
    }
    (bytes, false)
}

/// Delete the project folder `dir`, after every check. `game_running`: a game started from this
/// editor session is running (the editor cannot tell which project it belongs to, so any game
/// blocks deletion).
pub fn delete_project(dir: &Path, typed_name: &str, game_running: bool) -> Result<(), String> {
    if game_running {
        return Err("a game is running in this editor session: stop it first".into());
    }
    let md = std::fs::symlink_metadata(dir).map_err(|e| format!("the project folder cannot be read: {e}"))?;
    if md.file_type().is_symlink() {
        return Err("the project folder is a symbolic link: refusing to delete it".to_string());
    }
    if !md.is_dir() {
        return Err("the project is not a folder".to_string());
    }
    if dir.parent().is_none() {
        return Err("refusing to delete a root folder".into());
    }
    let file = efeengine_project::find(dir).filter(|f| f.parent() == Some(dir)).ok_or_else(|| "the folder does not directly contain a .efproj file: refusing to delete it".to_string())?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("the project file cannot be read: {e}"))?;
    let project = efeengine_project::parse(&text).map_err(|e| format!("not a valid project file ({e}): refusing to delete"))?;
    if typed_name != project.name {
        return Err(format!("type the project name `{}` exactly to confirm", project.name));
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("could not delete the project: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("efe-editor-projects-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make(parent: &Path, name: &str) -> PathBuf {
        efeengine_project::create(parent, name).unwrap()
    }

    #[test]
    fn delete_refused_without_matching_name() {
        let t = temp("name");
        let f = make(&t, "demo");
        let dir = f.parent().unwrap();
        assert!(delete_project(dir, "", false).is_err());
        assert!(delete_project(dir, "Demo", false).is_err());
        assert!(delete_project(dir, "demo ", false).is_err());
        assert!(dir.join("demo.efproj").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn delete_refused_while_a_game_runs() {
        let t = temp("running");
        let f = make(&t, "demo");
        let e = delete_project(f.parent().unwrap(), "demo", true).unwrap_err();
        assert!(e.contains("running"), "{e}");
        assert!(f.exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn delete_refused_without_efproj() {
        let t = temp("noproj");
        let dir = t.join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.txt"), "x").unwrap();
        assert!(delete_project(&dir, "plain", false).unwrap_err().contains("efproj"));
        // A project further up does not count: the .efproj must be directly in the folder.
        make(&t, "outer");
        let inner = t.join("outer").join("src");
        assert!(delete_project(&inner, "outer", false).is_err());
        assert!(inner.exists() && dir.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn delete_refused_for_an_invalid_efproj() {
        let t = temp("badproj");
        std::fs::write(t.join("x.efproj"), "garbage").unwrap();
        assert!(delete_project(&t, "x", false).is_err());
        assert!(t.join("x.efproj").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[cfg(unix)]
    #[test]
    fn delete_refused_for_a_symlink() {
        let t = temp("link");
        let f = make(&t, "demo");
        let real = f.parent().unwrap().to_path_buf();
        let link = t.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = delete_project(&link, "demo", false).unwrap_err();
        assert!(e.contains("symbolic link"), "{e}");
        assert!(f.exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn delete_succeeds_on_a_temp_project() {
        let t = temp("ok");
        let f = make(&t, "demo");
        let dir = f.parent().unwrap().to_path_buf();
        let sibling = t.join("other");
        std::fs::create_dir_all(&sibling).unwrap();
        delete_project(&dir, "demo", false).unwrap();
        assert!(!dir.exists());
        assert!(sibling.exists(), "only the project folder goes");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn size_walk_counts_and_caps() {
        let t = temp("size");
        let f = make(&t, "demo");
        let (bytes, capped) = dir_size(f.parent().unwrap(), SIZE_WALK_CAP);
        assert!(bytes > 0 && !capped);
        let (_, capped) = dir_size(f.parent().unwrap(), 2);
        assert!(capped);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn listing_finds_valid_and_marks_broken() {
        let t = temp("list");
        make(&t, "good");
        let bad = make(&t, "broken");
        std::fs::remove_file(bad.parent().unwrap().join("scenes/start.ron")).unwrap();
        std::fs::create_dir_all(t.join("plain")).unwrap();
        std::fs::write(t.join("file.txt"), "x").unwrap();
        let garbled = t.join("garbled");
        std::fs::create_dir_all(&garbled).unwrap();
        std::fs::write(garbled.join("g.efproj"), "nonsense").unwrap();
        let l = list_projects(&t);
        let names: Vec<_> = l.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["broken", "garbled", "good"]);
        assert!(l[2].issues.is_empty());
        assert!(l[0].issues.iter().any(|i| i.contains("Scene")), "{:?}", l[0].issues);
        assert!(!l[1].issues.is_empty());
        assert!(list_projects(&t.join("nothing")).is_empty());
        // Opening a listed good project works.
        assert!(load_project(&l[2].file).is_ok());
        assert!(load_project(&l[0].file).is_err());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(10), "10 bytes");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}
