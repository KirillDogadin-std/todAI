//! Tracked boards: every local board todai has seen, for the board list.
//!
//! Nothing else knows all boards on a machine (tabs, sessions and a
//! server's registry each know some), so `todai attach`, `todai open FILE`,
//! `todai-tui FILE` and the window's restored tabs record each board here.
//! A board never opened through todai isn't listed: the UI says "tracked
//! boards" for that reason.
//!
//! The file, `$XDG_STATE_HOME/todai/tracked`, holds one absolute path per
//! line and is only ever appended to: a short `O_APPEND` write is atomic,
//! so agents attaching at the same time can't lose each other's lines, and
//! no lock is needed. Duplicates and boards whose file is gone are dropped
//! when reading.

use std::io::Write;
use std::path::{Path, PathBuf};

use todai_core::api::BoardInfo;
use todai_core::store::{Actor, DecisionStatus, Log};

use crate::config::state_dir;

fn file() -> Option<PathBuf> {
    Some(state_dir()?.join("tracked"))
}

/// Records `path` (absolute) as a tracked board. Best effort: never fails
/// the caller, since tracking only feeds the list.
pub fn track(path: &Path) {
    let Some(file) = file() else { return };
    let _ = track_in(&file, path);
}

fn track_in(file: &Path, path: &Path) -> std::io::Result<()> {
    if !path.is_absolute() || read_in(file).iter().any(|p| p == path) {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let line = format!("{}\n", path.display());
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
    // One write call, so the line lands whole even with other writers.
    f.write_all(line.as_bytes())
}

fn read_in(file: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut seen = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let p = PathBuf::from(line);
        if !seen.contains(&p) {
            seen.push(p);
        }
    }
    seen
}

/// Tracked boards whose file still exists, in the order first seen.
pub fn list() -> Vec<PathBuf> {
    file().map(|f| list_in(&f)).unwrap_or_default()
}

fn list_in(file: &Path) -> Vec<PathBuf> {
    read_in(file).into_iter().filter(|p| p.is_file()).collect()
}

/// What the board list shows about one board.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub path: PathBuf,
    pub name: String,
    pub project: String,
    /// Decisions and todos waiting for the operator.
    pub waiting: usize,
    /// Elaborations the operator asked for that agents haven't answered.
    pub asked: usize,
    /// Unix ms of the newest event, and of the newest agent event; 0 if none.
    pub last: u64,
    pub last_agent: u64,
}

impl Summary {
    /// Reads the board. An unreadable board still gets a row (zeros), so
    /// the list never hides a board because of one bad line.
    pub fn load(path: &Path) -> Summary {
        // An archived board belongs to the project of the folder it left.
        let live = match path.file_name() {
            Some(f) if crate::manage::is_archived(path) => crate::manage::boards_dir(path).join(f),
            _ => path.to_owned(),
        };
        let info = BoardInfo::new(&live);
        let mut s = Summary {
            path: path.to_owned(),
            name: info.name,
            project: info.project,
            waiting: 0,
            asked: 0,
            last: 0,
            last_agent: 0,
        };
        if let Ok((events, state)) = Log::new(path.to_owned()).load_state() {
            s.waiting = state.decisions.iter().filter(|d| d.status == DecisionStatus::Pending).count()
                + state.todos.iter().filter(|t| t.done.is_none()).count();
            s.asked = state.elabs.iter().filter(|e| e.answer.is_none()).count();
            s.last = events.iter().map(|e| e.ts).max().unwrap_or(0);
            s.last_agent = events.iter().filter(|e| e.by == Actor::Agent).map(|e| e.ts).max().unwrap_or(0);
        }
        if s.last == 0 {
            // No events yet (just attached or created): the file's age.
            s.last = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todai_core::store::Kind;

    #[test]
    fn tracks_each_board_once_and_drops_missing_ones() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state/tracked");
        let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        track_in(&file, &a).unwrap();
        track_in(&file, &b).unwrap();
        track_in(&file, &a).unwrap();
        track_in(&file, Path::new("relative.jsonl")).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 2, "no duplicate, no relative path");
        assert_eq!(list_in(&file), vec![a.clone(), b.clone()]);
        std::fs::remove_file(&b).unwrap();
        assert_eq!(list_in(&file), vec![a]);
    }

    #[test]
    fn concurrent_trackers_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tracked");
        let paths: Vec<PathBuf> = (0..32).map(|i| dir.path().join(format!("b{i}.jsonl"))).collect();
        for p in &paths {
            std::fs::write(p, "").unwrap();
        }
        let threads: Vec<_> = paths
            .iter()
            .cloned()
            .map(|p| {
                let file = file.clone();
                std::thread::spawn(move || track_in(&file, &p).unwrap())
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        let mut got = list_in(&file);
        got.sort();
        let mut want = paths;
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn summary_counts_what_waits_for_whom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proj/.agents/todai/main.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let log = Log::new(path.clone());
        let agent = |k: Kind| log.append(Actor::Agent, |_| Ok(k)).unwrap();
        agent(Kind::DecisionAdd { id: "d1".into(), title: "A?".into(), context: None, options: vec![] });
        agent(Kind::DecisionAdd { id: "d2".into(), title: "B?".into(), context: None, options: vec![] });
        agent(Kind::TodoAdd { id: "t1".into(), title: "Do".into(), context: None });
        log.append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id: "d1".into(), text: "yes".into() })).unwrap();
        log.append(Actor::Operator, |s| {
            Ok(Kind::ElabRequest { rid: s.next_elab_id(), target: "d2".into(), selection: "B".into(), note: None, parent: None })
        })
        .unwrap();
        let s = Summary::load(&path);
        assert_eq!((s.name.as_str(), s.project.as_str()), ("main", "proj"));
        assert_eq!((s.waiting, s.asked), (2, 1), "d2 + t1 wait for the operator; r1 for the agent");
        assert!(s.last >= s.last_agent && s.last_agent > 0);

        let archived = crate::manage::archive(&path).unwrap();
        assert_eq!(Summary::load(&archived).project, "proj", "not \"archive\"");

        let empty = path.with_file_name("new.jsonl");
        std::fs::write(&empty, "").unwrap();
        assert!(Summary::load(&empty).last > 0, "a new board is as old as its file");
    }
}
