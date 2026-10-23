# todai

Decision & operator-todo board for agent workflows. Agents post decisions and
manual todos to an append-only JSONL log through the `todai` CLI; you answer
them in a small native window, and the agent picks the answers up.

```sh
todai board.jsonl                     # open the board (new tab if the window is running)
todai                                 # raise the window, or start it with last session's tabs
todai tabs                            # list open tabs
todai add-decision board.jsonl "Use X or Y?" -o X -o Y -c "context"
todai add-todo board.jsonl "Create the GitHub repo"
todai inbox board.jsonl               # agent: read and ack operator answers
todai wait board.jsonl                # agent: block until the operator answers
todai show board.jsonl                # text view of the board
```

See `todai --help` for the full command list and `docs/PLAN.md` for the design.

## Use it from your coding agent

```sh
todai setup
```

Pick your agents and where to install (this project only, or your user). It
lists every file it will write or change and asks first; `todai setup --remove`
undoes exactly that (files you edited since are kept). Scripted:
`todai setup --agent claude,pi --scope project --yes`.

Nothing is active until you ask for it: in a session, tell the agent to
**"use todai"**. It runs `todai attach`, which creates `.todai/board.jsonl` at
the repo root and opens it, and from then on routes its questions and manual
steps to the board.

| Agent | Installed | How your answers reach it |
|---|---|---|
| Claude Code | skill in `.claude/skills`, hooks in `.claude/settings.local.json` (project) or `~/.claude/settings.json` (user) | injected after its next tool call; it can't finish while answers are unread, and is reminded to leave a background `todai wait` running |
| pi | skill in `.agents/skills`, extension in `.pi/extensions` or `~/.pi/agent/extensions` | steers the running agent, or starts a turn if it is idle |
| Others (Codex, OpenCode, ...) | skill in `.agents/skills` | the agent checks `todai inbox` between steps and waits with `todai wait` |

Any agent with a shell works without setup too: tell it to run `todai guide`.
The protocol text ships inside the binary, so upgrading todai never needs a
re-setup.

## Window

One window, one tab per file. The first `todai FILE` starts the window in the
background (it outlives the terminal or agent shell that started it); later
calls hand their file to it over `$XDG_RUNTIME_DIR/todai.sock`, opening a tab
or switching to it. Open tabs are restored on the next start
(`~/.local/state/todai/session.json`). Window errors go to
`~/.local/state/todai/gui.log`; `todai open --foreground FILE` runs it attached
instead.

- Left column: accordion with **Pending** (decisions, todos) and **Done**.
- Elaborations get their own right column once the window fits 120 characters
  of body text; narrower windows show them as an accordion section instead.

### Input modes

Mouse works in both modes. **vim** mode (default) adds single-key commands;
**mouse** mode turns them off so stray letters do nothing. Switch with
`Ctrl+Shift+M` or the `vim`/`mouse` button. `?` or `F1` shows every key.

| Normal mode | |
|---|---|
| `j k` `↓ ↑` · `gg G` · `[ ]` · `Ctrl+d/u` | move · first/last · sections · ×5 |
| `h l` `← →` | collapse/expand section, switch to/from the elaborations column |
| `Enter` `i` `a` | answer the item, toggle a section, or jump from an elaboration to its item |
| `1-9` | pick option N (then `Enter`) |
| `x` | todo done/undone, reopen a resolved decision |
| `e` | elaborate on the whole item |
| `v` / `V` | select text (visual mode) / select all |
| `gt gT` · `za` | tabs · toggle section |

Visual mode: `h l w b e 0 ^ $ j k gg G` move the caret, `v` sets or drops the
anchor, `o` swaps ends, `Enter` opens the elaborate prompt for the selection
(or the word under the caret), `Esc` cancels.

When typing (answers, notes, the elaborate message): `Enter` submits,
`Shift+Enter` adds a newline, `Esc` returns to normal mode.

Anywhere: `Ctrl+Tab`/`Ctrl+1..9` tabs, `Ctrl+W` or a tab's `×` closes it, `Ctrl+Q` or **Quit** closes the window (asks first), `Ctrl+Shift+L`
dark/light (both saved in `~/.config/todai/config.json`), `Ctrl +/-` zoom.

With the mouse: select text in any item, then press `Enter`, right-click →
**Elaborate on selection**, or click **Elaborate**. With no selection those
elaborate on the whole item. **Follow up** asks about an answered elaboration.

## Memory on multi-vendor GL systems

libglvnd loads every installed EGL vendor. On an NVIDIA machine that also has
Mesa installed, Mesa pulls in libLLVM (~45MB RSS). Restrict it with:

```sh
export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json
```

Measured (release build, one tab): ~38MB PSS with NVIDIA only, ~54MB with both.
