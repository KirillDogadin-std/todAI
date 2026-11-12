//! The TUI's state and input handling: open tabs, dialogs, the socket that
//! `todai attach` / `todai FILE` talk to, and routing keys to the shared
//! navigation (`todai_client::nav`) or to the field being typed into.

use std::path::PathBuf;
use std::sync::mpsc;

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use todai_client::boardlist::{self, Action, BoardList};
use todai_client::follow::Follows;
use todai_client::{manage, tracked};
use todai_client::ipc::{Incoming, Reply, Request};
use todai_client::nav::{Env, Focus, FocusRequest, Input, Key, Mode, Mods, Outcome};
use todai_client::session::Session;
use todai_client::store::Kind;
use todai_client::tab::{Location, Tab, Wake};

use crate::board::Hit;
use crate::edit::{self, Edit, Target};
use crate::ui;

/// The TUI's open tabs, in the state dir (the window keeps its own).
pub const SESSION: &str = "tui-session.json";

/// One open board plus what only the terminal needs.
pub struct Open {
    pub tab: Tab,
    /// The field being typed into, if any.
    pub edit: Option<Edit>,
    /// Scroll offset, in lines, of the left and right columns.
    pub scroll: [usize; 2],
    /// Where the prompt panel wants the terminal cursor this frame.
    pub prompt_caret: Option<Position>,
}

/// A column as drawn last frame, for the mouse.
pub struct Column {
    pub rect: Rect,
    pub scroll: usize,
    pub hits: Vec<(usize, usize, usize, Hit)>,
    pub caret: Option<Position>,
}

pub struct App {
    pub tabs: Vec<Open>,
    pub active: usize,
    /// Cheatsheet scroll offset while it is open.
    pub cheatsheet: Option<usize>,
    pub confirm_quit: bool,
    pub quit: bool,
    /// Whether the terminal has focus (when it reports focus changes).
    pub focused: bool,
    /// A one-off message for the status line, cleared by the next key.
    pub notice: Option<String>,
    pub columns: Vec<Column>,
    pub tab_hits: Vec<(u16, u16, usize)>,
    requests: Option<mpsc::Receiver<Incoming>>,
    saved: Session,
    /// Persist tabs across runs (off in tests).
    persist: bool,
    /// Tests: the tracked boards, instead of the user's tracked file.
    pub test_tracked: Option<Vec<PathBuf>>,
    /// Tests: a trash folder, instead of the user's desktop trash.
    pub test_trash: Option<PathBuf>,
    wake: Wake,
    pub(crate) follows: Follows,
    /// Text to put on the terminal's clipboard after the next draw (`y`).
    pub clipboard: Option<String>,
    /// The board list, while open.
    pub boards: Option<BoardList>,
    /// Rows of the board list drawn this frame: (screen row, board), for clicks.
    pub board_hits: Vec<(u16, PathBuf)>,
    /// A board-list action waiting for "are you sure".
    pub confirm_boards: Option<Action>,
    /// A tab whose closing waits for "are you sure".
    pub confirm_close: Option<usize>,
}

impl App {
    pub fn new(wake: Wake, requests: Option<mpsc::Receiver<Incoming>>, persist: bool) -> App {
        App {
            tabs: Vec::new(),
            active: 0,
            cheatsheet: None,
            confirm_quit: false,
            quit: false,
            focused: true,
            notice: None,
            columns: Vec::new(),
            tab_hits: Vec::new(),
            requests,
            saved: Session::default(),
            persist,
            test_tracked: None,
            test_trash: None,
            wake,
            follows: Follows::default(),
            clipboard: None,
            boards: None,
            board_hits: Vec::new(),
            confirm_boards: None,
            confirm_close: None,
        }
    }

    /// Reopens last run's tabs, skipping boards deleted since.
    pub fn restore(&mut self) {
        let saved = Session::load(SESSION);
        for loc in &saved.tabs {
            if !matches!(loc, Location::File(p) if !p.exists()) {
                self.open_or_focus(loc.clone());
            }
        }
        self.active = saved.active.min(self.tabs.len().saturating_sub(1));
        self.follows.restore(&saved.follow, &self.wake);
        self.saved = saved;
    }

