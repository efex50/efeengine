//! Editor link (docs/EDITOR.md phases E1 and E2): a localhost TCP listener that lets an editor
//! read a running game, switch edit and play mode, save and load scene files, and (E4) set
//! fields, spawn and despawn entities, and (E7.2) select an entity to draw gizmo handles over it.
//!
//! - Off unless `EFE_EDITOR=1`: [`EditorLink::from_env`] then adds nothing, so release games
//!   never open a socket. The port is `EFE_EDITOR_PORT` (default 7878). It binds `127.0.0.1` only.
//! - Connection threads read frames and queue requests; one system in `PostUpdate` at
//!   `order::LATE + 100` drains the queue once per frame, applies the requests and writes the
//!   replies.
//! - On wasm32 the plugin does nothing.

mod handler;
pub mod gizmo;
pub mod protocol;

pub use gizmo::Selection;
pub use handler::{RingLogger, recent_logs, resolve_project_path};
pub use protocol::*;

use efeengine_core::prelude::*;

/// The handshake decision: the hello reply, or the error to send before closing.
pub fn handshake(msg: &ClientMessage, game: &str) -> Result<ServerMessage, ServerMessage> {
    match msg {
        ClientMessage::Hello { protocol, .. } if *protocol == PROTOCOL_VERSION => {
            Ok(ServerMessage::Hello { protocol: PROTOCOL_VERSION, game: game.to_string() })
        }
        ClientMessage::Hello { protocol, .. } => Err(ServerMessage::Error {
            code: "protocol_mismatch".into(),
            message: format!("server speaks protocol {PROTOCOL_VERSION}, client sent {protocol}"),
        }),
        _ => Err(ServerMessage::Error { code: "handshake_required".into(), message: "the first message must be hello".into() }),
    }
}

/// The plugin. `EditorLink::from_env()` for games, `EditorLink::new(port)` to force it on
/// (port 0 picks a free one, see [`EditorLink::port_handle`]).
pub struct EditorLink {
    port: Option<u16>,
    project_dir: Option<std::path::PathBuf>,
    bound: std::sync::Arc<std::sync::atomic::AtomicU16>,
    restore: Option<std::path::PathBuf>,
}

impl EditorLink {
    /// Always on, listening on `127.0.0.1:port`.
    pub fn new(port: u16) -> Self {
        Self { port: Some(port), project_dir: None, bound: Default::default(), restore: None }
    }

    /// On only when `EFE_EDITOR=1`; port from `EFE_EDITOR_PORT` (default 7878).
    pub fn from_env() -> Self {
        let on = std::env::var("EFE_EDITOR").is_ok_and(|v| v == "1");
        let port = std::env::var("EFE_EDITOR_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7878);
        let restore = std::env::var_os("EFE_RESTORE_SNAPSHOT").filter(|v| !v.is_empty()).map(Into::into);
        Self { port: on.then_some(port), project_dir: None, bound: Default::default(), restore }
    }

    /// The disabled plugin (adds nothing).
    pub fn off() -> Self {
        Self { port: None, project_dir: None, bound: Default::default(), restore: None }
    }

    /// The directory `save_scene` / `load_scene` paths are relative to. Default: the
    /// `EFE_EDITOR_PROJECT` environment variable, else the working directory.
    pub fn with_project_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.project_dir = Some(dir.into());
        self
    }

    /// Restore this restart snapshot (a file written by the `restart_snapshot` request) on the
    /// second frame, after the game's own setup. `from_env` sets it from `EFE_RESTORE_SNAPSHOT`.
    pub fn with_restore(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.restore = Some(path.into());
        self
    }

    /// The port actually bound once the plugin is built (0 before, or when off or failed).
    pub fn port_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicU16> {
        self.bound.clone()
    }
}

impl Plugin for EditorLink {
    fn name(&self) -> &str {
        "efeengine-editor-link"
    }

    #[cfg(target_family = "wasm")]
    fn build(&self, _engine: &mut Engine) {}

