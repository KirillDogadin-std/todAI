//! Decision memory: an append-only event store in SQLite.
//!
//! One database per project, under `$XDG_STATE_HOME/todai/memory/`
//! ([`path_for`]). Every change is an [`Event`] row appended by
//! [`Memory::append`] in its own transaction; rows are never updated or
//! deleted (triggers refuse it), so the reasoning behind a decision can't be
//! rewritten. Current state (cards, keys, edges) is derived from events.
//!
//! This crate doesn't depend on the rest of todai, so other programs can
//! use it as is. The database's schema is internal: read it through this
//! crate.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

mod schema;
#[cfg(test)]
mod tests;

/// How long a writer waits for another connection's transaction to finish
/// before giving up with `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    Agent,
    Operator,
}

impl Actor {
    fn as_str(self) -> &'static str {
        match self {
            Actor::Agent => "agent",
            Actor::Operator => "operator",
        }
    }

    fn parse(s: &str) -> Result<Actor> {
        match s {
            "agent" => Ok(Actor::Agent),
            "operator" => Ok(Actor::Operator),
            _ => bail!("unknown actor {s:?}"),
        }
    }
}

/// Where an event came from, as far as the writer knows.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// The board, by name (`splitter` for `.agents/todai/splitter.jsonl`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    /// The board item it was decided in (`d39`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// The agent session, if the agent knows its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// An event to append; the store assigns `seq` and `ts`.
#[derive(Clone, Debug, PartialEq)]
pub struct NewEvent {
    pub actor: Actor,
    /// The card it is about; `None` for events about the store itself.
    pub card: Option<String>,
    /// What happened (`created`, `retired`, ...). Event kinds and their
    /// payloads are defined by the steps that introduce cards.
    pub kind: String,
    pub payload: serde_json::Value,
    pub source: Option<Source>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub seq: i64,
    /// Milliseconds since the Unix epoch.
    pub ts: i64,
    pub actor: Actor,
    pub card: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
    pub source: Option<Source>,
}

pub struct Memory {
    conn: Connection,
    path: PathBuf,
}

impl Memory {
    /// Opens the database at `path`, creating it (and its directory) if
    /// needed, and brings its schema up to date.
    pub fn open(path: &Path) -> Result<Memory> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let mut conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // WAL: readers never block the writer, and a crash can't corrupt
        // committed events. NORMAL sync is durable with WAL except for the
        // last transactions on power loss, which is fine for this data.
        set_wal(&conn).with_context(|| format!("{}: switch to WAL", path.display()))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        schema::migrate(&mut conn).with_context(|| format!("migrate {}", path.display()))?;
        Ok(Memory { conn, path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one event in its own transaction and returns it as stored.
    pub fn append(&mut self, e: NewEvent) -> Result<Event> {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let payload = serde_json::to_string(&e.payload)?;
        let source = e.source.as_ref().map(serde_json::to_string).transpose()?;
        // IMMEDIATE takes the write lock up front, so two writers queue on
        // busy_timeout instead of one failing on lock upgrade.
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO event (ts, actor, card, kind, payload, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ts, e.actor.as_str(), e.card, e.kind, payload, source],
        )?;
        let seq = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Event { seq, ts, actor: e.actor, card: e.card, kind: e.kind, payload: e.payload, source: e.source })
    }

    /// Events with `seq > after`, oldest first.
    pub fn events(&self, after: i64) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, ts, actor, card, kind, payload, source FROM event WHERE seq > ?1 ORDER BY seq")?;
        let rows = stmt.query_map([after], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (seq, ts, actor, card, kind, payload, source) = row?;
            events.push(Event {
                seq,
                ts,
                actor: Actor::parse(&actor)?,
                card,
                kind,
                payload: serde_json::from_str(&payload).with_context(|| format!("event {seq}: payload"))?,
                source: source
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .with_context(|| format!("event {seq}: source"))?,
            });
        }
        Ok(events)
    }

    /// The newest event's `seq`, 0 when empty.
    pub fn last_seq(&self) -> Result<i64> {
        let seq: Option<i64> = self.conn.query_row("SELECT max(seq) FROM event", [], |r| r.get(0)).optional()?.flatten();
        Ok(seq.unwrap_or(0))
    }
}

/// WAL is stored in the file, so only a new database needs switching. That
/// takes an exclusive lock, and SQLite reports BUSY for it without waiting
/// on the busy handler, so retry when several connections open a new
/// database at once.
fn set_wal(conn: &Connection) -> Result<()> {
    let deadline = std::time::Instant::now() + BUSY_TIMEOUT;
    loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0)) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(mode) => bail!("journal mode stays {mode}"),
            Err(e) if is_busy(&e) && std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

/// `$XDG_STATE_HOME/todai/memory` (else `~/.local/state/todai/memory`).
pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("todai").join("memory"))
}

/// The database for the project rooted at `root` (an absolute path):
/// `NAME-HASH.sqlite` in [`dir`], NAME being the folder's name (readable)
/// and HASH a hash of the whole path (two `api` folders don't collide).
/// Moving the project means a new, empty memory.
pub fn path_for(root: &Path) -> Option<PathBuf> {
    Some(dir()?.join(file_name(root)))
}

fn file_name(root: &Path) -> String {
    let name: String = root
        .file_name()
        .map(|n| n.to_string_lossy().chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect())
        .filter(|n: &String| !n.is_empty())
        .unwrap_or_else(|| "root".into());
    format!("{name}-{:016x}.sqlite", fnv1a(root.as_os_str().as_encoded_bytes()))
}

/// FNV-1a, 64 bit: stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}
