//! User preferences in `$XDG_CONFIG_HOME/todai/config.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    pub theme: Theme,
}

impl Config {
    /// Missing or unreadable config falls back to defaults.
    pub fn load() -> Config {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("todai").join("config.json"))
}
