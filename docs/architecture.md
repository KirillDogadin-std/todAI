# Architecture

A Cargo workspace with two binaries: `todai` (CLI + desktop window) and
`todai-server` (headless HTTP server, [server.md](server.md)). Everything
goes through the board file, so the CLI, the window, the server and the
agent adapters never talk to each other directly. The exceptions are the
window's socket, which only opens tabs, and the server's HTTP API, which
remote frontends use instead of the file.

```
 agent shell ──todai add-decision──┐                ┌── todai window (egui)
 agent hooks ──todai hook …────────┼──▶ board.jsonl ◀┤   local tab: watches the file,
 pi extension ─todai wait──────────┘   (flock, append)│   appends operator events
                                         ▲            │
 todai FILE ──────────── $XDG_RUNTIME_DIR/todai.sock ─┘   (open tab / focus / list)
                                         │
                          todai-server ──┘  reads, long-polls, appends operator events
                                ▲
                                └── HTTP + token ── remote tab in a window elsewhere, web app (served at `/`)
```

## Crates

| Crate | Depends on | |
|---|---|---|
| `crates/todai-core` | serde | the event log, inbox text, API wire types, XDG paths; no GUI, no network |
| `crates/todai-memory` | rusqlite (bundled), serde, unicode tables | decision memory: an append-only SQLite event store, one database per project in `$XDG_STATE_HOME/todai/memory/`; depends on nothing else in todai |
| `crates/todai-server` | core, tiny_http | the HTTP server, as a library and the `todai-server` binary; no GUI |
| `crates/todai-client` | core, notify, ureq | what every frontend shares: open boards, keyboard navigation, card text, the server client, the window socket, config and session; no GUI |
| `crates/todai-tui` | core, client, ratatui | `todai-tui`: the board in a terminal; no GUI |
| `todai` (repo root) | core, memory, client, server, egui | the CLI, the window, agent integration; `todai serve` embeds the server |

Keep the arrows pointing this way: core, server, client and TUI must build
without egui (CI checks), so a headless install never pulls in GL. Only the
root `todai` may depend on `todai-memory`, which keeps SQLite (~2 MB) out
of the server, client and TUI (CI checks too).

## Modules

| Path | |
|---|---|
| `crates/todai-core/src/store/` | the event log: types (`event.rs`), folding into a board (`state.rs`), locked append and read (`mod.rs`) |
| `crates/todai-core/src/inbox.rs` | turns unprocessed operator events into the text agents receive |
| `crates/todai-core/src/api.rs` | the HTTP API's wire types and board ids |
| `crates/todai-memory/src/` | `lib.rs` (`Memory::open` with WAL and busy timeout, `append`, `events`; `path_for` a project), `schema.rs` (migrations by `user_version`; triggers that refuse UPDATE/DELETE of events), `card.rs` (card types, validation, key normalisation; derived `card`/`key`/`tagged` tables, the card itself read from its `created` event), `format.rs` (gist line, hit block, full card), `recall.rs` (phrase → nearest keys → cards; snapping new keys; the `key_vector` cache per embedder), `embed/` (the `Embedder` trait, the one place to change the model; `potion.rs`: built-in potion-base-8M, int8, from `model/`; `wordpiece.rs`: BERT's tokenizer, pinned to HuggingFace's by `reference.json`) |
| `crates/todai-server/src/` | `lib.rs` (args, bind, run), `http.rs` (routes, auth, long-poll), `changes.rs` (one shared file watcher that wakes long-polls), `registry.rs` (token, registered boards, address file) |
| `src/memory.rs` | `todai memory ...`: finds the project's database, parses card sections from stdin, infers `--from` boards |
| `src/cli.rs` | every subcommand; also starting the window (`open`, `spawn_window`, `gui_main`) and opening remote boards (`open_remote`) |
| `crates/todai-client/src/nav.rs` | the keyboard state machine, on its own `Input` type; frontends translate their events into it |
| `crates/todai-client/src/tab.rs` | one open board and its source (file watcher or server long-poll); wakes the frontend through a callback |
| `crates/todai-client/src/cardtext.rs`, `textnav.rs` | card markdown as styled runs; vim motions over text |
| `crates/todai-client/src/remote.rs` | HTTP client for the server; saved tokens; registering boards with a local server |
| `crates/todai-client/src/ipc.rs` | single-instance socket: bind / take over a stale socket, request and reply |
| `crates/todai-client/src/session.rs`, `config.rs` | open tabs (one file per frontend), restored at startup; theme and input mode |
| `crates/todai-client/src/tracked.rs`, `manage.rs`, `boardlist.rs` | the board list: the tracked-boards index (append-only `$XDG_STATE_HOME/todai/tracked`) and per-board summaries; archive / unarchive / delete-to-trash / create as whole-file moves; the list's rows, filter, marks and keys, drawn by `src/gui/boards.rs` |
| `crates/todai-client/src/follow.rs` | followed servers: a thread per server re-lists its boards every 15 s; new ones open as tabs |
| `src/gui/` | the window: `mod.rs` (app, tabs, panels, dialogs, cheatsheet), `board.rs` (rendering), `input.rs` (egui events → `nav::Input`) |
| `crates/todai-tui/src/` | the terminal board: `app.rs` (tabs, socket, key routing), `ui.rs` (frame, overlays, cheatsheet), `board.rs` (cards as lines), `markdown.rs`, `text.rs` (wrapping), `edit.rs` (the field being typed into) |
| `src/agent/` | what agents see: `guide.md` (the protocol), the skill, `claude.rs` (hooks), `pi-extension.ts`, `setup.rs` (install / remove) |

