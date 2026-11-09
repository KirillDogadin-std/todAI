//! One frame: tab bar, the board (one or two columns), the elaboration
//! prompt, the selection bar, the status line, and overlays (zoom,
//! cheatsheet, confirmations). Mirrors `src/gui/mod.rs`'s frame order:
//! render records the focus lists, then `Nav::end_render` syncs the mode.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use todai_client::nav::{self, Focus, FocusRequest, Mode};
use todai_client::store::Kind;

use crate::app::{App, Column, Open};
use crate::board::{self, Board, Doc};
use crate::edit::{Edit, Target};
use crate::text::{self, ACCENT, str_width};

/// Width, in columns, at which elaborations move into their own column
/// (the window's breakpoint is about 120 characters of body text too).
pub const SPLIT_COLS: u16 = 120;

/// Shown with `?` / `F1`; keep in sync with `nav.rs`, `app.rs` and
/// `docs/keyboard.md` (TUI section).
pub const CHEATSHEET: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("j k  Down Up", "next / previous"),
            ("gg  G", "first / last"),
            ("[  ]", "previous / next section"),
            ("Ctrl+d  Ctrl+u", "5 down / up"),
            ("h  Left", "to section header, collapse; back to left column"),
            ("l  Right", "expand section; to elaborations column"),
        ],
    ),
    (
        "Act",
        &[
            ("Enter  i  a", "answer item / toggle section / jump to elaborated item"),
            ("1-9", "pick option N, then Enter to submit"),
            ("x", "todo done / undone; reopen decision"),
            ("space", "mark / unmark a finished item or elaboration"),
            ("d", "remove marked items, or this one, from the board"),
            ("o  za", "on an elaboration thread: expand / collapse it"),
            ("zR  zM", "expand / collapse all elaborations (or the selected ones)"),
            ("K", "zoom: open the item or thread large"),
            ("Esc", "clear the selection"),
            ("e", "elaborate on the whole item; on a thread, follow up"),
            ("v  V", "select text / select all (visual)"),
            ("za  space", "toggle section (zo / zc open / close)"),
            ("gt  gT", "next / previous tab"),
        ],
    ),
    (
        "Visual (selecting text)",
        &[
            ("h l  w b e", "char / word"),
            ("0  ^  $", "line start / first char / end"),
            ("j k", "row down / up"),
            ("gg  G", "text start / end"),
            ("v", "set / drop selection anchor"),
            ("o", "swap selection ends"),
            ("y", "copy selection (whole text if none)"),
            ("Enter", "elaborate on selection (word under caret if none)"),
            ("Esc", "cancel"),
        ],
    ),
    (
        "Zoom (overlay)",
        &[
            ("j k  Down Up", "scroll"),
            ("Ctrl+d  Ctrl+u", "scroll a page (also PageDown / PageUp)"),
            ("gg  G", "top / bottom"),
            ("n  p", "next / previous item or thread"),
            ("e", "elaborate on the item / follow up on the thread"),
            ("Esc  q", "close"),
        ],
    ),
    (
        "Typing (answers, notes, elaborate message)",
        &[
            ("Enter", "submit"),
            ("Alt+Enter  Ctrl+J", "newline (Shift+Enter too, if the terminal sends it)"),
            ("Esc", "stop typing / cancel elaborate"),
        ],
    ),
    (
        "Anywhere",
        &[
            ("?  F1", "this cheatsheet"),
            ("gt  gT  Alt+1-9", "switch tab"),
            ("Ctrl+W", "close tab"),
            ("q  Ctrl+Q", "quit (asks first; tabs are restored next time)"),
        ],
    ),
];

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.columns.clear();
    app.tab_hits.clear();
    tab_bar(f, app, Rect { height: 1, ..area });
    let mut body = Rect { y: area.y + 1, height: area.height.saturating_sub(2), ..area };
    let status = Rect { y: area.bottom().saturating_sub(1), height: 1, ..area };

    let Some(open) = app.tabs.get_mut(app.active) else {
        status_line(f, app, status);
        let msg = Paragraph::new(vec![
            Line::default(),
            Line::styled("No boards open.", dim()),
            Line::styled("Run: todai-tui FILE, or attach one from an agent (todai attach).", dim()),
        ])
        .centered();
        f.render_widget(msg, body);
        return;
    };
    let tab = &mut open.tab;
    tab.nav.begin_render();
    tab.nav.zoom_line = 1.0;

    // Bottom panels, drawn first so they claim their rows.
    let mut actions = Vec::new();
    if let Some(h) = prompt_height(open, body.width) {
        let h = h.min(body.height / 2);
        let r = Rect { y: body.bottom() - h, height: h, ..body };
        prompt_panel(f, open, r);
        body.height -= h;
    }
    let open = app.tabs.get_mut(app.active).unwrap();
    if !open.tab.nav.marked.is_empty() && body.height > 3 {
        let r = Rect { y: body.bottom() - 1, height: 1, ..body };
        action_bar(f, open, r);
        body.height -= 1;
    }

    let split = body.width >= SPLIT_COLS;
    let tab = &mut open.tab;
    let mut board =
        Board { state: &tab.state, drafts: &mut tab.drafts, nav: &mut tab.nav, edit: &mut open.edit, out: Vec::new() };
    let (left_rect, right_rect) = if split {
        let lw = body.width / 2;
        let l = Rect { width: lw - 1, ..body };
        let r = Rect { x: body.x + lw + 1, width: body.width - lw - 1, ..body };
        // The column title stays put; the threads scroll below it.
        let title = board::header_line("Elaborations", &board::elab_count(&tab.state), None, false, r.width as usize - 1);
        f.render_widget(Paragraph::new(title), Rect { height: 1, ..r });
        (l, Some(Rect { y: r.y + 1, height: r.height.saturating_sub(1), ..r }))
    } else {
        (body, None)
    };
    let right_doc = right_rect.map(|r| board.right_column(r.width as usize - 1));
    let left_doc = board.left_column(left_rect.width as usize - 1, !split);
    actions.extend(board.out);

    let scroll_to = tab.nav.scroll_to_cursor;
    let cursor = tab.nav.cursor.clone();
    let caret_in_right = right_doc.as_ref().is_some_and(|d| d.caret.is_some());
    show_column(f, &mut app.columns, &mut open.scroll[0], left_rect, left_doc, cursor.as_ref(), scroll_to, !caret_in_right);
    if let (Some(r), Some(doc)) = (right_rect, right_doc) {
        let divider = Rect { x: r.x - 1, y: r.y - 1, width: 1, height: r.height + 1 };
        f.render_widget(Block::new().borders(Borders::LEFT).border_style(dim()), divider);
        show_column(f, &mut app.columns, &mut open.scroll[1], r, doc, cursor.as_ref(), scroll_to, true);
    }
    if let Some(p) = app.columns.iter().find_map(|c| c.caret) {
        f.set_cursor_position(p);
    }
    if let Some(p) = open.prompt_caret.take() {
        f.set_cursor_position(p);
    }

    open.tab.nav.end_render();
    if !matches!(open.tab.nav.mode, Mode::Insert | Mode::Prompt) {
        open.edit = None;
    }
    for kind in actions {
        app_send(open, kind);
    }

    status_line(f, app, status);
    let open = app.tabs.get_mut(app.active).unwrap();
    let overlay = open.tab.nav.zoom.is_some() || open.tab.confirm_trash.is_some() || app.confirm_quit || app.cheatsheet.is_some();
    if overlay {
        backdrop(f);
    }
    if open.tab.nav.zoom.is_some() {
        zoom(f, open, area);
    }
    if let Some(ids) = open.tab.confirm_trash.clone() {
        let title = match ids.len() {
            1 => format!("Remove {} from the board?", ids[0]),
            n => format!("Remove {n} items from the board?"),
        };
        let body = format!("{}\nThey are gone from the board for good; the agent is not told.", ids.join(", "));
        confirm(f, area, &title, &body, "Enter/y: remove · Esc/n: keep");
    }
    if app.confirm_quit {
        confirm(f, area, "Quit todai?", "Open tabs come back the next time you run todai-tui.", "Enter/y: quit · Esc/n: stay");
    }
    if let Some(scroll) = app.cheatsheet.as_mut() {
        cheatsheet(f, area, scroll);
    }
}

