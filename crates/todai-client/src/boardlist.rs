//! The board list ("tracked boards"): rows, filter, marks, sort, and its
//! keys, shared by the window and the TUI. Rows are local tracked boards
//! plus the boards of servers with a saved token (fetched in the
//! background). Frontends draw it and hand the [`Action`]s it returns to
//! [`archive_all`], [`delete_all`] and [`create`], which work on both.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use todai_core::api::BoardSummaryView;

use crate::manage;
use crate::nav::{Input, Key};
use crate::remote::Client;
use crate::tab::{Location, Wake};
use crate::tracked::Summary;

pub use todai_core::boards::STALE_AFTER_MS;

/// Where a row's board lives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Local(PathBuf),
    /// A board on a todai server, by its id there.
    Remote { server: String, id: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub source: Source,
    pub name: String,
    pub project: String,
    /// The server's host for remote boards (`box:7341`), else `None`.
    pub host: Option<String>,
    /// Decisions and todos waiting for the operator.
    pub waiting: usize,
    /// Elaborations the operator asked for, unanswered.
    pub asked: usize,
    /// Unix ms of the newest event (or the file's age).
    pub last: u64,
    pub archived: bool,
    pub stale: bool,
    /// Open as a tab right now.
    pub open: bool,
}

impl Row {
    fn local(path: &Path, now: u64) -> Row {
        let s = Summary::load(path);
        Row {
            source: Source::Local(path.to_owned()),
            stale: s.is_stale(now),
            archived: manage::is_archived(path),
            name: s.name,
            project: s.project,
            host: None,
            waiting: s.waiting,
            asked: s.asked,
            last: s.last,
            open: false,
        }
    }

    fn remote(server: &str, v: BoardSummaryView) -> Row {
        Row {
            source: Source::Remote { server: server.to_owned(), id: v.board.id },
            name: v.board.name,
            project: v.board.project,
            host: Some(host(server)),
            waiting: v.waiting,
            asked: v.asked,
            last: v.last,
            archived: v.archived,
            stale: v.stale,
            open: false,
        }
    }

    /// Where the board opens as a tab.
    pub fn location(&self) -> Location {
        match &self.source {
            Source::Local(p) => Location::File(p.clone()),
            Source::Remote { server, id } => {
                Location::Remote { server: server.clone(), board: id.clone(), name: self.name.clone() }
            }
        }
    }

    /// Project as shown: with the host for remote boards.
    pub fn place(&self) -> String {
        match &self.host {
            Some(h) => format!("{} @ {h}", self.project),
            None => self.project.clone(),
        }
    }
}

/// `http://box:7341` → `box:7341`.
fn host(server: &str) -> String {
    server.split_once("://").map_or(server, |(_, h)| h).trim_end_matches('/').to_owned()
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
    ToggleArchive(Vec<Row>),
    /// Ask first, then [`delete_all`].
    Delete(Vec<Row>),
    /// Ask first ("Archive N stale boards?"), then [`archive_all`].
    ArchiveStale(Vec<Row>),
    /// Create an empty board `name` in the project of board `next_to`.
    Create { next_to: Row, name: String },
    Close,
}

/// What a server's background fetch brought back.
type Fetched = (String, Result<Vec<BoardSummaryView>, String>);

#[derive(Default)]
pub struct BoardList {
    /// Local and remote rows together.
    pub rows: Vec<Row>,
    local: Vec<Row>,
    /// Per server: its rows, or why there are none.
    remote: BTreeMap<String, Result<Vec<Row>, String>>,
    open: Vec<Location>,
    fetches: Option<mpsc::Receiver<Fetched>>,
    /// Index into [`visible`](Self::visible).
    pub cursor: usize,
    pub filter: String,
    /// Typing goes into the filter (after `/` in vim mode; always in mouse mode).
    pub filtering: bool,
    pub show_archived: bool,
    pub sort: Sort,
    pub marked: BTreeSet<Source>,
    /// Typing a new board's name (after `n`), and which board's project it joins.
    pub naming: Option<(Row, String)>,
    /// The last action's result, shown above the rows.
    pub notice: Option<String>,
    prefix_g: bool,
}

