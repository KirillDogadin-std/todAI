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

/// Child half of `concurrent_processes_never_see_busy`: appends 100 events
/// to the database in `$TODAI_MEMORY_TEST_DB`. Ignored when run normally.
#[test]
#[ignore]
fn writer_process() {
    let Some(path) = std::env::var_os("TODAI_MEMORY_TEST_DB") else { return };
    let mut m = Memory::open(Path::new(&path)).unwrap();
    for i in 0..100 {
        m.append(event(&format!("p{}-{i}", std::process::id()), "created")).unwrap();
    }
}

#[test]
fn concurrent_processes_never_see_busy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.sqlite");
    // Separate processes, as agents in parallel sessions would be: SQLite's
    // file locks, not just its in-process ones. Each re-runs this test binary.
    let children: Vec<_> = (0..4)
        .map(|_| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::writer_process", "--ignored", "--quiet", "--test-threads=1"])
                .env("TODAI_MEMORY_TEST_DB", &path)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success(), "a writer process failed");
    }
    let events = Memory::open(&path).unwrap().events(0).unwrap();
    assert_eq!(events.len(), 400);
    assert!(events.iter().enumerate().all(|(i, e)| e.seq == i as i64 + 1));
}

// ---- cards ----

fn draft(class: Class, gist: &str, keys: &[&str]) -> CardDraft {
    CardDraft {
        class: Some(class),
        gist: gist.into(),
        context: "Picking the window toolkit.".into(),
        why: "Memory.".into(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
        ..Default::default()
    }
}

fn mem() -> (tempfile::TempDir, Memory) {
    let dir = tempfile::tempdir().unwrap();
    let m = Memory::open(&dir.path().join("m.sqlite")).unwrap();
    (dir, m)
}

#[test]
fn validation_reports_every_problem() {
    let bad = CardDraft { gist: "x".repeat(MAX_GIST + 1), keys: vec!["  ".into()], ..Default::default() };
    let err = format!("{:#}", bad.validate().unwrap_err());
    for want in ["gist is 121 characters", "context is empty", "why is empty", "no keys"] {
        assert!(err.contains(want), "{want}: {err}");
    }
    let long_key = draft(Class::Context, "g", &["one two three four five six"]);
    assert!(format!("{:#}", long_key.validate().unwrap_err()).contains("too long"));
    let too_many = draft(Class::Context, "g", &["a", "b", "c", "d", "e", "f"]);
    assert!(format!("{:#}", too_many.validate().unwrap_err()).contains("6 keys"));
    let two_lines = draft(Class::Context, "a\nb", &["k"]);
    assert!(two_lines.validate().is_err());
}

#[test]
fn keys_are_normalised_and_deduped() {
    assert_eq!(normalize_key("  GPU  Backend. "), "gpu backend");
    assert_eq!(normalize_key("\"memory usage\"?"), "memory usage");
    assert_eq!(normalize_key("c++ build"), "c++ build", "inner punctuation stays");
    let d = draft(Class::Context, "g", &["GPU backend", "gpu  backend", "Memory"]).validate().unwrap();
    assert_eq!(d.keys, vec!["gpu backend", "memory"]);
}

#[test]
fn create_then_read_back() {
    let (_d, mut m) = mem();
    let source = Source { board: Some("splitter".into()), item: Some("d3".into()), session: None };
    let mut d = draft(Class::Context, "  egui over iced: memory ", &["GUI toolkit", "memory usage"]);
    d.rejected = "iced: 90 MB.".into();
    let c = m.create(d, Actor::Agent, Some(source.clone())).unwrap();
    assert!(c.id.starts_with('m') && c.id.len() == 9, "{}", c.id);
    assert_eq!((c.gist.as_str(), c.status), ("egui over iced: memory", Status::Active));

    assert_eq!(m.card(&c.id).unwrap(), Some(c.clone()));
    assert_eq!(m.card(&c.id[..5]).unwrap(), Some(c.clone()), "unique prefix");
    assert_eq!(m.card("m").unwrap(), None, "too short to be a prefix");
    assert_eq!(m.card("nope").unwrap(), None);
    assert_eq!(c.source, Some(source));

    // One created event holds the whole card.
    let events = m.events(0).unwrap();
    assert_eq!((events.len(), events[0].kind.as_str(), events[0].card.as_deref()), (1, "created", Some(c.id.as_str())));
    assert_eq!(events[0].payload["rejected"], "iced: 90 MB.");
}

#[test]
fn agent_axioms_wait_for_the_operator() {
    let (_d, mut m) = mem();
    let a = m.create(draft(Class::Axiom, "agent axiom", &["k"]), Actor::Agent, None).unwrap();
    let o = m.create(draft(Class::Axiom, "operator axiom", &["k"]), Actor::Operator, None).unwrap();
    assert_eq!((a.status, o.status), (Status::Proposed, Status::Active));
    assert_eq!(m.card(&a.id).unwrap().unwrap().status, Status::Proposed);
}

#[test]
fn find_ranks_by_matched_keys_then_class_then_newest() {
    let (_d, mut m) = mem();
    let one_off = m.create(draft(Class::OneOff, "one-off", &["memory usage"]), Actor::Agent, None).unwrap();
    let ctx_old = m.create(draft(Class::Context, "old context", &["memory usage"]), Actor::Agent, None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let ctx_new = m.create(draft(Class::Context, "new context", &["memory usage"]), Actor::Agent, None).unwrap();
    let axiom = m.create(draft(Class::Axiom, "axiom", &["memory usage"]), Actor::Operator, None).unwrap();
    let both = m.create(draft(Class::OneOff, "two keys", &["memory usage", "gpu backend"]), Actor::Agent, None).unwrap();
    m.create(draft(Class::Axiom, "unrelated", &["licensing"]), Actor::Operator, None).unwrap();

    let ids = |cards: Vec<Card>| cards.into_iter().map(|c| c.id).collect::<Vec<_>>();
    let hits = ids(m.find(&["Memory Usage".into(), "gpu backend".into()]).unwrap());
    assert_eq!(hits, vec![both.id, axiom.id, ctx_new.id, ctx_old.id, one_off.id]);
    assert_eq!(m.find(&["no such key".into()]).unwrap(), vec![]);

    let vocab = m.keys().unwrap();
    assert_eq!(vocab[0], ("memory usage".to_owned(), 5));
    assert!(vocab.contains(&("licensing".to_owned(), 1)));
}

#[test]
fn a_step_01_database_gains_the_card_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.sqlite");
    // What step 01 created: just the event log, at version 1.
    let c = Connection::open(&path).unwrap();
    c.execute_batch(schema::MIGRATIONS[0]).unwrap();
    c.pragma_update(None, "user_version", 1).unwrap();
    c.execute("INSERT INTO event (ts, actor, kind, payload) VALUES (1, 'agent', 'note', '{}')", []).unwrap();
    drop(c);

    let mut m = Memory::open(&path).unwrap();
    assert_eq!(user_version(&path), schema::VERSION);
    let card = m.create(draft(Class::Context, "g", &["k"]), Actor::Agent, None).unwrap();
    assert_eq!(m.events(0).unwrap().len(), 2);
    assert_eq!(m.find(&["k".into()]).unwrap(), vec![card]);
}

#[test]
fn formats_fit_the_token_budget() {
    let (_d, mut m) = mem();
    let mut d = draft(Class::Context, "egui over iced: memory", &["gui toolkit", "memory usage"]);
    d.context = "A resident board app that sits next to every agent session all day. ".repeat(8);
    d.why = "Idle memory decides whether people keep it open. ".repeat(8);
    d.applies = "Long-running desktop windows.".into();
    let source = Source { board: Some("splitter".into()), item: Some("d3".into()), session: None };
    let c = m.create(d, Actor::Agent, Some(source)).unwrap();

    let line = format::gist_line(&c);
    assert_eq!(line, format!("{} · context · egui over iced: memory", c.id));
    let hit = format::hit_block(&c);
    // ~4 characters per token: about 100 tokens.
    assert!(hit.len() < 650, "{} chars:\n{hit}", hit.len());
    assert!(hit.contains("from splitter/d3") && hit.contains("…"), "{hit}");
    let full = format::full(&c);
    assert!(full.contains("## Why") && full.contains(&c.why.trim().to_owned()), "{full}");
}

// ---- recall and snapping (step 03) ----

fn e() -> &'static dyn embed::Embedder {
    embed::default()
}

