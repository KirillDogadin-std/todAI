# Server

`todai-server` serves boards over HTTP, so you can answer them from
somewhere other than the machine the agents run on: the desktop window on
another computer, or the built-in web app on your phone. It is a separate, headless
binary (about 1.4 MB, a few MB of memory, no GL); `todai serve` runs the same
server from the main binary.

Board files stay the source of truth. Agents keep writing them directly, and
nothing about agent integration changes. The server reads the files, follows
them for live updates, and appends the operator's actions through the same
locked append as every other writer.

```
 agents ──todai add-decision…──▶ board.jsonl ◀── todai window (local tabs)
                                     ▲
                          todai-server (reads, appends operator events)
                                     ▲  HTTP + bearer token
                 todai window on another machine (remote tabs), web app, curl
```

## Run it

```sh
cargo build --release -p todai-server      # or: cargo install --path crates/todai-server
todai-server [--listen ADDR] [FILE...]     # serve; FILEs are registered too
todai-server token [--rotate]              # print (or replace) the access token
todai-server boards                        # list registered boards; (missing) if the file is gone
todai-server forget FILE                   # stop serving a board
todai-server prune                         # forget boards whose file was deleted (also done at startup)
todai-server pair [--host HOST]            # link + QR code that open the web app with the token
```

It listens on `127.0.0.1:7341` by default. While it runs, `todai attach` and
`todai open FILE` register their boards with it, so boards your agents start
using show up without extra steps.

## Reach it from another machine

The server speaks plain HTTP. Binding to anything but loopback
(`--listen 0.0.0.0:7341`) exposes your boards and the token to the network
in the clear, so put it behind something that encrypts:

- **Tailscale** (or another WireGuard VPN): bind to the tailnet address, or
  `tailscale serve 7341` for HTTPS on your tailnet.
- **A reverse proxy with TLS** (Caddy, nginx) in front of `127.0.0.1:7341`.
  Registering boards is only allowed from loopback, and behind a proxy on
  the same machine every request looks like loopback, so keep the token
  secret.

Then, on the other machine:

```sh
todai open --server http://HOST:7341 --token TOKEN           # every board, and new ones as they come
todai open --server http://HOST:7341 --token TOKEN board     # by name, project/name, or id
```

The token is saved (0600, `~/.local/state/todai/remotes.json`), so later
`todai open --server URL` and session restore need no flag. `$TODAI_TOKEN`
works too. The desktop client speaks plain HTTP only; use the VPN route
for it.

