//! The pure side of build and run (docs/EDITOR.md E5): command lines, compiler output
//! classification, the bounded output buffer and the status state machine. The threads and
//! processes are in [`crate::runner`]; the iced code only shows what is here.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::doctor::{CheckResult, report_lines};

/// The Errors panel keeps this many lines.
pub const MAX_LINES: usize = 2000;

/// A process to start: program, arguments, working directory and extra environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// `cargo build -p <package> --bin <package> [--release]` in `project_dir`, without colour codes.
pub fn build_command(project_dir: &Path, package: &str, release: bool) -> CommandSpec {
    let mut args: Vec<String> = ["build", "-p", package, "--bin", package].iter().map(|s| s.to_string()).collect();
    if release {
        args.push("--release".into());
    }
    CommandSpec { program: "cargo".into(), args, cwd: project_dir.to_path_buf(), env: vec![("CARGO_TERM_COLOR".into(), "never".into())] }
}

/// Cargo's target directory: `$CARGO_TARGET_DIR` (relative to the project) or `<project>/target`.
pub fn target_dir(project_dir: &Path, env: Option<&str>) -> PathBuf {
    match env.filter(|e| !e.is_empty()) {
        Some(e) => project_dir.join(e), // join keeps an absolute `e` as is
        None => project_dir.join("target"),
    }
}

/// `<target>/<debug|release>/<package>` with the platform's executable suffix.
pub fn game_binary(target: &Path, release: bool, package: &str) -> PathBuf {
    target.join(if release { "release" } else { "debug" }).join(format!("{package}{}", std::env::consts::EXE_SUFFIX))
}

/// The game binary itself (not `cargo run`, so Stop kills the game), with the editor link on.
pub fn run_command(project_dir: &Path, target: &Path, package: &str, release: bool, socket: &str) -> CommandSpec {
    CommandSpec {
        program: game_binary(target, release, package).to_string_lossy().into_owned(),
        args: Vec::new(),
        cwd: project_dir.to_path_buf(),
        env: vec![("EFE_EDITOR".into(), "1".into()), ("EFE_EDITOR_SOCKET".into(), socket.to_string())],
    }
}

/// `spec` with `EFE_RESTORE_SNAPSHOT` set: the game restores that file after its setup (E6.2).
pub fn with_restore(mut spec: CommandSpec, snapshot: &Path) -> CommandSpec {
    spec.env.push(("EFE_RESTORE_SNAPSHOT".into(), snapshot.to_string_lossy().into_owned()));
    spec
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Error,
    Warning,
    Other,
}

