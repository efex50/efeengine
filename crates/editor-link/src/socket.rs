//! The local socket the game and the editor talk over: a Unix domain socket file on Unix, a
//! named pipe `\\.\pipe\<name>` on Windows. No port, and not reachable by other users.
//!
//! The name is internal: the editor picks it ([`new_name`]) and hands it to the game in
//! `EFE_EDITOR_SOCKET`. The game listens, the editor connects.
//!
//! Unix: the socket file lives in `$XDG_RUNTIME_DIR/efeengine/`, else
//! `<temp dir>/efeengine-<uid>/` (the directory is created with mode 0700, the file gets 0600).
//! A name that contains a `/` is used as a file path as it is (tests use this).

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::{Listener as _, Stream as _};
use interprocess::local_socket::{ListenerNonblockingMode, ListenerOptions, Stream};

pub use interprocess::local_socket::Listener;

/// A fresh socket name for this process: `efe-editor-<pid>`.
pub fn new_name() -> String {
    format!("efe-editor-{}", std::process::id())
}

#[cfg(unix)]
mod sys {
    use super::*;
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    use interprocess::os::unix::local_socket::ListenerOptionsExt;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;

    /// The directory sockets live in; created with mode 0700.
    fn socket_dir() -> io::Result<PathBuf> {
        let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            Some(rt) => PathBuf::from(rt).join("efeengine"),
            // SAFETY: geteuid has no preconditions.
            None => std::env::temp_dir().join(format!("efeengine-{}", unsafe { libc::geteuid() })),
        };
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        // `recursive` keeps an existing directory as it is; make sure it is private.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(dir)
    }

    /// The socket file for `name`.
    pub fn socket_path(name: &str) -> io::Result<PathBuf> {
        if name.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty socket name"));
        }
        if name.contains('/') {
            return Ok(PathBuf::from(name));
        }
        Ok(socket_dir()?.join(name))
    }

    pub fn connect(name: &str) -> io::Result<Stream> {
        let path = socket_path(name)?;
        Stream::connect(path.to_fs_name::<GenericFilePath>()?)
    }

    pub fn listen(name: &str) -> io::Result<Listener> {
        let path = socket_path(name)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        if path.exists() {
            // A live server on this name is not ours to replace; a dead one leaves a stale file.
            if connect(name).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("socket `{name}` is in use")));
            }
            std::fs::remove_file(&path)?;
        }
        let l = ListenerOptions::new()
            .name(path.clone().to_fs_name::<GenericFilePath>()?)
            .nonblocking(ListenerNonblockingMode::Both)
            .mode(0o600)
            .create_sync()?;
        // Belt and braces: the mode option is applied through the umask on some systems.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(l)
    }
}

#[cfg(windows)]
mod sys {
    use super::*;
    use interprocess::local_socket::{GenericNamespaced, ToNsName};

    pub fn connect(name: &str) -> io::Result<Stream> {
        Stream::connect(name.to_ns_name::<GenericNamespaced>()?)
    }

    pub fn listen(name: &str) -> io::Result<Listener> {
        ListenerOptions::new()
            .name(name.to_ns_name::<GenericNamespaced>()?)
            .nonblocking(ListenerNonblockingMode::Both)
            .create_sync()
    }
}

#[cfg(unix)]
pub use sys::socket_path;

/// Listen on `name`. A stale socket file is replaced; a name in use by a live server is an
/// `AddrInUse` error. `accept` returns `WouldBlock` when nobody is connecting.
pub fn listen(name: &str) -> io::Result<Listener> {
    sys::listen(name)
}

/// Accept one connection if there is one (`Ok(None)` otherwise).
pub fn accept(listener: &Listener) -> io::Result<Option<Conn>> {
    match listener.accept() {
        Ok(s) => Ok(Some(Conn::new(s)?)),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}

/// Connect to the server on `name`, giving up after `timeout`.
pub fn connect(name: &str, timeout: Duration) -> io::Result<Conn> {
    let _ = timeout; // local connects either work at once or fail at once
    Conn::new(sys::connect(name)?)
}

/// A connected local socket with read and write timeouts. It is nonblocking underneath, so the
/// timeouts work the same on Unix sockets and Windows pipes (which have none of their own).
/// Clones share the connection, so one thread can read while another writes.
#[derive(Clone)]
pub struct Conn {
    stream: Arc<Stream>,
    read: Option<Duration>,
    write: Option<Duration>,
}

impl Conn {
    fn new(stream: Stream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream: Arc::new(stream), read: None, write: None })
    }

    /// `None` waits forever. A timed-out read fails with `TimedOut`.
    pub fn set_read_timeout(&mut self, t: Option<Duration>) {
        self.read = t;
    }

    pub fn set_write_timeout(&mut self, t: Option<Duration>) {
        self.write = t;
    }
}

fn wait<T>(timeout: Option<Duration>, mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let start = Instant::now();
    loop {
        match op() {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if timeout.is_some_and(|t| start.elapsed() >= t) {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        wait(self.read, || (&*self.stream).read(buf))
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        wait(self.write, || (&*self.stream).write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        wait(self.write, || (&*self.stream).flush())
    }
}
