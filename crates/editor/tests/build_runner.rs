//! The build runner against a throwaway cargo project (no dependencies, so it works offline).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use efeengine_editor::build::{BuildState, RunnerEvent, build_command, BuildStatus, LineKind};
use efeengine_editor::runner::run_build;

fn project(name: &str, main_rs: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("efe-editor-build-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n")).unwrap();
    std::fs::write(dir.join("src/main.rs"), main_rs).unwrap();
    dir
}

fn build(dir: &Path, name: &str) -> (BuildState, Option<i32>) {
    let mut spec = build_command(dir, name, false);
    // Keep the throwaway build out of the workspace's target directory.
    spec.env.push(("CARGO_TARGET_DIR".into(), dir.join("target").to_string_lossy().into_owned()));
    let mut state = BuildState::default();
    state.apply(RunnerEvent::BuildStarted { release: false });
    let code = run_build(&spec, &AtomicBool::new(false), |l| state.apply(RunnerEvent::Line(l))).expect("cargo starts");
    state.apply(RunnerEvent::BuildFinished { code });
    (state, code)
}

#[test]
fn broken_project_fails_with_file_and_line() {
    let name = "efe_broken";
    let dir = project(name, "fn main() {\n    let x: u32 = \"no\";\n}\n");
    let (state, code) = build(&dir, name);
    assert_eq!(state.build, BuildStatus::Failed { code: Some(101) });
    assert_eq!(code, Some(101));
    let locs = state.output.error_locations();
    assert!(locs.iter().any(|l| l.ends_with("main.rs:2")), "{locs:?}");
    assert!(state.output.iter().any(|l| l.kind == LineKind::Error && l.text.contains("mismatched types")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn good_project_builds() {
    let name = "efe_good";
    let dir = project(name, "fn main() { println!(\"hi\"); }\n");
    let (state, code) = build(&dir, name);
    assert_eq!(code, Some(0), "{:?}", state.output.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(state.build, BuildStatus::Ok);
    assert!(dir.join("target/debug").join(format!("{name}{}", std::env::consts::EXE_SUFFIX)).exists());
    let _ = std::fs::remove_dir_all(&dir);
}
