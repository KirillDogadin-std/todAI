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
| `o` · `za` | on an elaboration: expand / collapse it (`zo` / `zc` too) |
| `zR` · `zM` | expand / collapse all elaborations, or only the selected ones |
| `e` | elaborate on the whole item |
| `v` · `V` | select text in the item (visual mode) / select all of it |
| `za` · `space` | toggle the section (`zo` open, `zc` close) |
| `gt` · `gT` | next / previous tab |

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
| `Enter` | elaborate on the selection (on the word under the caret if none) |
| `Esc` | cancel |

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
| `Ctrl+W` | close the tab (also its `×`) |
| `Ctrl+Q` | close the window, after confirming (`Enter`/`y`, `Esc`/`n`); closing it from the compositor asks too; tabs are restored next time |
| `Ctrl +` · `Ctrl -` | zoom; also moves the two-column breakpoint |

## Mouse

- Click any card to put the cursor there; click fields and buttons as usual.
- **Elaborate on part of an item:** drag-select text in its title or context,
  then press `Enter`, right-click → *Elaborate on selection*, or click
  **Elaborate**. With no selection, those act on the whole item.
- **Follow up** on an answered elaboration asks about the answer.
- Section headers toggle on click.
- **Select** finished items or elaborations with their checkboxes (or
  **Select all** in Done). Shift-click a checkbox to select everything
  between it and the last one you clicked, within Done or within the
  elaborations.
- While anything is selected, an **action bar** at the bottom offers
  **🗑 Delete** (asks first), **Expand** / **Collapse** for the selected
  elaborations, and **Clear**.
- **Elaborations** fold: **more** / **less**, or click a collapsed preview.

## Layout

- Left: an accordion with **Pending** (decisions, then todos) and **Done**
  (resolved decisions, withdrawn ones struck through, finished todos).
- Right: **Elaborations**, once the window fits about 120 characters of body
  text. Narrower windows show them as an accordion section on the left.
- Elaborations collapse to a two-line preview. By default an answer stays
  open while it is the newest one or its item is still pending, and
  unanswered requests are always open; your own expand / collapse wins.
