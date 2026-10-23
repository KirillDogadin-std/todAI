//! Claude Code hooks (`todai hook claude <event>`). Inert for any session that
//! has not run `todai attach`:
//! - post-tool-use: learns the session's board from `todai attach` output, then
//!   injects unseen operator updates as context after every tool call.
//! - stop: refuses to stop while updates are unread, and once asks the agent
//!   to leave a background `todai wait` running if items are still open.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::attached_board;
use crate::store::{Actor, DecisionStatus, Kind, Log, unprocessed};
use crate::{config, inbox};

pub fn run(event: &str) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let input: Value = serde_json::from_str(&input)?;
    let Some(session) = input["session_id"].as_str() else { return Ok(()) };
    let out = match event {
        "post-tool-use" => post_tool_use(session, &input)?,
        "stop" => stop(session, &input)?,
        _ => None,
    };
    if let Some(out) = out {
        println!("{out}");
    }
    Ok(())
}

fn post_tool_use(session: &str, input: &Value) -> Result<Option<Value>> {
    if input["tool_name"] == "Bash"
        && let Some(board) = strings(&input["tool_response"]).iter().find_map(|s| attached_board(s))
    {
        Bindings::update(|b| {
            b.0.insert(session.to_owned(), Binding { board, at: now() });
        })?;
    }
    let Some(board) = Bindings::load().board(session) else { return Ok(None) };
    let Some(summary) = take_updates(&board)? else { return Ok(None) };
    Ok(Some(json!({
        "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": summary }
    })))
}

fn stop(session: &str, input: &Value) -> Result<Option<Value>> {
    let Some(board) = Bindings::load().board(session) else { return Ok(None) };
    if let Some(summary) = take_updates(&board)? {
        let reason = format!("{summary}\nAct on these operator updates before finishing.");
        return Ok(Some(json!({ "decision": "block", "reason": reason })));
    }
    // Asked once per stop attempt (Claude sets stop_hook_active on the retry).
    if input["stop_hook_active"].as_bool() == Some(true) || waiter_alive(&board) {
        return Ok(None);
    }
    let (_, state) = Log::new(&board).load_state()?;
    let open = state.decisions.iter().filter(|d| d.status == DecisionStatus::Pending).count()
        + state.todos.iter().filter(|t| t.done.is_none()).count();
    if open == 0 {
        return Ok(None);
    }
    let b = board.display();
    let reason = format!(
        "todai: {open} item(s) on {b} still wait for the operator and nothing is watching for their answers. \
         Run `todai wait {b}` as a background command so you are woken when they answer, then finish your turn."
    );
    Ok(Some(json!({ "decision": "block", "reason": reason })))
}

/// Summarizes and acks unseen operator updates.
fn take_updates(board: &Path) -> Result<Option<String>> {
    let log = Log::new(board);
    let (events, state) = log.load_state()?;
    let pending = unprocessed(&events, &state);
    let Some(upto) = pending.last().map(|e| e.seq) else { return Ok(None) };
    let summary = inbox::summarize(board, &pending, &state);
    log.append(Actor::Agent, |_| Ok(Kind::Ack { upto_seq: upto }))?;
    Ok(Some(summary))
}

/// Every string inside a JSON value; tool responses vary in shape.
fn strings(v: &Value) -> Vec<&str> {
    match v {
        Value::String(s) => vec![s],
        Value::Array(a) => a.iter().flat_map(strings).collect(),
        Value::Object(o) => o.values().flat_map(strings).collect(),
        _ => vec![],
    }
}

// ---- `todai wait` registry, so the stop hook knows someone is listening ----

fn waiter_file(board: &Path) -> Option<PathBuf> {
    let name: String = board.to_string_lossy().chars().map(|c| if c == '/' { '%' } else { c }).collect();
    Some(config::state_dir()?.join("waiters").join(name))
}

/// Registers the current process as waiting on `board` until the guard drops.
pub fn register_waiter(board: &Path) -> Option<WaiterGuard> {
    let f = waiter_file(board)?;
    std::fs::create_dir_all(f.parent()?).ok()?;
    std::fs::write(&f, std::process::id().to_string()).ok()?;
    Some(WaiterGuard(f))
}

pub struct WaiterGuard(PathBuf);

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn waiter_alive(board: &Path) -> bool {
    let Some(pid) = waiter_file(board)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| s.trim().parse::<i32>().ok())
    else {
        return false;
    };
    // Signal 0 checks existence without touching the process.
    unsafe { libc::kill(pid, 0) == 0 }
}

// ---- session -> board bindings ----

#[derive(Serialize, Deserialize, Default)]
struct Bindings(HashMap<String, Binding>);

#[derive(Serialize, Deserialize)]
struct Binding {
    board: PathBuf,
    at: u64,
}

const BINDING_TTL_SECS: u64 = 30 * 24 * 3600;

impl Bindings {
    fn path() -> Option<PathBuf> {
        Some(config::state_dir()?.join("claude-sessions.json"))
    }

    fn load() -> Bindings {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn update(f: impl FnOnce(&mut Bindings)) -> Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        let mut b = Self::load();
        f(&mut b);
        let cutoff = now().saturating_sub(BINDING_TTL_SECS);
        b.0.retain(|_, v| v.at >= cutoff);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&b)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    fn board(&self, session: &str) -> Option<PathBuf> {
        self.0.get(session).map(|b| b.board.clone())
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_collects_nested_output() {
        let v = json!({ "stdout": "a\ntodai: attached /x.jsonl", "meta": { "k": ["b"] }, "n": 3 });
        let s = strings(&v);
        assert!(s.iter().any(|s| attached_board(s) == Some(PathBuf::from("/x.jsonl"))));
        assert_eq!(s.len(), 2);
    }
}