Opened without board names, the server is **followed**: boards registered
there later (an agent's `todai attach` on that machine) open as new tabs
within about 15 s, in the background so the tab you're on stays. A board
opens this way once; close its tab and it stays closed. Following ends when
you close the server's last tab, and survives a restart with the session.
`todai-tui --server URL` does the same.

Remote tabs look and work like local ones. The tab title is
`board@host`. If the server goes away the status bar says so and the tab
reconnects on its own; an action that could not be sent puts your text back
in its field.

## Web app

The server also serves a small web app at `/` for answering boards from a
phone: board list by project (filter, last activity, stale and archived
boards, archive or unarchive one, the selected ones, or all stale ones),
then a board with what waits for you,
elaboration threads and finished items. You can resolve, mark done, undo,
reopen, trash, and ask for an elaboration (select text first to ask about
just that, or tap **Ask** for the whole item). Changes show up live.

To pair a phone, start the server where the phone can reach it (see below),
then on that machine:

```sh
todai-server pair --host <tailscale-name-or-LAN-IP>
todai-server pair --host https://box.tailnet.ts.net     # behind `tailscale serve`
```

Scan the QR code. The token travels in the link's `#fragment`, which
browsers never send to the server or put in logs; the app keeps it in the
browser's storage and removes it from the address bar. Anyone who sees the
QR code or link can answer your boards, same as with the token itself; to
revoke every device, run `todai-server token --rotate` and pair again. The
pairing screen also takes a pasted token.

The app is plain HTML, CSS and JavaScript in `crates/todai-server/web/`,
built into the binary; no build step. Only same-origin scripts may run
(`Content-Security-Policy`), and board text is never parsed as HTML.

## Files

In `$XDG_STATE_HOME/todai/server/` (default `~/.local/state/todai/server/`):

| File | |
|---|---|
| `token` | the bearer token, 0600; created on first start |
| `boards.json` | registered board paths |
| `addr` | where the running server answers locally; how `todai attach` finds it |

## API

All bodies are JSON. Routes are versioned under `/api/v2`; only
`/api/health` is not, so a client can check compatibility first. Every
versioned route needs `Authorization: Bearer TOKEN`; a missing or wrong token
gets `401`. Errors come back as `{"error": "..."}`.

The contract is the types in `crates/todai-core/src/api.rs`, not the
internal board state: `BoardView` and friends are built from it in one place
and tested for their JSON shape, so refactors inside todai don't reach
clients. Any change to those types bumps `API_VERSION` (and the route
prefix). Events are the [board format](board-format.md) lines, which are
stable on their own.

| Route | |
|---|---|
| `GET /api/health` | `{"version", "api"}`: todai's release and the API version (`2`); no token needed |
| `GET /api/v2/boards` | `[{"id", "name", "project"}]` |
| `POST /api/v2/boards` `{"path"}` | register an existing `.jsonl` board by absolute path; loopback only (`403` otherwise); `201` with the board |
| `DELETE /api/v2/boards/{id}` | unregister; `204`, or `404` |
| `GET /api/v2/summaries` | `[BoardSummaryView]`: each served board with what a board list shows (below) |
| `POST /api/v2/boards/{id}/archive` · `/unarchive` | move the board file into `.agents/todai/archive/` or back; `200` with the board under its **new id**; `409` if already there, or if a live board of that name exists again |
| `GET /api/v2/boards/{id}` | the board as the operator sees it (`BoardView`, below) |
| `GET /api/v2/boards/{id}/events?after=SEQ&wait=SECS` | `{"events", "last_seq", "reset"?}`: events with `seq > after` |
| `POST /api/v2/boards/{id}/actions` | append one operator event (body: an event without `seq`, `ts`, `by`); returns the written event |

Any other path under `/api/` (including the unversioned `/api/boards`) gets
`404` with the version this server speaks.

### Board summaries and archiving

```json
{"board": {"id": "…", "name": "main", "project": "todai"},
 "waiting": 2, "asked": 1, "last": 1794258201740, "last_agent": 1794258201740,
 "archived": false, "stale": false}
```

`waiting` counts pending decisions and open todos, `asked` elaborations
waiting for an agent. `last` is the newest event in Unix ms (the file's age
if it has none), `last_agent` the newest agent event. `stale`: nothing
waits and no one wrote for a week. These two routes and the type came in
todai 0.7 without bumping the API version: they are additions, and no
existing response changed.

Archiving moves the file, and a board's id is a hash of its path, so the
board gets a new id; the old one answers `404` afterwards. There is
deliberately no delete: any token holder may archive, which is reversible.
The window and the TUI can also delete, into the trash of the machine they
run on.

### Board view

```json
{
  "board": {"id": "51c50ece8bf66d5b", "name": "board", "project": "todai"},
  "last_seq": 9,
  "decisions": [{"id": "d1", "title": "A?", "context": null, "options": ["x"], "status": "resolved", "resolution": "x"}],
  "todos": [{"id": "t1", "title": "Do", "context": null, "done": true, "note": "ok"}],
  "elaborations": [{"rid": "r1", "target": "d1", "selection": "A", "note": null, "parent": null, "answer": "**A** is…"}]
}
```

`status` is `pending`, `resolved` or `withdrawn`; `resolution` is set only
when resolved. Trashed items are left out. Elaborations with the same root
(follow `parent`) form a thread; `answer` is markdown, `null` until the agent
answers. To follow changes from here, long-poll `events` with
`after=last_seq`.

A registered board whose file is deleted while the server runs is left out
of `GET /api/v2/boards` and gets `404` on its routes; it stays registered, so
it comes back if the file does (an agent attaching with the same name).
Startup and `todai-server prune` forget it for good.

Board ids are 16 hex digits of a hash of the board's path. They are the only
way to name a board, so a client can never reach a file that wasn't
registered.

Responses never include a board's path, so a token holder doesn't learn the
server's directory layout. `name` is the file name without `.jsonl`, and
`project` tells same-named boards apart: the folder holding `.agents/todai/`
for a board in a project (`~/code/todai/.agents/todai/main.jsonl` →
`todai`), else the folder the file is in. On the server machine,
`todai-server boards` shows full paths.

API v1 (todai 0.4.0) returned `path` instead of `project`. A window and a
server that speak different versions refuse each other with a message
naming both.

### Following a board

Ask for `events?after=0` once and fold the result (or start from the board
view's `last_seq`), then loop on
`events?after=LAST_SEQ&wait=25`. The server answers as soon as the board
changes, or with an empty list when the wait (at most 60 s) runs out. If
`reset` is `true`, the file was replaced: drop what you have and fold the
returned events from scratch. The desktop window does exactly this
(`crates/todai-client/src/tab.rs`).

### Posting actions

```sh
curl -H "Authorization: Bearer $TOKEN" -d '{"type":"decision.resolve","id":"d1","text":"yes"}' \
  http://127.0.0.1:7341/api/v2/boards/$ID/actions
```

Only operator event types are accepted (`decision.resolve`,
`decision.reopen`, `todo.done`, `todo.undone`, `elab.request`,
`item.trash`); agent types get `403`. For `elab.request` the server assigns
`rid`, whatever the body says. An action the board's rules refuse (unknown
id, resolving a withdrawn decision, ...) gets `422` and is not written.

## Limits

- One thread per request, at most 64 in flight (long-polls included); more
  get `503`.
- Request bodies up to 1 MB.
- No TLS, CORS or rate limiting in-process.
- The web app's files are served without a token; they hold no data.