impl BoardList {
    /// The list for `paths` (tracked boards), given which are open in tabs
    /// and the time now (Unix ms). Remote rows come with [`fetch`](Self::fetch).
    pub fn new(paths: &[PathBuf], open: &[Location], now: u64) -> BoardList {
        let mut list = BoardList::default();
        list.reload(paths, open, now);
        list
    }

    /// Re-reads the local boards, keeping remote rows, filter, marks (still
    /// present), sort and the cursor's board.
    pub fn reload(&mut self, paths: &[PathBuf], open: &[Location], now: u64) {
        self.local = paths.iter().map(|p| Row::local(p, now)).collect();
        self.open = open.to_vec();
        self.rebuild();
    }

    /// Fetches each server's boards in the background; [`poll`](Self::poll)
    /// picks them up (`wake` repaints the frontend when one arrives).
    pub fn fetch(&mut self, servers: &[String], wake: Wake) {
        let (tx, rx) = mpsc::channel();
        for server in servers {
            let (tx, wake, server) = (tx.clone(), wake.clone(), server.clone());
            std::thread::spawn(move || {
                let got = match crate::remote::client_for(&server) {
                    Some(c) => summaries(&c),
                    None => Err("no token saved".into()),
                };
                let _ = tx.send((server, got));
                wake();
            });
        }
        self.fetches = Some(rx);
    }

