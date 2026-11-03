//! XDG directory lookups shared by the window, the CLI and the server.

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/todai`, for user preferences.
pub fn config_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_STATE_HOME/todai`, for session, log and server files.
pub fn state_dir() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

/// `$var/todai`, or `$HOME/fallback/todai`.
fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    let base = std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))?;
    Some(base.join("todai"))
}

/// `state_dir()/server`: the local server's token, registry and address.
pub fn server_dir() -> Option<PathBuf> {
    Some(state_dir()?.join("server"))
}

/// The address and token a local server left in `dir` (see `server_dir`),
/// if one has run there. The address may be stale; check before trusting it.
pub fn local_server(dir: &std::path::Path) -> Option<(std::net::SocketAddr, String)> {
    let addr = std::fs::read_to_string(dir.join("addr")).ok()?.trim().parse().ok()?;
    let token = std::fs::read_to_string(dir.join("token")).ok()?.trim().to_owned();
    Some((addr, token))
}