#[test]
fn recall_finds_cards_through_nearby_keys() {
    let (_d, mut m) = mem();
    let mem_card = m.create(draft(Class::Context, "egui: idle memory", &["memory usage"]), Actor::Agent, None).unwrap();
    let gpu = m.create(draft(Class::Context, "glow, not wgpu", &["gpu backend"]), Actor::Agent, None).unwrap();
    m.create(draft(Class::OneOff, "unrelated", &["release workflow"]), Actor::Agent, None).unwrap();

    let hits = m.recall(&["RAM usage".into()], e()).unwrap();
    assert_eq!(hits.iter().map(|h| &h.card.id).collect::<Vec<_>>(), vec![&mem_card.id]);
    let (phrase, key, s) = &hits[0].via[0];
    assert_eq!((phrase.as_str(), key.as_str()), ("RAM usage", "memory usage"));
    assert!(*s > 0.65 && *s < 0.9, "{s}");

    // Union, ranked by how many phrases reached a card.
    let both = m.create(draft(Class::OneOff, "both", &["memory usage", "gpu backend"]), Actor::Agent, None).unwrap();
    let hits = m.recall(&["memory use".into(), "graphics backend".into()], e()).unwrap();
    let ids: Vec<&str> = hits.iter().map(|h| h.card.id.as_str()).collect();
    assert_eq!(ids[0], both.id, "{hits:#?}");
    assert!(ids.contains(&gpu.id.as_str()) && ids.contains(&mem_card.id.as_str()));
    assert_eq!(hits[0].via.len(), 2);

    assert_eq!(m.recall(&["font rendering".into()], e()).unwrap(), vec![], "nothing near: new territory");
}

