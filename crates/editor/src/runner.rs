//! The build and run thread (docs/EDITOR.md E5.1, E5.3). It runs `cargo build` with its output
//! read line by line, starts and stops the game binary and runs the doctor, all off the UI
//! thread. The UI talks to it through [`Handle`] and reads [`RunnerEvent`]s.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::build::{CommandSpec, RunnerEvent};
use crate::doctor;

fn command(spec: &CommandSpec) -> Command {
    let mut c = Command::new(&spec.program);
    c.args(&spec.args).current_dir(&spec.cwd).envs(spec.env.iter().map(|(k, v)| (k, v)));
    c
}

/// Run a build, calling `on_line` for each stdout or stderr line as it arrives. Returns the exit
/// code (`None` when `cancel` killed it or a signal ended it), or an error when it did not start.
pub fn run_build(spec: &CommandSpec, cancel: &AtomicBool, mut on_line: impl FnMut(String)) -> Result<Option<i32>, String> {
    let mut child = command(spec)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", spec.program))?;
    let (tx, rx) = channel::<String>();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(pump(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(pump(err, tx.clone()));
    }
    drop(tx);
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(l) => on_line(l),
            Err(RecvTimeoutError::Timeout) => {
                if cancel.load(Ordering::Relaxed) {
                    // rustc children may keep the pipes open for a moment: do not wait for them.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(None);
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    for r in readers {
        let _ = r.join();
    }
    child.wait().map(|s| s.code()).map_err(|e| e.to_string())
}

fn pump(src: impl std::io::Read + Send + 'static, tx: Sender<String>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(src).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line).trim_end_matches('\r').to_string();
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

/// Owns the game's child process. Dropping it kills the game, so closing the editor never
/// leaves one behind.
#[derive(Default)]
pub struct GameHolder {
    child: Option<Child>,
}

impl GameHolder {
    pub fn running(&self) -> bool {
        self.child.is_some()
    }

    /// Start the game; its pid.
    pub fn start(&mut self, spec: &CommandSpec) -> Result<u32, String> {
        if self.child.is_some() {
            return Err("the game is already running".into());
        }
        let child = command(spec).stdin(Stdio::null()).spawn().map_err(|e| format!("could not start {}: {e}", spec.program))?;
        let pid = child.id();
        self.child = Some(child);
        Ok(pid)
    }

    /// The exit status text when the game ended on its own since the last call.
    pub fn poll_exit(&mut self) -> Option<String> {
        let status = self.child.as_mut()?.try_wait().ok().flatten()?;
        self.child = None;
        Some(status.to_string())
    }

    /// Kill the game. False when none was running.
    pub fn stop(&mut self) -> bool {
        match self.child.take() {
            Some(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
                true
            }
            None => false,
        }
    }
}

impl Drop for GameHolder {
    fn drop(&mut self) {
        self.stop();
    }
}

enum Cmd {
    Build(CommandSpec),
    /// Build, then start the game if the build succeeded.
    Run { build: CommandSpec, game: CommandSpec },
    /// Stop the game if it runs, then like `Run` (E6.2).
    Restart { build: CommandSpec, game: CommandSpec },
    Stop,
    Doctor(PathBuf),
}

/// The UI's end of the runner. Dropping it stops the thread and kills the game.
pub struct Handle {
    tx: Sender<Cmd>,
    rx: Mutex<Receiver<RunnerEvent>>,
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn build(&self, spec: CommandSpec) {
        let _ = self.tx.send(Cmd::Build(spec));
    }

    pub fn run(&self, build: CommandSpec, game: CommandSpec) {
        let _ = self.tx.send(Cmd::Run { build, game });
    }

    /// Stop the running game, build, and start the game again (seamless restart).
    pub fn restart(&self, build: CommandSpec, game: CommandSpec) {
        let _ = self.tx.send(Cmd::Restart { build, game });
    }

    /// Kill the game, or cancel the build in progress.
    pub fn stop_game(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn doctor(&self, project_dir: PathBuf) {
        let _ = self.tx.send(Cmd::Doctor(project_dir));
    }

    /// Everything that happened since the last call.
    pub fn poll(&self) -> Vec<RunnerEvent> {
        let Ok(rx) = self.rx.lock() else { return Vec::new() };
        rx.try_iter().collect()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn spawn() -> Handle {
    let (tx, cmds) = channel();
    let (events, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let (c, s) = (cancel.clone(), stop.clone());
    let thread = std::thread::spawn(move || run(cmds, events, c, s));
    Handle { tx, rx: Mutex::new(rx), cancel, stop, thread: Some(thread) }
}

/// Build with events; true when it succeeded.
fn build(spec: &CommandSpec, release: bool, events: &Sender<RunnerEvent>, cancel: &AtomicBool) -> bool {
    cancel.store(false, Ordering::Relaxed);
    let _ = events.send(RunnerEvent::BuildStarted { release });
    let result = run_build(spec, cancel, |l| {
        let _ = events.send(RunnerEvent::Line(l));
    });
    let code = match result {
        Ok(code) => code,
        Err(e) => {
            let _ = events.send(RunnerEvent::Notice(e));
            None
        }
    };
    let _ = events.send(RunnerEvent::BuildFinished { code });
    code == Some(0)
}

/// Build, and start the game when the build succeeded.
fn build_and_start(b: &CommandSpec, spec: &CommandSpec, game: &mut GameHolder, events: &Sender<RunnerEvent>, cancel: &AtomicBool) {
    if build(b, b.args.iter().any(|a| a == "--release"), events, cancel) {
        match game.start(spec) {
            Ok(pid) => {
                let _ = events.send(RunnerEvent::GameStarted { pid });
            }
            Err(e) => {
                let _ = events.send(RunnerEvent::Notice(e));
            }
        }
    }
}

/// The toolchain check, on this thread. False (after telling the UI) when a build cannot start.
fn toolchain_ok(events: &Sender<RunnerEvent>) -> bool {
    match doctor::gate_message(&doctor::toolchain_gate(&doctor::system_exec)) {
        None => true,
        Some(m) => {
            let _ = events.send(RunnerEvent::ToolchainMissing(m));
            false
        }
    }
}

fn run(cmds: Receiver<Cmd>, events: Sender<RunnerEvent>, cancel: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    let mut game = GameHolder::default();
    while !stop.load(Ordering::Relaxed) {
        if let Some(status) = game.poll_exit() {
            let _ = events.send(RunnerEvent::GameExited { status });
        }
        let cmd = match cmds.recv_timeout(Duration::from_millis(100)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match cmd {
            Cmd::Build(_) | Cmd::Run { .. } | Cmd::Restart { .. } if !toolchain_ok(&events) => {}
            Cmd::Build(spec) => {
                build(&spec, spec.args.iter().any(|a| a == "--release"), &events, &cancel);
            }
            Cmd::Run { build: b, game: spec } => {
                if game.running() {
                    let _ = events.send(RunnerEvent::Notice("the game is already running: stop it first".into()));
                    continue;
                }
                build_and_start(&b, &spec, &mut game, &events, &cancel);
            }
            Cmd::Restart { build: b, game: spec } => {
                if game.stop() {
                    let _ = events.send(RunnerEvent::GameExited { status: "stopped by the editor".into() });
                }
                build_and_start(&b, &spec, &mut game, &events, &cancel);
            }
            Cmd::Stop => {
                if game.stop() {
                    let _ = events.send(RunnerEvent::GameExited { status: "stopped by the editor".into() });
                }
            }
            Cmd::Doctor(dir) => {
                let _ = events.send(RunnerEvent::Doctor(doctor::doctor(&dir)));
            }
        }
    }
    // `game` drops here and kills the child.
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, env: &[(&str, &str)]) -> CommandSpec {
        CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn wait_exit(g: &mut GameHolder) -> String {
        for _ in 0..200 {
            if let Some(s) = g.poll_exit() {
                return s;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("game did not exit");
    }

    #[test]
    fn game_gets_the_env_and_reports_its_exit() {
        let mut g = GameHolder::default();
        g.start(&sh("test \"$EFE_EDITOR\" = 1 && test \"$EFE_EDITOR_PORT\" = 4321", &[("EFE_EDITOR", "1"), ("EFE_EDITOR_PORT", "4321")])).unwrap();
        assert_eq!(wait_exit(&mut g), "exit status: 0");
        assert!(!g.running());
        g.start(&sh("exit 3", &[])).unwrap();
        assert_eq!(wait_exit(&mut g), "exit status: 3");
    }

    #[test]
    fn stop_and_drop_kill_the_game() {
        let mut g = GameHolder::default();
        let pid = g.start(&sh("exec sleep 30", &[])).unwrap();
        assert!(g.running() && g.start(&sh("true", &[])).is_err());
        assert!(g.stop() && !g.running() && !g.stop());
        let alive = |pid: u32| std::path::Path::new(&format!("/proc/{pid}")).exists();
        assert!(!alive(pid));
        let pid = g.start(&sh("exec sleep 30", &[])).unwrap();
        drop(g);
        assert!(!alive(pid));
    }

    #[test]
    fn bad_program_is_an_error_not_a_panic() {
        let mut g = GameHolder::default();
        let mut spec = sh("", &[]);
        spec.program = "/no/such/game".into();
        assert!(g.start(&spec).unwrap_err().contains("could not start"));
        assert!(run_build(&spec, &AtomicBool::new(false), |_| {}).is_err());
    }

    #[test]
    fn handle_runs_a_game_and_stop_reports() {
        let h = spawn();
        // "build" is a quick successful command, then the "game" sleeps.
        h.run(sh("echo built", &[]), sh("exec sleep 30", &[]));
        let mut seen = Vec::new();
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < end && !seen.iter().any(|e| matches!(e, RunnerEvent::GameStarted { .. })) {
            seen.extend(h.poll());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(seen.contains(&RunnerEvent::Line("built".into())));
        assert!(seen.contains(&RunnerEvent::BuildFinished { code: Some(0) }));
        h.stop_game();
        let mut stopped = false;
        while std::time::Instant::now() < end && !stopped {
            stopped = h.poll().iter().any(|e| matches!(e, RunnerEvent::GameExited { .. }));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(stopped);
    }

    #[test]
    fn restart_stops_the_running_game_builds_and_starts_with_the_restore_env() {
        let h = spawn();
        let pid_of = |seen: &[RunnerEvent]| seen.iter().find_map(|e| if let RunnerEvent::GameStarted { pid } = e { Some(*pid) } else { None });
        let wait = |h: &Handle, seen: &mut Vec<RunnerEvent>, pred: &dyn Fn(&[RunnerEvent]) -> bool| {
            let end = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < end && !pred(seen) {
                seen.extend(h.poll());
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        h.run(sh("echo built", &[]), sh("exec sleep 30", &[]));
        let mut seen = Vec::new();
        wait(&h, &mut seen, &|s| pid_of(s).is_some());
        let first = pid_of(&seen).expect("first game started");
        seen.clear();
        let out = std::env::temp_dir().join(format!("efe_restart_env_{}", std::process::id()));
        let script = format!("printf %s \"$EFE_RESTORE_SNAPSHOT\" > {}; exec sleep 30", out.display());
        h.restart(sh("echo rebuilt", &[]), sh(&script, &[("EFE_RESTORE_SNAPSHOT", "/some/snap.ron")]));
        wait(&h, &mut seen, &|s| pid_of(s).is_some());
        let second = pid_of(&seen).expect("restarted game started");
        assert_ne!(first, second);
        assert!(!std::path::Path::new(&format!("/proc/{first}")).exists(), "the old game was killed");
        assert!(seen.iter().any(|e| matches!(e, RunnerEvent::GameExited { status } if status.contains("stopped"))));
        assert!(seen.contains(&RunnerEvent::Line("rebuilt".into())));
        let mut got = String::new();
        for _ in 0..100 {
            got = std::fs::read_to_string(&out).unwrap_or_default();
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(got, "/some/snap.ron");
        let _ = std::fs::remove_file(&out);
        h.stop_game();
    }

    #[test]
    fn failed_build_does_not_start_the_game() {
        let h = spawn();
        h.run(sh("echo 'error: boom'; exit 2", &[]), sh("exec sleep 30", &[]));
        let mut seen = Vec::new();
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < end && !seen.iter().any(|e| matches!(e, RunnerEvent::BuildFinished { .. })) {
            seen.extend(h.poll());
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        seen.extend(h.poll());
        assert!(seen.contains(&RunnerEvent::BuildFinished { code: Some(2) }));
        assert!(!seen.iter().any(|e| matches!(e, RunnerEvent::GameStarted { .. })));
    }
}
