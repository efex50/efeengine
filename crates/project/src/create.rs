use std::fs;
use std::path::{Path, PathBuf};

use crate::{ENGINE_VERSION, Project, ProjectError, write};

fn io(path: &Path, source: std::io::Error) -> ProjectError {
    ProjectError::Io { path: path.to_path_buf(), source }
}

fn valid_name(name: &str) -> Result<(), ProjectError> {
    let bad = |m: &str| Err(ProjectError::Invalid(format!("project name `{name}`: {m}")));
    if name.is_empty() {
        return bad("must not be empty");
    }
    if name.len() > 64 {
        return bad("must be at most 64 characters");
    }
    let mut ch = name.chars();
    if !ch.next().is_some_and(|c| c.is_ascii_lowercase()) {
        return bad("must start with a lowercase letter a-z");
    }
    if !ch.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return bad("may contain only a-z, 0-9 and _");
    }
    Ok(())
}

const MAIN_RS: &str = r#"use efeengine_core::prelude::*;

fn main() {
    match Engine::new("NAME") {
        Ok(engine) => engine.game_loop(),
        Err(e) => eprintln!("engine init failed: {e}"),
    }
}
"#;

const SCENE_RON: &str = "Scene(\n    format: 1,\n    name: \"start\",\n    requires: [],\n    systems: [],\n    entities: [],\n)\n";

/// The engine repository new projects depend on.
pub const ENGINE_GIT_URL: &str = "https://github.com/efex50/efeengine.git";

/// The git selector for the dependency: `rev = "<commit>"` from the build script (or the
/// `EFE_ENGINE_REV` environment variable at run time), else the `main` branch.
fn engine_selector(rev: Option<&str>) -> String {
    match rev.filter(|r| !r.is_empty()) {
        Some(r) => format!("rev = \"{r}\""),
        // git was not available when the editor was built, so no commit is known.
        None => "branch = \"main\"".to_string(),
    }
}

fn pinned_rev() -> Option<String> {
    std::env::var("EFE_ENGINE_REV").ok().filter(|s| !s.is_empty()).or_else(|| option_env!("EFE_ENGINE_REV").map(String::from))
}

/// Create `<parent>/<name>/` with `<name>.efproj`, `Cargo.toml`, `src/main.rs` and
/// `scenes/start.ron`, and nothing else. The game depends on `efeengine-core` from
/// [`ENGINE_GIT_URL`], pinned to the commit the build script found. Refuses an existing
/// folder that is not empty. Returns the path of the `.efproj` file.
pub fn create(parent: &Path, name: &str) -> Result<PathBuf, ProjectError> {
    valid_name(name)?;
    let dir = parent.join(name);
    if dir.exists() {
        let empty = dir.is_dir() && fs::read_dir(&dir).map_err(|e| io(&dir, e))?.next().is_none();
        if !empty {
            return Err(ProjectError::NotEmpty(dir));
        }
    }
    let selector = engine_selector(pinned_rev().as_deref());
    fs::create_dir_all(dir.join("src")).map_err(|e| io(&dir, e))?;
    fs::create_dir_all(dir.join("scenes")).map_err(|e| io(&dir, e))?;
    let cargo = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nefeengine-core = {{ git = \"{ENGINE_GIT_URL}\", {selector} }}\n\n[workspace]\n"
    );
    let project = Project {
        name: name.into(),
        package: name.into(),
        example: None,
        start_scene: "scenes/start.ron".into(),
        engine_version: ENGINE_VERSION,
        scenes: vec!["scenes/start.ron".into()],
        mods: vec![],
    };
    let efproj = dir.join(format!("{name}.efproj"));
    let files: [(PathBuf, String); 4] = [
        (efproj.clone(), write(&project)),
        (dir.join("Cargo.toml"), cargo),
        (dir.join("src/main.rs"), MAIN_RS.replace("NAME", name)),
        (dir.join("scenes/start.ron"), SCENE_RON.to_string()),
    ];
    for (p, text) in files {
        fs::write(&p, text).map_err(|e| io(&p, e))?;
    }
    Ok(efproj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_pins_a_rev_or_falls_back_to_main() {
        assert_eq!(engine_selector(Some("abc1234")), "rev = \"abc1234\"");
        assert_eq!(engine_selector(None), "branch = \"main\"");
        assert_eq!(engine_selector(Some("")), "branch = \"main\"");
    }
}
