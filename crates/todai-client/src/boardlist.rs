//! The board list ("tracked boards"): rows, filter, marks, sort, and its
//! keys, shared by the window and the TUI. Frontends draw it and carry out
//! the [`Action`]s it returns (file moves go through `manage`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::manage;
use crate::nav::{Input, Key};
use crate::tab::Location;
use crate::tracked::Summary;

/// No agent activity for this long, and nothing pending: a hint to archive.
pub const STALE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub summary: Summary,
    pub archived: bool,
    /// Open as a tab right now.
    pub open: bool,
    pub stale: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// By project, then name.
    #[default]
    Project,
    /// Most recently active first.
    Activity,
}

/// What the frontend should do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open(Location),
    /// Archive these; for archived ones, unarchive. Reversible: no confirm.
    ToggleArchive(Vec<PathBuf>),
    /// Ask first, then [`delete`](manage::delete).
    Delete(Vec<PathBuf>),
    /// Ask first ("Archive N stale boards?"), then archive.
    ArchiveStale(Vec<PathBuf>),
    /// Create `NAME.jsonl` in `dir` (`manage::create`).
    Create { dir: PathBuf, name: String },
    Close,
}

#[derive(Default)]
pub struct BoardList {
    pub rows: Vec<Row>,
    /// Index into [`visible`](Self::visible).
    pub cursor: usize,
    pub filter: String,
    /// Typing goes into the filter (after `/` in vim mode; always in mouse mode).
    pub filtering: bool,
    pub show_archived: bool,
    pub sort: Sort,
    pub marked: BTreeSet<PathBuf>,
    /// Typing a new board's name (after `n`), and where it goes.
    pub naming: Option<(PathBuf, String)>,
    /// The last action's result, shown above the rows.
    pub notice: Option<String>,
    prefix_g: bool,
}

impl BoardList {
    /// The list for `paths` (tracked boards), given which are open in tabs
    /// and the time now (Unix ms).
    pub fn new(paths: &[PathBuf], open: &[Location], now: u64) -> BoardList {
        let mut list = BoardList::default();
        list.reload(paths, open, now);
        list
    }

    /// Re-reads every board, keeping filter, marks (still present), sort
    /// and the cursor's board.
    pub fn reload(&mut self, paths: &[PathBuf], open: &[Location], now: u64) {
        let at = self.current().map(|r| r.summary.path.clone());
        self.rows = paths
            .iter()
            .map(|p| {
                let summary = Summary::load(p);
                let archived = manage::is_archived(p);
                let stale = !archived
                    && summary.waiting == 0
                    && summary.asked == 0
                    && now.saturating_sub(summary.last_agent.max(summary.last)) > STALE_AFTER_MS;
                let open = open.iter().any(|l| matches!(l, Location::File(f) if f == p));
                Row { summary, archived, open, stale }
            })
            .collect();
        self.marked.retain(|m| paths.contains(m));
        // Follow the cursor's board; if it left the list (archived,
        // deleted), stay at the same place.
        if let Some(i) = at.and_then(|p| self.visible().iter().position(|r| r.summary.path == p)) {
            self.cursor = i;
        }
        self.clamp();
    }

