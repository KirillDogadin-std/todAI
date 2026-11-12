//! Boards as files: archive, unarchive, create, and what a board's file
//! says at a glance ([`Summary`]). Each operation moves or creates a whole
//! file; a board's contents are only ever written by `Log::append`.
//! Shared by the frontends (`todai-client`) and the server.
//!
//! - archive: `.agents/todai/NAME.jsonl` → `.agents/todai/archive/NAME.jsonl`
//! - unarchive: back, refusing to overwrite a board an agent made since
//! - create: an empty `NAME.jsonl`, for an agent to `todai attach NAME`

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::api::BoardInfo;
use crate::store::{Actor, DecisionStatus, Log};

pub const ARCHIVE: &str = "archive";
/// Not a board name either: the frontends' fallback trash folder.
pub const TRASH: &str = "trash";

/// No agent activity for this long, and nothing pending: a hint to archive.
pub const STALE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;

/// Board names: what `todai attach NAME` accepts and a file name can hold.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !name.starts_with('-')
        && name != ARCHIVE
        && name != TRASH
}

/// Creates an empty board `NAME.jsonl` in `dir` (a project's
/// `.agents/todai`), for an agent to start with `todai attach NAME`.
pub fn create(dir: &Path, name: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        bail!("{name:?}: use letters, digits, - and _ (up to 64)");
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{name}.jsonl"));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("{} exists already", path.display()))?;
    Ok(path)
}

/// The live boards folder for a board (`.agents/todai`, also for archived).
pub fn boards_dir(path: &Path) -> PathBuf {
    board_root(path)
}

/// Whether `path` is an archived board.
pub fn is_archived(path: &Path) -> bool {
    path.parent().and_then(Path::file_name).is_some_and(|d| d == ARCHIVE)
}

/// Moves a board into its folder's `archive/`; returns the new path.
pub fn archive(path: &Path) -> Result<PathBuf> {
    if is_archived(path) {
        bail!("{} is already archived", path.display());
    }
    let dir = path.parent().context("board has no folder")?.join(ARCHIVE);
    std::fs::create_dir_all(&dir)?;
    let to = free_name(&dir, path)?;
    move_file(path, &to)?;
    Ok(to)
}

/// Moves an archived board back next to the live ones; returns the new path.
pub fn unarchive(path: &Path) -> Result<PathBuf> {
    if !is_archived(path) {
        bail!("{} is not archived", path.display());
    }
    let dir = path.parent().and_then(Path::parent).context("archive has no parent folder")?;
    let to = dir.join(path.file_name().context("no file name")?);
    if to.exists() {
        // An agent attached with this name since: don't clobber its board.
        bail!("a board {} exists again; archive or delete it first", to.display());
    }
    move_file(path, &to)?;
    Ok(to)
}

/// `.agents/todai` for a live or archived board.
pub fn board_root(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    if is_archived(path) { dir.parent().unwrap_or(dir).to_owned() } else { dir.to_owned() }
}

/// `dir/NAME.jsonl`, or `dir/NAME-2.jsonl`, ... if taken.
pub fn free_name(dir: &Path, path: &Path) -> Result<PathBuf> {
    let stem = path.file_stem().context("no file name")?.to_string_lossy().into_owned();
    for n in 1..1000 {
        let name = if n == 1 { format!("{stem}.jsonl") } else { format!("{stem}-{n}.jsonl") };
        let to = dir.join(name);
        if !to.exists() {
            return Ok(to);
        }
    }
    bail!("no free name for {stem} in {}", dir.display())
}

pub fn move_file(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).with_context(|| format!("move {} to {}", from.display(), to.display()))?;
    Ok(())
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
        let info = BoardInfo::new(path);
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

    /// Nothing waits on it and nobody wrote for [`STALE_AFTER_MS`].
    pub fn is_stale(&self, now: u64) -> bool {
        !is_archived(&self.path)
            && self.waiting == 0
            && self.asked == 0
            && now.saturating_sub(self.last_agent.max(self.last)) > STALE_AFTER_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Kind;

    fn board(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(".agents/todai").join(format!("{name}.jsonl"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "line\n").unwrap();
        p
    }

    #[test]
    fn archive_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let p = board(dir.path(), "main");
        let a = archive(&p).unwrap();
        assert_eq!(a, dir.path().join(".agents/todai/archive/main.jsonl"));
        assert!(is_archived(&a) && !p.exists());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "line\n", "moved, not rewritten");
        assert!(archive(&a).is_err(), "already archived");

        // A second board of the same name archives beside the first.
        let p2 = board(dir.path(), "main");
        assert_eq!(archive(&p2).unwrap(), dir.path().join(".agents/todai/archive/main-2.jsonl"));

        assert_eq!(unarchive(&a).unwrap(), p);
        // An agent attached `main` again meanwhile: unarchiving onto it is refused.
        let a = archive(&p).unwrap();
        board(dir.path(), "main");
        assert!(format!("{:#}", unarchive(&a).unwrap_err()).contains("exists again"));
        let a2 = dir.path().join(".agents/todai/archive/main-2.jsonl");
        assert_eq!(unarchive(&a2).unwrap(), dir.path().join(".agents/todai/main-2.jsonl"));
    }

    #[test]
    fn create_makes_an_empty_board_once() {
        let dir = tempfile::tempdir().unwrap();
        let boards = dir.path().join(".agents/todai");
        let p = create(&boards, "review-2").unwrap();
        assert_eq!((p.clone(), std::fs::read(&p).unwrap()), (boards.join("review-2.jsonl"), vec![]));
        assert!(create(&boards, "review-2").is_err(), "never over an existing board");
        for bad in ["", "a b", "../x", "archive", "-x", "ü"] {
            assert!(create(&boards, bad).is_err(), "{bad:?}");
        }
        let archived = archive(&p).unwrap();
        assert_eq!(boards_dir(&archived), boards);
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

        let archived = archive(&path).unwrap();
        assert_eq!(Summary::load(&archived).project, "proj", "not \"archive\"");

        let empty = path.with_file_name("new.jsonl");
        std::fs::write(&empty, "").unwrap();
        assert!(Summary::load(&empty).last > 0, "a new board is as old as its file");
    }
}
