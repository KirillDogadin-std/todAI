use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use notify::{RecursiveMode, Watcher};

use super::nav::Nav;
use crate::store::{Actor, Event, Kind, Log, State};

/// One open log file: its folded state, unsent drafts, and a watcher that
/// flags the tab dirty whenever the file changes on disk.
pub struct Tab {
    pub log: Log,
    pub state: State,
    /// Resolution drafts keyed by decision id, and note drafts keyed by todo id.
    pub drafts: HashMap<String, String>,
    pub error: Option<String>,
    pub nav: Nav,
    /// Items waiting for the operator to confirm their removal.
    pub confirm_trash: Option<Vec<String>>,
    /// Agent posts seen since the last check, for desktop notifications.
    pub new_posts: Vec<String>,
    /// Highest seq loaded so far; `None` until the first load, so opening a
    /// board never notifies about its existing items.
    seen_seq: Option<u64>,
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
            nav: Nav::default(),
            confirm_trash: None,
            new_posts: Vec::new(),
            seen_seq: None,
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

    /// Reloads regardless of the watcher, for tests.
    #[cfg(test)]
    pub fn reload_if_dirty_now(&mut self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.reload_if_dirty();
    }

    pub fn reload_if_dirty(&mut self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        match self.log.load_state() {
            Ok((events, state)) => {
                if let Some(seen) = self.seen_seq {
                    self.new_posts.extend(agent_posts(&events, seen));
                }
                self.seen_seq = Some(state.last_seq);
                self.state = state;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// Appends an operator event; on success the watcher triggers a reload.
    pub fn send(&mut self, mut kind: Kind) -> bool {
        let result = self.log.append(Actor::Operator, |s| {
            if let Kind::ElabRequest { rid, .. } = &mut kind {
                *rid = s.next_elab_id();
            }
            Ok(kind)
        });
        match result {
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

/// One line per agent post after `seen`: what the operator should look at.
pub fn agent_posts(events: &[Event], seen: u64) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.seq > seen && e.by == Actor::Agent)
        .filter_map(|e| match &e.kind {
            Kind::DecisionAdd { id, title, .. } => Some(format!("Decision {id}: {title}")),
            Kind::TodoAdd { id, title, .. } => Some(format!("Todo {id}: {title}")),
            Kind::ElabAdd { rid, .. } => Some(format!("Elaboration {rid} answered")),
            _ => None,
        })
        .collect()
}

fn watch(path: &Path, dirty: Arc<AtomicBool>, ctx: egui::Context) -> Result<notify::RecommendedWatcher> {
    // Watch the directory, not the file: the file may not exist yet, and some
    // writers replace it instead of appending. Access events (open, read,
    // close without writing) are skipped: our own reload opens the file, and
    // reacting to that would reload and repaint forever.
    let dir = path.parent().unwrap_or(Path::new("/")).to_owned();
    std::fs::create_dir_all(&dir)?;
    let target = path.to_owned();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && !ev.kind.is_access()
            && ev.paths.iter().any(|p| p == &target)
        {
            dirty.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        }
    })?;
    w.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn own_reload_does_not_mark_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.jsonl");
        Log::new(path.clone())
            .append(Actor::Agent, |_| Ok(Kind::TodoAdd { id: "t1".into(), title: "x".into(), context: None }))
            .unwrap();
        let mut tab = Tab::open(path.clone(), &egui::Context::default());
        tab.reload_if_dirty();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!tab.dirty.load(Ordering::Relaxed), "reading the board must not look like a change");

        Log::new(path)
            .append(Actor::Agent, |_| Ok(Kind::TodoAdd { id: "t2".into(), title: "y".into(), context: None }))
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(tab.dirty.load(Ordering::Relaxed), "an append must still be seen");
    }
}