/// Sends an action from the board; a sent answer clears its draft.
pub fn app_send(open: &mut Open, kind: Kind) {
    let draft_key = match &kind {
        Kind::DecisionResolve { id, .. } | Kind::TodoDone { id, .. } => Some(id.clone()),
        _ => None,
    };
    if open.tab.send(kind)
        && let Some(k) = draft_key
    {
        open.tab.drafts.remove(&k);
    }
}

/// Draws a column's visible slice, scrolled to keep the cursor (or the
/// typing caret) in view, and records it for the mouse.
#[allow(clippy::too_many_arguments)]
fn show_column(
    f: &mut Frame,
    columns: &mut Vec<Column>,
    scroll: &mut usize,
    rect: Rect,
    doc: Doc,
    cursor: Option<&Focus>,
    scroll_to: bool,
    show_caret: bool,
) {
    let h = rect.height as usize;
    let max = doc.lines.len().saturating_sub(h);
    if scroll_to && let Some((a, b)) = cursor.and_then(|c| doc.range_of(c)) {
        // Like vim: move only as far as needed; a tall card shows its top.
        if b - a > h || a < *scroll {
            *scroll = a;
        } else if b > *scroll + h {
            *scroll = b - h;
        }
    }
    if let Some((line, _)) = doc.caret {
        if line < *scroll {
            *scroll = line;
        } else if line >= *scroll + h {
            *scroll = line + 1 - h;
        }
    }
    *scroll = (*scroll).min(max);
    let visible: Vec<Line> = doc.lines.iter().skip(*scroll).take(h).cloned().collect();
    f.render_widget(Paragraph::new(visible), rect);
    // A scroll position marker when the column overflows.
    if doc.lines.len() > h && h > 2 {
        let bar_x = rect.right().saturating_sub(1);
        let pos = (*scroll * (h - 1)) / max.max(1);
        f.buffer_mut()[(bar_x, rect.y + pos as u16)].set_symbol("▐").set_style(dim());
    }
    let caret = doc.caret.filter(|_| show_caret).and_then(|(line, col)| {
        (line >= *scroll && line < *scroll + h).then(|| Position::new(rect.x + col as u16, rect.y + (line - *scroll) as u16))
    });
    columns.push(Column { rect, scroll: *scroll, hits: doc.hits, caret });
}

