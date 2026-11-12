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

pub use todai_core::boards::Summary;

#[cfg(test)]
mod tests {
    use super::*;

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
}
