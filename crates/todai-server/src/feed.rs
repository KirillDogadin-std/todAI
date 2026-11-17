//! `GET /api/v2/changes`: one long-poll for every served board, so a
//! client following a whole server (the board list, a phone's inbox, a
//! followed server's new boards) holds one connection instead of one per
//! board. Wakes on any board file or the registry changing.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use todai_core::api::{BoardChange, BoardInfo, ChangeFeed, encode_cursor};
use todai_core::store::{Event, Log};

use crate::changes::Changes;
use crate::registry::Registry;

/// What a board looked like at the last read, so an unchanged file (same
/// size; appends are the only writes) isn't parsed again.
struct Seen {
    len: Option<u64>,
    last_seq: u64,
}

/// Blocks until something differs from `cursor` (or at once without one),
/// or until `deadline` (then an empty answer with the same cursor).
pub fn wait(changes: &Changes, registry: &Registry, cursor: Option<BTreeMap<String, u64>>, deadline: Instant) -> Result<ChangeFeed> {
    let mut cache: HashMap<PathBuf, Seen> = HashMap::new();
    loop {
        let boards: Vec<_> = registry.list().into_iter().filter(|b| b.path.exists()).collect();
        let mut paths: Vec<PathBuf> = boards.iter().map(|b| b.path.clone()).collect();
        paths.push(registry.file().to_owned());
        // Subscribe before reading, so a change while we read still wakes us.
        let sub = changes.subscribe_all(&paths);
        let seen = sub.generation();

        let mut now: BTreeMap<String, u64> = BTreeMap::new();
        let mut changed = Vec::new();
        for b in &boards {
            let len = std::fs::metadata(&b.path).map(|m| m.len()).ok();
            let fresh = cache.get(&b.path).is_none_or(|s| s.len != len);
            let mut events: Option<Vec<Event>> = None;
            if fresh {
                let ev = Log::new(&b.path).load()?;
                cache.insert(b.path.clone(), Seen { len, last_seq: ev.last().map_or(0, |e| e.seq) });
                events = Some(ev);
            }
            let last_seq = cache[&b.path].last_seq;
            now.insert(b.id.clone(), last_seq);
            let Some(cursor) = &cursor else { continue };
            let info = BoardInfo::new(&b.path);
            match cursor.get(&b.id) {
                None => changed.push(BoardChange { board: info, last_seq, events: Vec::new(), new: true, reset: false }),
                Some(&after) if after == last_seq => {}
                Some(&after) => {
                    let all = match events {
                        Some(ev) => ev,
                        None => Log::new(&b.path).load()?,
                    };
                    let reset = after > last_seq;
                    let events = if reset { all } else { all.into_iter().filter(|e| e.seq > after).collect() };
                    changed.push(BoardChange { board: info, last_seq, events, new: false, reset });
                }
            }
        }
        let Some(cursor) = &cursor else {
            // A baseline: every board, nothing to replay.
            let boards = boards
                .iter()
                .map(|b| BoardChange { board: b.info(), last_seq: now[&b.id], events: Vec::new(), new: true, reset: false })
                .collect();
            return Ok(ChangeFeed { cursor: encode_cursor(&now), boards, removed: Vec::new() });
        };
        let removed: Vec<String> = cursor.keys().filter(|id| !now.contains_key(*id)).cloned().collect();
        if !changed.is_empty() || !removed.is_empty() || Instant::now() >= deadline {
            return Ok(ChangeFeed { cursor: encode_cursor(&now), boards: changed, removed });
        }
        sub.wait(seen, deadline);
    }
}
