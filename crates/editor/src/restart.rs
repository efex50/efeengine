//! Seamless restart (docs/EDITOR.md E6.2): the pure state machine. "Rebuild and restart" asks
//! the running game for a restart snapshot, then stops it, builds and starts the new binary with
//! `EFE_RESTORE_SNAPSHOT` pointing at the file; the game restores it lenient-ly (fields mapped
//! by name). The threads are in [`crate::runner`], the buttons in `main.rs`.
//!
//! ```text
//! Idle --begin(connected game)--> Snapshotting --saved--> Restarting --GameStarted--> Idle
//!   |                                   |                     |
//!   |                                   +--error--> Idle      +--build failed / spawn failed--> Idle
//!   +--begin(no game, kept snapshot)--> Restarting            (the snapshot file is kept)
//! ```

use std::path::{Path, PathBuf};

use efeengine_editor_link::protocol::Reply;

use crate::build::RunnerEvent;

/// The snapshot file, relative to the project directory (the link resolves it there).
pub const SNAPSHOT_PATH: &str = "target/efe-restart.ron";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// The `restart_snapshot` request is out.
    Snapshotting,
    /// Game stopped (or stopping), build and start in progress on the runner thread.
    Restarting,
}

/// What the UI must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send `Request::RestartSnapshot { path: SNAPSHOT_PATH }`.
    RequestSnapshot,
    /// Tell the runner to stop the game, build and start it; `restore` sets
    /// `EFE_RESTORE_SNAPSHOT` for the new process.
    Restart { restore: bool },
    Notice(String),
}

#[derive(Debug)]
pub struct Restart {
    phase: Phase,
    /// A snapshot file from an earlier restart whose build failed: the next restart uses it
    /// (the game is not running, so no new one can be taken).
    kept: bool,
}

impl Default for Restart {
    fn default() -> Self {
        Self { phase: Phase::Idle, kept: false }
    }
}

impl Restart {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether the button is enabled: a package, nothing building and no restart under way.
    /// Needs a connected game, or a kept snapshot (to retry after a failed build).
    pub fn can_begin(&self, has_package: bool, building: bool, connected: bool) -> bool {
        has_package && !building && self.phase == Phase::Idle && (connected || self.kept)
    }

    /// The user pressed "Rebuild and restart".
    pub fn begin(&mut self, has_package: bool, building: bool, connected: bool) -> Vec<Action> {
        if !self.can_begin(has_package, building, connected) {
            return vec![Action::Notice("restart needs a connected game (or a kept snapshot), a package and no build running".into())];
        }
        if connected {
            self.phase = Phase::Snapshotting;
            vec![Action::RequestSnapshot]
        } else {
            self.phase = Phase::Restarting;
            vec![Action::Restart { restore: true }]
        }
    }

    /// The reply to the snapshot request.
    pub fn on_snapshot_reply(&mut self, reply: &Reply) -> Vec<Action> {
        if self.phase != Phase::Snapshotting {
            return Vec::new();
        }
        match reply {
            Reply::Saved { .. } => {
                self.phase = Phase::Restarting;
                self.kept = true;
                vec![Action::Restart { restore: true }]
            }
            Reply::Error { code, message } => {
                self.phase = Phase::Idle;
                vec![Action::Notice(format!("restart cancelled, the game could not save its world ({code}): {message}"))]
            }
            other => {
                self.phase = Phase::Idle;
                vec![Action::Notice(format!("restart cancelled: unexpected reply {other:?}"))]
            }
        }
    }

    /// The connection dropped while waiting for the snapshot.
    pub fn on_disconnect(&mut self) -> Vec<Action> {
        if self.phase == Phase::Snapshotting {
            self.phase = Phase::Idle;
            return vec![Action::Notice("restart cancelled: the game disconnected before it saved its world".into())];
        }
        Vec::new()
    }

