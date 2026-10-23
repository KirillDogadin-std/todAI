//! Append-only JSONL event log. Every writer takes an exclusive `flock` on the
//! file, folds the current contents to validate, assigns the next `seq`, and
//! appends a single line. Readers take a shared lock.

mod event;
mod state;

pub use event::{Actor, Event, Kind};
pub use state::{Decision, DecisionStatus, State, Todo, unprocessed};

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Log {
    path: PathBuf,
}

impl Log {
    pub fn new(path: impl Into<PathBuf>) -> Log {
        Log { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads all events. A missing file is an empty log.
    pub fn load(&self) -> Result<Vec<Event>> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("open {}", self.path.display())),
        };
        file.lock_shared()?;
        self.read_events(&mut file)
    }

    pub fn load_state(&self) -> Result<(Vec<Event>, State)> {
        let events = self.load()?;
        let state = State::fold(&events)?;
        Ok((events, state))
    }

    /// Appends one event built from the current state. `build` runs under the
    /// lock, so ids it derives from `state` are unique.
    pub fn append(&self, by: Actor, build: impl FnOnce(&State) -> Result<Kind>) -> Result<Event> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&self.path)
            .with_context(|| format!("open {}", self.path.display()))?;
        file.lock()?;

        let events = self.read_events(&mut file)?;
        let state = State::fold(&events)?;
        let kind = build(&state)?;
        state.validate(by, &kind)?;

        let event = Event { seq: state.last_seq + 1, ts: now_ms(), by, kind };
        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');
        file.write_all(&line)?;
        Ok(event)
    }

    fn read_events(&self, file: &mut File) -> Result<Vec<Event>> {
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        // Every complete line ends in '\n'; text after the last one would be a
        // torn write from a crashed writer, so it is ignored.
        let complete = buf.rfind('\n').map_or("", |i| &buf[..i]);
        complete
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| {
                serde_json::from_str(l)
                    .with_context(|| format!("{}:{}: bad event", self.path.display(), i + 1))
            })
            .collect()
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests;
