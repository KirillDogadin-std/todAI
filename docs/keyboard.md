# Keyboard and mouse

todai has two input modes. Switch with `Ctrl+Shift+M` or the `vim` / `mouse`
button; the choice is saved.

- **vim** (default): single-key commands below are on.
- **mouse**: single-key commands are off, so stray letters do nothing.
  Everything else (clicks, `Ctrl` shortcuts, `Enter` to submit) is the same.

The mouse works in both modes. `?` or `F1` shows this reference in the window.
The status bar shows the current state: `NORMAL`, `INSERT`, `VISUAL`,
`ELABORATE`, or `MOUSE`.

## Normal mode

| Keys | Action |
|---|---|
| `j` `k` · `↓` `↑` | next / previous row (section headers are rows too) |
| `gg` · `G` | first / last row |
| `[` · `]` | previous / next section |
| `Ctrl+d` · `Ctrl+u` | 5 rows down / up |
| `h` · `←` | go to the section header; on a header, collapse it; in the elaborations column, back to the left |
| `l` · `→` | on a header, expand it; on an item, jump to its elaborations column |
| `Enter` · `i` · `a` | answer the item (decision resolution or todo note); toggle a section; on an elaboration, jump to its item |
| `1`–`9` | pick option N into the answer, then `Enter` submits |
| `x` | todo done ↔ not done; reopen a resolved decision |
| `space` | mark / unmark a finished item or an elaboration (multi-select) |
| `d` | remove the marked items, or the one under the cursor, from the board (asks first); removing an unanswered elaboration cancels it |
| `Esc` | clear the selection |
| `o` · `za` | on an elaboration thread: expand / collapse it (`zo` / `zc` too) |
| `zR` · `zM` | expand / collapse all elaborations, or only the selected ones |
| `K` | zoom: open the item or elaboration thread under the cursor in the overlay |
| `e` | elaborate on the whole item; on a thread, follow up on its newest answer |
| `v` · `V` | select text in the item (visual mode) / select all of it |
| `za` · `space` | toggle the section (`zo` open, `zc` close) |
| `gt` · `gT` | next / previous tab |
| `gb` | the board list (also `Ctrl+O`) |

## Visual mode (selecting text)

Starts with the caret at the beginning of the item's text and no selection,
like moving the cursor in vim before pressing `v`.

| Keys | Action |
|---|---|
| `h` `l` · `w` `b` `e` | move by character / word |
| `0` `^` `$` | line start / first non-blank / line end |
| `j` `k` | move by wrapped row |
| `gg` `G` | start / end of the text |
| `v` | set the selection anchor here (again: drop it) |
| `o` | swap the selection ends |
| `y` | copy the selection (the whole text if none) and leave visual mode |
| `Enter` | elaborate on the selection (on the word under the caret if none) |
| `Esc` | cancel |

## Zoom (overlay)

A large panel over the dimmed board for reading an item (title, status,
context as markdown, options, its elaboration threads) or a long thread. It owns the
keyboard while open; arrows, PageUp/PageDown, Home/End and `Esc` also work in
mouse mode.

| Keys | Action |
|---|---|
| `j` `k` · `Down` `Up` | scroll |
| `Ctrl+d` · `Ctrl+u` · `PageDown` · `PageUp` | scroll a page |
| `gg` · `G` · `Home` · `End` | top / bottom |
| `n` · `p` | next / previous item (as the left column lists them) or thread |
| `e` | elaborate on the item / follow up on the thread's newest answer |
| `Esc` · `q` | close (clicking outside closes too) |

## Typing

Answer fields, todo notes and the elaboration message:

| Keys | Action |
|---|---|
| `Enter` | submit |
| `Shift+Enter` | newline |
| `Esc` | stop typing (the draft is kept); cancels an elaboration message |

## Anywhere

| Keys | Action |
|---|---|
| `?` · `F1` | cheatsheet |
| `Ctrl+Shift+M` | vim ↔ mouse mode |
| `Ctrl+Shift+L` | dark ↔ light theme |
| `Ctrl+Tab` · `Ctrl+Shift+Tab` · `Ctrl+1`–`9` | switch tab |
| `Ctrl+W` | close the tab (also its `×`); asks first if items wait for you or you started an answer |
| `Ctrl+O` | the board list (also `gb`, and **Boards** in the top bar) |
| `Ctrl+Q` | close the window, after confirming (`Enter`/`y`, `Esc`/`n`); closing it from the compositor asks too; tabs are restored next time |
| `Ctrl +` · `Ctrl -` | zoom; also moves the two-column breakpoint |

## Board list

