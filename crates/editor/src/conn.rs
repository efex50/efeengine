//! The connection state machine: what the status bar shows and how long to wait before the
//! next attempt. Pure, so it is tested without a socket.

use std::time::Duration;

/// Where the connection to the game stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    /// Trying to connect for the first time.
    Connecting,
    Connected { game: String },
    /// The last attempt (or the connection) failed; the next attempt starts after `wait`.
    Retrying { attempt: u32, error: String, wait: Duration },
}

/// Delay before attempt number `attempt` (1-based after a failure): 0.5 s, doubling, at most 5 s.
pub fn backoff(attempt: u32) -> Duration {
    let ms = 500u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(4));
    Duration::from_millis(ms.min(5000))
}

impl ConnState {
    /// An attempt failed (or the connection dropped) with `error`.
    pub fn failed(&self, error: impl Into<String>) -> ConnState {
        let attempt = match self {
            ConnState::Retrying { attempt, .. } => attempt + 1,
            _ => 1,
        };
        ConnState::Retrying { attempt, error: error.into(), wait: backoff(attempt) }
    }

    /// The handshake succeeded.
    pub fn connected(game: impl Into<String>) -> ConnState {
        ConnState::Connected { game: game.into() }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self, ConnState::Connected { .. })
    }

    /// One line for the status bar.
    pub fn label(&self, port: u16) -> String {
        match self {
            ConnState::Connecting => format!("connecting to 127.0.0.1:{port}..."),
            ConnState::Connected { game } => format!("connected to {game} (127.0.0.1:{port})"),
            ConnState::Retrying { attempt, error, wait } => {
                format!("not connected: {error} (attempt {attempt}, retrying in {:.1} s)", wait.as_secs_f32())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let ms: Vec<u128> = (1..=8).map(|a| backoff(a).as_millis()).collect();
        assert_eq!(ms, [500, 1000, 2000, 4000, 5000, 5000, 5000, 5000]);
        assert_eq!(backoff(0).as_millis(), 500);
        assert_eq!(backoff(u32::MAX).as_millis(), 5000);
    }

    #[test]
    fn failures_count_up_and_connect_resets() {
        let s = ConnState::Connecting.failed("refused");
        assert_eq!(s, ConnState::Retrying { attempt: 1, error: "refused".into(), wait: backoff(1) });
        let s = s.failed("refused again");
        assert!(matches!(s, ConnState::Retrying { attempt: 2, .. }));
        let c = ConnState::connected("demo");
        assert!(c.is_connected());
        // A drop after a good connection starts counting from one again.
        assert!(matches!(c.failed("reset"), ConnState::Retrying { attempt: 1, .. }));
    }

    #[test]
    fn labels() {
        assert!(ConnState::Connecting.label(7878).contains("7878"));
        assert!(ConnState::connected("demo").label(7878).contains("demo"));
        let l = ConnState::Connecting.failed("refused").label(7878);
        assert!(l.contains("refused") && l.contains("attempt 1") && l.contains("0.5"));
    }
}
