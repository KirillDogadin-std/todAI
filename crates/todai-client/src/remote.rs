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
use todai_core::api::{API_VERSION, ApiError, BoardInfo, Events, Health, Register, prefix};
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

    /// Fails unless this is a todai server speaking our API version.
    pub fn check(&self) -> Result<()> {
        let h = self.health()?;
        if h.api != API_VERSION {
            bail!("{} speaks API v{} (todai {}); this todai speaks v{API_VERSION}", self.base, h.api, h.version);
        }
        Ok(())
    }

    pub fn boards(&self) -> Result<Vec<BoardInfo>> {
        self.call(self.agent.get(self.url("/boards")).header("Authorization", self.auth()).call())
    }

    /// Events after `after`, waiting up to `wait` for some to arrive.
    pub fn events(&self, id: &str, after: u64, wait: Duration) -> Result<Events> {
        let url = self.url(&format!("/boards/{id}/events?after={after}&wait={}", wait.as_secs()));
        let req = self.agent.get(url).header("Authorization", self.auth());
        let req = req.config().timeout_global(Some(wait + Duration::from_secs(10))).build();
        self.call(req.call())
    }

    /// Posts one operator action; the server assigns `seq` (and elaboration ids).
    pub fn act(&self, id: &str, kind: &Kind) -> Result<Event> {
        let req = self.agent.post(self.url(&format!("/boards/{id}/actions"))).header("Authorization", self.auth());
        self.call(req.send_json(kind))
    }

    /// Every served board with its board-list numbers (todai 0.7+ servers).
    pub fn summaries(&self) -> Result<Vec<todai_core::api::BoardSummaryView>> {
        self.call(self.agent.get(self.url("/summaries")).header("Authorization", self.auth()).call())
    }

    /// Archives (or unarchives) board `id`; the board under its new id.
    pub fn relocate(&self, id: &str, archive: bool) -> Result<BoardInfo> {
        let verb = if archive { "archive" } else { "unarchive" };
        let req = self.agent.post(self.url(&format!("/boards/{id}/{verb}"))).header("Authorization", self.auth());
        self.call(req.send_json(serde_json::json!({})))
    }

    /// Creates an empty board `name` in the project of board `id`.
    pub fn sibling(&self, id: &str, name: &str) -> Result<BoardInfo> {
        let req = self.agent.post(self.url(&format!("/boards/{id}/siblings"))).header("Authorization", self.auth());
        self.call(req.send_json(todai_core::api::NewBoard { name: name.to_owned() }))
    }

    /// Only accepted from the server's own machine.
    pub fn register(&self, path: &Path) -> Result<BoardInfo> {
        let req = self.agent.post(self.url("/boards")).header("Authorization", self.auth());
        self.call(req.send_json(Register { path: path.to_owned() }))
    }

    /// `path` is under the versioned prefix, except `/api/health`.
    fn url(&self, path: &str) -> String {
        if path.starts_with("/api/") { format!("{}{path}", self.base) } else { format!("{}{}{path}", self.base, prefix()) }
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

/// Servers with a saved token, except this machine's own server (its
/// boards are local files, listed as such).
pub fn saved_servers() -> Vec<String> {
    let local = local().map(|c| c.base);
    saved_tokens().into_keys().filter(|s| Some(s) != local.as_ref()).collect()
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
    let (addr, token) = todai_core::dirs::local_server(&todai_core::dirs::server_dir()?)?;
    Some(Client::new(&format!("http://{addr}"), &token))
}

/// The boards on `client`'s server matching `wanted` (a name,
/// `PROJECT/NAME`, or id each); every board if `wanted` is empty.
pub fn pick_boards(client: &Client, wanted: &[String]) -> Result<Vec<BoardInfo>> {
    let server = client.base();
    let boards = client.boards()?;
    let mut picked = Vec::new();
    for w in wanted {
        let matches = |b: &&BoardInfo| b.id == *w || b.name == *w || format!("{}/{}", b.project, b.name) == *w;
        match boards.iter().filter(matches).collect::<Vec<_>>()[..] {
            [b] => picked.push(b.clone()),
            [] => bail!("{server} has no board {w} (see `todai-server boards` there)"),
            _ => bail!("{server} has several boards named {w}; use PROJECT/{w} or the id"),
        }
    }
    if wanted.is_empty() {
        picked = boards;
    }
    if picked.is_empty() {
        bail!("{server} serves no boards yet");
    }
    Ok(picked)
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
    if client.check().is_ok() {
        let _ = client.register(path);
    }
}
