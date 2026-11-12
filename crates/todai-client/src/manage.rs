//! Board management on disk, for the frontends: delete-to-trash and the
//! batch helpers, on top of `todai_core::boards` (archive, unarchive,
//! create; re-exported here). Each is a move of
//! the whole file; a board's contents are never rewritten (`Log::append` is
//! still the only writer of what's inside).
//!
//! - archive: `.agents/todai/NAME.jsonl` → `.agents/todai/archive/NAME.jsonl`
//! - unarchive: back, refusing to overwrite a board an agent made since
//! - delete: into the desktop trash (freedesktop spec), restorable from
//!   the file manager; `.agents/todai/trash/` where that isn't possible
//!
//! Callers record a moved board's new path with `tracked::track`; these
//! functions touch nothing outside the board's folders and the trash.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub use todai_core::boards::{archive, boards_dir, create, is_archived, unarchive, valid_name};
use todai_core::boards::{TRASH, board_root, free_name, move_file};

/// Where a deleted board went.
#[derive(Debug, PartialEq)]
pub enum Deleted {
    /// The desktop trash: restorable from the file manager.
    Trash(PathBuf),
    /// `.agents/todai/trash/`, when the desktop trash wasn't usable.
    Folder(PathBuf),
}

/// Deletes a board by moving it to the desktop trash ([`home_trash`]).
pub fn delete(path: &Path) -> Result<Deleted> {
    delete_into(path, home_trash().as_deref())
}

/// Deletes a board by moving it into the freedesktop trash at `trash`
/// (tests pass their own), else into `.agents/todai/trash/`.
pub fn delete_into(path: &Path, trash: Option<&Path>) -> Result<Deleted> {
    let path = std::path::absolute(path)?;
    if !path.is_file() {
        bail!("no board {}", path.display());
    }
    if let Some(trash) = trash
        && let Ok(to) = trash_into(trash, &path)
    {
        return Ok(Deleted::Trash(to));
    }
    let dir = board_root(&path).join(TRASH);
    std::fs::create_dir_all(&dir)?;
    let to = free_name(&dir, &path)?;
    move_file(&path, &to)?;
    Ok(Deleted::Folder(to))
}

fn stem(p: &Path) -> String {
    p.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

/// Archives each board (unarchives archived ones) and says what happened.
/// `before` runs first for each board being archived (frontends close its
/// tabs); `track` records the new paths (off in tests).
pub fn toggle_archive_all(paths: &[PathBuf], track: bool, mut before: impl FnMut(&Path)) -> String {
    let (mut done, mut errors) = (Vec::new(), Vec::new());
    for p in paths {
        let r = if is_archived(p) {
            unarchive(p)
        } else {
            before(p);
            archive(p)
        };
        match r {
            Ok(to) => {
                if track {
                    crate::tracked::track(&to);
                    // A running local server serves it under its new path.
                    crate::remote::register_local(&to);
                }
                done.push(to);
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    let msg = match done.as_slice() {
        [] => String::new(),
        [one] if is_archived(one) => format!("Archived {}.", stem(one)),
        [one] => format!("Unarchived {}.", stem(one)),
        many => format!("Moved {} boards.", many.len()),
    };
    format!("{msg} {}", errors.join("; ")).trim().to_owned()
}

/// Deletes each board into `trash` (see [`delete_into`]) and says where
/// they went. `before` runs first for each (frontends close its tabs).
pub fn delete_all(paths: &[PathBuf], trash: Option<&Path>, mut before: impl FnMut(&Path)) -> String {
    let mut out = Vec::new();
    for p in paths {
        before(p);
        out.push(match delete_into(p, trash) {
            Ok(Deleted::Trash(_)) => format!("{} moved to the trash", stem(p)),
            Ok(Deleted::Folder(to)) => format!("{} moved to {}", stem(p), to.display()),
            Err(e) => format!("{e:#}"),
        });
    }
    out.join("; ") + "."
}

/// `$XDG_DATA_HOME/Trash` (else `~/.local/share/Trash`).
pub fn home_trash() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("Trash"))
}

/// The freedesktop trash: the file under `files/`, and an `info/` entry
/// saying where it came from, so a file manager can restore it. Only a
/// rename (same filesystem); the caller falls back otherwise.
fn trash_into(trash: &Path, path: &Path) -> Result<PathBuf> {
    let (files, info) = (trash.join("files"), trash.join("info"));
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let name = path.file_name().context("no file name")?.to_string_lossy().into_owned();
    // Claim a name by creating its info file exclusively, as the spec says.
    for n in 0..1000 {
        let candidate = if n == 0 { name.clone() } else { format!("{name}.{n}") };
        let info_file = info.join(format!("{candidate}.trashinfo"));
        let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).open(&info_file) else { continue };
        if files.join(&candidate).exists() {
            let _ = std::fs::remove_file(&info_file);
            continue;
        }
        use std::io::Write;
        let entry = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", url_path(path), local_now());
        f.write_all(entry.as_bytes())?;
        drop(f);
        let to = files.join(&candidate);
        if let Err(e) = std::fs::rename(path, &to) {
            let _ = std::fs::remove_file(&info_file);
            return Err(e.into());
        }
        return Ok(to);
    }
    bail!("no free name in {}", files.display())
}

/// Percent-encodes a path as the trash spec's `Path=` wants.
fn url_path(path: &Path) -> String {
    let mut s = String::new();
    for b in path.as_os_str().as_encoded_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(*b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// `YYYY-MM-DDThh:mm:ss`, local time (the spec's DeletionDate).
fn local_now() -> String {
    // SAFETY: localtime_r only writes into the struct we pass.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(".agents/todai").join(format!("{name}.jsonl"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "line\n").unwrap();
        p
    }

    #[test]
    fn delete_goes_to_a_restorable_trash() {
        let dir = tempfile::tempdir().unwrap();
        let p = board(dir.path(), "old");
        let trash = dir.path().join("Trash");
        let to = trash_into(&trash, &p).unwrap();
        assert_eq!(to, trash.join("files/old.jsonl"));
        let info = std::fs::read_to_string(trash.join("info/old.jsonl.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/") && info.contains("/.agents/todai/old.jsonl\n"), "{info}");
        assert!(info.contains("DeletionDate=20"), "{info}");

        // Same name again: a numbered entry, nothing overwritten.
        let p = board(dir.path(), "old");
        assert_eq!(trash_into(&trash, &p).unwrap(), trash.join("files/old.jsonl.1"));
    }

    #[test]
    fn url_path_escapes_what_the_spec_wants() {
        assert_eq!(url_path(Path::new("/a b/c%d/ü.jsonl")), "/a%20b/c%25d/%C3%BC.jsonl");
    }
}