    /// Rows as shown: filtered, archived ones only when asked for, sorted.
    pub fn visible(&self) -> Vec<&Row> {
        let needle = self.filter.to_lowercase();
        let mut rows: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| self.show_archived || !r.archived)
            .filter(|r| {
                needle.is_empty()
                    || r.summary.name.to_lowercase().contains(&needle)
                    || r.summary.project.to_lowercase().contains(&needle)
            })
            .collect();
        match self.sort {
            Sort::Project => rows.sort_by(|a, b| {
                (&a.summary.project, a.archived, &a.summary.name).cmp(&(&b.summary.project, b.archived, &b.summary.name))
            }),
            Sort::Activity => rows.sort_by(|a, b| b.summary.last.cmp(&a.summary.last)),
        }
        rows
    }

    pub fn current(&self) -> Option<&Row> {
        self.visible().get(self.cursor).copied()
    }

    /// Marked boards still visible, else the one under the cursor.
    pub fn targets(&self) -> Vec<PathBuf> {
        let marked: Vec<PathBuf> =
            self.visible().iter().map(|r| r.summary.path.clone()).filter(|p| self.marked.contains(p)).collect();
        if marked.is_empty() { self.current().map(|r| vec![r.summary.path.clone()]).unwrap_or_default() } else { marked }
    }

    pub fn stale(&self) -> Vec<PathBuf> {
        self.rows.iter().filter(|r| r.stale).map(|r| r.summary.path.clone()).collect()
    }

    pub fn toggle_mark(&mut self, path: &PathBuf) {
        if !self.marked.remove(path) {
            self.marked.insert(path.clone());
        }
    }

    fn clamp(&mut self) {
        self.cursor = self.cursor.min(self.visible().len().saturating_sub(1));
    }

    fn step(&mut self, d: isize) {
        let n = self.visible().len() as isize;
        if n > 0 {
            self.cursor = (self.cursor as isize + d).clamp(0, n - 1) as usize;
        }
    }

    /// One input event; `vim` enables single-letter commands (in mouse mode
    /// letters type into the filter). Returns what the frontend must do.
    pub fn input(&mut self, input: &Input, vim: bool) -> Option<Action> {
        if let Some((_, name)) = &mut self.naming {
            return match input {
                Input::Text(t) => {
                    name.push_str(t);
                    None
                }
                Input::Key { key: Key::Enter, pressed: true, .. } => {
                    let (dir, name) = self.naming.take().unwrap();
                    Some(Action::Create { dir, name: name.trim().to_owned() })
                }
                Input::Key { key: Key::Escape, pressed: true, .. } => {
                    self.naming = None;
                    None
                }
                _ => None,
            };
        }
        match input {
            Input::Text(t) => {
                if self.filtering || !vim {
                    self.filter.push_str(t);
                    self.cursor = 0;
                    return None;
                }
                let mut out = None;
                for c in t.chars() {
                    out = self.letter(c).or(out);
                }
                out
            }
            Input::Key { pressed: false, .. } => None,
            Input::Key { key, mods, .. } => {
                if mods.command && *key == Key::Letter('o') {
                    return Some(Action::Close);
                }
                match key {
                    Key::Escape => {
                        if self.filtering || !self.filter.is_empty() {
                            self.filtering = false;
                            self.filter.clear();
                        } else if !self.marked.is_empty() {
                            self.marked.clear();
                        } else {
                            return Some(Action::Close);
                        }
                        None
                    }
                    Key::Enter if self.filtering => {
                        self.filtering = false;
                        None
                    }
                    Key::Enter => self.current().map(|r| Action::Open(Location::File(r.summary.path.clone()))),
                    Key::Down => {
                        self.step(1);
                        None
                    }
                    Key::Up => {
                        self.step(-1);
                        None
                    }
                    Key::Home => {
                        self.cursor = 0;
                        None
                    }
                    Key::End => {
                        self.cursor = self.visible().len().saturating_sub(1);
                        None
                    }
                    _ => None,
                }
            }
        }
    }

    /// Backspace in whatever is being typed (frontends send it here).
    pub fn backspace(&mut self) {
        if let Some((_, name)) = &mut self.naming {
            name.pop();
        } else {
            self.filter.pop();
            self.cursor = 0;
        }
    }

    /// Starts naming a new board in the project of the board under the
    /// cursor (or `fallback`, a `.agents/todai` folder, when the list is empty).
    pub fn start_new(&mut self, fallback: Option<PathBuf>) {
        let dir = self.current().map(|r| manage::boards_dir(&r.summary.path)).or(fallback);
        if let Some(dir) = dir {
            self.naming = Some((dir, String::new()));
        }
    }

    fn letter(&mut self, c: char) -> Option<Action> {
        if std::mem::take(&mut self.prefix_g) {
            match c {
                'g' => self.cursor = 0,
                'b' => return Some(Action::Close),
                _ => {}
            }
            return None;
        }
        match c {
            'j' => self.step(1),
            'k' => self.step(-1),
            'G' => self.cursor = self.visible().len().saturating_sub(1),
            'g' => self.prefix_g = true,
            '/' => self.filtering = true,
            ' ' => {
                if let Some(p) = self.current().map(|r| r.summary.path.clone()) {
                    self.toggle_mark(&p);
                }
            }
            'a' => return Some(Action::ToggleArchive(self.targets())).filter(|_| !self.targets().is_empty()),
            'd' => return Some(Action::Delete(self.targets())).filter(|_| !self.targets().is_empty()),
            'S' => return Some(Action::ArchiveStale(self.stale())).filter(|_| !self.stale().is_empty()),
            '.' => {
                self.show_archived = !self.show_archived;
                self.clamp();
            }
            's' => {
                self.sort = match self.sort {
                    Sort::Project => Sort::Activity,
                    Sort::Activity => Sort::Project,
                };
            }
            'n' => self.start_new(None),
            'q' => return Some(Action::Close),
            _ => {}
        }
        None
    }
}

/// Title, body and confirm-button label for an action that asks first.
pub fn confirm_text(action: &Action) -> Option<(String, String, &'static str)> {
    let stem = |p: &PathBuf| p.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    match action {
        Action::Delete(paths) => {
            let waiting: usize = paths.iter().map(|p| Summary::load(p).waiting).sum();
            let names: Vec<String> = paths.iter().map(stem).collect();
            let title = match paths.len() {
                1 => format!("Delete {}?", names[0]),
                n => format!("Delete {n} boards?"),
            };
            let mut body = format!("{}\nThey go to the trash; restore them from your file manager.", names.join(", "));
            if waiting > 0 {
                body.push_str(&format!("\n{waiting} items still wait for you."));
            }
            Some((title, body, "Delete"))
        }
        Action::ArchiveStale(paths) => Some((
            format!("Archive {} stale boards?", paths.len()),
            "Nothing waits on them and no agent wrote for a week. Archived boards come back with Unarchive.".into(),
            "Archive",
        )),
        _ => None,
    }
}

