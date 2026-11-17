//! `GET /api/v2/stream`: the change feed pushed as server-sent events over
//! one long-lived response, instead of one long-poll per change. Each
//! message is a [`ChangeFeed`](todai_core::api::ChangeFeed), the same JSON
//! `/changes` answers; a comment line every [`HEARTBEAT`] keeps proxies from
//! timing the connection out and tells us when the client went away.
//!
//! tiny_http buffers chunked bodies, so this writes the response itself on
//! the raw connection (`Request::into_writer`) and flushes every message.
//! Long-poll stays: it is simpler for clients and passes every proxy.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, Instant};

use tiny_http::Request;
use todai_core::api::decode_cursor;

use crate::http::Shared;

pub const HEARTBEAT: Duration = Duration::from_secs(20);

/// Serves the stream until the client disconnects (or the server stops).
pub fn serve(shared: &Shared, req: Request, cursor: Option<BTreeMap<String, u64>>) {
    let mut out = req.into_writer();
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream; charset=utf-8\r\n\
                Cache-Control: no-cache\r\n\
                X-Content-Type-Options: nosniff\r\n\
                X-Accel-Buffering: no\r\n\
                Connection: close\r\n\r\n";
    if out.write_all(head.as_bytes()).and_then(|()| out.flush()).is_err() {
        return;
    }
    let mut cursor = cursor;
    loop {
        let feed = match crate::feed::wait(&shared.changes, &shared.registry, cursor.clone(), Instant::now() + HEARTBEAT) {
            Ok(f) => f,
            Err(e) => {
                let _ = write!(out, "event: error\ndata: {}\n\n", serde_json::json!({ "error": format!("{e:#}") }));
                return;
            }
        };
        let quiet = cursor.is_some() && feed.boards.is_empty() && feed.removed.is_empty();
        let msg = if quiet {
            ": still here\n\n".to_owned()
        } else {
            format!("event: changes\ndata: {}\n\n", serde_json::to_string(&feed).unwrap_or_default())
        };
        if out.write_all(msg.as_bytes()).and_then(|()| out.flush()).is_err() {
            return; // the client went away
        }
        cursor = decode_cursor(&feed.cursor);
    }
}
