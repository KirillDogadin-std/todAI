//! Wire types of the todai server's HTTP API, shared by the server and its
//! clients. The endpoints are documented in `docs/server.md`.
//!
//! These types are the contract, not the internal `store` types: a refactor
//! of `State` must not change what clients see. The views below are built
//! from `State` in one place ([`BoardView::new`]); changing a field here is a
//! breaking change and needs a new [`API_VERSION`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{DecisionStatus, Event, State};

/// Version of the routes under `/api/v{N}` and of the types in this module.
pub const API_VERSION: u32 = 1;

/// Prefix of every versioned route, e.g. `/api/v1/boards`.
pub fn prefix() -> String {
    format!("/api/v{API_VERSION}")
}

/// Where `todai-server` listens unless told otherwise.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7341";

/// Longest long-poll a client may ask for, in seconds.
pub const MAX_WAIT_SECS: u64 = 60;

/// `GET /api/health`: unversioned, so a client can check compatibility
/// before using anything else.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Health {
    /// The server's release, for display.
    pub version: String,
    /// The API it speaks; a client compares it with its own [`API_VERSION`].
    pub api: u32,
}

/// One registered board, as listed by `GET /api/boards`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoardInfo {
    /// Opaque id used in URLs; see [`board_id`].
    pub id: String,
    /// The file name without `.jsonl`.
    pub name: String,
    /// Where the board lives on the server's machine, for display only.
    pub path: PathBuf,
}

impl BoardInfo {
    pub fn new(path: PathBuf) -> BoardInfo {
        BoardInfo { id: board_id(&path), name: board_name(&path), path }
    }
}

/// `POST /api/boards`: start serving the board at `path` (absolute).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Register {
    pub path: PathBuf,
}

/// `GET /api/boards/{id}/events?after=SEQ&wait=SECS`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Events {
    /// Events with `seq > after`, in order.
    pub events: Vec<Event>,
    pub last_seq: u64,
    /// `after` was past the end of the log (the file was replaced): `events`
    /// is the whole log and the client must start over from it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
}