fn tab_bar(f: &mut Frame, app: &mut App, r: Rect) {
    let mut spans = Vec::new();
    let mut x = r.x;
    for (i, o) in app.tabs.iter().enumerate() {
        let pending = o.tab.state.decisions.iter().filter(|d| d.status == todai_client::store::DecisionStatus::Pending).count()
            + o.tab.state.todos.iter().filter(|t| t.done.is_none()).count();
        let label = match pending {
            0 => format!(" {} ", o.tab.title()),
            n => format!(" {} {n} ", o.tab.title()),
        };
        let style = if i == app.active { Style::new().fg(ACCENT).add_modifier(Modifier::REVERSED | Modifier::BOLD) } else { Style::new() };
        let w = str_width(&label) as u16;
        app.tab_hits.push((x, x + w, i));
        x += w + 1;
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), r);
    let help = " ? keys ";
    let hw = str_width(help) as u16;
    if r.width > x - r.x + hw {
        f.render_widget(Paragraph::new(Span::styled(help, dim())), Rect { x: r.right() - hw, width: hw, ..r });
    }
}

fn status_line(f: &mut Frame, app: &App, r: Rect) {
    let tab = app.tabs.get(app.active).map(|o| &o.tab);
    let mode = match tab.map(|t| (&t.nav, t.nav.mode)) {
        Some((n, _)) if n.zoom.is_some() => "ZOOM",
        Some((_, Mode::Insert)) => "INSERT",
        Some((_, Mode::Prompt)) => "ELABORATE",
        Some((_, Mode::Visual)) => "VISUAL",
        _ => "NORMAL",
    };
    let mut spans = vec![Span::styled(format!(" {mode} "), Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)), Span::raw(" ")];
    if let Some(p) = tab.and_then(|t| t.nav.prefix) {
        spans.push(Span::raw(format!("{p} ")));
    }
    let error = tab.and_then(|t| t.error.clone());
    if let Some(e) = &error {
        spans.push(Span::styled(e.clone(), Style::new().fg(Color::Red)));
    } else if let Some(n) = &app.notice {
        spans.push(Span::styled(n.clone(), dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), r);
    // An error can be long (a remote tab's server is down); it wins the space.
    if error.is_none() && app.notice.is_none() {
        let hint = match tab.map(|t| t.nav.mode) {
            Some(Mode::Insert | Mode::Prompt) => "Enter submit · Alt+Enter newline · Esc stop ",
            _ => "? keys · q quit ",
        };
        let w = str_width(hint) as u16;
        if r.width > w + 12 {
            f.render_widget(Paragraph::new(Span::styled(hint, dim())), Rect { x: r.right() - w, width: w, ..r });
        }
    }
}

/// What you can do with the selection; shown only while something is selected.
fn action_bar(f: &mut Frame, open: &Open, r: Rect) {
    let n = open.tab.nav.marked.len();
    let elabs = open.tab.nav.marked.iter().any(|id| open.tab.state.elab(id).is_some());
    let mut spans = vec![Span::styled(format!(" {n} selected "), Style::new().fg(ACCENT).add_modifier(Modifier::REVERSED | Modifier::BOLD))];
    spans.push(Span::styled("  d remove", Style::new()));
    if elabs {
        spans.push(Span::styled(" · zR expand · zM collapse", Style::new()));
    }
    spans.push(Span::styled(" · Esc deselect", dim()));
    f.render_widget(Paragraph::new(Line::from(spans)), r);
}

