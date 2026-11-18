//! The doctor (docs/EDITOR.md E5.2): toolchain checks with a fix hint for each. Mirrors
//! `./build.py doctor`. The check list and the report are pure; [`run_checks`] takes the
//! command runner and the file test as arguments so it is tested without a toolchain, and the
//! editor calls [`doctor`] on its worker thread.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::build::LineKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    /// What was found (a version) when ok.
    pub detail: String,
    /// How to fix it when missing.
    pub hint: String,
}

/// One printed line and how the Errors panel colours it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportLine {
    pub kind: LineKind,
    pub text: String,
}

/// Run every check. `exec(program, args)` returns the trimmed stdout when the command ran and
/// succeeded; `exists` tests a path.
pub fn run_checks(project_dir: &Path, native: bool, exec: &dyn Fn(&str, &[&str]) -> Option<String>, exists: &dyn Fn(&Path) -> bool) -> Vec<CheckResult> {
    let mut out = Vec::new();
    let mut tool = |name: &str, program: &str, args: &[&str], hint: &str| {
        let found = exec(program, args);
        out.push(CheckResult { name: name.into(), ok: found.is_some(), detail: found.unwrap_or_default(), hint: hint.into() });
    };
    tool("cargo", "cargo", &["--version"], "install rustup: https://rustup.rs");
    tool("rustc", "rustc", &["--version"], "install rustup: https://rustup.rs");
    if native && !cfg!(windows) {
        tool(
            "SDL3 dev files (not needed with --static)",
            "pkg-config",
            &["--modversion", "sdl3"],
            "install SDL3-devel: sudo dnf install SDL3-devel / sudo apt install libsdl3-dev / sudo pacman -S sdl3",
        );
    }
    // Paths stay out of the report: it is shown in the UI.
    let found = exists(&project_dir.join("Cargo.toml"));
    out.push(CheckResult {
        name: "project Cargo.toml".into(),
        ok: found,
        detail: String::new(),
        hint: "the project folder has no Cargo.toml: open a project made by the editor".into(),
    });
    out
}

pub const RUSTUP_URL: &str = "https://rustup.rs";

/// What to tell the user when Build or Run cannot start.
pub const TOOLCHAIN_MISSING: &str = "Rust is not installed. Install it from rustup.rs, then press Build again.";
/// Shown with every build: the first one fetches the engine.
pub const FIRST_BUILD_NOTE: &str = "The first build downloads the engine from GitHub, so it needs network access.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Ok,
    /// The names of the missing tools.
    Missing(Vec<String>),
}

/// The toolchain check before a build: `cargo` and `rustc` must run. `rustup` is only the way to
/// install them, so it is not required.
pub fn toolchain_gate(exec: &dyn Fn(&str, &[&str]) -> Option<String>) -> Gate {
    let missing: Vec<String> = ["cargo", "rustc"].into_iter().filter(|t| exec(t, &["--version"]).is_none()).map(String::from).collect();
    if missing.is_empty() { Gate::Ok } else { Gate::Missing(missing) }
}

/// The gate message for the user, `None` when the toolchain is fine.
pub fn gate_message(g: &Gate) -> Option<String> {
    match g {
        Gate::Ok => None,
        Gate::Missing(_) => Some(format!("{TOOLCHAIN_MISSING} {FIRST_BUILD_NOTE}")),
    }
}

/// Open `url` in the system browser. Errors come back as text.
pub fn open_url(url: &str) -> Result<(), String> {
    let mut c;
    if cfg!(windows) {
        c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
    } else if cfg!(target_os = "macos") {
        c = Command::new("open");
        c.arg(url);
    } else {
        c = Command::new("xdg-open");
        c.arg(url);
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ()).map_err(|e| format!("could not open the browser ({e}); go to {url}"))
}

