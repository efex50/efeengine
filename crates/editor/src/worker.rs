//! The connection thread: connects to the game, performs the handshake, sends queued requests
//! one at a time and reports replies and state changes. It reconnects forever (see
//! [`crate::conn`]) and never panics on a dropped connection; the UI thread only talks to it
//! through channels.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use efeengine_editor_link::protocol::{ClientMessage, PROTOCOL_VERSION, Reply, Request, ServerMessage, recv, send};
use efeengine_editor_link::socket::{self, Conn};

use crate::conn::ConnState;

/// What the worker tells the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    State(ConnState),
    /// The reply to `request`.
    Reply { request: Request, reply: Reply },
}

/// The UI's end of the worker. Dropping it stops the thread.
pub struct Handle {
    tx: Sender<Request>,
    rx: Mutex<Receiver<Event>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// Queue a request (dropped while disconnected; the poll timer sends it again).
    pub fn send(&self, request: Request) {
        let _ = self.tx.send(request);
    }

    /// Everything that happened since the last call.
    pub fn poll(&self) -> Vec<Event> {
        let Ok(rx) = self.rx.lock() else { return Vec::new() };
        rx.try_iter().collect()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start the worker for the game's local socket `name`.
pub fn spawn(name: String) -> Handle {
    let (tx, cmd_rx) = channel();
    let (evt_tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let thread = std::thread::spawn(move || run(name, cmd_rx, evt_tx, s));
    Handle { tx, rx: Mutex::new(rx), stop, thread: Some(thread) }
}

/// Drop repeated identical read requests from one batch, keeping the first of each, in order.
/// Requests that change the game are always kept (each one gets its own reply).
pub fn dedupe(batch: Vec<Request>) -> Vec<Request> {
    let mut out: Vec<Request> = Vec::with_capacity(batch.len());
    for r in batch {
        if crate::edit::is_mutation(&r) || !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

fn sleep_unless_stopped(total: Duration, stop: &AtomicBool, cmds: &Receiver<Request>) {
    let step = Duration::from_millis(50);
    let mut left = total;
    while !left.is_zero() && !stop.load(Ordering::Relaxed) {
        let d = step.min(left);
        std::thread::sleep(d);
        left -= d;
        // Requests made while disconnected are stale by the time we reconnect.
        while cmds.try_recv().is_ok() {}
    }
}

fn run(name: String, cmds: Receiver<Request>, events: Sender<Event>, stop: Arc<AtomicBool>) {
    let mut state = ConnState::Connecting;
    let _ = events.send(Event::State(state.clone()));
    while !stop.load(Ordering::Relaxed) {
        let err = match connect(&name) {
            Ok((mut stream, game)) => {
                state = ConnState::connected(game);
                let _ = events.send(Event::State(state.clone()));
                session(&mut stream, &cmds, &events, &stop).err().unwrap_or_default()
            }
            Err(e) => e,
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        state = state.failed(err);
        let _ = events.send(Event::State(state.clone()));
        if let ConnState::Retrying { wait, .. } = &state {
            sleep_unless_stopped(*wait, &stop, &cmds);
        }
    }
}

fn connect(name: &str) -> Result<(Conn, String), String> {
    let mut s = socket::connect(name, Duration::from_millis(500)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(3)));
    s.set_write_timeout(Some(Duration::from_secs(3)));
    send(&mut s, &ClientMessage::Hello { protocol: PROTOCOL_VERSION, client: "efe-editor".into() }).map_err(|e| e.to_string())?;
    match recv::<ServerMessage>(&mut s).map_err(|e| format!("handshake: {e}"))? {
        ServerMessage::Hello { game, .. } => Ok((s, game)),
        ServerMessage::Error { code, message } => Err(format!("{code}: {message}")),
        other => Err(format!("unexpected handshake reply: {other:?}")),
    }
}

/// Serve requests until the connection fails (the error) or the UI stops us (`Ok`).
fn session(stream: &mut Conn, cmds: &Receiver<Request>, events: &Sender<Event>, stop: &AtomicBool) -> Result<(), String> {
    let mut next_id = 1u64;
    while !stop.load(Ordering::Relaxed) {
        let first = match cmds.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let mut batch = vec![first];
        batch.extend(cmds.try_iter());
        for request in dedupe(batch) {
            let id = next_id;
            next_id += 1;
            send(stream, &ClientMessage::Request { id, request: request.clone() }).map_err(|e| format!("send: {e}"))?;
            loop {
                match recv::<ServerMessage>(stream).map_err(|e| format!("receive: {e}"))? {
                    ServerMessage::Reply { id: rid, reply } if rid == id => {
                        let _ = events.send(Event::Reply { request, reply });
                        break;
                    }
                    ServerMessage::Reply { .. } => continue, // a late reply to an earlier request
                    ServerMessage::Error { code, message } => return Err(format!("{code}: {message}")),
                    ServerMessage::Hello { .. } => return Err("unexpected hello".into()),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use efeengine_editor_link::protocol::{Mode, read_frame, write_frame};
    use efeengine_editor_link::socket::Listener;

    #[test]
    fn dedupe_keeps_first_of_each_in_order() {
        let v = dedupe(vec![Request::Stats, Request::Entities, Request::Stats, Request::SetMode { mode: Mode::Edit, keep_changes: false }, Request::Entities]);
        assert_eq!(v, vec![Request::Stats, Request::Entities, Request::SetMode { mode: Mode::Edit, keep_changes: false }]);
        let d = Request::Despawn { entity: Some(1), editor_id: None };
        assert_eq!(dedupe(vec![d.clone(), d.clone()]), vec![d.clone(), d]);
    }

    /// A socket name unique to this test and process (a temp-dir path on Unix).
    fn unique_name(test: &str) -> String {
        let id = format!("efe-worker-{test}-{}", std::process::id());
        if cfg!(unix) { format!("{}/{id}.sock", std::env::temp_dir().display()) } else { id }
    }

    fn wait_for(h: &Handle, mut pred: impl FnMut(&Event) -> bool) -> Event {
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < end {
            for e in h.poll() {
                if pred(&e) {
                    return e;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for an event");
    }

    /// A fake game: greets, answers `stats` once, then drops the connection.
    fn fake_game(listener: &Listener) {
        let mut s = loop {
            if let Some(s) = socket::accept(listener).unwrap() {
                break s;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        s.set_read_timeout(Some(Duration::from_secs(10)));
        let _hello: ClientMessage = recv(&mut s).unwrap();
        send(&mut s, &ServerMessage::Hello { protocol: PROTOCOL_VERSION, game: "fake".into() }).unwrap();
        let msg: ClientMessage = serde_json::from_slice(&read_frame(&mut s).unwrap()).unwrap();
        let ClientMessage::Request { id, .. } = msg else { panic!() };
        let reply = ServerMessage::Reply { id, reply: Reply::Stats { frame: 1, frame_time: 0.01, entity_count: 3 } };
        write_frame(&mut s, &serde_json::to_vec(&reply).unwrap()).unwrap();
    }

    #[test]
    fn connects_gets_a_reply_survives_a_drop_and_reconnects() {
        let name = unique_name("reconnect");
        let listener = socket::listen(&name).unwrap();
        let h = spawn(name);
        let t = std::thread::spawn(move || {
            fake_game(&listener);
            fake_game(&listener); // second connection after the drop
        });
        wait_for(&h, |e| matches!(e, Event::State(ConnState::Connected { game }) if game == "fake"));
        h.send(Request::Stats);
        let e = wait_for(&h, |e| matches!(e, Event::Reply { .. }));
        assert!(matches!(e, Event::Reply { reply: Reply::Stats { entity_count: 3, .. }, .. }));
        // The fake game closed the socket: the next request fails, the UI sees a retry...
        h.send(Request::Entities);
        wait_for(&h, |e| matches!(e, Event::State(ConnState::Retrying { .. })));
        // ...and the worker connects again by itself.
        wait_for(&h, |e| matches!(e, Event::State(ConnState::Connected { .. })));
        h.send(Request::Stats);
        wait_for(&h, |e| matches!(e, Event::Reply { .. }));
        t.join().unwrap();
    }

    #[test]
    fn refused_connection_is_reported_not_fatal() {
        // Nobody listens on this name.
        let h = spawn(unique_name("refused"));
        let e = wait_for(&h, |e| matches!(e, Event::State(ConnState::Retrying { .. })));
        assert!(matches!(e, Event::State(ConnState::Retrying { attempt: 1, .. })));
        drop(h); // joins the thread promptly
    }
}
