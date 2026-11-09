//! The headless todai server: serves registered boards over HTTP so operator
//! frontends (the desktop window in remote mode, later a web app) can read
//! them and post operator actions from another machine.
//!
//! Board files stay the source of truth. Agents keep writing them directly;
//! the server reads them, watches them for long-polls, and appends operator
//! events through `Log::append` like every other writer. The API is
//! documented in `docs/server.md`.

mod changes;
mod http;
mod registry;

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args as ClapArgs, Subcommand};
use todai_core::api::DEFAULT_LISTEN;

pub use registry::{Board, Registry};

/// `todai-server` and `todai serve` take these.
#[derive(ClapArgs, Debug, Clone)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Address to listen on. Anything but loopback exposes your boards to the
    /// network over plain HTTP: put it behind Tailscale or a TLS proxy.
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: String,
    /// Boards to serve, in addition to the ones registered earlier.
    files: Vec<PathBuf>,
}

#[derive(Subcommand, Debug, Clone)]
enum Cmd {
    /// Print the access token clients need (creating it if there is none).
    Token {
        /// Replace the token; clients using the old one stop working.
        #[arg(long)]
        rotate: bool,
    },
    /// List the registered boards.
    Boards,
    /// Stop serving a board (the file is left alone).
    Forget { file: PathBuf },
    /// Remove boards from the server's list whose .jsonl file no longer exists.
    Prune,
    /// Print a link and QR code that open the web app with the token, for
    /// pairing a phone. Anyone who sees them has full access, like the token.
    Pair {
        /// Host the phone reaches this server at (Tailscale name, LAN IP), or
        /// a full base URL such as https://box.tailnet.ts.net from
        /// `tailscale serve`. Defaults to the address the server listens on.
        #[arg(long)]
        host: Option<String>,
    },
}

pub fn run(args: Args) -> Result<()> {
    let dir = state_dir()?;
    match args.cmd {
        Some(Cmd::Token { rotate }) => {
            let token = if rotate { registry::rotate_token(&dir)? } else { registry::load_token(&dir)? };
            println!("{token}");
        }
        Some(Cmd::Boards) => {
            for b in Registry::load(&dir)?.list() {
                let note = if b.path.exists() { "" } else { "  (missing)" };
                println!("{}  {}{note}", b.id, b.path.display());
            }
        }
        Some(Cmd::Forget { file }) => {
            let path = std::path::absolute(file)?;
            if !Registry::load(&dir)?.remove(&todai_core::api::board_id(&path))? {
                bail!("{} is not registered", path.display());
            }
        }
        Some(Cmd::Prune) => {
            let pruned = Registry::load(&dir)?.prune()?;
            for b in &pruned {
                println!("{}  {}", b.id, b.path.display());
            }
            // On stderr, so piping `prune` gets only the removed boards.
            eprintln!("pruned {} board(s)", pruned.len());
        }
        Some(Cmd::Pair { host }) => pair(&dir, host.as_deref())?,
        None => {
            let files = args.files.iter().map(std::path::absolute).collect::<std::io::Result<Vec<_>>>()?;
            // Boards deleted while the server was down would otherwise stay
            // registered (and read as empty) until someone runs `prune`.
            for b in Registry::load(&dir)?.prune()? {
                eprintln!("todai-server: forgot {} (file is gone)", b.path.display());
            }
            let server = Server::bind(&Config { listen: args.listen, dir, boards: files })?;
            let addr = server.addr();
            eprintln!("todai-server: listening on http://{addr}");
            if !addr.ip().is_loopback() {
                eprintln!("todai-server: reachable from the network over plain HTTP; use Tailscale or a TLS proxy");
            }
            eprintln!("todai-server: token: `todai-server token`; boards: `todai-server boards`");
            server.run();
        }
    }
    Ok(())
}

/// `$XDG_STATE_HOME/todai/server`: token, registered boards, and the address
/// of the running server (for `todai attach` to register with).
pub fn state_dir() -> Result<PathBuf> {
    todai_core::dirs::server_dir().context("no state dir: set XDG_STATE_HOME or HOME")
}

