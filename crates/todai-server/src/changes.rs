//! Wakes long-polls when their board file changes: one shared file watcher
//! for the whole server instead of every waiting request re-statting its
//! board.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// How often a waiter re-checks its file when the watcher can't cover it.
const FALLBACK: Duration = Duration::from_millis(200);
/// A waiter re-checks its file at least this often even when watched, in
/// case an event was missed (e.g. the directory was replaced).
const SAFETY: Duration = Duration::from_secs(5);

pub struct Changes {
    files: Arc<Files>,
    /// `None` if the platform watcher couldn't start: every waiter falls back
    /// to re-checking every [`FALLBACK`].
    watcher: Option<Mutex<Watched>>,
}

/// Change counters of the files someone waits on. The watcher's callback
/// takes this lock, so never call into the watcher while holding it.
#[derive(Default)]
struct Files {
    gens: Mutex<HashMap<PathBuf, (usize, u64)>>, // waiters, generation
    changed: Condvar,
}

struct Watched {
    watcher: RecommendedWatcher,
    dirs: HashMap<PathBuf, usize>, // waiters per watched directory
}

impl Changes {
    pub fn new() -> Changes {
        let files = Arc::new(Files::default());
        let cb = files.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // Access events (open, read, close without writing) are our own
            // reads; only writes, creates, removes and renames matter.
            let Ok(ev) = res else { return };
            if ev.kind.is_access() {
                return;
            }
            let mut gens = cb.gens.lock().unwrap();
            let mut any = false;
            for p in &ev.paths {
                if let Some((_, g)) = gens.get_mut(p) {
                    *g += 1;
                    any = true;
                }
            }
            drop(gens);
            if any {
                cb.changed.notify_all();
            }
        });
        Changes { files, watcher: watcher.ok().map(|watcher| Mutex::new(Watched { watcher, dirs: HashMap::new() })) }
    }

    /// Starts watching `path` until the returned guard drops.
    pub fn subscribe(&self, path: &Path) -> Subscription<'_> {
        self.subscribe_all(std::slice::from_ref(&path.to_owned()))
    }

    /// Watches several files at once: the guard's generation moves when any
    /// of them changes (the change feed waits on every board this way).
    pub fn subscribe_all(&self, paths: &[PathBuf]) -> Subscription<'_> {
        let mut paths = paths.to_vec();
        paths.sort();
        paths.dedup();
        {
            let mut gens = self.files.gens.lock().unwrap();
            for p in &paths {
                gens.entry(p.clone()).or_default().0 += 1;
            }
        }
        // Watch directories, not files: a file may not exist yet, and a
        // writer may replace it.
        let mut dirs: Vec<PathBuf> = paths.iter().map(|p| p.parent().unwrap_or(Path::new("/")).to_owned()).collect();
        dirs.sort();
        dirs.dedup();
        let mut watched = Vec::new();
        let mut all = true;
        if let Some(w) = &self.watcher {
            let mut w = w.lock().unwrap();
            for dir in dirs {
                let ok = match w.dirs.get_mut(&dir) {
                    Some(n) => {
                        *n += 1;
                        true
                    }
                    None => {
                        let ok = w.watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok();
                        if ok {
                            w.dirs.insert(dir.clone(), 1);
                        }
                        ok
                    }
                };
                if ok {
                    watched.push(dir);
                } else {
                    all = false;
                }
            }
        } else {
            all = paths.is_empty();
        }
        Subscription { changes: self, paths, dirs: watched, all_watched: all }
    }
}

pub struct Subscription<'a> {
    changes: &'a Changes,
    paths: Vec<PathBuf>,
    /// The directories this guard watches (and must unwatch).
    dirs: Vec<PathBuf>,
    /// Every file's directory is watched; else waits re-check every [`FALLBACK`].
    all_watched: bool,
}

