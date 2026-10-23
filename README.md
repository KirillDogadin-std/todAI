# todai

Decision & operator-todo board for agent workflows. Agents post decisions and
manual todos to an append-only JSONL log through the `todai` CLI; you answer
them in a small native window, and the agent picks the answers up.

```sh
todai board.jsonl                     # open the board window
todai add-decision board.jsonl "Use X or Y?" -o X -o Y -c "context"
todai add-todo board.jsonl "Create the GitHub repo"
todai inbox board.jsonl               # agent: read and ack operator answers
todai wait board.jsonl                # agent: block until the operator answers
todai show board.jsonl                # text view of the board
```

See `todai --help` for the full command list and `docs/PLAN.md` for the design.

## Window

- Left column: accordion with **Pending** (decisions, todos) and **Done**.
- Elaborations get their own right column once the window fits 240 characters
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

Anywhere: `Ctrl+Tab`/`Ctrl+1..9` tabs, `Ctrl+W` close tab, `Ctrl+Shift+L`
dark/light (both saved in `~/.config/todai/config.json`), `Ctrl +/-` zoom.

With the mouse: select text in any item and press **Elaborate** (no selection
elaborates the whole item); **Follow up** on an answered elaboration.

## Memory on multi-vendor GL systems

libglvnd loads every installed EGL vendor. On an NVIDIA machine that also has
Mesa installed, Mesa pulls in libLLVM (~45MB RSS). Restrict it with:

```sh
export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json
```

Measured (release build, one tab): ~38MB PSS with NVIDIA only, ~54MB with both.
