//! The server's files in its state dir: the access token, the registered
//! boards, and the address of the running server.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use todai_core::api::{BoardInfo, board_id};

/// Boards the server exposes, persisted in `boards.json`. Only files
/// registered here can be reached through the API. Every call reads the file,
/// so `todai-server forget` takes effect on a running server too.
pub struct Registry {
    file: PathBuf,
    /// Serializes this process's read-modify-write cycles.
    write: Mutex<()>,
}

impl Registry {
    pub fn load(dir: &Path) -> Result<Registry> {
        let registry = Registry { file: dir.join("boards.json"), write: Mutex::new(()) };
        registry.read()?; // fail early on a corrupt file
        Ok(registry)
    }

    fn read(&self) -> Result<BTreeMap<String, PathBuf>> {
        let paths: Vec<PathBuf> = match std::fs::read_to_string(&self.file) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("parse {}", self.file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("read {}", self.file.display())),
        };
        Ok(paths.into_iter().map(|p| (board_id(&p), p)).collect())
    }

    /// Unreadable means empty: a request must never see a half-broken registry.
    pub fn list(&self) -> Vec<BoardInfo> {
        self.read().unwrap_or_default().into_values().map(BoardInfo::new).collect()
    }

    pub fn get(&self, id: &str) -> Option<PathBuf> {
        self.read().ok()?.remove(id)
    }

    /// Registers an existing board file. Adding one twice is a no-op.
    pub fn add(&self, path: &Path) -> Result<BoardInfo> {
        if !path.is_absolute() {
            bail!("board path must be absolute: {}", path.display());
        }
        if path.extension().is_none_or(|e| e != "jsonl") {
            bail!("not a board (.jsonl): {}", path.display());
        }
        if !path.is_file() {
            bail!("no such board: {}", path.display());
        }
        let _guard = self.write.lock().unwrap();
        let mut boards = self.read()?;
        if boards.insert(board_id(path), path.to_owned()).is_none() {
            self.save(&boards)?;
        }
        Ok(BoardInfo::new(path.to_owned()))
    }

    /// Unregisters a board; whether it was registered.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let _guard = self.write.lock().unwrap();
        let mut boards = self.read()?;
        let removed = boards.remove(id).is_some();
        if removed {
            self.save(&boards)?;
        }
        Ok(removed)
    }

    fn save(&self, boards: &BTreeMap<String, PathBuf>) -> Result<()> {
        let paths: Vec<&PathBuf> = boards.values().collect();
        write_atomic(&self.file, serde_json::to_string_pretty(&paths)?.as_bytes(), 0o644)
    }
}

/// The token in `token`, created on first use.
pub fn load_token(dir: &Path) -> Result<String> {
    match std::fs::read_to_string(dir.join("token")) {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_owned()),
        Ok(_) => rotate_token(dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => rotate_token(dir),
        Err(e) => Err(e).context("read server token"),
    }
}

/// Writes a fresh random token, readable only by the user.
pub fn rotate_token(dir: &Path) -> Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    write_atomic(&dir.join("token"), token.as_bytes(), 0o600)?;
    Ok(token)
}

/// Records where this server answers locally. A wildcard bind is reachable
/// on loopback, which is what local clients should use.
pub fn write_addr(dir: &Path, mut addr: SocketAddr) -> Result<()> {
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr.ip() {
            IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
        });
    }
    write_atomic(&dir.join("addr"), addr.to_string().as_bytes(), 0o644)
}

/// Write-then-rename so readers never see half a file.
fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
    f.write_all(data)?;
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
}
