//! The TUI's state and input handling: open tabs, dialogs, the socket that
//! `todai attach` / `todai FILE` talk to, and routing keys to the shared
//! navigation (`todai_client::nav`) or to the field being typed into.

use std::path::PathBuf;
use std::sync::mpsc;

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
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
    wake: Wake,
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
            wake,
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
        self.saved = saved;
    }

    pub fn open_or_focus(&mut self, loc: Location) {
        if let Some(i) = self.tabs.iter().position(|o| o.tab.location.same(&loc)) {
            self.active = i;
            return;
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
        self.active = self.tabs.len() - 1;
    }

    pub fn open_file(&mut self, path: PathBuf) {
        self.open_or_focus(Location::File(path));
    }

    /// Socket requests, board reloads, notifications, session: everything
    /// that isn't a key. Called before every frame.
    pub fn tick(&mut self) {
        self.serve_requests();
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
        let now = Session { tabs: self.tabs.iter().map(|o| o.tab.location.clone()).collect(), active: self.active };
        if now != self.saved {
            if let Err(e) = now.save(SESSION) {
                self.notice = Some(format!("saving session: {e:#}"));
            }
            self.saved = now;
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.tabs.remove(i);
            if self.active > i || self.active >= self.tabs.len() {
                self.active = self.active.saturating_sub(1);
            }
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

        // Anywhere.
        match k.code {
            KeyCode::Char('q' | 'c') if ctrl => return self.confirm_quit = true,
            KeyCode::Char('w') if ctrl => return self.close_tab(self.active),
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
        for kind in out.kinds {
            ui::app_send(o, kind);
        }
        self.step_tab(out.tab_step);
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let pos = (m.column, m.row);
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
