# todai — decision & operator-todo board for agent workflows

## Context
Agents ask the operator questions and hand off manual tasks. Today these go into a hand-kept md file, which is ugly, hard to navigate, and the agent never finds out when the operator answers. Goal: a `todai <file>` command that opens a lightweight native window (one tab per file) showing pending decisions and operator todos, lets the operator answer and request elaborations by keyboard, and makes the agent (Claude Code first) aware of answers **without interrupting its current work**.

Greenfield: `/home/kirill/Coding/todai` is empty. Target: Arch + Hyprland (Wayland). Rust toolchain present.

## Locked decisions
| Area | Choice |
|---|---|
| UI | Rust + eframe/egui (wgpu), single static binary |
| Storage | Append-only JSONL event log; all writes go through the `todai` CLI |
| Agent awareness | Claude Code hooks (PostToolUse inject, Stop block) + backgrounded `todai wait`. Agent-agnostic CLI core. |
| Processes | Single GUI instance, unix socket, one tab per file |

## Architecture
One binary, `todai`, with these modes:
- `todai open <file>` (alias `todai <file>`): connect to `$XDG_RUNTIME_DIR/todai.sock`. If it connects, send `Open{path}` and the running GUI adds or focuses a tab. If not, spawn the detached GUI daemon, then send.
- Agent subcommands: `add-decision`, `add-todo`, `elaborate`, `withdraw`, `inbox`, `ack`, `show`, `wait`.
- Operator writes happen inside the GUI through the same store code.
- `todai hook <event>`: Claude Code hook entrypoints. They read hook JSON on stdin and print hook JSON.
- `todai install`: installs the skill to `~/.claude/skills/todai/` and merges hooks into `~/.claude/settings.json`, after showing a diff and asking first.

### Event log (`*.todai.jsonl`)
One JSON object per line: `{seq, ts, by: "agent"|"operator", type, ...}`.
- `decision.add {id:"d3", title, context?, options?[]}`: title is ≤ ~80 chars and is the short form the UI shows.
- `decision.resolve {id, text}` / `decision.reopen {id}` / `decision.withdraw {id}`
- `todo.add {id:"t2", title, context?}` / `todo.done {id, note?}` / `todo.undone {id}`
- `elab.request {rid, target, selection, note?}` comes from the operator. `elab.add {rid, text_md}` comes from the agent.
- `ack {upto_seq}`: the agent has consumed operator events up to this seq.

Store rules: `flock` + `O_APPEND`. The CLI assigns `seq` and ids under the lock. State = fold over events. "Unprocessed" = operator events with seq > last agent `ack`. Readers tail from a byte offset, and the GUI watches files with `notify` (inotify).

### Claude Code integration
- **Session↔file binding**: the PostToolUse hook watches Bash commands. When the agent runs `todai … <file>`, the hook records `session_id → file` in `$XDG_STATE_HOME/todai/bindings.json`. The agent does nothing extra.
- **PostToolUse** (matcher `*`): if a bound file has unprocessed operator events, emit `hookSpecificOutput.additionalContext` with a compact summary (e.g. "Operator resolved d3: X; requested elaboration r2 on d5: '<selection>' note: …"), then auto-ack. This runs only between tool calls, so it never interrupts work.
- **Stop**: if there are unprocessed events, return `decision: block` with the summary so the agent keeps going. If decisions are still pending and no `todai wait` is alive (pidfile), remind the agent to start one.
- **Idle wake**: the skill tells the agent to run `todai wait <file>` with `run_in_background` after posting decisions. It exits when a new operator event arrives, and Claude Code then re-invokes the agent.
- **Skill** (`skill/SKILL.md`): when to post decisions or todos (short titles, optional context), how to answer `elab.request` (`todai elaborate <rid> --md -`), the ack semantics, and never hand-editing the file.

### GUI (egui)
- Tab bar: one tab per file. Ctrl+1..9 / Ctrl+Tab switch, Ctrl+W closes. Open tabs persist across restarts.
- Layout is responsive at a breakpoint of ~1000px:
  - Wide: left column has **Pending decisions** (each with a resolution input, plus quick-pick option chips if `options` are given) and **Todos**. Right column shows the elaboration thread and context of the focused item, and **Decisions made** / **Done** below it.
  - Narrow: a single column in the order of your sketch, with elaborations indented under their item.
- Keyboard model (modal, so typing in inputs never conflicts with shortcuts):
  - `j/k` move between items. `Enter`/`i` edits the resolution. `Ctrl+Enter` submits. `Esc` leaves the input.
  - `x` toggles a todo done, with an optional note.
  - `v` enters select mode on the focused item's text (read-only TextEdit; shift+arrows, `w/b` word jumps). `e` elaborates the selection, opening an optional note input, then `Ctrl+Enter` appends `elab.request`.
  - The mouse works for everything too.
- Elaborations render markdown with `egui_commonmark`. Each pending request shows a spinner until its `elab.add` arrives.

### Crates
`eframe`/`egui`, `egui_commonmark`, `serde`/`serde_json`, `clap`, `notify`, `fs2` (flock), `directories`, `anyhow`. IPC uses std `UnixListener` plus newline-delimited JSON.

### Layout
Single crate, with modules `store/` (events, fold, lock), `cli/`, `gui/`, `ipc/`, `hooks/`, and `install/`, plus `skill/SKILL.md` and `tests/`.

## Milestones
1. **Core**: event types, store (lock/append/fold/tail), agent CLI subcommands, `inbox`/`ack`/`wait`. Unit tests for fold and concurrent appends.
2. **GUI, single file**: sections, resolve and todo-done input, live reload on file change.
3. **Elaborate**: keyboard select mode, request flow, markdown rendering, right-pane layout.
4. **Single instance + tabs**: socket, daemon spawn, tab persistence.
5. **Claude Code integration**: hook subcommands, binding, `install`, skill.
6. **Polish**: theme/typography, desktop notification on a new decision (optional), Hyprland window rule docs.

## Small details still open (decide per milestone)
- Default file location (`.todai/<name>.jsonl` in the repo vs. anywhere) and extension.
- Decision priority / "agent blocked on this" flag.
- Whether the operator can add their own todos or decisions.
- Reopening resolved decisions, and agent withdraw semantics in the UI.
- Theme (light/dark, follow system).

## Verification
- `cargo test`: fold correctness, 2 processes appending 1000 events concurrently with no corruption, seq monotonic.
- Manual: `todai demo.jsonl` opens a window. A second `todai other.jsonl` opens a new tab in the same process (check `pgrep todai`). RSS is under ~80MB (`ps -o rss`).
- From a shell, `todai add-decision demo.jsonl "Use X or Y?"` makes the item appear live. Resolving it in the GUI appends the event, and `todai inbox demo.jsonl` prints it.
- Claude Code E2E: after `todai install`, start a session, have the agent post a decision and then run a long task, answer in the GUI mid-task, and confirm the answer shows up as context after the next tool call. Then test idle: the agent is waiting with `todai wait` in the background, the operator answers, and the agent wakes. Then test Stop: try to finish with unprocessed answers and confirm it is blocked.
- Keyboard-only run-through: navigate, select text, elaborate with a note, receive the elaboration, resolve.