impl Subscription<'_> {
    /// The files' change counter. Read it before looking at the files, then
    /// pass it to [`wait`](Self::wait), so a change in between isn't lost.
    pub fn generation(&self) -> u64 {
        let gens = self.changes.files.gens.lock().unwrap();
        self.sum(&gens)
    }

    fn sum(&self, gens: &HashMap<PathBuf, (usize, u64)>) -> u64 {
        self.paths.iter().map(|p| gens.get(p).map_or(0, |e| e.1)).fold(0, u64::wrapping_add)
    }

    /// Blocks until a file changed since `seen`, or until `deadline`.
    /// Returns early now and then anyway (see [`SAFETY`]); callers re-check
    /// the files either way.
    pub fn wait(&self, seen: u64, deadline: Instant) {
        let step = if self.all_watched { SAFETY } else { FALLBACK };
        let until = deadline.min(Instant::now() + step);
        let files = &self.changes.files;
        let mut gens = files.gens.lock().unwrap();
        loop {
            let now = Instant::now();
            if now >= until || self.sum(&gens) != seen {
                return;
            }
            gens = files.changed.wait_timeout(gens, until - now).unwrap().0;
        }
    }
}

impl Drop for Subscription<'_> {
    fn drop(&mut self) {
        let mut gens = self.changes.files.gens.lock().unwrap();
        for p in &self.paths {
            if let Some(e) = gens.get_mut(p) {
                e.0 -= 1;
                if e.0 == 0 {
                    gens.remove(p);
                }
            }
        }
        drop(gens);
        if let Some(w) = &self.changes.watcher {
            let mut w = w.lock().unwrap();
            for dir in &self.dirs {
                if let Some(n) = w.dirs.get_mut(dir) {
                    *n -= 1;
                    if *n == 0 {
                        w.dirs.remove(dir);
                        let _ = w.watcher.unwatch(dir);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_write_wakes_the_waiter_well_before_the_safety_recheck() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.jsonl");
        std::fs::write(&path, "").unwrap();
        let changes = Changes::new();
        let sub = changes.subscribe(&path);
        assert!(sub.all_watched, "the directory is watched");

        let seen = sub.generation();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(b"x\n").unwrap();
            })
        };
        let start = Instant::now();
        sub.wait(seen, start + Duration::from_secs(30));
        assert!(start.elapsed() < Duration::from_secs(2), "woken by the write: {:?}", start.elapsed());
        assert_ne!(sub.generation(), seen);
        writer.join().unwrap();
    }

    #[test]
    fn other_files_and_own_reads_do_not_wake() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.jsonl");
        std::fs::write(&path, "").unwrap();
        let changes = Changes::new();
        let sub = changes.subscribe(&path);
        let seen = sub.generation();
        std::fs::write(dir.path().join("other.jsonl"), "x").unwrap();
        let _ = std::fs::read(&path).unwrap();
        let start = Instant::now();
        sub.wait(seen, start + Duration::from_millis(500));
        assert!(start.elapsed() >= Duration::from_millis(500));
        assert_eq!(sub.generation(), seen);
    }

    #[test]
    fn last_subscriber_unwatches_and_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.jsonl");
        let changes = Changes::new();
        let (a, b) = (changes.subscribe(&path), changes.subscribe(&path));
        drop(a);
        assert_eq!(changes.watcher.as_ref().unwrap().lock().unwrap().dirs.len(), 1);
        drop(b);
        assert!(changes.watcher.as_ref().unwrap().lock().unwrap().dirs.is_empty());
        assert!(changes.files.gens.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unwatchable_directory_falls_back_to_polling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone").join("b.jsonl");
        let changes = Changes::new();
        let sub = changes.subscribe(&path);
        assert!(!sub.all_watched);
        let start = Instant::now();
        sub.wait(sub.generation(), start + Duration::from_secs(30));
        assert!(start.elapsed() < Duration::from_secs(1), "re-checks every {FALLBACK:?}");
    }

    #[test]
    fn a_subscription_to_several_files_wakes_on_any() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (pa, pb) = (a.path().join("a.jsonl"), b.path().join("b.jsonl"));
        let changes = Changes::new();
        let sub = changes.subscribe_all(&[pa, pb.clone()]);
        assert!(sub.all_watched);
        let seen = sub.generation();
        std::fs::write(&pb, "x").unwrap();
        let start = Instant::now();
        sub.wait(seen, start + Duration::from_secs(30));
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(sub);
        assert!(changes.watcher.as_ref().unwrap().lock().unwrap().dirs.is_empty());
    }
}
