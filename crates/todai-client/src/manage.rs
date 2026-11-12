//! Board management on disk: archive, unarchive, delete. Each is a move of
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

const ARCHIVE: &str = "archive";
const TRASH: &str = "trash";

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

/// Where a deleted board went.
#[derive(Debug, PartialEq)]
pub enum Deleted {
    /// The desktop trash: restorable from the file manager.
    Trash(PathBuf),
    /// `.agents/todai/trash/`, when the desktop trash wasn't usable.
    Folder(PathBuf),
}

/// Deletes a board by moving it to the trash.
pub fn delete(path: &Path) -> Result<Deleted> {
    let path = std::path::absolute(path)?;
    if !path.is_file() {
        bail!("no board {}", path.display());
    }
    if let Some(trash) = home_trash()
        && let Ok(to) = trash_into(&trash, &path)
    {
        return Ok(Deleted::Trash(to));
    }
    let dir = board_root(&path).join(TRASH);
    std::fs::create_dir_all(&dir)?;
    let to = free_name(&dir, &path)?;
    move_file(&path, &to)?;
    Ok(Deleted::Folder(to))
}

/// `.agents/todai` for a live or archived board.
fn board_root(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    if is_archived(path) { dir.parent().unwrap_or(dir).to_owned() } else { dir.to_owned() }
}

/// `$XDG_DATA_HOME/Trash` (else `~/.local/share/Trash`).
fn home_trash() -> Option<PathBuf> {
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

/// `dir/NAME.jsonl`, or `dir/NAME-2.jsonl`, ... if taken.
fn free_name(dir: &Path, path: &Path) -> Result<PathBuf> {
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

fn move_file(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).with_context(|| format!("move {} to {}", from.display(), to.display()))?;
    Ok(())
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
    fn url_path_escapes_what_the_spec_wants() {
        assert_eq!(url_path(Path::new("/a b/c%d/ü.jsonl")), "/a%20b/c%25d/%C3%BC.jsonl");
    }
}