## Board state

`State::fold` applies events in order and validates each one, so a board is
always consistent with the rules in [board-format.md](board-format.md).
`Log::append` takes a builder closure that runs under the lock with the
current state, which is how ids and `seq` stay unique across processes.

## The terminal board

`todai-tui` is a second frontend over the same `todai-client`: tabs, `Nav`
and card text are shared, so keys behave identically. It translates
crossterm events into `nav::Input`, renders the board into lines (recording
the same focus lists the window does), and redraws after every event. Tabs
wake its event loop through the same callback the window uses for repaints.
It serves the window socket when it is free, so `todai attach` opens boards
in it, and keeps its own session file (`tui-session.json`). With
`"frontend": "tui"` in the config, `todai FILE` / `attach` never start the
window when nothing answers.

## The window

- **Single instance.** `todai FILE` tries the socket first. If no window
  answers, it starts `todai __gui FILE` in a new session (`setsid`, so it
  outlives the terminal or agent shell) and waits until the socket answers. A
  window that loses a startup race to another forwards its files and exits.
- **Live updates.** A local tab watches its board's directory (inotify via
  `notify`) and reloads on change. The window's own writes trigger the same
  reload. A remote tab long-polls the server on a thread and folds the
  events it gets, so everything downstream (rendering, notifications) is the
  same; its actions go out in order on a second thread.
- **Notifications.** When a reload brings new agent posts (decisions, todos,
  elaboration answers) and the window is unfocused, one `notify-send` per tab
  lists them. Existing items never notify when a board is opened.
- **Hidden windows still answer.** Socket requests are handled in
  `App::logic`, which eframe runs even when the window isn't being drawn.
  On Wayland vsync is off: compositors send no frame callbacks to hidden
  windows, so a vsynced `swap_buffers` would block the event loop and the
  compositor would flag the window as not responding.
- **Frame order** (`App::ui`): shortcuts → key handling against the previous
  frame's focus lists (`Nav::handle`) → panels → board render, which records
  the new focus lists → `Nav::end_render` syncs mode with widget focus →
  operator events are appended.
- **Input modes.** `Nav` consumes key events before widgets see them, only in
  vim normal or visual mode. Mouse mode leaves keys to widgets. Text fields
  always get `Enter` (submit) and `Esc` (leave) from `Nav`.
- **Mouse selection.** Item text is a read-only `TextEdit`. egui reports its
  selection only while it has focus and resets it on any button press, so
  `Nav::mouse_selection` keeps the last selection (and restores it on
  right-click) for `Enter`, the context menu and the Elaborate button.

## Testing

- `cargo test` covers the store (folding, validation, torn lines, 4 threads
  appending concurrently), vim motions, IPC, setup's settings merge, the
  server's API over real HTTP, a remote tab against a live server, and the
  window itself.
- Window tests (`src/gui/tests.rs`) run the real board code headlessly with
  `egui::Context::run_ui`, feeding synthetic key and pointer events and
  checking what lands in the board file.
- TUI tests (`crates/todai-tui/src/tests.rs`) do the same with crossterm
  events and ratatui's `TestBackend`, checking the board file and the screen.
- Not covered automatically: real Claude Code and pi sessions, and how the
  window looks. `examples/demo.sh` builds a populated board for manual checks.

## Known gaps

- Focus requests from `todai FILE` may be refused by Wayland compositors.
- On machines with both NVIDIA and Mesa installed, libglvnd loads both GL
  stacks (~16 MB extra). Setting
  `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json`
  avoids it.
