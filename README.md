# 💀 todai

A small native board where your coding agent puts **decisions it needs from
you** and **things only you can do by hand**, so they stop scrolling past in
chat. You answer in the window; the agent picks the answers up without being
interrupted.

- **Native and light:** Rust + egui, one ~10 MB binary, ~40–55 MB of memory (PSS).
- **Keyboard first, mouse friendly:** vim-style navigation, a selection mode
  for asking "what does *this* part mean?", and a mouse mode with no
  single-key surprises.
- **Agent agnostic and opt-in:** works with Claude Code, pi, Codex, OpenCode,
  or anything with a shell. Nothing is installed or active until you ask.
- **Plain data:** each board is an append-only JSONL file you can read, grep,
  or commit.

## Install

**Prebuilt (Linux x86_64, glibc 2.30+):** download `todai-*-x86_64-linux-gnu.tar.gz`
from [Releases](../../releases), then

```sh
tar xzf todai-*-x86_64-linux-gnu.tar.gz
install -m755 todai-*/todai ~/.local/bin/
```

**From source** (stable Rust; the repo pins `stable`):

```sh
git clone https://github.com/KirillDogadin-std/todAI && cd todai
cargo install --path .
```

Runs on Wayland and X11. It needs a GL driver at runtime, plus `notify-send`
if you want desktop notifications.

## Use it with your agent

```sh
todai setup
```

Choose your agents and where to install (this project only, or every project).
Setup lists every file it will write or change and asks first.
`todai setup --remove` undoes exactly that.

Then, in a session, tell your agent **"use todai"**. It runs `todai attach`,
which creates `.agents/todai/board.jsonl` at the repo root and opens the window, and
from then on sends its questions and manual steps to the board.

| Agent | What setup installs | How your answers reach the agent |
|---|---|---|
| Claude Code | a skill and two hooks | injected after its next tool call; it cannot finish while answers are unread |
| pi | a skill and an extension | steers the running agent, or starts a turn when it is idle |
| Codex, OpenCode, others | a skill in `.agents/skills` | the agent checks `todai inbox` between steps, `todai wait` when blocked |

No setup at all? Any agent with a shell can use todai: tell it to run
`todai guide`. Details: [docs/agents.md](docs/agents.md).

## Use the board

```sh
todai FILE              # open a board (a new tab if the window is already open)
todai                   # raise the window, or reopen last session's tabs
examples/demo.sh        # a populated demo board to play with
```

| | |
|---|---|
| Answer a decision | type in its field (or press `1`–`9` for an option), `Enter` |
| Finish a todo | **Done**, or `x`; add an optional note for the agent |
| Ask what something means | select the text (mouse, or `v`), then `Enter` / right-click / **Elaborate**; add a message |
| Read a long answer | **Zoom** on the elaboration, double-click it, or `K`: a large overlay over the dimmed board |
| Change your mind | **Reopen** a decision or **Undo** a todo in *Done* |
| Clean up | select finished items or elaborations (checkbox, shift-click for a range, or `space`), then **🗑 Delete** in the action bar (or `d`) |

When the window isn't focused, new posts from the agent show up as a desktop
notification. The window shows *Pending* and *Done* as collapsible sections; elaborations
get their own column when the window is wide enough. `?` shows every key.
Full reference: [docs/keyboard.md](docs/keyboard.md).

## Commands

| | |
|---|---|
| `todai open [FILE...]` | open boards in the window (`--foreground` to run attached) |
| `todai tabs` | list open tabs |
| `todai setup` | install agent integrations (`--agent`, `--scope`, `--dry-run`, `--remove`) |
| `todai attach [NAME]` | *agent*: start using `.agents/todai/NAME.jsonl` and print its path |
| `todai guide` | *agent*: print the protocol |
| `todai add-decision BOARD TITLE [-c CONTEXT] [-o OPTION]...` | *agent*: ask |
| `todai add-todo BOARD TITLE [-c CONTEXT]` | *agent*: hand over a manual step |
| `todai inbox BOARD` / `todai wait BOARD` | *agent*: read new answers / block until there are some |
| `todai elaborate BOARD rN [TEXT\|-]` | *agent*: answer an elaboration request |
| `todai withdraw BOARD dN` | *agent*: drop a decision |
| `todai show BOARD [--json]` | print the board |
| `todai resolve`, `reopen`, `done`, `undone`, `ask`, `trash` | the operator's actions, from a shell |

## Files

| Path | What |
|---|---|
| `.agents/todai/NAME.jsonl` | a board (one per workflow); gitignore it or commit it as a decision record |
| `.agents/skills/todai/` | the skill, if installed for the project (linked from `.claude/skills/todai` for Claude Code) |
| `~/.config/todai/config.json` | theme and input mode |
| `~/.local/state/todai/` | open tabs, setup records, hook state, window log |
| `$XDG_RUNTIME_DIR/todai.sock` | the running window's socket |

## Docs

- [docs/agents.md](docs/agents.md): how agent integration works, per agent
- [docs/keyboard.md](docs/keyboard.md): every key and mouse action
- [docs/board-format.md](docs/board-format.md): the event log
- [docs/architecture.md](docs/architecture.md): how the code fits together

## License

[Apache-2.0](LICENSE)
