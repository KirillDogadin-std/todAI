//! Open tabs, restored when a frontend starts again. The window and the TUI
//! keep separate files, so running both never mixes their tabs.

use serde::{Deserialize, Serialize};

use crate::config::state_dir;
use crate::follow::Followed;
use crate::tab::Location;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<Location>,
    pub active: usize,
    /// Servers whose new boards open as tabs (see `follow`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub follow: Vec<Followed>,
}

impl Session {
    /// `file` names the frontend's session in the state dir.
    pub fn load(file: &str) -> Session {
        state_dir()
            .and_then(|d| std::fs::read_to_string(d.join(file)).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, file: &str) -> anyhow::Result<()> {
        let dir = state_dir().ok_or_else(|| anyhow::anyhow!("no state dir"))?;
        std::fs::create_dir_all(&dir)?;
        // Write-then-rename so a crash never leaves half a session file.
        let tmp = dir.join(format!("{file}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, dir.join(file))?;
        Ok(())
    }
}
