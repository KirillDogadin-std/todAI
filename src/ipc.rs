//! Single-instance plumbing: the GUI listens on a unix socket, and every other
//! `todai open` hands it paths instead of starting a second window.
//! Protocol: one JSON request line, one JSON reply line.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    /// Open the file in a tab, or switch to it if already open.
    Open { path: PathBuf },
    /// Raise the window.
    Focus,
    /// Report open tabs.
    List,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<PathBuf>,
    #[serde(default)]
    pub active: usize,
}

impl Reply {
    pub fn ok() -> Reply {
        Reply { ok: true, ..Default::default() }
    }
}

/// `$TODAI_SOCKET`, else `$XDG_RUNTIME_DIR/todai.sock`.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TODAI_SOCKET") {
        return p.into();
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("todai-{}", unsafe { libc::getuid() })));
    dir.join("todai.sock")
}

/// Sends one request. `Ok(None)` means no instance is listening.
pub fn send(socket: &Path, req: &Request) -> Result<Option<Reply>> {
    let mut stream = match UnixStream::connect(socket) {
        Ok(s) => s,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = serde_json::to_vec(req)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(Some(serde_json::from_str(&reply)?))
}

pub enum Bind {
    Listening(UnixListener),
    /// Another live instance owns the socket.
    Taken,
}

/// Claims the socket, replacing a stale one left by a crashed instance.
pub fn bind(socket: &Path) -> Result<Bind> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match UnixListener::bind(socket) {
        Ok(l) => Ok(Bind::Listening(l)),
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            if UnixStream::connect(socket).is_ok() {
                return Ok(Bind::Taken);
            }
            std::fs::remove_file(socket)?;
            Ok(Bind::Listening(UnixListener::bind(socket)?))
        }
        Err(e) => Err(e.into()),
    }
}

/// A request from a client plus the channel its reply goes back on.
pub type Incoming = (Request, mpsc::Sender<Reply>);

/// Accepts connections on a background thread and forwards each request to
/// `tx`. `wake` runs after every request so the GUI processes it promptly.
pub fn serve(listener: UnixListener, tx: mpsc::Sender<Incoming>, wake: impl Fn() + Send + Clone + 'static) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            let wake = wake.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &tx, wake);
            });
        }
    });
}

fn handle(stream: UnixStream, tx: &mpsc::Sender<Incoming>, wake: impl Fn()) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let reply = match serde_json::from_str::<Request>(&line) {
        Ok(req) => {
            let (reply_tx, reply_rx) = mpsc::channel();
            tx.send((req, reply_tx))?;
            wake();
            reply_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| Reply { error: Some("GUI did not answer".into()), ..Default::default() })
        }
        Err(e) => Reply { error: Some(format!("bad request: {e}")), ..Default::default() },
    };
    let mut out = serde_json::to_vec(&reply)?;
    out.push(b'\n');
    (&stream).write_all(&out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_and_takeover() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        assert_eq!(send(&sock, &Request::Focus).unwrap(), None, "nobody listening");

        let Bind::Listening(l) = bind(&sock).unwrap() else { panic!("socket free") };
        assert!(matches!(bind(&sock).unwrap(), Bind::Taken));

        let (tx, rx) = mpsc::channel::<Incoming>();
        serve(l, tx, || {});
        std::thread::spawn(move || {
            for (req, reply) in rx {
                let r = match req {
                    Request::List => Reply { ok: true, tabs: vec!["/a".into()], active: 0, error: None },
                    _ => Reply::ok(),
                };
                reply.send(r).unwrap();
            }
        });
        let open = Request::Open { path: "/x/b.jsonl".into() };
        assert_eq!(send(&sock, &open).unwrap(), Some(Reply::ok()));
        assert_eq!(send(&sock, &Request::List).unwrap().unwrap().tabs, vec![PathBuf::from("/a")]);
    }

    #[test]
    fn stale_socket_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        drop(UnixListener::bind(&sock).unwrap()); // leaves the file behind
        assert!(sock.exists());
        assert!(matches!(bind(&sock).unwrap(), Bind::Listening(_)));
    }
}
