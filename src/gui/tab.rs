use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use notify::{RecursiveMode, Watcher};

use crate::store::{Actor, Kind, Log, State};

/// One open log file: its folded state, unsent drafts, and a watcher that
/// flags the tab dirty whenever the file changes on disk.
pub struct Tab {
    pub log: Log,
    pub state: State,
    /// Resolution drafts keyed by decision id, and note drafts keyed by todo id.
    pub drafts: HashMap<String, String>,
    pub error: Option<String>,
    dirty: Arc<AtomicBool>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Tab {
    pub fn open(path: PathBuf, ctx: &egui::Context) -> Tab {
        let dirty = Arc::new(AtomicBool::new(true));
        let mut tab = Tab {
            _watcher: None,
            log: Log::new(path),
            state: State::default(),
            drafts: HashMap::new(),
            error: None,
            dirty: dirty.clone(),
        };
        match watch(tab.log.path(), dirty, ctx.clone()) {
            Ok(w) => tab._watcher = Some(w),
            Err(e) => tab.error = Some(format!("not watching for changes: {e}")),
        }
        tab
    }

    pub fn title(&self) -> String {
        let name = self.log.path().file_name().unwrap_or_default().to_string_lossy();
        name.strip_suffix(".jsonl").unwrap_or(&name).to_owned()
    }

    pub fn reload_if_dirty(&mut self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        match self.log.load_state() {
            Ok((_, state)) => {
                self.state = state;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// Appends an operator event; on success the watcher triggers a reload.
    pub fn send(&mut self, kind: Kind) -> bool {
        match self.log.append(Actor::Operator, |_| Ok(kind)) {
            Ok(_) => {
                // Reload now rather than waiting a frame for the watcher.
                self.dirty.store(true, Ordering::Relaxed);
                true
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                false
            }
        }
    }
}

fn watch(path: &Path, dirty: Arc<AtomicBool>, ctx: egui::Context) -> Result<notify::RecommendedWatcher> {
    // Watch the directory, not the file: the file may not exist yet, and some
    // writers replace it instead of appending.
    let dir = path.parent().unwrap_or(Path::new("/")).to_owned();
    std::fs::create_dir_all(&dir)?;
    let target = path.to_owned();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && ev.paths.iter().any(|p| p == &target)
        {
            dirty.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        }
    })?;
    w.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(w)
}
