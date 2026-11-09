//! Schema migrations. `PRAGMA user_version` holds how many have run; each
//! migration runs once, in order, in the same transaction as its version
//! bump. Append new ones at the end, never edit a released one.

use anyhow::{Result, bail};
use rusqlite::{Connection, TransactionBehavior};

pub(crate) const MIGRATIONS: &[&str] = &[
    // 1: the event log. Rows are never updated or deleted.
    "CREATE TABLE event (
        seq     INTEGER PRIMARY KEY,
        ts      INTEGER NOT NULL,
        actor   TEXT NOT NULL CHECK (actor IN ('agent', 'operator')),
        card    TEXT,
        kind    TEXT NOT NULL,
        payload TEXT NOT NULL CHECK (json_valid(payload)),
        source  TEXT CHECK (source IS NULL OR json_valid(source))
    );
    CREATE INDEX event_card ON event (card, seq);
    CREATE TRIGGER event_no_update BEFORE UPDATE ON event
        BEGIN SELECT RAISE(ABORT, 'memory events are append-only'); END;
    CREATE TRIGGER event_no_delete BEFORE DELETE ON event
        BEGIN SELECT RAISE(ABORT, 'memory events are append-only'); END;",
    // 2: cards and keys, derived from `created` events (rebuildable from
    // the log; never the only copy of anything).
    "CREATE TABLE card (
        id      TEXT PRIMARY KEY,
        class   TEXT NOT NULL,
        status  TEXT NOT NULL,
        gist    TEXT NOT NULL,
        created INTEGER NOT NULL REFERENCES event (seq)
    );
    CREATE TABLE key (
        id     INTEGER PRIMARY KEY,
        phrase TEXT NOT NULL UNIQUE
    );
    CREATE TABLE tagged (
        key  INTEGER NOT NULL REFERENCES key (id),
        card TEXT NOT NULL REFERENCES card (id),
        PRIMARY KEY (key, card)
    ) WITHOUT ROWID;
    CREATE INDEX tagged_card ON tagged (card);",
];

/// The schema version this build writes.
pub const VERSION: i64 = MIGRATIONS.len() as i64;

pub fn migrate(conn: &mut Connection) -> Result<()> {
    // Read the version inside a write transaction: two processes opening a
    // fresh database at once must not both run migration 1.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > VERSION {
        bail!("the memory database is at schema {current}, this todai knows up to {VERSION}: update todai");
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
    }
    tx.commit()?;
    Ok(())
}