    pub fn open_or_focus(&mut self, loc: Location) {
        self.active = self.open(loc);
    }

    /// Opens `loc` in a new tab unless one shows it already; its index.
    fn open(&mut self, loc: Location) -> usize {
        if let Some(i) = self.tabs.iter().position(|o| o.tab.location.same(&loc)) {
            return i;
        }
        let tab = match &loc {
            Location::File(p) => Tab::open(p.clone(), self.wake.clone()),
            Location::Remote { server, .. } => match todai_client::remote::client_for(server) {
                Some(client) => Tab::remote(loc.clone(), client, self.wake.clone()),
                None => {
                    // Without a token the tab can't load; keep it so the
                    // session remembers it, and say why it is empty.
                    let mut tab = Tab::open_failed(loc.clone());
                    tab.error = Some(format!("no token saved for {server}; run: todai-tui --server {server} --token ..."));
                    tab
                }
            },
        };
        self.tabs.push(Open { tab, edit: None, scroll: [0, 0], prompt_caret: None });
        self.tabs.len() - 1
    }

    /// Follows `client`'s server: boards it registers from now on open as
    /// tabs; `known` ids never do.
    pub fn follow(&mut self, client: todai_client::remote::Client, known: Vec<String>) {
        self.follows.follow(client, known, self.wake.clone());
    }

    /// Boards new on followed servers open as background tabs; following
    /// ends with a server's last tab.
    fn follow_servers(&mut self) {
        for loc in self.follows.new_boards() {
            self.open(loc);
        }
        self.follows.retain_open(&self.tabs.iter().map(|o| &o.tab.location).collect::<Vec<_>>());
    }

    pub fn open_file(&mut self, path: PathBuf) {
        self.open_or_focus(Location::File(path));
    }

    /// Socket requests, board reloads, notifications, session: everything
    /// that isn't a key. Called before every frame.
    pub fn tick(&mut self) {
        self.serve_requests();
        self.follow_servers();
        for o in &mut self.tabs {
            o.tab.reload_if_dirty();
        }
        self.notify();
        self.save_session();
    }

    fn serve_requests(&mut self) {
        let Some(rx) = &self.requests else { return };
        let pending: Vec<Incoming> = rx.try_iter().collect();
        for (req, reply) in pending {
            let r = match req {
                Request::Open { path } => {
                    self.open_or_focus(Location::File(path));
                    Reply::ok()
                }
                Request::OpenRemote { server, board, name } => {
                    self.open_or_focus(Location::Remote { server, board, name });
                    Reply::ok()
                }
                Request::Follow { server, known } => match todai_client::remote::client_for(&server) {
                    Some(client) => {
                        self.follow(client, known);
                        Reply::ok()
                    }
                    None => Reply { error: Some(format!("no token saved for {server}")), ..Reply::default() },
                },
                // A terminal can't raise itself; the board is there when you look.
                Request::Focus => Reply::ok(),
                Request::List => Reply {
                    ok: true,
                    tabs: self.tabs.iter().map(|o| o.tab.location.label()).collect(),
                    active: self.active,
                    error: None,
                },
            };
            let _ = reply.send(r);
        }
    }

    /// Desktop notification (and a bell) for agent posts that arrived while
    /// the terminal was unfocused.
    fn notify(&mut self) {
        let mut any = false;
        for o in &mut self.tabs {
            let posts = std::mem::take(&mut o.tab.new_posts);
            if !self.focused && !posts.is_empty() {
                todai_client::notify::desktop(&o.tab.title(), &posts);
                any = true;
            }
        }
        if any {
            use std::io::Write;
            let _ = std::io::stdout().write_all(b"\x07");
            let _ = std::io::stdout().flush();
        }
    }

    fn save_session(&mut self) {
        if !self.persist {
            return;
        }
        let now = Session {
            tabs: self.tabs.iter().map(|o| o.tab.location.clone()).collect(),
            active: self.active,
            follow: self.follows.saved(),
        };
        if now != self.saved {
            if let Err(e) = now.save(SESSION) {
                self.notice = Some(format!("saving session: {e:#}"));
            }
            self.saved = now;
        }
    }