const QUOTE_ROWS: usize = 3;
const NOTE_ROWS: usize = 6;

fn prompt_height(open: &Open, width: u16) -> Option<u16> {
    let p = open.tab.nav.prompt.as_ref()?;
    let w = (width as usize).saturating_sub(4).max(1);
    let quote = text::wrap(&text::plain_cells(&p.selection, Style::new()), w).len().min(QUOTE_ROWS);
    let note = text::wrap(&text::plain_cells(&p.note, Style::new()), w).len().clamp(1, NOTE_ROWS);
    Some((1 + quote + note) as u16)
}

/// The elaboration message editor, shown while a request is being written.
fn prompt_panel(f: &mut Frame, open: &mut Open, r: Rect) {
    let (state, nav) = (&open.tab.state, &mut open.tab.nav);
    let Some(p) = nav.prompt.as_mut() else { return };
    if matches!(nav.focus_request, Some(FocusRequest::Prompt)) {
        open.edit = Some(Edit { target: Target::Prompt, caret: p.note.chars().count(), multiline: true });
        nav.focus_request = None;
    }
    let editing = open.edit.as_ref().filter(|e| e.target == Target::Prompt).map(|e| e.caret);
    nav.prompt_focused = editing.is_some();
    let w = (r.width as usize).saturating_sub(4).max(1);

    let mut lines = Vec::new();
    let (what, about) = match &p.parent {
        Some(rid) => (format!("Follow up on {rid}"), format!("in thread {} on {}", state.thread_root(rid), p.target)),
        None => ("Elaborate on".to_owned(), p.target.clone()),
    };
    let title = board::item_title(state, &p.target);
    let head = format!("─ {what} {about} ");
    let rest = text::truncate(title, (r.width as usize).saturating_sub(str_width(&head) + 1));
    lines.push(Line::from(vec![
        Span::styled(head, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(rest, dim()),
    ]));
    let quote = text::plain_cells(&format!("“{}”", p.selection), Style::new().add_modifier(Modifier::ITALIC));
    let mut q = text::wrap_lines(&quote, w, vec![Span::raw("  ")], vec![Span::raw("  ")]);
    if q.len() > QUOTE_ROWS {
        q.truncate(QUOTE_ROWS);
        q[QUOTE_ROWS - 1].spans.push(Span::styled("…", dim()));
    }
    lines.extend(q);
    let top = lines.len();
    let prompt = Span::styled("› ", Style::new().fg(ACCENT));
    if p.note.is_empty() {
        let hint = "message for the agent (optional) · Enter sends · Alt+Enter newline · Esc cancels";
        lines.push(Line::from(vec![prompt, Span::styled(hint, dim().add_modifier(Modifier::ITALIC))]));
        if editing.is_some() {
            open.prompt_caret = Some(Position::new(r.x + 2, r.y + top as u16));
        }
    } else {
        let cells = text::plain_cells(&p.note, Style::new());
        let rows = text::wrap(&cells, w);
        let caret = editing.unwrap_or(0).min(cells.len());
        let crow = rows.iter().rposition(|&(a, _)| a <= caret).unwrap_or(0);
        let ccol: usize = cells[rows[crow].0..caret].iter().map(|c| text::char_width(c.0)).sum();
        // Keep the caret's row in the visible rows.
        let first = crow.saturating_sub(NOTE_ROWS - 1);
        for (i, &(a, b)) in rows.iter().enumerate().skip(first).take(NOTE_ROWS) {
            let lead = if i == 0 { prompt.clone() } else { Span::raw("  ") };
            lines.push(text::row_line(&cells[a..b], vec![lead]));
        }
        if editing.is_some() {
            let y = r.y + (top + crow - first) as u16;
            open.prompt_caret = Some(Position::new(r.x + 2 + ccol as u16, y));
        }
    }
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines), r);
}

