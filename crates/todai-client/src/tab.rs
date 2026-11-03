use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

use crate::nav::Nav;
use crate::remote::Client;
use crate::store::{Actor, Event, Kind, Log, State};

/// Called from watcher and network threads when a tab has something new,
/// so the frontend redraws (the window requests a repaint, the TUI wakes its
/// event loop).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Where a tab's board lives. Saved in the session; a bare string is a file,
/// which keeps sessions from before remote tabs readable.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Location {
    File(PathBuf),
    /// A board on a todai server, by its id there.
    Remote {
        server: String,
        board: String,
        #[serde(default)]
        name: String,
    },
}

impl Location {
    /// Same board, whatever name it was saved under.
    pub fn same(&self, other: &Location) -> bool {
        match (self, other) {
            (Location::File(a), Location::File(b)) => a == b,
            (Location::Remote { server: s, board: b, .. }, Location::Remote { server: s2, board: b2, .. }) => {
                s == s2 && b == b2
            }
            _ => false,
        }
    }

    /// Full description, for tooltips and `todai tabs`.
    pub fn label(&self) -> String {
        match self {
            Location::File(p) => p.display().to_string(),
            Location::Remote { server, board, name } => format!("{server} {} ({board})", or_id(name, board)),
        }
    }
}

fn or_id<'a>(name: &'a str, id: &'a str) -> &'a str {
    if name.is_empty() { id } else { name }
}

/// One open board: its folded state, unsent drafts, and the source that
/// flags the tab dirty whenever the board changes.
pub struct Tab {
    pub location: Location,
    pub state: State,
    /// Resolution drafts keyed by decision id, and note drafts keyed by todo id.
    pub drafts: HashMap<String, String>,
    pub error: Option<String>,
    pub nav: Nav,
    /// Items waiting for the operator to confirm their removal.
    pub confirm_trash: Option<Vec<String>>,
    /// Agent posts seen since the last check, for desktop notifications.
    pub new_posts: Vec<String>,
    /// Highest seq loaded so far; `None` until the first load, so opening a
    /// board never notifies about its existing items.
    seen_seq: Option<u64>,
    source: Source,
}

enum Source {
    /// A board file on this machine, watched with inotify.
    File { log: Log, dirty: Arc<AtomicBool>, _watcher: Option<notify::RecommendedWatcher> },
    Remote(Remote),
    /// A board that can't be reached at all, e.g. a server with no token.
    Unavailable,
}

impl Tab {
    pub fn open(path: PathBuf, wake: Wake) -> Tab {
        let dirty = Arc::new(AtomicBool::new(true));
        let mut error = None;
        let watcher = watch(&path, dirty.clone(), wake)
            .inspect_err(|e| error = Some(format!("not watching for changes: {e}")))
            .ok();
        let source = Source::File { log: Log::new(path.clone()), dirty, _watcher: watcher };
        Tab { error, ..Tab::new(Location::File(path), source) }
    }

    /// A board on a server; `location` must be `Location::Remote`.
    pub fn remote(location: Location, client: Client, wake: Wake) -> Tab {
        let Location::Remote { board, .. } = &location else { panic!("not a remote location") };
        let source = Source::Remote(Remote::start(client, board.clone(), wake));
        Tab::new(location, source)
    }

    /// A tab that only shows its error; kept so the session remembers it.
    pub fn open_failed(location: Location) -> Tab {
        Tab::new(location, Source::Unavailable)
    }

    fn new(location: Location, source: Source) -> Tab {
        Tab {
            location,
            state: State::default(),
            drafts: HashMap::new(),
            error: None,
            nav: Nav::default(),
            confirm_trash: None,
            new_posts: Vec::new(),
            seen_seq: None,
            source,
        }
    }

    pub fn title(&self) -> String {
        match &self.location {
            Location::File(p) => todai_core::api::board_name(p),
            Location::Remote { server, board, name } => {
                let host = server.split_once("://").map_or(server.as_str(), |(_, h)| h);
                format!("{}@{host}", or_id(name, board))
            }
        }
    }

