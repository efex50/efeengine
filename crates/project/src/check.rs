use std::fmt;
use std::path::Path;

use crate::Project;

/// One problem with a project. `field` names the property or item (`Package`, `StartScene`,
/// `Scene`, `Mod`, `Cargo.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckIssue {
    pub field: String,
    pub message: String,
}

impl fmt::Display for CheckIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

fn issue(field: &str, message: String) -> CheckIssue {
    CheckIssue { field: field.to_string(), message }
}

/// Check the project in `project_dir`: finds its `*.efproj` there, parses it (a missing or
/// bad file is one issue, field `.efproj`) and runs [`check_project`].
pub fn check(project_dir: &Path) -> Vec<CheckIssue> {
    let mut files: Vec<_> = match std::fs::read_dir(project_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "efproj"))
            .collect(),
        Err(e) => return vec![issue(".efproj", format!("cannot read folder `{}`: {e}", project_dir.display()))],
    };
    files.sort();
    let Some(file) = files.into_iter().next() else {
        return vec![issue(".efproj", format!("no .efproj file in `{}`", project_dir.display()))];
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) => return vec![issue(".efproj", format!("{}: {e}", file.display()))],
    };
    match crate::parse(&text) {
        Ok(p) => check_project(&p, project_dir),
        Err(e) => vec![issue(".efproj", format!("{}: {e}", file.display()))],
    }
}

/// Check an already parsed project against its folder. Returns every problem.
pub fn check_project(project: &Project, project_dir: &Path) -> Vec<CheckIssue> {
    let mut out = Vec::new();
    let cargo = project_dir.join("Cargo.toml");
    match std::fs::read_to_string(&cargo) {
        Err(_) => out.push(issue("Cargo.toml", format!("`{}` does not exist or cannot be read", cargo.display()))),
        Ok(text) => match text.parse::<toml::Table>() {
            Err(e) => out.push(issue("Cargo.toml", format!("cannot parse: {}", e.message()))),
            Ok(t) => match t.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) {
                None => out.push(issue("Package", format!("Cargo.toml has no [package] name, so `{}` names no package", project.package))),
                Some(n) if n != project.package => out.push(issue(
                    "Package",
                    format!("`{}` does not match the Cargo.toml package `{n}`", project.package),
                )),
                _ => {}
            },
        },
    }
    if !project.scenes.contains(&project.start_scene) {
        out.push(issue("StartScene", format!("`{}` is not in the Scene list", project.start_scene)));
    }
    for s in &project.scenes {
        if !project_dir.join(s).is_file() {
            out.push(issue("Scene", format!("file `{s}` does not exist")));
        }
    }
    for m in &project.mods {
        if !project_dir.join(m).is_file() {
            out.push(issue("Mod", format!("file `{m}` does not exist")));
        }
    }
    out
}