pub struct Config {
    pub listen: String,
    /// Where the token, registry and address file live.
    pub dir: PathBuf,
    /// Boards to register at startup (absolute paths).
    pub boards: Vec<PathBuf>,
}

pub struct Server {
    http: Arc<tiny_http::Server>,
    shared: Arc<http::Shared>,
}

impl Server {
    /// Binds the socket, loads (or creates) the token and registry, and
    /// records the address so local `todai` commands can find this server.
    pub fn bind(cfg: &Config) -> Result<Server> {
        let token = registry::load_token(&cfg.dir)?;
        let registry = Registry::load(&cfg.dir)?;
        for b in &cfg.boards {
            registry.add(b)?;
        }
        let http = tiny_http::Server::http(&cfg.listen).map_err(|e| anyhow::anyhow!("listen on {}: {e}", cfg.listen))?;
        let server = Server { http: Arc::new(http), shared: Arc::new(http::Shared::new(token, registry)) };
        registry::write_addr(&cfg.dir, server.addr())?;
        Ok(server)
    }

    pub fn addr(&self) -> SocketAddr {
        self.http.server_addr().to_ip().expect("bound to a TCP address")
    }

    pub fn token(&self) -> &str {
        &self.shared.token
    }

    /// Serves until the process exits. Each request gets its own thread, so
    /// long-polls never hold up other clients.
    pub fn run(self) {
        for req in self.http.incoming_requests() {
            http::dispatch(&self.shared, req);
        }
    }

    /// Serves on a background thread until the handle is dropped; for tests
    /// and for embedding.
    pub fn spawn(self) -> Handle {
        let http = self.http.clone();
        let addr = self.addr();
        let token = self.shared.token.clone();
        std::thread::spawn(move || self.run());
        Handle { http, addr, token }
    }
}

pub struct Handle {
    http: Arc<tiny_http::Server>,
    pub addr: SocketAddr,
    pub token: String,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.http.unblock();
    }
}

/// Where a running local server listens and the token it accepts, if one
/// has run with this state dir. The CLI uses it to register boards.
/// The web app's address with the token in the fragment, which browsers
/// never send to the server or put in logs; the app moves it to storage.
pub fn pair_url(base: &str, token: &str) -> String {
    format!("{}/#token={token}", base.trim_end_matches('/'))
}

fn pair(dir: &Path, host: Option<&str>) -> Result<()> {
    let token = registry::load_token(dir)?;
    let addr = local(dir).map(|(a, _)| a);
    let port = addr.map_or_else(|| DEFAULT_LISTEN.rsplit(':').next().unwrap_or("7341").to_owned(), |a| a.port().to_string());
    let base = match host {
        Some(h) if h.contains("://") => h.to_owned(),
        Some(h) => format!("http://{h}:{port}"),
        None => {
            let ip = addr.map_or_else(|| "127.0.0.1".to_owned(), |a| a.ip().to_string());
            if addr.is_none_or(|a| a.ip().is_loopback()) {
                eprintln!("todai-server: a phone can't reach {ip}; pass --host with this machine's Tailscale name or LAN IP");
            }
            format!("http://{ip}:{port}")
        }
    };
    if addr.is_none() {
        eprintln!("todai-server: no server is running here; start `todai-server` before scanning");
    }
    let url = pair_url(&base, &token);
    let qr = qrcode::QrCode::new(url.as_bytes()).context("make QR code")?;
    let art = qr.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build();
    // Scanners want dark modules on a light background. On a dark terminal
    // the blocks would draw light, so paint each line black on white.
    let art = if std::io::stdout().is_terminal() {
        art.lines().map(|l| format!("\x1b[30;107m{l}\x1b[0m\n")).collect()
    } else {
        art + "\n"
    };
    println!("{art}{url}");
    Ok(())
}

pub fn local(dir: &Path) -> Option<(SocketAddr, String)> {
    todai_core::dirs::local_server(dir)
}

#[cfg(test)]
mod tests;
