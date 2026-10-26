//! Client for a todai server's HTTP API (`docs/server.md`): the window's
//! remote tabs, `todai open --server`, and registering boards with a local
//! server on `todai attach`. Plain HTTP only.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use serde::de::DeserializeOwned;
use todai_core::api::{ApiError, BoardInfo, Events, Health, Register};
use todai_core::store::{Event, Kind};

#[derive(Clone)]
pub struct Client {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    /// `base` is the server's URL, e.g. `http://host:7341`.
    pub fn new(base: &str, token: &str) -> Client {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Client { base: base.trim_end_matches('/').to_owned(), token: token.to_owned(), agent }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn health(&self) -> Result<Health> {
        self.call(self.agent.get(self.url("/api/health")).call())
    }

    pub fn boards(&self) -> Result<Vec<BoardInfo>> {
        self.call(self.agent.get(self.url("/api/boards")).header("Authorization", self.auth()).call())
    }

    /// Events after `after`, waiting up to `wait` for some to arrive.
    pub fn events(&self, id: &str, after: u64, wait: Duration) -> Result<Events> {
        let url = self.url(&format!("/api/boards/{id}/events?after={after}&wait={}", wait.as_secs()));
        let req = self.agent.get(url).header("Authorization", self.auth());
        let req = req.config().timeout_global(Some(wait + Duration::from_secs(10))).build();
        self.call(req.call())
    }

    /// Posts one operator action; the server assigns `seq` (and elaboration ids).
    pub fn act(&self, id: &str, kind: &Kind) -> Result<Event> {
        let req = self.agent.post(self.url(&format!("/api/boards/{id}/actions"))).header("Authorization", self.auth());
        self.call(req.send_json(kind))
    }

    /// Only accepted from the server's own machine.
    pub fn register(&self, path: &Path) -> Result<BoardInfo> {
        let req = self.agent.post(self.url("/api/boards")).header("Authorization", self.auth());
        self.call(req.send_json(Register { path: path.to_owned() }))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    fn call<T: DeserializeOwned>(&self, r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<T> {
        let mut r = r.map_err(|e| anyhow::anyhow!("{}: {e}", self.base))?;
        if !r.status().is_success() {
            let status = r.status();
            match r.body_mut().read_json::<ApiError>() {
                Ok(e) => bail!("{}: {status}: {}", self.base, e.error),
                Err(_) => bail!("{}: {status}", self.base),
            }
        }
        Ok(r.body_mut().read_json()?)
    }
}

/// Tokens for servers the window has opened boards from, by URL, in
/// `$XDG_STATE_HOME/todai/remotes.json` (readable only by the user).
fn tokens_file() -> Option<std::path::PathBuf> {
    Some(crate::config::state_dir()?.join("remotes.json"))
}

fn saved_tokens() -> BTreeMap<String, String> {
    tokens_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Remembers `token` for `server`, so remote tabs come back next session.
pub fn save_token(server: &str, token: &str) -> Result<()> {
    let mut tokens = saved_tokens();
    if tokens.get(server).map(String::as_str) == Some(token) {
        return Ok(());
    }
    tokens.insert(server.to_owned(), token.to_owned());
    let path = tokens_file().ok_or_else(|| anyhow::anyhow!("no state dir"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(serde_json::to_string_pretty(&tokens)?.as_bytes())?;
    drop(f);
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// The client for `server`: its saved token, or the local server's own.
pub fn client_for(server: &str) -> Option<Client> {
    let server = server.trim_end_matches('/');
    if let Some(t) = saved_tokens().get(server) {
        return Some(Client::new(server, t));
    }
    local().filter(|c| c.base == server)
}

/// The local server's client, if one has run with this state dir.
pub fn local() -> Option<Client> {
    let (addr, token) = todai_server::local(&todai_server::state_dir().ok()?)?;
    Some(Client::new(&format!("http://{addr}"), &token))
}

/// Best effort: makes `path` reachable through a running local server, so
/// boards agents attach show up on remote frontends. Never fails the caller.
pub fn register_local(path: &Path) {
    let Some(client) = local() else { return };
    let client = Client {
        agent: ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(1)))
            .build()
            .into(),
        ..client
    };
    // The address file outlives a killed server, and something else may
    // own that port now: only send the token to a todai server.
    if client.health().is_ok() {
        let _ = client.register(path);
    }
}
