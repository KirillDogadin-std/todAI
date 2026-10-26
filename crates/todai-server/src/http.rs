//! Request routing and the handlers behind `/api/...`.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tiny_http::{Header, Method, Request, Response};
use todai_core::api::{ApiError, BoardInfo, Events, Health, MAX_WAIT_SECS, Register};
use todai_core::store::{Actor, Event, Kind, Log};

use crate::Registry;

/// Requests in flight, long-polls included. Past this the server answers 503
/// instead of starting more threads.
const MAX_IN_FLIGHT: usize = 64;
const MAX_BODY: u64 = 1 << 20;
/// How often a long-poll checks its board file for growth.
const POLL: Duration = Duration::from_millis(200);

pub struct Shared {
    pub token: String,
    registry: Registry,
    in_flight: AtomicUsize,
}

impl Shared {
    pub fn new(token: String, registry: Registry) -> Shared {
        Shared { token, registry, in_flight: AtomicUsize::new(0) }
    }
}

struct Reply {
    status: u16,
    body: Option<Vec<u8>>,
}

fn json(status: u16, body: &impl Serialize) -> Reply {
    Reply { status, body: Some(serde_json::to_vec(body).unwrap_or_default()) }
}

fn error(status: u16, msg: impl Into<String>) -> Reply {
    json(status, &ApiError { error: msg.into() })
}

/// Handles `req` on its own thread.
pub fn dispatch(shared: &Arc<Shared>, req: Request) {
    if shared.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
        shared.in_flight.fetch_sub(1, Ordering::SeqCst);
        respond(req, error(503, "too many requests in flight"));
        return;
    }
    let shared = shared.clone();
    std::thread::spawn(move || {
        let mut req = req;
        let reply = handle(&shared, &mut req);
        respond(req, reply);
        shared.in_flight.fetch_sub(1, Ordering::SeqCst);
    });
}

fn respond(req: Request, reply: Reply) {
    let result = match reply.body {
        Some(body) => {
            let ct = Header::from_bytes("Content-Type", "application/json").unwrap();
            req.respond(Response::from_data(body).with_status_code(reply.status).with_header(ct))
        }
        None => req.respond(Response::empty(reply.status)),
    };
    let _ = result; // the client went away; nothing to do
}

fn handle(shared: &Shared, req: &mut Request) -> Reply {
    let url = req.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    let method = req.method().clone();

    if method == Method::Get && segs == ["api", "health"] {
        return json(200, &Health { version: env!("CARGO_PKG_VERSION").into() });
    }
    if !authorized(req, &shared.token) {
        return error(401, "missing or wrong token (Authorization: Bearer ...)");
    }
    let board = |id: &str| shared.registry.get(id).ok_or_else(|| error(404, format!("no board {id}")));

    let result = match (&method, segs.as_slice()) {
        (Method::Get, ["api", "boards"]) => Ok(json(200, &shared.registry.list())),
        (Method::Post, ["api", "boards"]) => register(shared, req),
        (Method::Get, ["api", "boards", id]) => board(id).map(|p| state(&p)),
        (Method::Delete, ["api", "boards", id]) => Ok(match shared.registry.remove(id) {
            Ok(true) => Reply { status: 204, body: None },
            Ok(false) => error(404, format!("no board {id}")),
            Err(e) => error(500, format!("{e:#}")),
        }),
        (Method::Get, ["api", "boards", id, "events"]) => board(id).map(|p| events(&p, query)),
        (Method::Post, ["api", "boards", id, "actions"]) => board(id).map(|p| action(&p, req)),
        (_, ["api", ..]) => Err(error(404, format!("no route {method} {path}"))),
        _ => Err(error(404, "not found; the API is under /api")),
    };
    result.unwrap_or_else(|e| e)
}