    /// Reloads regardless of the watcher (tests, and frontends that know
    /// the file changed).
    pub fn reload_if_dirty_now(&mut self) {
        if let Source::File { dirty, .. } = &self.source {
            dirty.store(true, Ordering::Relaxed);
        }
        self.reload_if_dirty();
    }

    pub fn reload_if_dirty(&mut self) {
        let loaded = match &mut self.source {
            Source::File { log, dirty, .. } => {
                if !dirty.swap(false, Ordering::Relaxed) {
                    return;
                }
                log.load_state()
            }
            Source::Remote(r) => {
                let Some(loaded) = r.take_updates(&mut self.drafts, &mut self.error) else { return };
                loaded
            }
            Source::Unavailable => return,
        };
        match loaded {
            Ok((events, state)) => {
                if let Some(seen) = self.seen_seq {
                    self.new_posts.extend(agent_posts(&events, seen));
                }
                self.seen_seq = Some(state.last_seq);
                self.state = state;
                if matches!(self.source, Source::File { .. }) {
                    self.error = None;
                }
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// Appends an operator event; on success the source triggers a reload.
    /// A remote tab only queues it: a failure shows up as the tab's error
    /// and puts the text back in its draft.
    pub fn send(&mut self, mut kind: Kind) -> bool {
        let (log, dirty) = match &self.source {
            Source::File { log, dirty, .. } => (log, dirty),
            Source::Remote(r) => {
                r.send(kind);
                return true;
            }
            Source::Unavailable => return false,
        };
        let result = log.append(Actor::Operator, |s| {
            if let Kind::ElabRequest { rid, .. } = &mut kind {
                *rid = s.next_elab_id();
            }
            Ok(kind)
        });
        match result {
            Ok(_) => {
                // Reload now rather than waiting a frame for the watcher.
                dirty.store(true, Ordering::Relaxed);
                true
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                false
            }
        }
    }
}

/// How long one long-poll waits on the server before asking again.
const LONG_POLL: Duration = Duration::from_secs(25);
/// Pause after a failed poll, so a down server isn't hammered.
const RETRY: Duration = Duration::from_secs(3);

/// A remote board: a thread long-polls the server for events and another
/// posts the operator's actions in order. The tab folds the events itself,
/// exactly as it would a local file.
struct Remote {
    events: Vec<Event>,
    inbox: Arc<Mutex<Inbox>>,
    actions: mpsc::Sender<Kind>,
    stop: Arc<AtomicBool>,
}

/// What the worker threads hand to the tab.
#[derive(Default)]
struct Inbox {
    dirty: bool,
    events: Vec<Event>,
    /// `events` replaces everything loaded so far.
    reset: bool,
    poll_error: Option<String>,
    send_error: Option<String>,
    /// Drafts to put back after a failed send.
    restore: Vec<(String, String)>,
}

impl Remote {
    fn start(client: Client, board: String, wake: Wake) -> Remote {
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (actions, rx) = mpsc::channel::<Kind>();

        let (c, b, i, s, w) = (client.clone(), board.clone(), inbox.clone(), stop.clone(), wake.clone());
        std::thread::spawn(move || poll_loop(&c, &b, &i, &s, &*w));
        let shared = inbox.clone();
        std::thread::spawn(move || {
            let inbox = shared;
            // Ends when the tab (the only sender) is dropped.
            for kind in rx {
                let failed = client.act(&board, &kind).err();
                let mut inbox = inbox.lock().unwrap();
                if let Some(e) = &failed {
                    inbox.restore.extend(draft_of(&kind));
                    inbox.send_error = Some(format!("not sent: {e:#}"));
                } else {
                    inbox.send_error = None;
                }
                inbox.dirty = true;
                drop(inbox);
                wake();
            }
        });
        Remote { events: Vec::new(), inbox, actions, stop }
    }

    fn send(&self, kind: Kind) {
        let _ = self.actions.send(kind);
    }

    /// New state if the workers brought anything since the last call.
    fn take_updates(
        &mut self,
        drafts: &mut HashMap<String, String>,
        error: &mut Option<String>,
    ) -> Option<Result<(Vec<Event>, State)>> {
        let mut inbox = self.inbox.lock().unwrap();
        if !std::mem::take(&mut inbox.dirty) {
            return None;
        }
        if std::mem::take(&mut inbox.reset) {
            self.events.clear();
        }
        self.events.append(&mut inbox.events);
        for (k, v) in inbox.restore.drain(..) {
            drafts.entry(k).or_insert(v);
        }
        *error = inbox.send_error.clone().or_else(|| inbox.poll_error.clone());
        drop(inbox);
        Some(State::fold(&self.events).map(|s| (self.events.clone(), s)))
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // The poll thread notices after its current long-poll returns.
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn poll_loop(client: &Client, board: &str, inbox: &Mutex<Inbox>, stop: &AtomicBool, wake: &(dyn Fn() + Send + Sync)) {
    let mut after = 0;
    // The first request, and the first after a failure, return at once, so
    // the tab fills (or its error clears) without waiting for a change.
    let mut quick = true;
    while !stop.load(Ordering::Relaxed) {
        let wait = if quick { Duration::ZERO } else { LONG_POLL };
        match client.events(board, after, wait) {
            Ok(ev) => {
                let first_load = std::mem::take(&mut quick);
                after = ev.last_seq;
                let mut i = inbox.lock().unwrap();
                if ev.reset {
                    i.events = ev.events;
                    i.reset = true;
                } else {
                    i.events.extend(ev.events);
                }
                // An empty answer still matters on a quick poll: a first load, or a recovery.
                i.dirty |= first_load || !i.events.is_empty() || i.reset || i.poll_error.is_some();
                i.poll_error = None;
            }
            Err(e) => {
                quick = true;
                let mut i = inbox.lock().unwrap();
                i.poll_error = Some(format!("{e:#} (retrying)"));
                i.dirty = true;
                drop(i);
                wake();
                std::thread::sleep(RETRY);
                continue;
            }
        }
        wake();
    }
}

/// The draft a failed action came from, to give the operator their text back.
fn draft_of(kind: &Kind) -> Option<(String, String)> {
    match kind {
        Kind::DecisionResolve { id, text } => Some((id.clone(), text.clone())),
        Kind::TodoDone { id, note: Some(n) } => Some((id.clone(), n.clone())),
        _ => None,
    }
}

/// One line per agent post after `seen`: what the operator should look at.
pub fn agent_posts(events: &[Event], seen: u64) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.seq > seen && e.by == Actor::Agent)
        .filter_map(|e| match &e.kind {
            Kind::DecisionAdd { id, title, .. } => Some(format!("Decision {id}: {title}")),
            Kind::TodoAdd { id, title, .. } => Some(format!("Todo {id}: {title}")),
            Kind::ElabAdd { rid, .. } => Some(format!("Elaboration {rid} answered")),
            _ => None,
        })
        .collect()
}

fn watch(path: &Path, dirty: Arc<AtomicBool>, wake: Wake) -> Result<notify::RecommendedWatcher> {
    // Watch the directory, not the file: the file may not exist yet, and some
    // writers replace it instead of appending. Access events (open, read,
    // close without writing) are skipped: our own reload opens the file, and
    // reacting to that would reload and repaint forever.
    let dir = path.parent().unwrap_or(Path::new("/")).to_owned();
    std::fs::create_dir_all(&dir)?;
    let target = path.to_owned();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && !ev.kind.is_access()
            && ev.paths.iter().any(|p| p == &target)
        {
            dirty.store(true, Ordering::Relaxed);
            wake();
        }
    })?;
    w.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn own_reload_does_not_mark_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.jsonl");
        Log::new(path.clone())
            .append(Actor::Agent, |_| Ok(Kind::TodoAdd { id: "t1".into(), title: "x".into(), context: None }))
            .unwrap();
        let mut tab = Tab::open(path.clone(), Arc::new(|| {}));
        tab.reload_if_dirty();
        std::thread::sleep(Duration::from_millis(300));
        let Source::File { dirty, .. } = &tab.source else { unreachable!() };
        assert!(!dirty.load(Ordering::Relaxed), "reading the board must not look like a change");

        Log::new(path)
            .append(Actor::Agent, |_| Ok(Kind::TodoAdd { id: "t2".into(), title: "y".into(), context: None }))
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(dirty.load(Ordering::Relaxed), "an append must still be seen");
    }

