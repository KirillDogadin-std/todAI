//! Open tabs, restored when the window starts again.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::state_dir;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<PathBuf>,
    pub active: usize,
}

impl Session {
    pub fn load() -> Session {
        state_dir()
            .and_then(|d| std::fs::read_to_string(d.join("session.json")).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = state_dir().ok_or_else(|| anyhow::anyhow!("no state dir"))?;
        std::fs::create_dir_all(&dir)?;
        // Write-then-rename so a crash never leaves half a session file.
        let tmp = dir.join("session.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, dir.join("session.json"))?;
        Ok(())
    }
}
