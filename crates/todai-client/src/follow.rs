//! Following a server: boards registered there after a frontend opened it
//! (an agent's `todai attach`) open as new tabs, like local boards do.
//!
//! A server is followed when it was opened without naming boards
//! (`todai open --server URL`, `todai-tui --server URL`). Each followed
//! server gets a thread that re-lists its boards every [`INTERVAL`]; the
//! frontend drains [`Follows::new_boards`] on its tick. A board is opened at
//! most once: ids already seen are remembered (and saved in the session), so
//! a tab the operator closed stays closed. Following stops when the last tab
//! from that server is closed.

use std::collections::BTreeSet;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use todai_core::api::BoardInfo;

use crate::remote::Client;
use crate::tab::{Location, Wake};

/// How often a followed server's board list is fetched.
pub const INTERVAL: Duration = Duration::from_secs(15);

/// A followed server as saved in the session.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Followed {
    pub server: String,
    /// Board ids already opened or deliberately skipped.
    pub known: BTreeSet<String>,
}

#[derive(Default)]
pub struct Follows {
    list: Vec<Follow>,
    interval: Option<Duration>,
}

struct Follow {
    saved: Followed,
    /// The latest board list the poll thread fetched, not yet looked at.
    latest: Arc<Mutex<Option<Vec<BoardInfo>>>>,
    /// Dropping it stops the poll thread.
    _stop: mpsc::Sender<()>,
}

impl Follows {
    /// Polls every `interval` instead of [`INTERVAL`]; for tests.
    pub fn with_interval(interval: Duration) -> Follows {
        Follows { list: Vec::new(), interval: Some(interval) }
    }

    /// Follows the servers saved in a session; those without a saved token
    /// are dropped (their tabs already say why they are empty).
    pub fn restore(&mut self, saved: &[Followed], wake: &Wake) {
        for f in saved {
            if let Some(client) = crate::remote::client_for(&f.server) {
                self.follow(client, f.known.iter().cloned(), wake.clone());
            }
        }
    }

    /// Starts following `client`'s server; boards in `known` are never
    /// opened by it. Following a server again adds to its known boards.
    pub fn follow(&mut self, client: Client, known: impl IntoIterator<Item = String>, wake: Wake) {
        if let Some(f) = self.list.iter_mut().find(|f| f.saved.server == client.base()) {
            f.saved.known.extend(known);
            return;
        }
        let latest = Arc::new(Mutex::new(None));
        let (stop, stopped) = mpsc::channel();
        let interval = self.interval.unwrap_or(INTERVAL);
        let saved = Followed { server: client.base().to_owned(), known: known.into_iter().collect() };
        {
            let latest = latest.clone();
            std::thread::spawn(move || {
                loop {
                    // An unreachable server just means no news; its tabs
                    // already show the error.
                    if let Ok(boards) = client.boards() {
                        *latest.lock().unwrap() = Some(boards);
                        wake();
                    }
                    match stopped.recv_timeout(interval) {
                        Err(RecvTimeoutError::Timeout) => continue,
                        _ => return,
                    }
                }
            });
        }
        self.list.push(Follow { saved, latest, _stop: stop });
    }

    /// Boards that appeared on followed servers since the last call, to open
    /// as tabs. Each is returned once.
    pub fn new_boards(&mut self) -> Vec<Location> {
        let mut new = Vec::new();
        for f in &mut self.list {
            let Some(boards) = f.latest.lock().unwrap().take() else { continue };
            for b in boards {
                if f.saved.known.insert(b.id.clone()) {
                    new.push(Location::Remote { server: f.saved.server.clone(), board: b.id, name: b.name });
                }
            }
        }
        new
    }

    /// Stops following servers none of `tabs` comes from.
    pub fn retain_open(&mut self, tabs: &[&Location]) {
        self.list.retain(|f| tabs.iter().any(|t| matches!(t, Location::Remote { server, .. } if *server == f.saved.server)));
    }

    /// What to save in the session.
    pub fn saved(&self) -> Vec<Followed> {
        self.list.iter().map(|f| f.saved.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use todai_core::store::{Actor, Kind, Log};

    fn board(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let log = Log::new(dir.join(format!("{name}.jsonl")));
        log.append(Actor::Agent, |_| Ok(Kind::TodoAdd { id: "t1".into(), title: "x".into(), context: None })).unwrap();
        log.path().to_owned()
    }

    fn until_new(follows: &mut Follows) -> Vec<Location> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let new = follows.new_boards();
            if !new.is_empty() {
                return new;
            }
            assert!(Instant::now() < deadline, "timed out waiting for a new board");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn opens_boards_registered_later_once() {
        let dir = tempfile::tempdir().unwrap();
        let first = board(dir.path(), "first");
        let cfg = todai_server::Config {
            listen: "127.0.0.1:0".into(),
            dir: dir.path().join("server"),
            boards: vec![first.clone()],
        };
        let server = todai_server::Server::bind(&cfg).unwrap().spawn();
        let client = Client::new(&format!("http://{}", server.addr), &server.token);

        let mut follows = Follows::with_interval(Duration::from_millis(50));
        follows.follow(client.clone(), [todai_core::api::board_id(&first)], Arc::new(|| {}));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(follows.new_boards(), vec![], "boards opened with the server aren't new");

        // An agent attaches: `todai attach` registers with the server.
        let later = board(dir.path(), "later");
        client.register(&later).unwrap();
        let new = until_new(&mut follows);
        let id = todai_core::api::board_id(&later);
        assert_eq!(new, vec![Location::Remote { server: client.base().into(), board: id.clone(), name: "later".into() }]);

        // Closing that tab doesn't bring it back.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(follows.new_boards(), vec![]);
        assert_eq!(follows.saved()[0].known.len(), 2);
        assert!(follows.saved()[0].known.contains(&id));

        // Closing the server's last tab stops following it.
        let other = Location::File("/x.jsonl".into());
        follows.retain_open(&[&new[0]]);
        assert_eq!(follows.saved().len(), 1);
        follows.retain_open(&[&other]);
        assert_eq!(follows.saved(), vec![]);
    }
}
