//! Wire types of the todai server's HTTP API, shared by the server and its
//! clients. The endpoints are documented in `docs/server.md`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::Event;

/// Where `todai-server` listens unless told otherwise.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7341";

/// Longest long-poll a client may ask for, in seconds.
pub const MAX_WAIT_SECS: u64 = 60;

/// `GET /api/health`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Health {
    pub version: String,
}

/// One registered board, as listed by `GET /api/boards`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoardInfo {
    /// Opaque id used in URLs; see [`board_id`].
    pub id: String,
    /// The file name without `.jsonl`.
    pub name: String,
    /// Where the board lives on the server's machine, for display only.
    pub path: PathBuf,
}

impl BoardInfo {
    pub fn new(path: PathBuf) -> BoardInfo {
        BoardInfo { id: board_id(&path), name: board_name(&path), path }
    }
}

/// `POST /api/boards`: start serving the board at `path` (absolute).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Register {
    pub path: PathBuf,
}

/// `GET /api/boards/{id}/events?after=SEQ&wait=SECS`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Events {
    /// Events with `seq > after`, in order.
    pub events: Vec<Event>,
    pub last_seq: u64,
    /// `after` was past the end of the log (the file was replaced): `events`
    /// is the whole log and the client must start over from it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
}

/// The body of every non-2xx reply.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApiError {
    pub error: String,
}

/// A stable id for a board path: 16 hex digits of its FNV-1a hash. URLs carry
/// this instead of the path, so clients can never name arbitrary files.
pub fn board_id(path: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn board_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name.strip_suffix(".jsonl").unwrap_or(&name).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_id_is_stable() {
        // Changing the hash would orphan every remote tab saved in a session.
        assert_eq!(board_id(Path::new("/a/b.jsonl")), board_id(Path::new("/a/b.jsonl")));
        assert_ne!(board_id(Path::new("/a/b.jsonl")), board_id(Path::new("/a/c.jsonl")));
        assert_eq!(board_id(Path::new("")), "cbf29ce484222325");
        assert_eq!(board_name(Path::new("/x/.agents/todai/board.jsonl")), "board");
    }
}
