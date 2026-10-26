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