/// What to tell the operator after creating board `name`.
pub fn created_notice(name: &str) -> String {
    format!("Created {name}. Have the agent in that project run: todai attach {name}")
}

/// `now`, `5 min ago`, `3 h ago`, `2 d ago`; `never` for 0.
pub fn ago(now: u64, ts: u64) -> String {
    if ts == 0 {
        return "never".into();
    }
    let s = now.saturating_sub(ts) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// Unix ms now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::Mods;
    use todai_core::store::{Actor, Kind, Log};

    fn board(root: &std::path::Path, project: &str, name: &str, pending: bool) -> PathBuf {
        let p = root.join(project).join(".agents/todai").join(format!("{name}.jsonl"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let log = Log::new(p.clone());
        if pending {
            log.append(Actor::Agent, |_| Ok(Kind::TodoAdd { id: "t1".into(), title: "x".into(), context: None })).unwrap();
        } else {
            std::fs::write(&p, "").unwrap();
        }
        p
    }

    fn key(k: Key) -> Input {
        Input::Key { key: k, pressed: true, mods: Mods::default() }
    }

    fn text(s: &str) -> Input {
        Input::Text(s.into())
    }

    #[test]
    fn rows_group_filter_mark_and_act() {
        let dir = tempfile::tempdir().unwrap();
        let a = board(dir.path(), "alpha", "main", true);
        let b = board(dir.path(), "beta", "main", false);
        let c = board(dir.path(), "alpha", "docs", false);
        let now = now_ms() + STALE_AFTER_MS + 60_000;
        let mut l = BoardList::new(&[a.clone(), b.clone(), c.clone()], &[Location::File(a.clone())], now);
        let names: Vec<_> = l.visible().iter().map(|r| format!("{}/{}", r.summary.project, r.summary.name)).collect();
        assert_eq!(names, ["alpha/docs", "alpha/main", "beta/main"]);
        assert!(l.rows[0].open && !l.rows[1].open);
        assert_eq!(l.stale(), vec![b.clone(), c.clone()], "empty and old; a has a pending todo");

        l.input(&text("j"), true);
        assert_eq!(l.input(&key(Key::Enter), true), Some(Action::Open(Location::File(a.clone()))));
        // Marks make the targets; Esc clears them before closing.
        l.input(&text(" j "), true);
        assert_eq!(l.targets(), vec![a.clone(), b.clone()]);
        assert_eq!(l.input(&text("d"), true), Some(Action::Delete(vec![a.clone(), b.clone()])));
        assert_eq!(l.input(&key(Key::Escape), true), None);
        assert_eq!(l.input(&key(Key::Escape), true), Some(Action::Close));

        // Filter: `/` in vim mode, plain typing in mouse mode.
        l.input(&text("/"), true);
        l.input(&text("bet"), true);
        assert_eq!(l.visible().len(), 1);
        l.input(&key(Key::Escape), true);
        assert_eq!(l.visible().len(), 3);
        l.input(&text("doc"), false);
        assert_eq!(l.visible()[0].summary.path, c);
    }

    #[test]
    fn ago_reads_naturally() {
        let h = 3_600_000;
        assert_eq!([ago(10 * h, 0), ago(10 * h, 10 * h - 5000), ago(10 * h, 9 * h), ago(100 * h, 2 * h)], ["never", "now", "1 h ago", "4 d ago"]);
        assert_eq!(ago(h, h - 5 * 60_000), "5 min ago");
    }

    #[test]
    fn n_names_a_new_board_in_the_current_project() {
        let dir = tempfile::tempdir().unwrap();
        let a = board(dir.path(), "alpha", "main", false);
        let mut l = BoardList::new(&[a.clone()], &[], 0);
        l.input(&text("n"), true);
        l.input(&text("reviex"), true);
        l.backspace();
        l.input(&text("w"), true);
        let want = Action::Create { dir: dir.path().join("alpha/.agents/todai"), name: "review".into() };
        assert_eq!(l.input(&key(Key::Enter), true), Some(want));
        assert!(l.naming.is_none());
    }

    #[test]
    fn archived_rows_hide_until_asked_and_sort_by_activity() {
        let dir = tempfile::tempdir().unwrap();
        let live = board(dir.path(), "p", "live", true);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let newer = board(dir.path(), "p", "newer", true);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let old = manage::archive(&board(dir.path(), "p", "old", true)).unwrap();
        let mut l = BoardList::new(&[live.clone(), newer.clone(), old.clone()], &[], 0);
        assert_eq!(l.visible().len(), 2);
        l.input(&text("."), true);
        assert_eq!(l.visible().len(), 3);
        assert!(l.rows.iter().find(|r| r.summary.path == old).unwrap().archived);
        l.input(&text("s"), true);
        assert_eq!(l.visible()[0].summary.path, old, "most recent event first (it was written last)");
        assert_eq!(l.visible()[1].summary.path, newer);
    }
}