#[test]
fn new_keys_snap_ask_or_stay() {
    let (_d, mut m) = mem();
    m.create(draft(Class::Context, "first", &["memory usage", "http server"]), Actor::Agent, None).unwrap();

    // A plural snaps onto the existing key.
    let (c, snaps) = m.remember(draft(Class::Context, "g", &["Memory Usages"]), Actor::Agent, None, e(), &[]).unwrap();
    assert_eq!(c.keys, vec!["memory usage"]);
    assert_eq!((snaps[0].from.as_str(), snaps[0].to.as_str()), ("memory usages", "memory usage"));

    // A possible synonym is refused, naming the near key...
    let err = m.remember(draft(Class::Context, "g", &["ram usage", "web server"]), Actor::Agent, None, e(), &[]);
    let err = format!("{:#}", err.unwrap_err());
    assert!(err.contains("\"ram usage\" is close to \"memory usage\"") && err.contains("\"web server\""), "{err}");
    assert_eq!(m.keys().unwrap().len(), 2, "nothing written");
    // ...unless confirmed as new.
    let new = ["RAM usage".to_owned(), "web server".to_owned()];
    let (c, _) = m.remember(draft(Class::Context, "g", &["ram usage", "web server"]), Actor::Agent, None, e(), &new).unwrap();
    assert_eq!(c.keys, vec!["ram usage", "web server"]);

    // Unrelated keys are just new.
    let (c, snaps) = m.remember(draft(Class::Context, "g", &["font rendering"]), Actor::Agent, None, e(), &[]).unwrap();
    assert_eq!((c.keys, snaps), (vec!["font rendering".to_owned()], vec![]));
}

/// A deliberately different embedder: vectors from a word hash.
struct Toy;

impl embed::Embedder for Toy {
    fn id(&self) -> &str {
        "toy/1"
    }
    fn embed(&self, phrase: &str) -> Option<Vec<f32>> {
        let mut v = vec![0f32; 16];
        for w in phrase.split_whitespace() {
            v[(fnv1a(w.as_bytes()) % 16) as usize] += 1.0;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        (n > 0.0).then(|| v.iter().map(|x| x / n).collect())
    }
    fn snap(&self) -> f32 {
        0.99
    }
    fn ask(&self) -> f32 {
        0.6
    }
    fn recall(&self) -> f32 {
        0.5
    }
}

#[test]
fn swapping_the_embedder_needs_no_migration() {
    let (_d, mut m) = mem();
    let card = m.create(draft(Class::Context, "g", &["memory usage"]), Actor::Agent, None).unwrap();
    assert_eq!(m.recall(&["ram usage".into()], e()).unwrap()[0].card.id, card.id);
    // Another embedder computes its own vectors on first use, next to the
    // old ones; recall works with it at once, and switching back is free.
    let toy = m.recall(&["memory".into()], &Toy).unwrap();
    assert_eq!(toy[0].card.id, card.id, "shares the word: toy similarity 0.71");
    assert_eq!(m.recall(&["ram usage".into()], e()).unwrap()[0].card.id, card.id);
    let models: Vec<String> = {
        let c = Connection::open(m.path()).unwrap();
        let mut s = c.prepare("SELECT DISTINCT model FROM key_vector ORDER BY model").unwrap();
        s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(models, vec!["potion-base-8M/int8/1", "toy/1"]);
}