    /// Takes in fetched server boards; whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.fetches else { return false };
        let got: Vec<Fetched> = rx.try_iter().collect();
        if got.is_empty() {
            return false;
        }
        for (server, r) in got {
            let rows = r.map(|views| views.into_iter().map(|v| Row::remote(&server, v)).collect());
            self.remote.insert(server, rows);
        }
        self.rebuild();
        true
    }

    /// One line per server that couldn't be listed.
    pub fn remote_notes(&self) -> Vec<String> {
        self.remote.iter().filter_map(|(s, r)| r.as_ref().err().map(|e| format!("{}: {e}", host(s)))).collect()
    }

    fn rebuild(&mut self) {
        let at = self.current().map(|r| r.source.clone());
        let remote = self.remote.values().flat_map(|r| r.iter().flatten().cloned());
        self.rows = self.local.iter().cloned().chain(remote).collect();
        for r in &mut self.rows {
            let loc = r.location();
            r.open = self.open.iter().any(|l| l.same(&loc));
        }
        let present: BTreeSet<&Source> = self.rows.iter().map(|r| &r.source).collect();
        self.marked.retain(|m| present.contains(m));
        // Follow the cursor's board; if it left the list (archived,
        // deleted), stay at the same place.
        if let Some(i) = at.and_then(|s| self.visible().iter().position(|r| r.source == s)) {
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
                needle.is_empty() || r.name.to_lowercase().contains(&needle) || r.place().to_lowercase().contains(&needle)
            })
            .collect();
        match self.sort {
            Sort::Project => rows.sort_by(|a, b| (a.place(), a.archived, &a.name).cmp(&(b.place(), b.archived, &b.name))),
            Sort::Activity => rows.sort_by(|a, b| b.last.cmp(&a.last)),
        }
        rows
    }

    pub fn current(&self) -> Option<&Row> {
        self.visible().get(self.cursor).copied()
    }

    /// Marked boards still visible, else the one under the cursor.
    pub fn targets(&self) -> Vec<Row> {
        let marked: Vec<Row> = self.visible().into_iter().filter(|r| self.marked.contains(&r.source)).cloned().collect();
        if marked.is_empty() { self.current().cloned().into_iter().collect() } else { marked }
    }

    pub fn stale(&self) -> Vec<Row> {
        self.rows.iter().filter(|r| r.stale).cloned().collect()
    }

    pub fn toggle_mark(&mut self, source: &Source) {
        if !self.marked.remove(source) {
            self.marked.insert(source.clone());
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
                    let (next_to, name) = self.naming.take().unwrap();
                    Some(Action::Create { next_to, name: name.trim().to_owned() })
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
                    Key::Enter => self.current().map(|r| Action::Open(r.location())),
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

    /// Starts naming a new board in the project of the board under the cursor.
    pub fn start_new(&mut self) {
        if let Some(r) = self.current().cloned() {
            self.naming = Some((r, String::new()));
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
        let some = |v: Vec<Row>, f: fn(Vec<Row>) -> Action| (!v.is_empty()).then(|| f(v));
        match c {
            'j' => self.step(1),
            'k' => self.step(-1),
            'G' => self.cursor = self.visible().len().saturating_sub(1),
            'g' => self.prefix_g = true,
            '/' => self.filtering = true,
            ' ' => {
                if let Some(s) = self.current().map(|r| r.source.clone()) {
                    self.toggle_mark(&s);
                }
            }
            'a' => return some(self.targets(), Action::ToggleArchive),
            'd' => return some(self.targets(), Action::Delete),
            'S' => return some(self.stale(), Action::ArchiveStale),
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
            'n' => self.start_new(),
            'q' => return Some(Action::Close),
            _ => {}
        }
        None
    }
}

/// A server's boards with their numbers; from a server older than 0.7
/// (no `/summaries`), the board list without numbers.
fn summaries(c: &Client) -> Result<Vec<BoardSummaryView>, String> {
    match c.summaries() {
        Ok(v) => Ok(v),
        Err(e) if e.to_string().contains("404") => c
            .boards()
            .map(|bs| {
                let blank = |board| BoardSummaryView {
                    board,
                    waiting: 0,
                    asked: 0,
                    last: 0,
                    last_agent: 0,
                    archived: false,
                    stale: false,
                };
                bs.into_iter().map(blank).collect()
            })
            .map_err(|e| format!("{e:#}")),
        Err(e) => Err(format!("{e:#}")),
    }
}

/// Servers whose rows changed, so the frontend refetches them.
pub type Refetch = BTreeSet<String>;

/// Archives each board (unarchives archived ones): local files through
/// `manage`, remote boards through their server. `before` runs first for
/// each local board being archived (frontends close its tabs); `track`
/// records moved local boards (off in tests).
pub fn archive_all(rows: &[Row], track: bool, mut before: impl FnMut(&Path)) -> (String, Refetch) {
    let local: Vec<PathBuf> = rows
        .iter()
        .filter_map(|r| match &r.source {
            Source::Local(p) => Some(p.clone()),
            Source::Remote { .. } => None,
        })
        .collect();
    let mut msgs = Vec::new();
    if !local.is_empty() {
        msgs.push(manage::toggle_archive_all(&local, track, &mut before));
    }
    let mut refetch = Refetch::new();
    for r in rows {
        let Source::Remote { server, id } = &r.source else { continue };
        refetch.insert(server.clone());
        let done = crate::remote::client_for(server)
            .ok_or_else(|| anyhow::anyhow!("no token saved for {server}"))
            .and_then(|c| c.relocate(id, !r.archived));
        msgs.push(match done {
            Ok(_) if r.archived => format!("Unarchived {} on {}.", r.name, host(server)),
            Ok(_) => format!("Archived {} on {}.", r.name, host(server)),
            Err(e) => format!("{e:#}"),
        });
    }
    (msgs.join(" "), refetch)
}

/// Deletes local boards into `trash` (see `manage::delete_into`). Remote
/// boards can't be deleted (the API has no delete), and say so.
pub fn delete_all(rows: &[Row], trash: Option<&Path>, before: impl FnMut(&Path)) -> String {
    let (local, remote): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| matches!(r.source, Source::Local(_)));
    let paths: Vec<PathBuf> = local
        .iter()
        .filter_map(|r| match &r.source {
            Source::Local(p) => Some(p.clone()),
            Source::Remote { .. } => None,
        })
        .collect();
    let mut msgs = Vec::new();
    if !paths.is_empty() {
        msgs.push(manage::delete_all(&paths, trash, before));
    }
    if !remote.is_empty() {
        let names: Vec<String> = remote.iter().map(|r| format!("{}@{}", r.name, r.host.as_deref().unwrap_or(""))).collect();
        msgs.push(format!("Not deleted (servers only archive): {}.", names.join(", ")));
    }
    msgs.join(" ")
}

/// Creates an empty board `name` in `next_to`'s project; where it opens.
/// A local board isn't tracked here: the caller does that.
pub fn create(next_to: &Row, name: &str) -> anyhow::Result<Location> {
    match &next_to.source {
        Source::Local(p) => Ok(Location::File(manage::create(&manage::boards_dir(p), name)?)),
        Source::Remote { server, id } => {
            let c = crate::remote::client_for(server).ok_or_else(|| anyhow::anyhow!("no token saved for {server}"))?;
            let b = c.sibling(id, name)?;
            Ok(Location::Remote { server: server.clone(), board: b.id, name: b.name })
        }
    }
}

/// Title, body and confirm-button label for an action that asks first.
pub fn confirm_text(action: &Action) -> Option<(String, String, &'static str)> {
    match action {
        Action::Delete(rows) => {
            let waiting: usize = rows.iter().map(|r| r.waiting).sum();
            let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
            let title = match rows.len() {
                1 => format!("Delete {}?", names[0]),
                n => format!("Delete {n} boards?"),
            };
            let mut body = format!("{}\nThey go to the trash; restore them from your file manager.", names.join(", "));
            if rows.iter().any(|r| r.host.is_some()) {
                body.push_str("\nBoards on servers stay: a server only archives.");
            }
            if waiting > 0 {
                body.push_str(&format!("\n{waiting} items still wait for you."));
            }
            Some((title, body, "Delete"))
        }
        Action::ArchiveStale(rows) => Some((
            format!("Archive {} stale boards?", rows.len()),
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
    todai_core::store::now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::Mods;
    use std::sync::Arc;
    use todai_core::store::{Actor, Kind, Log};

    fn board(root: &Path, project: &str, name: &str, pending: bool) -> PathBuf {
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

    fn paths(rows: &[Row]) -> Vec<PathBuf> {
        rows.iter()
            .map(|r| match &r.source {
                Source::Local(p) => p.clone(),
                Source::Remote { .. } => panic!("remote"),
            })
            .collect()
    }

    #[test]
    fn rows_group_filter_mark_and_act() {
        let dir = tempfile::tempdir().unwrap();
        let a = board(dir.path(), "alpha", "main", true);
        let b = board(dir.path(), "beta", "main", false);
        let c = board(dir.path(), "alpha", "docs", false);
        let now = now_ms() + STALE_AFTER_MS + 60_000;
        let mut l = BoardList::new(&[a.clone(), b.clone(), c.clone()], &[Location::File(a.clone())], now);
        let names: Vec<_> = l.visible().iter().map(|r| format!("{}/{}", r.project, r.name)).collect();
        assert_eq!(names, ["alpha/docs", "alpha/main", "beta/main"]);
        assert!(l.rows[0].open && !l.rows[1].open);
        assert_eq!(paths(&l.stale()), vec![b.clone(), c.clone()], "empty and old; a has a pending todo");

        l.input(&text("j"), true);
        assert_eq!(l.input(&key(Key::Enter), true), Some(Action::Open(Location::File(a.clone()))));
        // Marks make the targets; Esc clears them before closing.
        l.input(&text(" j "), true);
        assert_eq!(paths(&l.targets()), vec![a.clone(), b.clone()]);
        assert!(matches!(l.input(&text("d"), true), Some(Action::Delete(r)) if paths(&r) == vec![a.clone(), b.clone()]));
        assert_eq!(l.input(&key(Key::Escape), true), None);
        assert_eq!(l.input(&key(Key::Escape), true), Some(Action::Close));

        // Filter: `/` in vim mode, plain typing in mouse mode.
        l.input(&text("/"), true);
        l.input(&text("bet"), true);
        assert_eq!(l.visible().len(), 1);
        l.input(&key(Key::Escape), true);
        assert_eq!(l.visible().len(), 3);
        l.input(&text("doc"), false);
        assert_eq!(l.visible()[0].source, Source::Local(c));
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
        let Some(Action::Create { next_to, name }) = l.input(&key(Key::Enter), true) else { panic!("no create") };
        assert_eq!((next_to.source, name.as_str()), (Source::Local(a.clone()), "review"));
        assert!(l.naming.is_none());
        let made = create(&l.rows[0], "review").unwrap();
        assert_eq!(made, Location::File(dir.path().join("alpha/.agents/todai/review.jsonl")));
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
        assert!(l.rows.iter().find(|r| r.source == Source::Local(old.clone())).unwrap().archived);
        l.input(&text("s"), true);
        assert_eq!(l.visible()[0].source, Source::Local(old), "most recent event first (it was written last)");
        assert_eq!(l.visible()[1].source, Source::Local(newer));
    }

    /// A live server with a saved token: its boards join the list, open as
    /// remote tabs, and archive / create through its API; delete refuses.
    #[test]
    fn remote_boards_come_from_saved_servers() {
        let dir = tempfile::tempdir().unwrap();
        let remote_board = board(dir.path(), "far", "main", true);
        let cfg = todai_server::Config {
            listen: "127.0.0.1:0".into(),
            dir: dir.path().join("server"),
            boards: vec![remote_board.clone()],
        };
        let server = todai_server::Server::bind(&cfg).unwrap().spawn();
        let base = format!("http://{}", server.addr);
        let client = Client::new(&base, &server.token);
        let rows = summaries(&client).unwrap().into_iter().map(|v| Row::remote(&base, v)).collect::<Vec<_>>();

        let local = board(dir.path(), "near", "main", false);
        let mut l = BoardList::new(&[local.clone()], &[], now_ms());
        l.remote.insert(base.clone(), Ok(rows));
        l.remote.insert("http://down:7341".into(), Err("connection refused".into()));
        l.rebuild();
        let places: Vec<String> = l.visible().iter().map(|r| format!("{}/{}", r.place(), r.name)).collect();
        assert_eq!(places, [format!("far @ {}/main", host(&base)), "near/main".to_owned()]);
        assert_eq!(l.remote_notes(), ["down:7341: connection refused"]);
        let far = l.visible()[0].clone();
        assert_eq!(far.waiting, 1);
        let id = todai_core::api::board_id(&remote_board);
        assert_eq!(far.location(), Location::Remote { server: base.clone(), board: id, name: "main".into() });

        // Delete never reaches a server.
        let msg = delete_all(std::slice::from_ref(&far), Some(&dir.path().join("Trash")), |_| {});
        assert!(msg.contains("Not deleted") && remote_board.exists(), "{msg}");
        // The server side of archive / create is tested in todai-server;
        // here the client calls, without the saved-token lookup.
        let moved = client.relocate(&todai_core::api::board_id(&remote_board), true).unwrap();
        assert!(dir.path().join("far/.agents/todai/archive/main.jsonl").exists());
        let made = client.sibling(&moved.id, "review").unwrap();
        assert_eq!((made.name.as_str(), made.project.as_str()), ("review", "far"));

        // Fetching in the background, as the frontends do.
        let mut l = BoardList::new(&[], &[], now_ms());
        l.fetch(&["http://127.0.0.1:1".into()], Arc::new(|| {}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !l.poll() {
            assert!(std::time::Instant::now() < deadline, "the fetch never reported");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(l.remote_notes().len(), 1, "an unreachable server is a note, not a crash");
    }
}