    #[cfg(not(target_family = "wasm"))]
    fn build(&self, engine: &mut Engine) {
        let Some(port) = self.port else { return };
        let dir = self
            .project_dir
            .clone()
            .or_else(|| std::env::var_os("EFE_EDITOR_PROJECT").map(Into::into))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
        native::build(engine, port, &self.bound, dir, self.restore.clone());
    }
}

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::io;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use efeengine_core::engine::order;
    use efeengine_core::log::{NewDefaultLogger, set_global_logger};
    use efeengine_core::prelude::*;

    use crate::handler::{LinkState, RingLogger, handle};
    use crate::protocol::*;
    use crate::handshake;

    const MAX_CLIENTS: usize = 4;
    const QUEUE: usize = 256;

    type Writer = Arc<Mutex<TcpStream>>;

    struct Pending {
        id: u64,
        request: Request,
        out: Writer,
    }

    /// Stops the listener and connection threads when the drain system is dropped.
    struct Stop(Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    pub fn build(engine: &mut Engine, port: u16, bound: &AtomicU16, project_dir: std::path::PathBuf, restore: Option<std::path::PathBuf>) {
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(l) => l,
            Err(e) => {
                engine.logger.error(&format!("editor-link: cannot listen on 127.0.0.1:{port}: {e}"));
                return;
            }
        };
        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        bound.store(actual, Ordering::Relaxed);
        let _ = listener.set_nonblocking(true);

        // Record log lines for the `logs` request; the old logger keeps printing.
        let old = std::mem::replace(&mut engine.logger, Box::new(NoLog));
        engine.logger = Box::new(RingLogger { inner: old });
        set_global_logger(Box::new(RingLogger { inner: Box::new(NewDefaultLogger()) }));
        engine.logger.info(&format!("editor-link: listening on 127.0.0.1:{actual}"));

        let game = engine.api_index().game;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx): (SyncSender<Pending>, Receiver<Pending>) = sync_channel(QUEUE);
        let clients = Arc::new(AtomicUsize::new(0));
        {
            let stop = stop.clone();
            std::thread::spawn(move || accept_loop(listener, game, tx, stop, clients));
        }

        let guard = Stop(stop);
        let state: Arc<Mutex<LinkState>> = Arc::new(Mutex::new(LinkState::new(project_dir)));
        engine.resources_mut().insert(crate::gizmo::Selection::default());
        // The editor names the game's window so it can find and place it (E7.1).
        let mut title = std::env::var("EFE_WINDOW_TITLE").ok().filter(|t| !t.is_empty());
        let mut restore = restore.map(|p| (p, 0u32));
        engine
            .add_system(Stage::PostUpdate, move |ctx| {
                let _keep = &guard;
                if let Some((path, frames)) = &mut restore {
                    *frames += 1;
                    if *frames == 2 {
                        let path = path.clone();
                        restore = None;
                        ctx.commands.engine_task(move |e| restore_snapshot(e, &path));
                    }
                }
                while let Ok(p) = rx.try_recv() {
                    let (id, out) = (p.id, p.out);
                    let respond = Box::new(move |reply: Reply| {
                        if let Ok(mut w) = out.lock() {
                            let _ = send(&mut *w, &ServerMessage::Reply { id, reply });
                        }
                    });
                    handle(ctx, &p.request, &state, respond);
                }
                if let Some(t) = title.take() {
                    ctx.commands.set_title(t);
                }
                // Gizmo handles (E7.2): edit mode only, drawn by the game through DebugDraw2D.
                let editing = state.lock().map_or_else(|p| p.into_inner().mode, |s| s.mode) == Mode::Edit;
                if editing {
                    crate::gizmo::draw_selection(ctx.world, ctx.resources);
                }
            })
            .order(order::LATE + 100)
            .name("editor_link_drain");
        // Edit mode must not stop the link itself.
        if let Err(e) = engine.set_global_system("editor_link_drain") {
            engine.logger.error(&format!("editor-link: {e}"));
        }
    }

    /// Read the restart snapshot and restore it; every outcome is logged, never fatal.
    pub(crate) fn restore_snapshot(e: &mut Engine, path: &std::path::Path) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(err) => return e.logger.error(&format!("editor-link: cannot read restart snapshot {}: {err}", path.display())),
        };
        match efeengine_core::ecs::RestartSnapshot::from_ron(&text).and_then(|s| e.restore_lenient(&s)) {
            Ok(report) => {
                for w in &report.warnings {
                    e.logger.warn(&format!("restart restore: {w}"));
                }
                e.logger.info(&format!("restart restore: {} entities restored, {} warnings", report.entities, report.warnings.len()));
            }
            Err(err) => e.logger.error(&format!("restart restore failed, world left as the game set it up: {err}")),
        }
    }

    struct NoLog;
    impl efeengine_core::log::Logger for NoLog {
        fn logger_name(&self) -> &str { "none" }
        fn info(&mut self, _: &str) {}
        fn warn(&mut self, _: &str) {}
        fn error(&mut self, _: &str) {}
        fn alert(&mut self, _: &str) {}
        fn log(&mut self, _: &efeengine_core::log::LogMsg) {}
    }

    fn accept_loop(listener: TcpListener, game: String, tx: SyncSender<Pending>, stop: Arc<AtomicBool>, clients: Arc<AtomicUsize>) {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if clients.load(Ordering::Relaxed) >= MAX_CLIENTS {
                        continue; // dropped: the client sees the connection close
                    }
                    clients.fetch_add(1, Ordering::Relaxed);
                    let (game, tx, stop, clients) = (game.clone(), tx.clone(), stop.clone(), clients.clone());
                    std::thread::spawn(move || {
                        let _ = serve(stream, &game, tx, &stop);
                        clients.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    fn serve(stream: TcpStream, game: &str, tx: SyncSender<Pending>, stop: &AtomicBool) -> io::Result<()> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut reader = stream.try_clone()?;
        let out: Writer = Arc::new(Mutex::new(stream));
        let mut frames = FrameReader::default();
        let mut greeted = false;
        let write = |m: &ServerMessage| -> io::Result<()> {
            let mut w = out.lock().map_err(|_| io::Error::other("poisoned"))?;
            send(&mut *w, m)
        };
        while !stop.load(Ordering::Relaxed) {
            let Some(frame) = frames.poll(&mut reader)? else { continue };
            let msg: ClientMessage = match serde_json::from_slice(&frame) {
                Ok(m) => m,
                Err(e) => {
                    write(&ServerMessage::Error { code: "bad_message".into(), message: e.to_string() })?;
                    return Ok(());
                }
            };
            if !greeted {
                match handshake(&msg, game) {
                    Ok(hello) => {
                        write(&hello)?;
                        greeted = true;
                    }
                    Err(err) => {
                        write(&err)?;
                        return Ok(());
                    }
                }
                continue;
            }
            match msg {
                ClientMessage::Request { id, request } => {
                    if tx.try_send(Pending { id, request, out: out.clone() }).is_err() {
                        write(&ServerMessage::Reply { id, reply: Reply::error("busy", "request queue is full") })?;
                    }
                }
                ClientMessage::Hello { .. } => {
                    write(&ServerMessage::Error { code: "bad_message".into(), message: "already greeted".into() })?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