Every tracked board, grouped by project: the boards todai opened or an
agent attached (boards never opened through todai aren't listed). Each row
shows what waits for you, elaborations you asked for, last activity, and
`open` / `stale` / `archived` badges. A board is **stale** when nothing
waits on it and no agent wrote for 7 days. It owns the keyboard while open.

| Keys | Action |
|---|---|
| `j` `k` · `Down` `Up` · `gg` `G` | move |
| `Enter` · click the name | open the board (or switch to its tab) |
| `space` · checkbox | mark (actions apply to marked boards, else the one under the cursor) |
| `/` | filter by name or project; in mouse mode just type. `Enter` keeps it, `Esc` clears it |
| `a` | archive (moves the file to `.agents/todai/archive/`); on an archived board, unarchive |
| `d` | delete: moves the file to the trash, after asking |
| `n` | new empty board in the selected board's project; type a name, `Enter`. The agent then runs `todai attach NAME` |
| `S` | archive every stale board, after asking |
| `.` | show / hide archived boards |
| `s` | sort by project / by last activity |
| `Esc` · `q` · `Ctrl+O` | clear the filter, then the marks, then close |

## Mouse

- Click any card to put the cursor there; click fields and buttons as usual.
- **Elaborate on part of an item:** drag-select text in its title or context,
  then press `Enter`, right-click → *Elaborate on selection*, or click
  **Elaborate**. With no selection, those act on the whole item.
- **Follow up** on an answered elaboration asks about the answer, in the same
  thread. Selecting text in a thread (`v`) and pressing `Enter` follows up on
  the turn that holds it. Each thread is one card, oldest turn first.
- Section headers toggle on click.
- **Select** finished items or elaborations with their checkboxes (or
  **Select all** in Done). Shift-click a checkbox to select everything
  between it and the last one you clicked, within Done or within the
  elaborations.
- While anything is selected, an **action bar** at the bottom offers
  **🗑 Delete** (asks first), **Expand** / **Collapse** for the selected
  elaborations, and **Deselect all**.
- **Zoom** on an item card opens the item in the overlay, with **Prev** /
  **Next**, **Elaborate** and **×**.
- **Elaborations** fold: **more** / **less**, or click a collapsed preview.
  **Zoom** (or double-click the card) opens one in the overlay, with
  **Prev** / **Next**, **Follow up** and **×**.

## Layout

- Left: an accordion with **Pending** (decisions, then todos) and **Done**
  (resolved decisions, withdrawn ones struck through, finished todos).
- Right: **Elaborations**, once the window fits about 120 characters of body
  text. Narrower windows show them as an accordion section on the left.
- Elaborations collapse to a two-line preview. By default an answer stays
  open while it is the newest one or its item is still pending, and
  unanswered requests are always open; your own expand / collapse wins.

## Terminal (todai-tui)

`todai-tui` uses the same keymap (the code is shared), with these
differences, because terminals can't send some chords:

| Window | Terminal |
|---|---|
| `Ctrl+Tab` · `Ctrl+Shift+Tab` · `Ctrl+1`–`9` | `gt` · `gT` · `Alt+1`–`9` |
| `Shift+Enter` (newline while typing) | `Alt+Enter` or `Ctrl+J`; `Shift+Enter` where the terminal supports the kitty keyboard protocol |
| `Ctrl+Q` (quit, asks first) | `Ctrl+Q`, `Ctrl+C`, or `q` in normal mode |
| `Ctrl+Shift+M` (vim / mouse) | none: always vim keys; the mouse works too |
| `Ctrl+Shift+L` (theme) | none: the terminal's own colours |
| `Ctrl +` · `Ctrl -` (zoom) | none: elaborations get their own column at 120 columns |

While typing: `Ctrl+A` / `Ctrl+E` line start / end, `Ctrl+U` delete to line
start, `Ctrl+W` or `Alt+Backspace` delete a word (so `Ctrl+W` closes the tab
only when you aren't typing). Pasting works as one insertion.

The board list works the same; the mouse wheel moves its cursor and a click
on a row opens that board.

Mouse: the wheel scrolls the column under it (or the zoom overlay); a click
puts the cursor on a card, toggles a section header, picks an option (and
starts typing), ticks a checkbox (shift-click for a range), or folds an
elaboration (click its id). Clicking the cursor card again starts typing
its answer. Text selection for elaborating is `v` only; hold `Shift` to
select with your terminal as usual.

`y` in visual mode copies through the terminal (OSC 52), so it reaches your
clipboard over SSH too. In tmux, enable it with `set -g set-clipboard on`;
a terminal without OSC 52 support ignores it.
