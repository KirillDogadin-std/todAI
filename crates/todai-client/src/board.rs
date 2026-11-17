//! A board as an agent names it: a `.jsonl` file on this machine, or a board
//! on a todai server, `http(s)://HOST:PORT/b/ID`, for agents that don't
//! share the operator's filesystem (another machine, a sandbox). The agent
//! commands (`todai add-decision`, `inbox`, `wait`, ...) and the hooks take
//! either through this type, so the protocol is the same both ways.
//!
//! Remote boards need an agent token (`todai-server device add NAME
//! --agent`), which may post agent events and nothing else.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use todai_core::store::{Actor, Event, Kind, Log, State};

use crate::remote::Client;

pub enum Board {
    File(Log),
    Remote { client: Client, id: String },
}

/// The address of board `id` on the server at `base`, as `todai attach
/// --server` prints it after the attached marker.
pub fn remote_url(base: &str, id: &str) -> String {
    format!("{}/b/{id}", base.trim_end_matches('/'))
}

/// `(base, id)` of a remote board address; `/#/b/ID` (the web app's link)
/// works too.
pub fn parse_url(s: &str) -> Option<(String, String)> {
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return None;
    }
    let s = s.trim_end_matches('/');
    let (base, id) = s.rsplit_once("/b/")?;
    let base = base.strip_suffix("/#").unwrap_or(base);
    (id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (base.to_owned(), id.to_owned()))
}

impl Board {
    /// A path (relative ones resolve against the current directory) or a
    /// remote board address.
    pub fn open(arg: &str) -> Result<Board> {
        if arg.starts_with("http://") || arg.starts_with("https://") {
            let Some((base, id)) = parse_url(arg) else {
                bail!("not a board address: {arg} (expected http(s)://HOST:PORT/b/ID, as `todai attach --server` prints)");
            };
            let client = crate::remote::agent_client_for(&base).with_context(|| {
                format!("no agent token for {base}: set TODAI_AGENT_TOKEN or run `todai attach --server {base} --token T NAME`")
            })?;
            return Ok(Board::Remote { client, id });
        }
        Ok(Board::File(Log::new(std::path::absolute(arg)?)))
    }

    /// The path or address, as the agent passes it back.
    pub fn label(&self) -> String {
        match self {
            Board::File(log) => log.path().display().to_string(),
            Board::Remote { client, id } => remote_url(client.base(), id),
        }
    }

    pub fn load(&self) -> Result<(Vec<Event>, State)> {
        match self {
            Board::File(log) => log.load_state(),
            Board::Remote { client, id } => {
                let events = client.events(id, 0, Duration::ZERO)?.events;
                let state = State::fold(&events)?;
                Ok((events, state))
            }
        }
    }

    /// Appends an event built from the current state. On a server the
    /// server picks new ids itself, so the returned event is the one to read
    /// them from.
    pub fn append(&self, by: Actor, build: impl FnOnce(&State) -> Result<Kind>) -> Result<Event> {
        match self {
            Board::File(log) => log.append(by, build),
            Board::Remote { client, id } => {
                if by != Actor::Agent {
                    bail!("operator commands work on files; answer remote boards in the window or the web app");
                }
                let (_, state) = self.load()?;
                client.act(id, &build(&state)?)
            }
        }
    }

    /// Blocks until the board may have events after `seq`, or `wait` passes.
    pub fn wait_after(&self, seq: u64, wait: Duration) -> Result<()> {
        match self {
            Board::Remote { client, id } => {
                client.events(id, seq, wait.min(Duration::from_secs(todai_core::api::MAX_WAIT_SECS)))?;
                Ok(())
            }
            Board::File(log) => {
                // Polling the size is cheap and avoids inotify edge cases with
                // editors and network filesystems; agents aren't latency-bound.
                let len = |l: &Log| std::fs::metadata(l.path()).map(|m| m.len()).ok();
                let (start, until) = (len(log), Instant::now() + wait);
                while Instant::now() < until && len(log) == start {
                    std::thread::sleep(Duration::from_millis(500).min(until.saturating_duration_since(Instant::now())));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip() {
        let id = "0123456789abcdef";
        assert_eq!(parse_url(&remote_url("https://box:7341/", id)), Some(("https://box:7341".into(), id.into())));
        assert_eq!(parse_url(&format!("http://h/#/b/{id}")), Some(("http://h".into(), id.into())));
        assert_eq!(parse_url("/home/u/.agents/todai/b/x.jsonl"), None);
        assert_eq!(parse_url("http://h/b/nothex"), None);
    }

    #[test]
    fn an_agent_token_uses_a_remote_board_like_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path().join("b.jsonl"));
        log.append(Actor::Agent, |_| Ok(Kind::TodoAdd { id: "t1".into(), title: "x".into(), context: None })).unwrap();
        let state_dir = dir.path().join("server");
        let cfg = todai_server::Config { listen: "127.0.0.1:0".into(), dir: state_dir.clone(), boards: vec![log.path().into()] };
        let server = todai_server::Server::bind(&cfg).unwrap().spawn();
        let token = todai_server::Tokens::load(&state_dir).unwrap().add("ci", todai_server::Role::Agent).unwrap();
        let client = Client::new(&format!("http://{}", server.addr), &token);
        let board = Board::Remote { client, id: todai_core::api::board_id(log.path()) };

        let e = board
            .append(Actor::Agent, |s| Ok(Kind::DecisionAdd { id: s.next_decision_id(), title: "Ship?".into(), context: None, options: vec![] }))
            .unwrap();
        assert!(matches!(e.kind, Kind::DecisionAdd { ref id, .. } if id == "d1"));
        assert_eq!(log.load_state().unwrap().1.decisions.len(), 1, "it lands in the file");
        assert!(board.append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id: "d1".into(), text: "y".into() })).is_err());

        // `wait` wakes on the operator's answer.
        let (_, state) = board.load().unwrap();
        let path = log.path().to_owned();
        let answer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            Log::new(path).append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id: "d1".into(), text: "yes".into() })).unwrap();
        });
        let start = Instant::now();
        board.wait_after(state.last_seq, Duration::from_secs(20)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(5));
        answer.join().unwrap();
        assert_eq!(board.load().unwrap().1.last_seq, state.last_seq + 1);
    }
}