/// `error:`, `error[E0432]:` and `warning:` lines (cargo and rustc) are diagnostics.
pub fn classify(line: &str) -> LineKind {
    let t = line.trim_start();
    let head = |w: &str| t.strip_prefix(w).is_some_and(|r| r.starts_with(':') || r.starts_with('['));
    if head("error") {
        LineKind::Error
    } else if head("warning") {
        LineKind::Warning
    } else {
        LineKind::Other
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    pub line: u32,
    pub col: Option<u32>,
}

impl Location {
    /// `file:line`.
    pub fn short(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// The `--> src/main.rs:3:5` line rustc prints under a diagnostic.
pub fn location(line: &str) -> Option<Location> {
    let rest = line.trim_start().strip_prefix("-->")?.trim();
    let mut parts = rest.rsplitn(3, ':');
    let last = parts.next()?.trim().parse::<u32>().ok()?;
    match (parts.next(), parts.next()) {
        (Some(mid), Some(file)) => match mid.trim().parse::<u32>() {
            Ok(line) => Some(Location { file: file.into(), line, col: Some(last) }),
            Err(_) => None,
        },
        (Some(file), None) => Some(Location { file: file.into(), line: last, col: None }),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub text: String,
    pub kind: LineKind,
    pub location: Option<Location>,
}

/// The last [`MAX_LINES`] lines, newest at the back. A `-->` line takes the colour of the
/// diagnostic above it.
#[derive(Debug, Default)]
pub struct OutputBuffer {
    lines: VecDeque<OutputLine>,
    context: Option<LineKind>,
}

impl OutputBuffer {
    pub fn push(&mut self, text: impl Into<String>) {
        let text = text.into();
        let loc = location(&text);
        let mut kind = classify(&text);
        if kind == LineKind::Other && loc.is_some() {
            kind = self.context.unwrap_or(LineKind::Other);
        }
        if kind != LineKind::Other {
            self.context = Some(kind);
        } else if text.trim().is_empty() {
            self.context = None;
        }
        self.push_line(OutputLine { text, kind, location: loc });
    }

    /// Add a line whose kind is already known.
    pub fn push_line(&mut self, line: OutputLine) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.context = None;
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &OutputLine> {
        self.lines.iter()
    }

    /// The `file:line` of every error in the buffer.
    pub fn error_locations(&self) -> Vec<String> {
        self.lines.iter().filter(|l| l.kind == LineKind::Error).filter_map(|l| l.location.as_ref()).map(Location::short).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStatus {
    Idle,
    Building { release: bool },
    Ok,
    /// `code` is `None` when cargo was stopped or killed.
    Failed { code: Option<i32> },
}

impl BuildStatus {
    pub fn label(&self) -> String {
        match self {
            BuildStatus::Idle => "build: idle".into(),
            BuildStatus::Building { release } => format!("build: building{}...", if *release { " (release)" } else { "" }),
            BuildStatus::Ok => "build: ok".into(),
            BuildStatus::Failed { code: Some(c) } => format!("build: failed (exit code {c})"),
            BuildStatus::Failed { code: None } => "build: failed (stopped)".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Stopped,
    Running { pid: u32 },
    /// The game ended; `status` is the exit status text.
    Exited { status: String },
}

impl RunStatus {
    pub fn label(&self) -> String {
        match self {
            RunStatus::Stopped => "game: not running".into(),
            RunStatus::Running { pid } => format!("game: running (pid {pid})"),
            RunStatus::Exited { status } => format!("game: exited ({status})"),
        }
    }
}

/// What the runner thread tells the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerEvent {
    BuildStarted { release: bool },
    Line(String),
    BuildFinished { code: Option<i32> },
    GameStarted { pid: u32 },
    GameExited { status: String },
    Doctor(Vec<CheckResult>),
    /// Build or Run was refused: the Rust toolchain is missing. Carries the message.
    ToolchainMissing(String),
    /// Something the user should read (a failed spawn, a refused Run).
    Notice(String),
}

/// Everything the header and the Errors panel show, advanced by [`RunnerEvent`]s.
#[derive(Debug)]
pub struct BuildState {
    pub build: BuildStatus,
    pub run: RunStatus,
    pub output: OutputBuffer,
    pub notice: Option<String>,
    /// The last Build or Run was refused for a missing toolchain: show "Open rustup.rs".
    pub toolchain_missing: bool,
}

impl Default for BuildState {
    fn default() -> Self {
        Self { build: BuildStatus::Idle, run: RunStatus::Stopped, output: OutputBuffer::default(), notice: None, toolchain_missing: false }
    }
}

impl BuildState {
    pub fn apply(&mut self, ev: RunnerEvent) {
        match ev {
            RunnerEvent::BuildStarted { release } => {
                self.build = BuildStatus::Building { release };
                self.output.clear();
                self.notice = None;
                self.toolchain_missing = false;
            }
            RunnerEvent::Line(l) => self.output.push(l),
            RunnerEvent::BuildFinished { code } => {
                self.build = if code == Some(0) { BuildStatus::Ok } else { BuildStatus::Failed { code } };
            }
            RunnerEvent::GameStarted { pid } => {
                self.run = RunStatus::Running { pid };
                self.notice = None;
            }
            RunnerEvent::GameExited { status } => {
                self.output.push(format!("game exited: {status}"));
                self.run = RunStatus::Exited { status };
            }
            RunnerEvent::Doctor(results) => {
                for l in report_lines(&results) {
                    self.output.push_line(OutputLine { text: l.text, kind: l.kind, location: None });
                }
            }
            RunnerEvent::ToolchainMissing(n) => {
                self.output.push_line(OutputLine { text: n.clone(), kind: LineKind::Error, location: None });
                self.notice = Some(n);
                self.toolchain_missing = true;
            }
            RunnerEvent::Notice(n) => {
                self.output.push_line(OutputLine { text: n.clone(), kind: LineKind::Warning, location: None });
                self.notice = Some(n);
            }
        }
    }

    pub fn building(&self) -> bool {
        matches!(self.build, BuildStatus::Building { .. })
    }

    pub fn running(&self) -> bool {
        matches!(self.run, RunStatus::Running { .. })
    }

    /// Build is possible with a package and no build in progress.
    pub fn can_build(&self, has_package: bool) -> bool {
        has_package && !self.building()
    }

    /// Run needs a package, no build in progress and no game already running.
    pub fn can_run(&self, has_package: bool) -> bool {
        has_package && !self.building() && !self.running()
    }

    /// Stop ends a running game or a build in progress.
    pub fn can_stop(&self) -> bool {
        self.building() || self.running()
    }

    /// The header line.
    pub fn header(&self) -> String {
        format!("{}   {}", self.build.label(), self.run.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_command_line() {
        let d = Path::new("/proj");
        let c = build_command(d, "demo", false);
        assert_eq!((c.program.as_str(), c.args.as_slice()), ("cargo", ["build", "-p", "demo", "--bin", "demo"].map(String::from).as_slice()));
        assert_eq!(c.cwd, d);
        let r = build_command(d, "demo", true);
        assert_eq!(r.args.last().map(String::as_str), Some("--release"));
        assert_eq!(r.args.len(), 6);
    }

    #[test]
    fn run_command_line_uses_the_binary_and_env() {
        let d = Path::new("/proj");
        let t = target_dir(d, None);
        assert_eq!(t, Path::new("/proj/target"));
        let c = run_command(d, &t, "demo", false, "efe-editor-9");
        assert_eq!(Path::new(&c.program), Path::new("/proj/target/debug").join(format!("demo{}", std::env::consts::EXE_SUFFIX)));
        assert!(c.args.is_empty());
        assert!(c.env.contains(&("EFE_EDITOR".into(), "1".into())));
        assert!(c.env.contains(&("EFE_EDITOR_SOCKET".into(), "efe-editor-9".into())));
        let r = run_command(d, &t, "demo", true, "s");
        assert!(r.program.contains("target/release") || r.program.contains("target\\release"));
        assert_eq!(target_dir(d, Some("out")), Path::new("/proj/out"));
        assert_eq!(target_dir(d, Some("/abs")), Path::new("/abs"));
        assert_eq!(target_dir(d, Some("")), Path::new("/proj/target"));
    }

    #[test]
    fn restore_env_is_added() {
        let t = target_dir(Path::new("/p"), None);
        let c = with_restore(run_command(Path::new("/p"), &t, "demo", false, "s"), Path::new("/p/target/efe-restart.ron"));
        assert!(c.env.contains(&("EFE_RESTORE_SNAPSHOT".into(), "/p/target/efe-restart.ron".into())));
        assert!(c.env.contains(&("EFE_EDITOR".into(), "1".into())));
    }

    #[test]
    fn classifies_lines() {
        assert_eq!(classify("error[E0432]: unresolved import `x`"), LineKind::Error);
        assert_eq!(classify("error: could not compile `demo` (bin \"demo\") due to 1 previous error"), LineKind::Error);
        assert_eq!(classify("warning: unused variable: `x`"), LineKind::Warning);
        assert_eq!(classify("   Compiling demo v0.1.0"), LineKind::Other);
        assert_eq!(classify("errors are fun"), LineKind::Other);
        assert_eq!(classify("    Finished `dev` profile"), LineKind::Other);
    }

    #[test]
    fn extracts_locations() {
        let l = location("  --> src/main.rs:3:5").unwrap();
        assert_eq!(l, Location { file: "src/main.rs".into(), line: 3, col: Some(5) });
        assert_eq!(l.short(), "src/main.rs:3");
        assert_eq!(location("--> a/b.rs:12").unwrap().line, 12);
        assert_eq!(location("--> C:\\x\\a.rs:7:1").unwrap().file, "C:\\x\\a.rs");
        assert_eq!(location("error: nope"), None);
        assert_eq!(location("--> no_numbers"), None);
    }

    #[test]
    fn buffer_is_bounded_and_ordered() {
        let mut b = OutputBuffer::default();
        for i in 0..MAX_LINES + 50 {
            b.push(format!("line {i}"));
        }
        assert_eq!(b.len(), MAX_LINES);
        assert_eq!(b.iter().next().unwrap().text, "line 50");
        assert_eq!(b.iter().last().unwrap().text, format!("line {}", MAX_LINES + 49));
        b.clear();
        assert!(b.is_empty());
    }

    #[test]
    fn location_line_takes_the_diagnostic_colour() {
        let mut b = OutputBuffer::default();
        b.push("warning: unused");
        b.push(" --> src/a.rs:1:1");
        b.push("");
        b.push(" --> src/b.rs:2:2"); // not under a diagnostic any more
        b.push("error[E0308]: mismatched types");
        b.push(" --> src/main.rs:4:9");
        let kinds: Vec<_> = b.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [LineKind::Warning, LineKind::Warning, LineKind::Other, LineKind::Other, LineKind::Error, LineKind::Error]);
        assert_eq!(b.error_locations(), ["src/main.rs:4"]);
    }

    #[test]
    fn status_machine_build() {
        let mut s = BuildState::default();
        assert_eq!(s.build, BuildStatus::Idle);
        assert!(s.can_build(true) && !s.can_build(false) && !s.can_stop());
        s.apply(RunnerEvent::BuildStarted { release: false });
        assert_eq!(s.build, BuildStatus::Building { release: false });
        assert!(!s.can_build(true) && !s.can_run(true) && s.can_stop());
        s.apply(RunnerEvent::Line("error: boom".into()));
        s.apply(RunnerEvent::BuildFinished { code: Some(101) });
        assert_eq!(s.build, BuildStatus::Failed { code: Some(101) });
        assert!(s.build.label().contains("exit code 101"));
        s.apply(RunnerEvent::BuildStarted { release: true });
        assert!(s.output.is_empty(), "a new build starts a fresh log");
        assert!(s.build.label().contains("release"));
        s.apply(RunnerEvent::BuildFinished { code: Some(0) });
        assert_eq!(s.build, BuildStatus::Ok);
        s.apply(RunnerEvent::BuildStarted { release: false });
        s.apply(RunnerEvent::BuildFinished { code: None });
        assert_eq!(s.build, BuildStatus::Failed { code: None });
    }

    #[test]
    fn status_machine_run() {
        let mut s = BuildState::default();
        s.apply(RunnerEvent::BuildStarted { release: false });
        s.apply(RunnerEvent::BuildFinished { code: Some(0) });
        s.apply(RunnerEvent::GameStarted { pid: 42 });
        assert_eq!(s.run, RunStatus::Running { pid: 42 });
        assert!(s.can_stop() && !s.can_run(true) && s.can_build(true));
        assert!(s.header().contains("build: ok") && s.header().contains("pid 42"));
        s.apply(RunnerEvent::GameExited { status: "exit status: 1".into() });
        assert_eq!(s.run, RunStatus::Exited { status: "exit status: 1".into() });
        assert!(s.header().contains("exited (exit status: 1)"));
        assert!(s.can_run(true) && !s.can_stop());
        assert_eq!(s.output.iter().last().unwrap().text, "game exited: exit status: 1");
        s.apply(RunnerEvent::Notice("could not start".into()));
        assert_eq!(s.notice.as_deref(), Some("could not start"));
    }
}
