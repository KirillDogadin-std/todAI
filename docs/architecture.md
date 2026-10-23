# Architecture

One binary, `todai`. Everything goes through the board file, so the CLI, the
window and the agent adapters never talk to each other directly. The one
exception is the window's socket, which only opens tabs.

```
 agent shell ──todai add-decision──┐                ┌── todai window (egui)
 agent hooks ──todai hook …────────┼──▶ board.jsonl ◀┤   watches the file,
 pi extension ─todai wait──────────┘   (flock, append)│   appends operator events
                                                      │
 todai FILE ──────────── $XDG_RUNTIME_DIR/todai.sock ─┘   (open tab / focus / list)
```

## Modules

| Path | |
|---|---|
| `src/store/` | the event log: types (`event.rs`), folding into a board (`state.rs`), locked append and read (`mod.rs`) |
| `src/cli.rs` | every subcommand; also starting the window (`open`, `spawn_window`, `gui_main`) |
| `src/inbox.rs` | turns unprocessed operator events into the text agents receive |
| `src/gui/` | the window: `mod.rs` (app, tabs, panels, dialogs, cheatsheet), `board.rs` (rendering), `nav.rs` (keyboard state machine), `textnav.rs` (vim motions), `tab.rs` (one open board + file watcher) |
| `src/ipc.rs` | single-instance socket: bind / take over a stale socket, request and reply |
| `src/session.rs` | open tabs, restored at startup |
| `src/config.rs` | theme and input mode; XDG directory helpers |
| `src/agent/` | what agents see: `guide.md` (the protocol), the skill, `claude.rs` (hooks), `pi-extension.ts`, `setup.rs` (install / remove) |

## Board state

`State::fold` applies events in order and validates each one, so a board is
always consistent with the rules in [board-format.md](board-format.md).
`Log::append` takes a builder closure that runs under the lock with the
current state, which is how ids and `seq` stay unique across processes.

## The window

- **Single instance.** `todai FILE` tries the socket first. If no window
  answers, it starts `todai __gui FILE` in a new session (`setsid`, so it
  outlives the terminal or agent shell) and waits until the socket answers. A
  window that loses a startup race to another forwards its files and exits.
- **Live updates.** Each tab watches its board's directory (inotify via
  `notify`) and reloads on change. The window's own writes trigger the same
  reload.
- **Notifications.** When a reload brings new agent posts (decisions, todos,
  elaboration answers) and the window is unfocused, one `notify-send` per tab
  lists them. Existing items never notify when a board is opened.
- **Hidden windows still answer.** Socket requests are handled in
  `App::logic`, which eframe runs even when the window isn't being drawn.
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
  appending concurrently), vim motions, IPC, setup's settings merge, and the
  window itself.
- Window tests (`src/gui/tests.rs`) run the real board code headlessly with
  `egui::Context::run_ui`, feeding synthetic key and pointer events and
  checking what lands in the board file.
- Not covered automatically: real Claude Code and pi sessions, and how the
  window looks. `examples/demo.sh` builds a populated board for manual checks.

## Known gaps

- Focus requests from `todai FILE` may be refused by Wayland compositors.
- On machines with both NVIDIA and Mesa installed, libglvnd loads both GL
  stacks (~16 MB extra). Setting
  `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json`
  avoids it.