    #[test]
    fn sessions_with_plain_paths_still_load() {
        let tabs: Vec<Location> = serde_json::from_str(r#"["/a/b.jsonl", {"server": "http://h:1", "board": "x"}]"#).unwrap();
        assert_eq!(tabs[0], Location::File("/a/b.jsonl".into()));
        assert!(tabs[1].same(&Location::Remote { server: "http://h:1".into(), board: "x".into(), name: "b".into() }));
        assert_eq!(serde_json::to_string(&tabs[0]).unwrap(), r#""/a/b.jsonl""#);
    }

    /// Reloads until `done` holds, or panics after a few seconds.
    fn reload_until(tab: &mut Tab, what: &str, done: impl Fn(&Tab) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(tab) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}; error: {:?}", tab.error);
            std::thread::sleep(Duration::from_millis(20));
            tab.reload_if_dirty();
        }
    }

    #[test]
    fn remote_tab_follows_and_writes_the_board() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path().join("b.jsonl"));
        let agent = |k: Kind| log.append(Actor::Agent, |_| Ok(k)).unwrap();
        agent(Kind::DecisionAdd { id: "d1".into(), title: "Ship?".into(), context: None, options: vec![] });
        let cfg = todai_server::Config {
            listen: "127.0.0.1:0".into(),
            dir: dir.path().join("server"),
            boards: vec![log.path().into()],
        };
        let server = todai_server::Server::bind(&cfg).unwrap().spawn();
        let client = Client::new(&format!("http://{}", server.addr), &server.token);
        let board = todai_core::api::board_id(log.path());
        let loc = Location::Remote { server: client.base().into(), board, name: "b".into() };
        let mut tab = Tab::remote(loc, client, Arc::new(|| {}));
        assert_eq!(tab.title(), format!("b@{}", server.addr));

        reload_until(&mut tab, "the first load", |t| t.state.decisions.len() == 1);
        assert!(tab.new_posts.is_empty(), "opening a board never notifies");

        // An agent post arrives through the long-poll.
        agent(Kind::TodoAdd { id: "t1".into(), title: "Push".into(), context: None });
        reload_until(&mut tab, "the agent's todo", |t| t.state.todos.len() == 1);
        assert_eq!(tab.new_posts, vec!["Todo t1: Push".to_owned()]);

        // The operator's answer lands in the file.
        assert!(tab.send(Kind::DecisionResolve { id: "d1".into(), text: "yes".into() }));
        reload_until(&mut tab, "the resolution", |t| t.state.decisions[0].status != crate::store::DecisionStatus::Pending);
        assert!(log.load().unwrap().iter().any(|e| e.by == Actor::Operator));

        // A refused action reports and gives the draft back.
        tab.send(Kind::DecisionResolve { id: "d9".into(), text: "lost?".into() });
        reload_until(&mut tab, "the refusal", |t| t.error.is_some());
        assert_eq!(tab.drafts.get("d9").map(String::as_str), Some("lost?"));
        assert!(tab.error.as_deref().unwrap().contains("not sent"), "{:?}", tab.error);
    }
}
