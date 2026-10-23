use super::*;

fn tmp_log() -> (tempfile::TempDir, Log) {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path().join("x.jsonl"));
    (dir, log)
}

fn add_decision(log: &Log, title: &str) -> String {
    let e = log
        .append(Actor::Agent, |s| {
            Ok(Kind::DecisionAdd {
                id: s.next_decision_id(),
                title: title.into(),
                context: None,
                options: vec![],
            })
        })
        .unwrap();
    match e.kind {
        Kind::DecisionAdd { id, .. } => id,
        _ => unreachable!(),
    }
}

#[test]
fn missing_file_is_empty() {
    let (_d, log) = tmp_log();
    assert!(log.load().unwrap().is_empty());
}

#[test]
fn resolve_flow_and_ids() {
    let (_d, log) = tmp_log();
    assert_eq!(add_decision(&log, "a?"), "d1");
    assert_eq!(add_decision(&log, "b?"), "d2");
    log.append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id: "d2".into(), text: "yes".into() }))
        .unwrap();

    let (_, s) = log.load_state().unwrap();
    assert_eq!(s.decision("d1").unwrap().status, DecisionStatus::Pending);
    assert!(matches!(&s.decision("d2").unwrap().status, DecisionStatus::Resolved { text, .. } if text == "yes"));
    assert_eq!(s.last_seq, 3);
}

#[test]
fn actor_and_target_validation() {
    let (_d, log) = tmp_log();
    add_decision(&log, "a?");
    let agent_resolves =
        log.append(Actor::Agent, |_| Ok(Kind::DecisionResolve { id: "d1".into(), text: "x".into() }));
    assert!(agent_resolves.is_err());
    let unknown =
        log.append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id: "d9".into(), text: "x".into() }));
    assert!(unknown.is_err());
    let reopen_pending = log.append(Actor::Operator, |_| Ok(Kind::DecisionReopen { id: "d1".into() }));
    assert!(reopen_pending.is_err());
    // Rejected appends leave the log untouched.
    assert_eq!(log.load().unwrap().len(), 1);
}

#[test]
fn unprocessed_until_acked() {
    let (_d, log) = tmp_log();
    add_decision(&log, "a?");
    log.append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id: "d1".into(), text: "x".into() }))
        .unwrap();
    let (ev, s) = log.load_state().unwrap();
    assert_eq!(unprocessed(&ev, &s).len(), 1);

    log.append(Actor::Agent, |s| Ok(Kind::Ack { upto_seq: s.last_seq })).unwrap();
    let (ev, s) = log.load_state().unwrap();
    assert!(unprocessed(&ev, &s).is_empty());
}

#[test]
fn elaboration_roundtrip() {
    let (_d, log) = tmp_log();
    add_decision(&log, "Use X or Y?");
    log.append(Actor::Operator, |s| {
        Ok(Kind::ElabRequest {
            rid: s.next_elab_id(),
            target: "d1".into(),
            selection: "X".into(),
            note: Some("what is X".into()),
        })
    })
    .unwrap();
    log.append(Actor::Agent, |_| Ok(Kind::ElabAdd { rid: "r1".into(), text_md: "X is **fast**".into() }))
        .unwrap();
    let (_, s) = log.load_state().unwrap();
    assert_eq!(s.elab("r1").unwrap().answer.as_deref(), Some("X is **fast**"));
}

#[test]
fn torn_trailing_line_ignored() {
    let (_d, log) = tmp_log();
    add_decision(&log, "a?");
    let mut f = OpenOptions::new().append(true).open(log.path()).unwrap();
    f.write_all(br#"{"seq":2,"ts":0,"by":"ag"#).unwrap();
    assert_eq!(log.load().unwrap().len(), 1);
}

#[test]
fn concurrent_appends_stay_consistent() {
    let (_d, log) = tmp_log();
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let log = log.clone();
            std::thread::spawn(move || {
                for _ in 0..250 {
                    log.append(Actor::Agent, |s| {
                        Ok(Kind::TodoAdd { id: s.next_todo_id(), title: "t".into(), context: None })
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let (ev, s) = log.load_state().unwrap();
    assert_eq!(ev.len(), 1000);
    assert!(ev.windows(2).all(|w| w[1].seq == w[0].seq + 1));
    assert_eq!(s.todos.last().unwrap().id, "t1000");
}

#[test]
fn trash_removes_finished_items_and_keeps_ids_unique() {
    let (_d, log) = tmp_log();
    add_decision(&log, "a?");
    add_decision(&log, "b?");
    let op = |k: Kind| log.append(Actor::Operator, |_| Ok(k));
    op(Kind::DecisionResolve { id: "d1".into(), text: "x".into() }).unwrap();
    op(Kind::ElabRequest { rid: "r1".into(), target: "d1".into(), selection: "a".into(), note: None }).unwrap();

    assert!(op(Kind::Trash { ids: vec!["d2".into()] }).is_err(), "pending items can't be trashed");
    assert!(op(Kind::Trash { ids: vec![] }).is_err());
    op(Kind::Trash { ids: vec!["d1".into()] }).unwrap();

    let (ev, s) = log.load_state().unwrap();
    assert!(s.decision("d1").is_none() && s.elab("r1").is_none(), "item and its elaborations are gone");
    assert_eq!(add_decision(&log, "c?"), "d3", "ids are not reused");
    // Late agent events about trashed things are accepted, and ignored.
    log.append(Actor::Agent, |_| Ok(Kind::ElabAdd { rid: "r1".into(), text_md: "late".into() })).unwrap();
    assert!(op(Kind::DecisionReopen { id: "d1".into() }).is_err());
    // Elaborations can be trashed on their own, answered or not.
    op(Kind::ElabRequest { rid: "r2".into(), target: "d2".into(), selection: "b".into(), note: None }).unwrap();
    op(Kind::Trash { ids: vec!["r2".into()] }).unwrap();
    let (_, s2) = log.load_state().unwrap();
    assert!(s2.elab("r2").is_none() && s2.decision("d2").is_some());
    log.append(Actor::Agent, |_| Ok(Kind::ElabAdd { rid: "r2".into(), text_md: "late".into() })).unwrap();
    // The agent is never told about trashing.
    assert!(unprocessed(&ev, &s).iter().all(|e| !matches!(e.kind, Kind::Trash { .. })));
}