/// Dims everything drawn so far, so an overlay stands out like the window's
/// darkened backdrop.
fn backdrop(f: &mut Frame) {
    for cell in f.buffer_mut().content.iter_mut() {
        cell.modifier.insert(Modifier::DIM);
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn confirm(f: &mut Frame, area: Rect, title: &str, body: &str, keys: &str) {
    let w = (str_width(title).max(str_width(keys)).max(40) + 4).min(area.width as usize);
    let mut lines: Vec<Line> = Vec::new();
    for l in body.lines() {
        lines.extend(text::wrap_lines(&text::plain_cells(l, Style::new()), w - 4, vec![], vec![]));
    }
    lines.push(Line::default());
    lines.push(Line::styled(keys.to_owned(), dim()));
    let r = centered(area, w as u16, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    let block = Block::bordered().title(Span::styled(format!(" {title} "), Style::new().add_modifier(Modifier::BOLD))).border_style(Style::new().fg(ACCENT));
    f.render_widget(Paragraph::new(lines).block(block), r);
}

fn cheatsheet(f: &mut Frame, area: Rect, scroll: &mut usize) {
    let key_w = 20;
    let mut lines = Vec::new();
    for (group, keys) in CHEATSHEET {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(group.to_string(), Style::new().add_modifier(Modifier::BOLD)));
        for (k, what) in *keys {
            lines.push(Line::from(vec![Span::styled(format!("  {k:<key_w$}"), Style::new().fg(ACCENT)), Span::raw(what.to_string())]));
        }
    }
    let w = 84.min(area.width);
    let r = centered(area, w, area.height.saturating_sub(2));
    let inner_h = r.height.saturating_sub(2) as usize;
    *scroll = (*scroll).min(lines.len().saturating_sub(inner_h));
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .title(Span::styled(" Keys ", Style::new().add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(" j k scroll · Esc close ", dim()))
        .border_style(Style::new().fg(ACCENT));
    f.render_widget(Paragraph::new(lines.into_iter().skip(*scroll).collect::<Vec<_>>()).block(block), r);
}

/// The zoomed item or elaboration thread, large, over the board. Keys are
/// handled in `Nav` (its zoom mode); this only draws and clamps scrolling.
fn zoom(f: &mut Frame, open: &mut Open, area: Rect) {
    let (state, nav) = (&open.tab.state, &mut open.tab.nav);
    let Some(id) = nav.zoom.clone() else { return };
    let focus = nav::zoom_focus(&id);
    let is_item = matches!(focus, Focus::Item(_));
    let thread = state.thread(&id);
    let target = if is_item { id.clone() } else { thread.first().map(|r| r.target.clone()).unwrap_or_default() };
    if state.decision(&target).is_none() && state.todo(&target).is_none() {
        nav.zoom = None; // trashed while open
        return;
    }
    let order: Vec<String> = if is_item {
        nav.left.iter().filter_map(|f| if let Focus::Item(i) = f { Some(i.clone()) } else { None }).collect()
    } else {
        state.threads().into_iter().map(|t| t[0].rid.clone()).collect()
    };
    let index = order.iter().position(|x| *x == id).unwrap_or(0);

    let w = (area.width as f32 * 0.9).clamp(40.0, 110.0) as u16;
    let h = (area.height as f32 * 0.9) as u16;
    let r = centered(area, w, h.max(8));
    let inner_w = r.width.saturating_sub(4) as usize;
    let body_h = r.height.saturating_sub(2) as usize;
    let mut lines = if is_item {
        board::item_zoom(state, &id, inner_w)
    } else {
        let mut v = Vec::new();
        v.extend(text::wrap_lines(&text::plain_cells(board::item_title(state, &target), Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)), inner_w, vec![], vec![]));
        for (i, t) in thread.iter().enumerate() {
            v.push(Line::default());
            if i > 0 {
                v.push(Line::styled("─".repeat(inner_w.min(30)), dim()));
            }
            v.extend(board::turn_lines(state, t, inner_w));
        }
        v
    };
    let max = lines.len().saturating_sub(body_h);
    nav.zoom_page = body_h.saturating_sub(2).max(1) as f32;
    let offset = match nav.zoom_scroll_to.take() {
        Some(to) => (to.max(0.0) as usize).min(max),
        None => (nav.zoom_offset as usize).min(max),
    };
    nav.zoom_offset = offset as f32;
    let visible: Vec<Line> = lines.drain(..).skip(offset).take(body_h).map(|l| {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(l.spans);
        Line::from(spans)
    }).collect();

    let about = if is_item { String::new() } else { format!(" on {target}") };
    let title = format!(" {id}{about} · {} / {} ", index + 1, order.len());
    let more = (offset * 100).checked_div(max).map(|p| format!(" {p}% ")).unwrap_or_default();
    let action = if is_item { "e elaborate" } else { "e follow up" };
    let hints = format!(" j k scroll · n p next / prev · {action} · Esc close{more}");
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .title(Span::styled(title, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(hints, dim()))
        .border_style(Style::new().fg(ACCENT));
    f.render_widget(Paragraph::new(visible).block(block), r);
}