    /// Closes tab `i`, asking first if items wait for you or you started
    /// an answer there.
    fn close_tab(&mut self, i: usize) {
        let Some(o) = self.tabs.get(i) else { return };
        if waiting(&o.tab) > 0 || o.tab.drafts.values().any(|d| !d.trim().is_empty()) {
            self.confirm_close = Some(i);
        } else {
            self.close_tab_now(i);
        }
    }

    pub fn close_tab_now(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.tabs.remove(i);
            if self.active > i || self.active >= self.tabs.len() {
                self.active = self.active.saturating_sub(1);
            }
        }
    }

    fn open_locations(&self) -> Vec<Location> {
        self.tabs.iter().map(|o| o.tab.location.clone()).collect()
    }

    /// Opens (or closes) the board list over `paths`; `tracked::list()` in
    /// the real app.
    pub fn toggle_boards(&mut self) {
        self.boards = match self.boards {
            Some(_) => None,
            None => Some(BoardList::new(&self.tracked(), &self.open_locations(), boardlist::now_ms())),
        };
    }

    /// The tracked boards; tests set `test_tracked` instead of touching the
    /// user's state.
    fn tracked(&self) -> Vec<PathBuf> {
        match &self.test_tracked {
            Some(paths) => paths.iter().filter(|p| p.is_file()).cloned().collect(),
            None => tracked::list(),
        }
    }

    fn reload_boards(&mut self) {
        let (paths, open) = (self.tracked(), self.open_locations());
        if let Some(list) = &mut self.boards {
            list.reload(&paths, &open, boardlist::now_ms());
        }
    }

    fn close_tabs_of(&mut self, path: &std::path::Path) {
        while let Some(i) = self.tabs.iter().position(|o| matches!(&o.tab.location, Location::File(p) if p == path)) {
            self.close_tab_now(i);
        }
    }

    fn track(&mut self, path: &std::path::Path) {
        match &mut self.test_tracked {
            Some(paths) => paths.push(path.to_owned()),
            None => tracked::track(path),
        }
    }

    pub fn board_action(&mut self, action: Action) {
        let notice = match action {
            Action::Close => {
                self.boards = None;
                return;
            }
            Action::Open(loc) => {
                self.open_or_focus(loc);
                self.boards = None;
                return;
            }
            Action::Delete(_) | Action::ArchiveStale(_) => {
                self.confirm_boards = Some(action);
                return;
            }
            Action::ToggleArchive(paths) => self.toggle_archive(&paths),
            Action::Create { dir, name } => match manage::create(&dir, &name) {
                Ok(path) => {
                    self.track(&path);
                    let at = self.active;
                    self.open(Location::File(path));
                    self.active = at.min(self.tabs.len().saturating_sub(1));
                    boardlist::created_notice(&name)
                }
                Err(e) => format!("{e:#}"),
            },
        };
        self.reload_boards();
        if let Some(list) = &mut self.boards {
            list.notice = Some(notice);
        }
    }

    fn toggle_archive(&mut self, paths: &[PathBuf]) -> String {
        let track = self.test_tracked.is_none();
        let msg = manage::toggle_archive_all(paths, track, |p| self.close_tabs_of(p));
        if let Some(t) = &mut self.test_tracked {
            // What tracking would have recorded: the boards' new places.
            for p in paths {
                let other = if manage::is_archived(p) {
                    manage::boards_dir(p).join(p.file_name().unwrap())
                } else {
                    p.parent().unwrap().join("archive").join(p.file_name().unwrap())
                };
                t.push(other);
            }
        }
        msg
    }

    /// Carries out a confirmed delete / archive-stale.
    fn confirmed(&mut self, action: Action) {
        let notice = match action {
            Action::Delete(paths) => {
                let trash = self.test_trash.clone().or_else(manage::home_trash);
                manage::delete_all(&paths, trash.as_deref(), |p| self.close_tabs_of(p))
            }
            Action::ArchiveStale(paths) => self.toggle_archive(&paths),
            _ => return,
        };
        self.reload_boards();
        if let Some(list) = &mut self.boards {
            list.notice = Some(notice);
        }
    }

    fn step_tab(&mut self, d: i32) {
        let n = self.tabs.len() as i32;
        if n > 0 {
            self.active = (self.active as i32 + d).rem_euclid(n) as usize;
        }
    }

    // ---- input -----------------------------------------------------------

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                self.notice = None;
                self.on_key(k);
            }
            Event::FocusGained => self.focused = true,
            Event::FocusLost => self.focused = false,
            Event::Paste(s) => {
                if let Some(o) = self.tabs.get_mut(self.active)
                    && let Some(e) = o.edit.as_mut()
                    && let Some(text) = edit_text(&mut o.tab, &e.target)
                {
                    edit::paste(e, text, &s);
                }
            }
            Event::Mouse(m) => self.on_mouse(m),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let yes = matches!(k.code, KeyCode::Enter | KeyCode::Char('y'));
        let no = matches!(k.code, KeyCode::Esc | KeyCode::Char('n'));

        // Dialogs own the keyboard while shown.
        if let Some(i) = self.confirm_close {
            if yes {
                self.confirm_close = None;
                self.close_tab_now(i);
            } else if no {
                self.confirm_close = None;
            }
            return;
        }
        if let Some(action) = self.confirm_boards.clone() {
            if yes {
                self.confirm_boards = None;
                self.confirmed(action);
            } else if no {
                self.confirm_boards = None;
            }
            return;
        }
        if self.confirm_quit {
            if yes {
                self.quit = true;
            } else if no {
                self.confirm_quit = false;
            }
            return;
        }
        if let Some(o) = self.tabs.get_mut(self.active)
            && let Some(ids) = o.tab.confirm_trash.clone()
        {
            if yes {
                o.tab.confirm_trash = None;
                if o.tab.send(Kind::Trash { ids }) {
                    o.tab.nav.marked.clear();
                }
            } else if no {
                o.tab.confirm_trash = None;
            }
            return;
        }
        if let Some(scroll) = self.cheatsheet.as_mut() {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q' | '?') | KeyCode::F(1) => self.cheatsheet = None,
                KeyCode::Char('j') | KeyCode::Down => *scroll += 1,
                KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll += 10,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
                KeyCode::Char('G') | KeyCode::End => *scroll = usize::MAX,
                _ => {}
            }
            return;
        }

        // The board list owns the keyboard while open.
        if let Some(list) = &mut self.boards {
            match k.code {
                KeyCode::Char('o') if ctrl => self.boards = None,
                KeyCode::Char('q' | 'c') if ctrl => self.confirm_quit = true,
                KeyCode::Char('?') | KeyCode::F(1) if list.naming.is_none() && !list.filtering => {
                    self.cheatsheet = Some(0);
                }
                KeyCode::Backspace => list.backspace(),
                _ => {
                    if let Some(a) = input(&k).and_then(|i| list.input(&i, true)) {
                        self.board_action(a);
                    }
                }
            }
            return;
        }

        // Anywhere.
        let typing = self
            .tabs
            .get(self.active)
            .is_some_and(|o| o.edit.is_some() && matches!(o.tab.nav.mode, Mode::Insert | Mode::Prompt));
        match k.code {
            KeyCode::Char('q' | 'c') if ctrl => return self.confirm_quit = true,
            // While typing, Ctrl+W deletes a word (as in a shell): see edit.rs.
            KeyCode::Char('w') if ctrl && !typing => return self.close_tab(self.active),
            KeyCode::Char('o') if ctrl => return self.toggle_boards(),
            KeyCode::Char(c @ '1'..='9') if alt => {
                let i = c as usize - '1' as usize;
                if i < self.tabs.len() {
                    self.active = i;
                }
                return;
            }
            KeyCode::F(1) => return self.cheatsheet = Some(0),
            _ => {}
        }
        let Some(o) = self.tabs.get_mut(self.active) else {
            if k.code == KeyCode::Char('g') {
                // `gb` with no tab open: the list is how you open one.
                return self.toggle_boards();
            }
            if matches!(k.code, KeyCode::Char('q' | '?')) {
                if k.code == KeyCode::Char('q') {
                    self.confirm_quit = true;
                } else {
                    self.cheatsheet = Some(0);
                }
            }
            return;
        };
        let nav = &o.tab.nav;
        let plain_q = k.code == KeyCode::Char('q') && !ctrl && !alt;
        if plain_q && nav.mode == Mode::Normal && nav.zoom.is_none() && nav.prefix.is_none() {
            self.confirm_quit = true;
            return;
        }

        let mut out = Outcome::default();
        let used = match input(&k) {
            Some(i) => o.tab.nav.input(&i, Env::default(), &o.tab.state, &mut o.tab.drafts, true, &mut out),
            None => false,
        };
        if !used
            && matches!(o.tab.nav.mode, Mode::Insert | Mode::Prompt)
            && let Some(e) = o.edit.as_mut()
            && let Some(text) = edit_text(&mut o.tab, &e.target)
        {
            edit::key(e, text, &k);
        }
        self.apply(out);
    }

    /// What navigation asked for, beyond its own state.
    fn apply(&mut self, out: Outcome) {
        let Some(o) = self.tabs.get_mut(self.active) else { return };
        if out.leave_input {
            o.edit = None;
        }
        if out.toggle_cheatsheet {
            self.cheatsheet = if self.cheatsheet.is_some() { None } else { Some(0) };
        }
        if out.close_cheatsheet {
            self.cheatsheet = None;
        }
        if let Some(ids) = out.trash {
            o.tab.confirm_trash = Some(ids);
        }
        let open_boards = out.open_boards;
        if let Some(text) = out.copy {
            self.notice = Some(format!("copied {} characters", text.chars().count()));
            self.clipboard = Some(text);
        }
        for kind in out.kinds {
            ui::app_send(o, kind);
        }
        self.step_tab(out.tab_step);
        if open_boards && self.boards.is_none() {
            self.toggle_boards();
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let pos = (m.column, m.row);
        if let Some(list) = &mut self.boards {
            // Over the list: the wheel moves the cursor, a click opens a row.
            let a = match m.kind {
                MouseEventKind::ScrollDown => list.input(&Input::Key { key: Key::Down, pressed: true, mods: Mods::default() }, true),
                MouseEventKind::ScrollUp => list.input(&Input::Key { key: Key::Up, pressed: true, mods: Mods::default() }, true),
                MouseEventKind::Down(MouseButton::Left) if self.confirm_boards.is_none() && !self.confirm_quit => {
                    let row = self.board_hits.iter().find(|(y, _)| *y == m.row).map(|(_, p)| p.clone());
                    row.map(|p| Action::Open(Location::File(p)))
                }
                _ => None,
            };
            if let Some(a) = a {
                self.board_action(a);
            }
            return;
        }
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = m.kind == MouseEventKind::ScrollDown;
                let Some(o) = self.tabs.get_mut(self.active) else { return };
                if o.tab.nav.zoom.is_some() {
                    let by = if down { 3.0 } else { -3.0 };
                    o.tab.nav.zoom_scroll_to = Some((o.tab.nav.zoom_offset + by).max(0.0));
                    return;
                }
                if let Some(i) = self.columns.iter().position(|c| c.rect.contains(pos.into())) {
                    let s = &mut o.scroll[i.min(1)];
                    *s = if down { *s + 3 } else { s.saturating_sub(3) };
                }
            }
            MouseEventKind::Down(MouseButton::Left) => self.click(pos, m.modifiers.contains(KeyModifiers::SHIFT)),
            _ => {}
        }
    }

    fn click(&mut self, (x, y): (u16, u16), shift: bool) {
        if self.confirm_quit || self.cheatsheet.is_some() {
            return;
        }
        if y == 0 {
            if let Some(&(_, _, i)) = self.tab_hits.iter().find(|(a, b, _)| (*a..*b).contains(&x)) {
                self.active = i;
            }
            return;
        }
        let Some(col) = self.columns.iter().find(|c| c.rect.contains(Position::new(x, y))) else { return };
        let line = col.scroll + (y - col.rect.y) as usize;
        let cx = (x - col.rect.x) as usize;
        let here: Vec<&Hit> = col.hits.iter().filter(|(l, a, b, _)| *l == line && (*a..*b).contains(&cx)).map(|h| &h.3).collect();
        // The most specific target wins over the card around it.
        let Some(hit) = here.iter().find(|h| !matches!(h, Hit::Focus(_))).or(here.first()).map(|h| (*h).clone()) else { return };
        let Some(o) = self.tabs.get_mut(self.active) else { return };
        if o.tab.nav.zoom.is_some() || o.tab.confirm_trash.is_some() {
            return;
        }
        let nav = &mut o.tab.nav;
        match hit {
            Hit::Focus(Focus::Section(s)) => {
                nav.set_open(s, !nav.is_open(s));
                nav.cursor = Some(Focus::Section(s));
            }
            Hit::Focus(f) => {
                if nav.cursor.as_ref() == Some(&f) && let Focus::Item(id) = &f {
                    // A second click on a pending item starts typing its answer.
                    let pending = o.tab.state.decision(id).is_some_and(|d| d.status == todai_client::store::DecisionStatus::Pending)
                        || o.tab.state.todo(id).is_some_and(|t| t.done.is_none());
                    if pending {
                        nav.focus_request = Some(FocusRequest::Input(id.clone()));
                    }
                }
                nav.cursor = Some(f);
            }
            Hit::Option { id, n } => {
                if let Some(opt) = o.tab.state.decision(&id).and_then(|d| d.options.get(n)) {
                    o.tab.drafts.insert(id.clone(), opt.clone());
                    nav.cursor = Some(Focus::Item(id.clone()));
                    nav.focus_request = Some(FocusRequest::Input(id));
                }
            }
            Hit::Mark(id) => nav.click_mark(&id, shift),
            Hit::Fold(rid) => {
                let open = nav.elab_is_open(&o.tab.state, &rid);
                nav.elab_open.insert(rid.clone(), !open);
                nav.cursor = Some(Focus::Elab(rid));
            }
        }
        // Clicking elsewhere stops typing, as in the window.
        if !matches!(nav.focus_request, Some(FocusRequest::Input(_))) && nav.mode == Mode::Insert {
            o.edit = None;
        }
    }
}