/// `GET /api/v1/boards/{id}`: the board as the operator sees it. Trashed
/// items are already gone; elaborations are in request order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoardView {
    pub board: BoardInfo,
    /// Seq of the last event folded in; pass it as `after` to follow changes.
    pub last_seq: u64,
    pub decisions: Vec<DecisionView>,
    pub todos: Vec<TodoView>,
    pub elaborations: Vec<ElabView>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DecisionView {
    pub id: String,
    pub title: String,
    pub context: Option<String>,
    /// Quick-pick answers; the operator may still answer anything.
    pub options: Vec<String>,
    pub status: DecisionState,
    /// The operator's answer, while `status` is `resolved`.
    pub resolution: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DecisionState {
    Pending,
    Resolved,
    Withdrawn,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TodoView {
    pub id: String,
    pub title: String,
    pub context: Option<String>,
    pub done: bool,
    /// The operator's note when marking it done.
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ElabView {
    pub rid: String,
    /// The decision or todo it is about.
    pub target: String,
    /// The text the operator selected.
    pub selection: String,
    pub note: Option<String>,
    /// The elaboration this one follows up on; `None` starts a thread.
    pub parent: Option<String>,
    /// The agent's answer as markdown, once there is one.
    pub answer: Option<String>,
}

impl BoardView {
    /// The only place internal state turns into API types.
    pub fn new(board: BoardInfo, s: &State) -> BoardView {
        let decisions = s
            .decisions
            .iter()
            .map(|d| {
                let (status, resolution) = match &d.status {
                    DecisionStatus::Pending => (DecisionState::Pending, None),
                    DecisionStatus::Resolved { text, .. } => (DecisionState::Resolved, Some(text.clone())),
                    DecisionStatus::Withdrawn => (DecisionState::Withdrawn, None),
                };
                DecisionView {
                    id: d.id.clone(),
                    title: d.title.clone(),
                    context: d.context.clone(),
                    options: d.options.clone(),
                    status,
                    resolution,
                }
            })
            .collect();
        let todos = s
            .todos
            .iter()
            .map(|t| TodoView {
                id: t.id.clone(),
                title: t.title.clone(),
                context: t.context.clone(),
                done: t.done.is_some(),
                note: t.done.clone().flatten(),
            })
            .collect();
        let elaborations = s
            .elabs
            .iter()
            .map(|e| ElabView {
                rid: e.rid.clone(),
                target: e.target.clone(),
                selection: e.selection.clone(),
                note: e.note.clone(),
                parent: e.parent.clone(),
                answer: e.answer.clone(),
            })
            .collect();
        BoardView { board, last_seq: s.last_seq, decisions, todos, elaborations }
    }
}

/// The body of every non-2xx reply.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApiError {
    pub error: String,
}

/// A stable id for a board path: 16 hex digits of its FNV-1a hash. URLs carry
/// this instead of the path, so clients can never name arbitrary files.
pub fn board_id(path: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn board_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name.strip_suffix(".jsonl").unwrap_or(&name).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Actor, Kind};

    #[test]
    fn board_view_covers_every_status() {
        let ev = |seq, by, kind| Event { seq, ts: 0, by, kind };
        let s = |s: &str| s.to_owned();
        let events = [
            ev(1, Actor::Agent, Kind::DecisionAdd { id: s("d1"), title: s("A?"), context: None, options: vec![s("x")] }),
            ev(2, Actor::Agent, Kind::DecisionAdd { id: s("d2"), title: s("B?"), context: Some(s("c")), options: vec![] }),
            ev(3, Actor::Agent, Kind::DecisionAdd { id: s("d3"), title: s("C?"), context: None, options: vec![] }),
            ev(4, Actor::Agent, Kind::TodoAdd { id: s("t1"), title: s("Do"), context: None }),
            ev(5, Actor::Operator, Kind::DecisionResolve { id: s("d1"), text: s("x") }),
            ev(6, Actor::Agent, Kind::DecisionWithdraw { id: s("d2") }),
            ev(7, Actor::Operator, Kind::TodoDone { id: s("t1"), note: Some(s("ok")) }),
            ev(8, Actor::Operator, Kind::ElabRequest { rid: s("r1"), target: s("d3"), selection: s("C"), note: None, parent: None }),
            ev(9, Actor::Agent, Kind::ElabAdd { rid: s("r1"), text_md: s("**C** is") }),
        ];
        let state = State::fold(&events).unwrap();
        let view = BoardView::new(BoardInfo::new("/b.jsonl".into()), &state);
        let statuses: Vec<_> = view.decisions.iter().map(|d| (d.status, d.resolution.as_deref())).collect();
        assert_eq!(
            statuses,
            [(DecisionState::Resolved, Some("x")), (DecisionState::Withdrawn, None), (DecisionState::Pending, None)]
        );
        assert_eq!((view.todos[0].done, view.todos[0].note.as_deref()), (true, Some("ok")));
        assert_eq!(view.elaborations[0].answer.as_deref(), Some("**C** is"));
        assert_eq!(view.last_seq, 9);

        // The JSON shape clients rely on; a change here needs a new API_VERSION.
        let json = serde_json::to_value(&view.decisions[0]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id": "d1", "title": "A?", "context": null, "options": ["x"], "status": "resolved", "resolution": "x"})
        );
        let back: BoardView = serde_json::from_value(serde_json::to_value(&view).unwrap()).unwrap();
        assert_eq!(back, view);
    }

    #[test]
    fn board_id_is_stable() {
        // Changing the hash would orphan every remote tab saved in a session.
        assert_eq!(board_id(Path::new("/a/b.jsonl")), board_id(Path::new("/a/b.jsonl")));
        assert_ne!(board_id(Path::new("/a/b.jsonl")), board_id(Path::new("/a/c.jsonl")));
        assert_eq!(board_id(Path::new("")), "cbf29ce484222325");
        assert_eq!(board_name(Path::new("/x/.agents/todai/board.jsonl")), "board");
    }
}
