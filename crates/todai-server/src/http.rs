//! Request routing: the web app's static files at `/`, and the handlers
//! behind `/api/...`.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tiny_http::{Header, Method, Request, Response};
use todai_core::api::{
    API_VERSION, ApiError, BoardInfo, BoardSummaryView, BoardView, Events, Health, MAX_WAIT_SECS, NewBoard, Register,
};
use todai_core::store::{Actor, Event, Kind, Log};

use crate::auth::{Principal, Role, Tokens};
use crate::changes::Changes;
use crate::limit::{Limiter, Policy};
use crate::{Board, Registry};

/// Requests in flight, long-polls included. Past this the server answers 503
/// instead of starting more threads.
const MAX_IN_FLIGHT: usize = 64;
const MAX_BODY: u64 = 1 << 20;

pub struct Shared {
    pub tokens: Tokens,
    registry: Registry,
    in_flight: AtomicUsize,
    changes: Changes,
    limiter: Limiter,
}

impl Shared {
    pub fn new(tokens: Tokens, registry: Registry) -> Shared {
        Shared { tokens, registry, in_flight: AtomicUsize::new(0), changes: Changes::new(), limiter: Limiter::new(Policy::default()) }
    }
}

struct Reply {
    status: u16,
    body: Option<Vec<u8>>,
    content_type: &'static str,
}

fn json(status: u16, body: &impl Serialize) -> Reply {
    Reply { status, body: Some(serde_json::to_vec(body).unwrap_or_default()), content_type: "application/json" }
}

/// The web app (`crates/todai-server/web/`), built into the binary. Served
/// without a token: it holds no data, and asks for the token itself.
fn asset(path: &str) -> Option<Reply> {
    let text = |s: &'static str| s.as_bytes();
    let (body, content_type): (&'static [u8], _) = match path {
        "/" | "/index.html" => (text(include_str!("../web/index.html")), "text/html; charset=utf-8"),
        "/app.js" => (text(include_str!("../web/app.js")), "text/javascript; charset=utf-8"),
        "/markdown.js" => (text(include_str!("../web/markdown.js")), "text/javascript; charset=utf-8"),
        "/sw.js" => (text(include_str!("../web/sw.js")), "text/javascript; charset=utf-8"),
        "/app.css" => (text(include_str!("../web/app.css")), "text/css; charset=utf-8"),
        "/manifest.webmanifest" => (text(include_str!("../web/manifest.webmanifest")), "application/manifest+json"),
        "/icon.svg" => (text(include_str!("../web/icon.svg")), "image/svg+xml"),
        "/icon-192.png" => (include_bytes!("../web/icon-192.png"), "image/png"),
        "/icon-512.png" => (include_bytes!("../web/icon-512.png"), "image/png"),
        _ => return None,
    };
    Some(Reply { status: 200, body: Some(body.to_vec()), content_type })
}

/// Same-origin only and no inline code, so text an agent wrote can't run as
/// script even if the app had an escaping bug.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; \
                   manifest-src 'self'; worker-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

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
            let header = |k: &str, v: &str| Header::from_bytes(k, v).unwrap();
            let mut res = Response::from_data(body)
                .with_status_code(reply.status)
                .with_header(header("Content-Type", reply.content_type))
                .with_header(header("X-Content-Type-Options", "nosniff"))
                .with_header(header("Referrer-Policy", "no-referrer"))
                .with_header(header("Cache-Control", "no-cache"));
            if reply.content_type.starts_with("text/html") {
                res.add_header(header("Content-Security-Policy", CSP));
            }
            req.respond(res)
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

    // Unversioned and open, so clients can check compatibility first.
    if method == Method::Get && segs == ["api", "health"] {
        return json(200, &Health { version: env!("CARGO_PKG_VERSION").into(), api: API_VERSION });
    }
    if method == Method::Get && segs.first() != Some(&"api") {
        return asset(path).unwrap_or_else(|| error(404, "not found; the API is under /api"));
    }
    let version = format!("v{API_VERSION}");
    let route = match segs.as_slice() {
        ["api", v, rest @ ..] if *v == version => rest,
        ["api", ..] => return error(404, format!("no route {method} {path}; this server speaks /api/{version}")),
        _ => return error(404, "not found; the API is under /api"),
    };
    let ip = req.remote_addr().map(|a| a.ip());
    if let Some(ip) = ip
        && let Err(secs) = shared.limiter.check(ip)
    {
        return error(429, format!("too many wrong tokens from this address; try again in {secs} s"));
    }
    let Some(who) = authorized(req, &shared.tokens) else {
        if let Some(ip) = ip {
            shared.limiter.failure(ip);
        }
        return error(401, "missing or wrong token (Authorization: Bearer ...)");
    };
    if let Some(ip) = ip {
        shared.limiter.success(ip);
    }
    // A board whose file was deleted while the server runs stays registered
    // (it may come back, e.g. `todai attach` with the same name; startup and
    // `prune` forget it), but reads as absent rather than as an empty board.
    let board = |id: &str| {
        shared.registry.get(id).filter(|p| p.exists()).ok_or_else(|| error(404, format!("no board {id}")))
    };

    let result = match (&method, route) {
        (Method::Get, ["boards"]) => {
            let boards: Vec<BoardInfo> =
                shared.registry.list().iter().filter(|b| b.path.exists()).map(Board::info).collect();
            Ok(json(200, &boards))
        }
        (Method::Get, ["summaries"]) => {
            let now = todai_core::store::now_ms();
            let rows: Vec<BoardSummaryView> = shared
                .registry
                .list()
                .iter()
                .filter(|b| b.path.exists())
                .map(|b| BoardSummaryView::new(&b.path, now))
                .collect();
            Ok(json(200, &rows))
        }
        (Method::Post, ["boards"]) => register(shared, req),
        (Method::Post, ["boards", id, "archive"]) => board(id).map(|p| relocate(shared, id, &p, true)),
        (Method::Post, ["boards", id, "unarchive"]) => board(id).map(|p| relocate(shared, id, &p, false)),
        (Method::Post, ["boards", id, "siblings"]) => board(id).and_then(|p| sibling(shared, &p, req)),
        (Method::Get, ["boards", id]) => board(id).map(|p| state(&p)),
        (Method::Delete, ["boards", id]) => Ok(match shared.registry.remove(id) {
            Ok(true) => Reply { status: 204, body: None, content_type: "" },
            Ok(false) => error(404, format!("no board {id}")),
            Err(e) => error(500, format!("{e:#}")),
        }),
        (Method::Get, ["changes"]) => Ok(feed(shared, query)),
        (Method::Get, ["boards", id, "events"]) => board(id).map(|p| events(&shared.changes, &p, query)),
        (Method::Post, ["boards", id, "actions"]) => board(id).map(|p| action(&p, &who, req)),
        _ => Err(error(404, format!("no route {method} {path}"))),
    };
    result.unwrap_or_else(|e| e)
}