/// Constant-time, so response timing doesn't leak how much of a guess matched.
fn authorized(req: &Request, token: &str) -> bool {
    let Some(given) = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
    else {
        return false;
    };
    let (a, b) = (given.trim().as_bytes(), token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn read_json<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T, Reply> {
    let mut body = Vec::new();
    req.as_reader().take(MAX_BODY + 1).read_to_end(&mut body).map_err(|e| error(400, format!("read body: {e}")))?;
    if body.len() as u64 > MAX_BODY {
        return Err(error(413, "body too large"));
    }
    serde_json::from_slice(&body).map_err(|e| error(400, format!("bad JSON: {e}")))
}

/// Registering names a path on this machine, so only local clients may: a
/// phone holding the token can read and answer boards, not pick files.
fn register(shared: &Shared, req: &mut Request) -> Result<Reply, Reply> {
    if !req.remote_addr().is_some_and(|a| a.ip().is_loopback()) {
        return Err(error(403, "boards can only be registered from this machine"));
    }
    let Register { path } = read_json(req)?;
    match shared.registry.add(&path) {
        Ok(info) => Ok(json(201, &info)),
        Err(e) => Err(error(422, format!("{e:#}"))),
    }
}

/// The folded board, for clients that don't fold events themselves.
fn state(path: &Path) -> Reply {
    match Log::new(path).load_state() {
        Ok((_, state)) => json(200, &serde_json::json!({ "board": BoardInfo::new(path.to_owned()), "state": state })),
        Err(e) => error(500, format!("{e:#}")),
    }
}

/// Events after `after`; with `wait`, blocks until there are some or the
/// wait runs out (an empty list).
fn events(path: &Path, query: &str) -> Reply {
    let param = |name: &str| -> Result<u64, Reply> {
        query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == name)
            .map_or(Ok(0), |(_, v)| v.parse().map_err(|_| error(400, format!("bad {name}: {v}"))))
    };
    let (after, wait) = match (param("after"), param("wait")) {
        (Ok(a), Ok(w)) => (a, w.min(MAX_WAIT_SECS)),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    match poll(&Log::new(path), after, Instant::now() + Duration::from_secs(wait)) {
        Ok(ev) => json(200, &ev),
        Err(e) => error(500, format!("{e:#}")),
    }
}

/// Re-reads the log only when the file's size changed: appends are the only
/// writes, so an unchanged size means nothing new.
fn poll(log: &Log, after: u64, deadline: Instant) -> anyhow::Result<Events> {
    let mut last_len: Option<Option<u64>> = None; // None: not read yet
    let mut last_seq = 0;
    loop {
        let len = std::fs::metadata(log.path()).map(|m| m.len()).ok();
        if last_len != Some(len) {
            last_len = Some(len);
            let events: Vec<Event> = log.load()?;
            last_seq = events.last().map_or(0, |e| e.seq);
            if after > last_seq {
                return Ok(Events { events, last_seq, reset: true });
            }
            let new: Vec<Event> = events.into_iter().filter(|e| e.seq > after).collect();
            if !new.is_empty() {
                return Ok(Events { events: new, last_seq, reset: false });
            }
        }
        if Instant::now() >= deadline {
            return Ok(Events { events: Vec::new(), last_seq, reset: false });
        }
        std::thread::sleep(POLL);
    }
}

/// Appends one operator event. Agent events are refused: agents write board
/// files directly, and the token is the operator's.
fn action(path: &Path, req: &mut Request) -> Reply {
    let mut kind: Kind = match read_json(req) {
        Ok(k) => k,
        Err(e) => return e,
    };
    if kind.allowed_actor() != Some(Actor::Operator) {
        return error(403, "only operator actions can be posted");
    }
    let mut rejected = false;
    let result = Log::new(path).append(Actor::Operator, |s| {
        if let Kind::ElabRequest { rid, .. } = &mut kind {
            *rid = s.next_elab_id();
        }
        // Validate here too, to tell a refused action from a failed write.
        s.validate(Actor::Operator, &kind).inspect_err(|_| rejected = true)?;
        Ok(kind)
    });
    match result {
        Ok(event) => json(200, &event),
        Err(e) if rejected => error(422, format!("{e:#}")),
        Err(e) => error(500, format!("{e:#}")),
    }
}