/// Decisions and todos waiting for the operator on a tab.
pub fn waiting(t: &Tab) -> usize {
    use todai_client::store::DecisionStatus;
    t.state.decisions.iter().filter(|d| d.status == DecisionStatus::Pending).count()
        + t.state.todos.iter().filter(|t| t.done.is_none()).count()
}

/// The text a field edits: a draft (created on first keystroke) or the
/// prompt's note.
fn edit_text<'t>(tab: &'t mut Tab, target: &Target) -> Option<&'t mut String> {
    match target {
        Target::Draft(id) => Some(tab.drafts.entry(id.clone()).or_default()),
        Target::Prompt => tab.nav.prompt.as_mut().map(|p| &mut p.note),
    }
}

/// A terminal key as navigation input. Plain characters are text, like
/// typed text in the window; with Ctrl or Alt they are keys.
pub fn input(k: &KeyEvent) -> Option<Input> {
    let m = k.modifiers;
    let (ctrl, alt) = (m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::ALT));
    let mods = Mods { ctrl, shift: m.contains(KeyModifiers::SHIFT), alt, command: ctrl };
    let key = match k.code {
        KeyCode::Char(c) if !ctrl && !alt => return Some(Input::Text(c.to_string())),
        KeyCode::Char(c) => Key::Letter(c.to_ascii_lowercase()),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Tab | KeyCode::BackTab => Key::Tab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::F(1) => return None,
        _ => Key::Other,
    };
    Some(Input::Key { key, pressed: true, mods })
}