/// The caller, if the request carries a known token.
fn authorized(req: &Request, tokens: &Tokens) -> Option<Principal> {
    let given = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer "))?;
    tokens.identify(given)
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
        Ok(board) => Ok(json(201, &board.info())),
        Err(e) => Err(error(422, format!("{e:#}"))),
    }
}

/// Archives (or unarchives) a board: the file moves, so the server serves
/// it under its new path, and so its new id, which the reply carries. Any
/// token holder may: it is reversible (unlike delete, which the API leaves
/// out).
fn relocate(shared: &Shared, id: &str, path: &Path, archive: bool) -> Reply {
    let moved = if archive { todai_core::boards::archive(path) } else { todai_core::boards::unarchive(path) };
    let to = match moved {
        Ok(to) => to,
        // Already (un)archived, or a live board of that name exists again.
        Err(e) => return error(409, format!("{e:#}")),
    };
    if let Err(e) = shared.registry.add(&to).and_then(|_| shared.registry.remove(id)) {
        return error(500, format!("moved to {}, but the registry failed: {e:#}", to.display()));
    }
    json(200, &BoardInfo::new(&to))
}

/// Creates an empty board next to `anchor` (in its live folder, also for an
/// archived anchor) and serves it. A token holder only ever gets an empty
/// file with a plain name, in a folder that already holds a served board.
fn sibling(shared: &Shared, anchor: &Path, req: &mut Request) -> Result<Reply, Reply> {
    let NewBoard { name } = read_json(req)?;
    let dir = todai_core::boards::boards_dir(anchor);
    let path = todai_core::boards::create(&dir, name.trim()).map_err(|e| error(422, format!("{e:#}")))?;
    shared.registry.add(&path).map_err(|e| error(500, format!("{e:#}")))?;
    Ok(json(201, &BoardInfo::new(&path)))
}

/// The folded board, for clients that don't fold events themselves.
fn state(path: &Path) -> Reply {
    match Log::new(path).load_state() {
        Ok((_, state)) => json(200, &BoardView::new(BoardInfo::new(path), &state)),
        Err(e) => error(500, format!("{e:#}")),
    }
}

/// `name`'s value in a query string.
fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

/// The change feed across boards; see `feed.rs`.
fn feed(shared: &Shared, query: &str) -> Reply {
    let cursor = match query_param(query, "cursor").filter(|c| !c.is_empty()) {
        None => None,
        Some(c) => match todai_core::api::decode_cursor(&c.replace("%3A", ":").replace("%3a", ":").replace("%2C", ",").replace("%2c", ",")) {
            Some(c) => Some(c),
            None => return error(400, "bad cursor; start over without one"),
        },
    };
    let wait = match query_param(query, "wait").map(str::parse::<u64>) {
        None => 0,
        Some(Ok(w)) => w.min(MAX_WAIT_SECS),
        Some(Err(_)) => return error(400, "bad wait"),
    };
    match crate::feed::wait(&shared.changes, &shared.registry, cursor, Instant::now() + Duration::from_secs(wait)) {
        Ok(f) => json(200, &f),
        Err(e) => error(500, format!("{e:#}")),
    }
}

/// Events after `after`; with `wait`, blocks until there are some or the
/// wait runs out (an empty list).
fn events(changes: &Changes, path: &Path, query: &str) -> Reply {
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
    match poll(changes, &Log::new(path), after, Instant::now() + Duration::from_secs(wait)) {
        Ok(ev) => json(200, &ev),
        Err(e) => error(500, format!("{e:#}")),
    }
}

/// Sleeps until the board file changes (see [`Changes`]), and re-reads the
/// log only when its size changed: appends are the only writes, so an
/// unchanged size means nothing new.
fn poll(changes: &Changes, log: &Log, after: u64, deadline: Instant) -> anyhow::Result<Events> {
    let sub = changes.subscribe(log.path());
    let mut last_len: Option<Option<u64>> = None; // None: not read yet
    let mut last_seq = 0;
    loop {
        let seen = sub.generation();
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
        sub.wait(seen, deadline);
    }
}

/// Appends one operator event. Agent events are refused: agents write board
/// files directly, and the token is the operator's.
fn action(path: &Path, who: &Principal, req: &mut Request) -> Reply {
    let mut kind: Kind = match read_json(req) {
        Ok(k) => k,
        Err(e) => return e,
    };
    if who.role != Role::Operator {
        return error(403, "an agent token can't post operator actions");
    }
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