/// The report, line by line: `  ok    name: detail`, or `  MISS  name` with a `fix:` line,
/// then a summary.
pub fn report_lines(results: &[CheckResult]) -> Vec<ReportLine> {
    let mut lines = vec![ReportLine { kind: LineKind::Other, text: "doctor".into() }];
    for r in results {
        if r.ok {
            let text = if r.detail.is_empty() { format!("  ok    {}", r.name) } else { format!("  ok    {}: {}", r.name, r.detail) };
            lines.push(ReportLine { kind: LineKind::Other, text });
        } else {
            lines.push(ReportLine { kind: LineKind::Error, text: format!("  MISS  {}", r.name) });
            lines.push(ReportLine { kind: LineKind::Warning, text: format!("        fix: {}", r.hint) });
        }
    }
    let bad = results.iter().any(|r| !r.ok);
    lines.push(ReportLine {
        kind: if bad { LineKind::Warning } else { LineKind::Other },
        text: if bad { "Some items are missing (see fixes above)." } else { "All good." }.into(),
    });
    lines
}

/// The report as plain text.
pub fn format_report(results: &[CheckResult]) -> String {
    report_lines(results).into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n")
}

/// Run `program args`; the trimmed stdout when it started and exited successfully.
pub fn system_exec(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The real doctor: runs the commands. Call it off the UI thread.
pub fn doctor(project_dir: &Path) -> Vec<CheckResult> {
    run_checks(project_dir, true, &system_exec, &|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(have: &'static [&'static str]) -> impl Fn(&str, &[&str]) -> Option<String> {
        move |p, _| have.contains(&p).then(|| format!("{p} 1.0"))
    }

    #[test]
    fn checks_cover_the_toolchain() {
        let r = run_checks(Path::new("/p"), true, &fake(&["cargo", "rustc", "pkg-config"]), &|_| true);
        let names: Vec<_> = r.iter().map(|c| c.name.as_str()).collect();
        if cfg!(windows) {
            assert_eq!(names, ["cargo", "rustc", "project Cargo.toml"]);
        } else {
            assert_eq!(names, ["cargo", "rustc", "SDL3 dev files (not needed with --static)", "project Cargo.toml"]);
        }
        assert!(r.iter().all(|c| c.ok));
        let web = run_checks(Path::new("/p"), false, &fake(&[]), &|_| false);
        assert_eq!(web.len(), 3);
        assert!(web.iter().all(|c| !c.ok));
    }

    #[test]
    fn missing_things_get_hints() {
        let r = run_checks(Path::new("/p"), true, &fake(&["cargo"]), &|_| false);
        assert!(!r[1].ok && r[1].hint.contains("rustup"));
        if !cfg!(windows) {
            let sdl = r.iter().find(|c| c.name.starts_with("SDL3")).unwrap();
            assert!(sdl.hint.contains("sudo dnf install SDL3-devel"));
        }
        let manifest = r.last().unwrap();
        assert!(!manifest.ok && !manifest.hint.contains("/p/"));
    }

    #[test]
    fn gate_missing_when_cargo_fails() {
        let g = toolchain_gate(&fake(&["rustc"]));
        assert_eq!(g, Gate::Missing(vec!["cargo".into()]));
        let m = gate_message(&g).unwrap();
        assert!(m.starts_with("Rust is not installed. Install it from rustup.rs, then press Build again."), "{m}");
        assert!(m.contains("network access"));
        assert!(matches!(toolchain_gate(&fake(&[])), Gate::Missing(v) if v.len() == 2));
    }

    #[test]
    fn gate_ok_when_all_succeed() {
        let g = toolchain_gate(&fake(&["cargo", "rustc"]));
        assert_eq!(g, Gate::Ok);
        assert_eq!(gate_message(&g), None);
    }

    #[test]
    fn report_formatting() {
        let results = vec![
            CheckResult { name: "cargo".into(), ok: true, detail: "cargo 1.90".into(), hint: String::new() },
            CheckResult { name: "SDL3".into(), ok: false, detail: String::new(), hint: "sudo dnf install SDL3-devel".into() },
        ];
        let text = format_report(&results);
        assert_eq!(
            text,
            "doctor\n  ok    cargo: cargo 1.90\n  MISS  SDL3\n        fix: sudo dnf install SDL3-devel\nSome items are missing (see fixes above)."
        );
        let lines = report_lines(&results);
        assert_eq!(lines[2].kind, LineKind::Error);
        assert_eq!(lines[3].kind, LineKind::Warning);
        assert_eq!(format_report(&results[..1]), "doctor\n  ok    cargo: cargo 1.90\nAll good.");
    }
}