    pub fn on_runner_event(&mut self, ev: &RunnerEvent) {
        if self.phase != Phase::Restarting {
            return;
        }
        match ev {
            RunnerEvent::GameStarted { .. } => {
                self.phase = Phase::Idle;
                self.kept = false;
            }
            RunnerEvent::BuildFinished { code } if *code != Some(0) => self.phase = Phase::Idle,
            RunnerEvent::Notice(_) | RunnerEvent::ToolchainMissing(_) => self.phase = Phase::Idle,
            _ => {}
        }
    }
}

/// Absolute snapshot path for `EFE_RESTORE_SNAPSHOT`.
pub fn snapshot_file(project_dir: &Path) -> PathBuf {
    project_dir.join(SNAPSHOT_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Reply {
        Reply::Saved { path: SNAPSHOT_PATH.into() }
    }

    #[test]
    fn happy_path() {
        let mut r = Restart::default();
        assert!(!r.can_begin(true, false, false));
        assert_eq!(r.begin(true, false, true), vec![Action::RequestSnapshot]);
        assert_eq!(r.phase(), Phase::Snapshotting);
        assert!(!r.can_begin(true, false, true), "no second restart while one runs");
        assert_eq!(r.on_snapshot_reply(&saved()), vec![Action::Restart { restore: true }]);
        assert_eq!(r.phase(), Phase::Restarting);
        r.on_runner_event(&RunnerEvent::BuildStarted { release: false });
        r.on_runner_event(&RunnerEvent::GameExited { status: "stopped by the editor".into() });
        assert_eq!(r.phase(), Phase::Restarting, "our own stop does not end the restart");
        r.on_runner_event(&RunnerEvent::BuildFinished { code: Some(0) });
        assert_eq!(r.phase(), Phase::Restarting);
        r.on_runner_event(&RunnerEvent::GameStarted { pid: 5 });
        assert_eq!(r.phase(), Phase::Idle);
        assert!(!r.can_begin(true, false, false), "the kept snapshot was consumed");
    }

    #[test]
    fn snapshot_error_or_disconnect_cancels_and_touches_nothing() {
        let mut r = Restart::default();
        r.begin(true, false, true);
        let a = r.on_snapshot_reply(&Reply::error("scene_error", "disk full"));
        assert!(matches!(&a[..], [Action::Notice(m)] if m.contains("disk full")));
        assert_eq!(r.phase(), Phase::Idle);
        r.begin(true, false, true);
        assert!(matches!(&r.on_disconnect()[..], [Action::Notice(_)]));
        assert_eq!(r.phase(), Phase::Idle);
        assert!(r.on_disconnect().is_empty());
    }

    #[test]
    fn failed_build_keeps_the_snapshot_for_a_retry() {
        let mut r = Restart::default();
        r.begin(true, false, true);
        r.on_snapshot_reply(&saved());
        r.on_runner_event(&RunnerEvent::BuildFinished { code: Some(101) });
        assert_eq!(r.phase(), Phase::Idle);
        // The game is gone, but a retry is possible and reuses the file.
        assert!(r.can_begin(true, false, false));
        assert_eq!(r.begin(true, false, false), vec![Action::Restart { restore: true }]);
        r.on_runner_event(&RunnerEvent::Notice("could not start".into()));
        assert_eq!(r.phase(), Phase::Idle);
        assert!(r.can_begin(true, false, false), "still kept after a failed start");
    }

    #[test]
    fn refused_without_package_or_while_building() {
        let mut r = Restart::default();
        assert!(matches!(&r.begin(false, false, true)[..], [Action::Notice(_)]));
        assert!(matches!(&r.begin(true, true, true)[..], [Action::Notice(_)]));
        assert_eq!(r.phase(), Phase::Idle);
    }

    #[test]
    fn stray_events_are_ignored_when_idle() {
        let mut r = Restart::default();
        assert!(r.on_snapshot_reply(&saved()).is_empty());
        r.on_runner_event(&RunnerEvent::GameStarted { pid: 1 });
        assert_eq!(r.phase(), Phase::Idle);
    }

    #[test]
    fn snapshot_file_is_under_the_project() {
        assert_eq!(snapshot_file(Path::new("/p")), Path::new("/p/target/efe-restart.ron"));
    }
}
