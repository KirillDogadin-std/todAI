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
- `Ctrl+Enter` submits the focused resolution or todo note.
- `Ctrl+Tab` / `Ctrl+Shift+Tab` / `Ctrl+1..9` switch tabs, `Ctrl+W` closes one.
- `Ctrl+Shift+L` toggles dark/light (saved in `~/.config/todai/config.json`).
- `Ctrl +` / `Ctrl -` zoom.

## Memory on multi-vendor GL systems

libglvnd loads every installed EGL vendor. On an NVIDIA machine that also has
Mesa installed, Mesa pulls in libLLVM (~45MB RSS). Restrict it with:

```sh
export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json
```

Measured (release build, one tab): ~38MB PSS with NVIDIA only, ~54MB with both.
