use std::path::Path;

use rusqlite::Connection;
use serde_json::json;

use super::*;

fn event(card: &str, kind: &str) -> NewEvent {
    NewEvent { actor: Actor::Agent, card: Some(card.into()), kind: kind.into(), payload: json!({"gist": card}), source: None }
}

fn user_version(path: &Path) -> i64 {
    Connection::open(path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}

#[test]
fn fresh_db_migrates_and_reopens_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/m.sqlite");
    let mut m = Memory::open(&path).unwrap();
    assert_eq!(user_version(&path), schema::VERSION);
    assert_eq!(m.last_seq().unwrap(), 0);

    let source = Source { board: Some("splitter".into()), item: Some("d39".into()), session: None };
    let e = m.append(NewEvent { source: Some(source.clone()), actor: Actor::Operator, ..event("c1", "created") }).unwrap();
    assert_eq!((e.seq, e.actor), (1, Actor::Operator));
    assert!(e.ts > 1_700_000_000_000, "milliseconds: {}", e.ts);
    drop(m);

    let m = Memory::open(&path).unwrap();
    assert_eq!(user_version(&path), schema::VERSION);
    let events = m.events(0).unwrap();
    assert_eq!(events, vec![e]);
    assert_eq!(events[0].source, Some(source));
    assert_eq!(events[0].payload, json!({"gist": "c1"}));
    assert_eq!(m.events(1).unwrap(), vec![]);
}

#[test]
fn concurrent_writers_never_see_busy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.sqlite");
    // Both open a fresh file at once: migration 1 must run exactly once.
    let writers: Vec<_> = (0..4)
        .map(|w| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut m = Memory::open(&path).unwrap();
                for i in 0..100 {
                    m.append(event(&format!("w{w}-{i}"), "created")).unwrap();
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    let events = Memory::open(&path).unwrap().events(0).unwrap();
    assert_eq!(events.len(), 400);
    assert!(events.iter().enumerate().all(|(i, e)| e.seq == i as i64 + 1), "seqs are 1..=400 without gaps");
}

#[test]
fn older_db_is_migrated_and_keeps_what_it_had() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.sqlite");
    // A version-0 database: exists, has something in it, no schema of ours.
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE note (x TEXT); INSERT INTO note VALUES ('kept');").unwrap();
    drop(c);
    assert_eq!(user_version(&path), 0);

    let mut m = Memory::open(&path).unwrap();
    assert_eq!(user_version(&path), schema::VERSION);
    m.append(event("c1", "created")).unwrap();
    let kept: String = Connection::open(&path).unwrap().query_row("SELECT x FROM note", [], |r| r.get(0)).unwrap();
    assert_eq!(kept, "kept");
}

#[test]
fn newer_db_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.sqlite");
    Connection::open(&path).unwrap().pragma_update(None, "user_version", schema::VERSION + 1).unwrap();
    let err = Memory::open(&path).err().expect("refused");
    assert!(format!("{err:#}").contains("update todai"), "{err:#}");
}

#[test]
fn events_cannot_be_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.sqlite");
    Memory::open(&path).unwrap().append(event("c1", "created")).unwrap();
    let c = Connection::open(&path).unwrap();
    for sql in ["UPDATE event SET kind = 'forged'", "DELETE FROM event"] {
        let err = c.execute(sql, []).expect_err(sql);
        assert!(err.to_string().contains("append-only"), "{sql}: {err}");
    }
    let bad = "INSERT INTO event (ts, actor, kind, payload) VALUES (0, 'agent', 'x', 'not json')";
    assert!(c.execute(bad, []).is_err(), "payload must be JSON");
}

#[test]
fn project_files_are_readable_stable_and_distinct() {
    // Pinned (checked against an independent FNV-1a): changing it orphans every saved memory.
    assert_eq!(file_name(Path::new("/home/u/code/todai")), "todai-b0fe89afb2b5feed.sqlite");
    assert_ne!(file_name(Path::new("/a/api")), file_name(Path::new("/b/api")));
    assert!(file_name(Path::new("/x/my repo!")).starts_with("my_repo_-"));
    assert!(file_name(Path::new("/")).starts_with("root-"));
}
