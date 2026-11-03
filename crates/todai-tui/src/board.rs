//! The board as terminal lines: the accordion of pending and done items, and
//! the elaboration threads (a right column when wide, a section when not).
//! Like the window's renderer, it records every focusable thing into `Nav`
//! in display order, so the shared navigation works the same.
//!
//! Cards are compact rows (no borders): a cursor bar in the gutter, the id,
//! the text with a hanging indent, then options and the answer field.

use std::collections::HashMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use todai_client::nav::{self, Focus, FocusRequest, Nav, Section};
use todai_client::store::{Decision, DecisionStatus, Elab, Kind, State, Todo};

use crate::edit::{Edit, Target};
use crate::markdown;
use crate::text::{self, ACCENT, Cell, WrapRows, str_width};

/// Something clickable, by line and column range within its column.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    /// Puts the cursor on the card (and toggles a section header).
    Focus(Focus),
    Option { id: String, n: usize },
    Mark(String),
    Fold(String),
}

/// One rendered column.
#[derive(Default)]
pub struct Doc {
    pub lines: Vec<Line<'static>>,
    /// Line range `[start, end)` of each focusable thing.
    pub cards: Vec<(Focus, usize, usize)>,
    /// Where the typing caret is: line and column.
    pub caret: Option<(usize, usize)>,
    pub hits: Vec<(usize, usize, usize, Hit)>,
}

impl Doc {
    pub fn range_of(&self, f: &Focus) -> Option<(usize, usize)> {
        self.cards.iter().find(|(x, ..)| x == f).map(|&(_, a, b)| (a, b))
    }

    fn push(&mut self, l: Line<'static>) {
        self.lines.push(l);
    }

    fn hit(&mut self, col: usize, width: usize, h: Hit) {
        self.hits.push((self.lines.len(), col, col + width, h));
    }
}

pub struct Board<'a> {
    pub state: &'a State,
    pub drafts: &'a mut HashMap<String, String>,
    pub nav: &'a mut Nav,
    pub edit: &'a mut Option<Edit>,
    pub out: Vec<Kind>,
}

const GUTTER: usize = 2;

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

impl Board<'_> {
    pub fn left_column(&mut self, width: usize, with_elabs: bool) -> Doc {
        let state = self.state;
        let mut doc = Doc::default();
        let pending: Vec<_> = state.decisions.iter().filter(|d| d.status == DecisionStatus::Pending).collect();
        let todos: Vec<_> = state.todos.iter().filter(|t| t.done.is_none()).collect();
        // Withdrawn decisions stay visible, struck through, so the history is complete.
        let made: Vec<_> = state.decisions.iter().filter(|d| d.status != DecisionStatus::Pending).collect();
        let done: Vec<_> = state.todos.iter().filter(|t| t.done.is_some()).collect();

        let count = (pending.len() + todos.len()).to_string();
        if self.section(&mut doc, width, Section::Pending, "Pending", &count) {
            subheading(&mut doc, "Decisions");
            if pending.is_empty() {
                note(&mut doc, "Nothing to decide.");
            }
            for d in &pending {
                self.decision_card(&mut doc, width, d);
            }
            doc.push(Line::default());
            subheading(&mut doc, "Todos");
            if todos.is_empty() {
                note(&mut doc, "Nothing to do by hand.");
            }
            for t in &todos {
                self.todo_card(&mut doc, width, t);
            }
            doc.push(Line::default());
        }

        if with_elabs {
            let count = elab_count(state);
            if self.section(&mut doc, width, Section::Elabs, "Elaborations", &count) {
                self.threads(&mut doc, width, false);
                doc.push(Line::default());
            }
        }

        self.nav.marked.retain(|id| state.can_trash(id));
        let count = (made.len() + done.len()).to_string();
        if self.section(&mut doc, width, Section::Done, "Done", &count) {
            self.nav.mark_groups.push(made.iter().map(|d| d.id.clone()).chain(done.iter().map(|t| t.id.clone())).collect());
            subheading(&mut doc, "Decisions made");
            if made.is_empty() {
                note(&mut doc, "None yet.");
            }
            for d in &made {
                self.resolved_card(&mut doc, width, d);
            }
            doc.push(Line::default());
            subheading(&mut doc, "Done");
            if done.is_empty() {
                note(&mut doc, "None yet.");
            }
            for t in &done {
                self.done_card(&mut doc, width, t);
            }
        }
        doc
    }

    /// The elaborations column, below its fixed header (see `ui`).
    pub fn right_column(&mut self, width: usize) -> Doc {
        let mut doc = Doc::default();
        self.threads(&mut doc, width, true);
        doc
    }

    fn threads(&mut self, doc: &mut Doc, width: usize, right: bool) {
        let state = self.state;
        self.nav.marked.retain(|id| state.can_trash(id));
        if state.elabs.is_empty() {
            note(doc, "No elaborations requested.");
            return;
        }
        let threads = state.threads();
        self.nav.mark_groups.push(threads.iter().map(|t| t[0].rid.clone()).collect());
        for thread in threads {
            self.thread_card(doc, width, &thread, right);
        }
    }

    // ---- cards -----------------------------------------------------------

    /// Starts a card: records it for navigation and returns whether the
    /// cursor is on it.
    fn begin(&mut self, doc: &mut Doc, focus: &Focus, right: bool) -> (bool, usize) {
        if right {
            self.nav.right.push(focus.clone());
        } else {
            self.nav.left.push(focus.clone());
        }
        (self.nav.cursor.as_ref() == Some(focus), doc.lines.len())
    }

    fn end(&mut self, doc: &mut Doc, focus: Focus, start: usize, cursor: bool) {
        let end = doc.lines.len();
        // Every card line starts with blank gutter columns; the cursor card
        // draws its bar there.
        if cursor {
            for l in &mut doc.lines[start..end] {
                let Some(first) = l.spans.first() else { continue };
                let Some(rest) = first.content.strip_prefix("  ").map(str::to_owned) else { continue };
                l.spans[0] = Span::styled("▌ ", Style::new().fg(ACCENT));
                if !rest.is_empty() {
                    l.spans.insert(1, Span::raw(rest));
                }
            }
        }
        for line in start..end {
            doc.hits.push((line, 0, usize::MAX, Hit::Focus(focus.clone())));
        }
        doc.cards.push((focus, start, end));
    }

    /// Gutter, optional checkbox and id; returns the spans and their width.
    fn lead(&mut self, doc: &mut Doc, id: &str, markable: bool) -> (Vec<Span<'static>>, usize) {
        let mut spans = vec![Span::raw("  ")];
        let mut w = GUTTER;
        if markable {
            let marked = self.nav.marked.contains(id);
            doc.hit(w, 3, Hit::Mark(id.to_owned()));
            spans.push(Span::styled(if marked { "[x] " } else { "[ ] " }, if marked { Style::new().fg(ACCENT) } else { dim() }));
            w += 4;
        }
        let id_w = str_width(id).max(3) + 1;
        spans.push(Span::styled(format!("{id:<id_w$}"), dim()));
        (spans, w + id_w)
    }

    /// An item's title and context, or its visual-mode selection.
    fn item_text(&mut self, doc: &mut Doc, width: usize, focus: &Focus, lead: Vec<Span<'static>>, indent: usize) {
        let parts = nav::item_parts(self.state, focus);
        let mut cells = text::part_cells(&parts);
        let w = width.saturating_sub(indent);
        if let Some(v) = self.nav.visual.as_mut().filter(|v| &v.target == focus) {
            highlight(&mut cells, v.range(), v.caret);
            let rows = text::wrap(&cells, w);
            v.rows = Some(Box::new(WrapRows { chars: cells.iter().map(|c| c.0).collect(), rows: rows.clone() }));
            let pad = Span::raw(" ".repeat(indent));
            for (i, (a, b)) in rows.into_iter().enumerate() {
                let prefix = if i == 0 { lead.clone() } else { vec![Span::raw("  "), pad.clone()] };
                let mut line = text::row_line(&cells[a..b], prefix);
                // The caret on a newline: show it as a block at the row's end.
                if v.caret == b && cells.get(b).is_some_and(|c| c.0 == '\n') {
                    line.spans.push(Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)));
                }
                doc.push(line);
            }
            return;
        }
        let rest = vec![Span::raw("  "), Span::raw(" ".repeat(indent - GUTTER))];
        for l in text::wrap_lines(&cells, w, lead, rest) {
            doc.push(l);
        }
    }

    fn decision_card(&mut self, doc: &mut Doc, width: usize, d: &Decision) {
        let focus = Focus::Item(d.id.clone());
        let (cursor, start) = self.begin(doc, &focus, false);
        let (lead, indent) = self.lead(doc, &d.id, false);
        self.item_text(doc, width, &focus, lead, indent);
        if !d.options.is_empty() {
            self.options(doc, width, indent, d);
        }
        let hint = if cursor { "i answer · 1-9 pick · e elaborate · K zoom" } else { "" };
        self.field(doc, width, indent, &d.id, cursor, true, hint);
        self.end(doc, focus, start, cursor);
    }

    fn todo_card(&mut self, doc: &mut Doc, width: usize, t: &Todo) {
        let focus = Focus::Item(t.id.clone());
        let (cursor, start) = self.begin(doc, &focus, false);
        let (lead, indent) = self.lead(doc, &t.id, false);
        self.item_text(doc, width, &focus, lead, indent);
        let hint = if cursor { "x done · i add a note first · e elaborate · K zoom" } else { "" };
        self.field(doc, width, indent, &t.id, cursor, false, hint);
        self.end(doc, focus, start, cursor);
    }

    /// Option chips, wrapped; the one matching the draft is highlighted.
    fn options(&mut self, doc: &mut Doc, width: usize, indent: usize, d: &Decision) {
        let draft = self.drafts.get(&d.id).cloned().unwrap_or_default();
        let pad = || vec![Span::raw(" ".repeat(indent))];
        let mut line = pad();
        let mut col = indent;
        for (i, o) in d.options.iter().enumerate() {
            let num = if i < 9 { format!("{} ", i + 1) } else { String::new() };
            let label = text::truncate(o, width.saturating_sub(indent + 4).max(4));
            let w = str_width(&num) + str_width(&label);
            if col > indent && col + w > width {
                doc.push(Line::from(std::mem::replace(&mut line, pad())));
                col = indent;
            }
            doc.hit(col, w, Hit::Option { id: d.id.clone(), n: i });
            if draft == *o {
                // The picked option, as in the window's selected chip.
                line.push(Span::styled(format!("{num}{label}"), Style::new().fg(ACCENT).add_modifier(Modifier::REVERSED)));
            } else {
                line.push(Span::styled(num, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)));
                line.push(Span::raw(label));
            }
            line.push(Span::raw("   "));
            col += w + 3;
        }
        doc.push(Line::from(line));
    }

    /// The answer field: shown on the cursor card, while typing, or when it
    /// holds a draft. Applies a pending focus request for it.
    #[allow(clippy::too_many_arguments)]
    fn field(&mut self, doc: &mut Doc, width: usize, indent: usize, id: &str, cursor: bool, multiline: bool, hint: &str) {
        if let Some(FocusRequest::Input(want)) = &self.nav.focus_request
            && want == id
        {
            let caret = self.drafts.get(id).map_or(0, |d| d.chars().count());
            *self.edit = Some(Edit { target: Target::Draft(id.to_owned()), caret, multiline });
            self.nav.focus_request = None;
        }
        let editing = self.edit.as_ref().filter(|e| e.target == Target::Draft(id.to_owned())).map(|e| e.caret);
        if editing.is_some() {
            self.nav.input_focused = Some(id.to_owned());
        }
        let draft = self.drafts.get(id).cloned().unwrap_or_default();
        if !cursor && editing.is_none() && draft.is_empty() {
            return;
        }
        let pad = " ".repeat(indent.saturating_sub(2));
        let prompt = Span::styled("› ", if editing.is_some() { Style::new().fg(ACCENT) } else { dim() });
        if draft.is_empty() && editing.is_none() {
            doc.push(Line::from(vec![Span::raw(pad), prompt, Span::styled(hint.to_owned(), dim().add_modifier(Modifier::ITALIC))]));
            return;
        }
        if draft.is_empty() {
            let placeholder = if multiline { "resolution · Enter submits · Alt+Enter newline · Esc stops" } else { "note for the agent (optional) · Enter: done · Esc stops" };
            doc.caret = Some((doc.lines.len(), indent));
            doc.push(Line::from(vec![Span::raw(pad), prompt, Span::styled(placeholder, dim().add_modifier(Modifier::ITALIC))]));
            return;
        }
        let cells = text::plain_cells(&draft, Style::new());
        let first = vec![Span::raw(pad.clone()), prompt];
        let rest = vec![Span::raw(" ".repeat(indent))];
        let w = width.saturating_sub(indent).max(1);
        let top = doc.lines.len();
        if let Some(c) = editing {
            doc.caret = Some(caret_pos(&cells, w, c, top, indent));
        }
        for l in text::wrap_lines(&cells, w, first, rest) {
            doc.push(l);
        }
    }

    fn resolved_card(&mut self, doc: &mut Doc, width: usize, d: &Decision) {
        let focus = Focus::Item(d.id.clone());
        let (cursor, start) = self.begin(doc, &focus, false);
        let (lead, indent) = self.lead(doc, &d.id, true);
        self.item_text(doc, width, &focus, lead, indent);
        let pad = Span::raw(" ".repeat(indent));
        match &d.status {
            DecisionStatus::Resolved { text: t, .. } => {
                let cells = text::plain_cells(t, bold());
                let first = vec![Span::raw(" ".repeat(indent - 2)), Span::styled("» ", dim())];
                for l in text::wrap_lines(&cells, width.saturating_sub(indent), first, vec![pad]) {
                    doc.push(l);
                }
            }
            _ => doc.push(Line::from(vec![pad, Span::styled("withdrawn by the agent", dim().add_modifier(Modifier::ITALIC))])),
        }
        if cursor {
            let hint = if matches!(d.status, DecisionStatus::Resolved { .. }) { "x reopen · space select · d remove" } else { "space select · d remove" };
            hint_line(doc, indent, hint);
        }
        self.end(doc, focus, start, cursor);
    }

    fn done_card(&mut self, doc: &mut Doc, width: usize, t: &Todo) {
        let focus = Focus::Item(t.id.clone());
        let (cursor, start) = self.begin(doc, &focus, false);
        let (lead, indent) = self.lead(doc, &t.id, true);
        self.item_text(doc, width, &focus, lead, indent);
        if let Some(Some(n)) = &t.done {
            let cells = text::plain_cells(&format!("note: {n}"), dim());
            let pad = vec![Span::raw(" ".repeat(indent))];
            for l in text::wrap_lines(&cells, width.saturating_sub(indent), pad.clone(), pad) {
                doc.push(l);
            }
        }
        if cursor {
            hint_line(doc, indent, "x undo · space select · d remove");
        }
        self.end(doc, focus, start, cursor);
    }

    fn thread_card(&mut self, doc: &mut Doc, width: usize, thread: &[&Elab], right: bool) {
        let state = self.state;
        let root = thread[0];
        let focus = Focus::Elab(root.rid.clone());
        let (cursor, start) = self.begin(doc, &focus, right);
        let open = self.nav.elab_is_open(state, &root.rid);
        let waiting = thread.iter().any(|r| r.answer.is_none());

        let (mut lead, indent) = self.lead(doc, &root.rid, true);
        let mut info = format!("on {}", root.target);
        if thread.len() > 1 {
            info += &format!(" · {} turns", thread.len());
        }
        if waiting {
            info += " · waiting";
        }
        let used = indent + str_width(&info) + 2;
        lead.push(Span::styled(info, if waiting { Style::new().fg(ACCENT) } else { dim() }));
        let title = text::truncate(nav_title(state, &root.target), width.saturating_sub(used + 2));
        lead.push(Span::raw("  "));
        lead.push(Span::styled(title, dim()));
        doc.hit(GUTTER, indent - GUTTER, Hit::Fold(root.rid.clone()));
        doc.push(Line::from(lead));

        // The body uses most of the width: answers are often long.
        let indent = GUTTER + 2;
        let pad = Span::raw(" ".repeat(indent));
        let w = width.saturating_sub(indent);
        if !open {
            let quote = text::truncate(&format!("“{}”", root.selection), w);
            doc.push(Line::from(vec![pad.clone(), Span::styled(quote, Style::new().add_modifier(Modifier::ITALIC))]));
            // The newest answer as a two-line preview.
            if let Some(a) = thread.iter().rev().find_map(|r| r.answer.as_deref()) {
                let cells = text::plain_cells(&markdown::preview(a), dim());
                let rows = text::wrap(&cells, w);
                for (i, &(a, b)) in rows.iter().take(2).enumerate() {
                    let mut l = text::row_line(&cells[a..b], vec![pad.clone()]);
                    if i == 1 && rows.len() > 2 {
                        l.spans.push(Span::styled("…", dim()));
                    }
                    doc.push(l);
                }
            }
            self.end(doc, focus, start, cursor);
            return;
        }
        if self.nav.visual.as_ref().is_some_and(|v| v.target == focus) {
            self.item_text(doc, width, &focus, vec![Span::raw("  "), Span::raw(" ".repeat(indent - GUTTER))], indent);
        } else {
            for (i, r) in thread.iter().enumerate() {
                if i > 0 {
                    doc.push(Line::from(vec![pad.clone(), Span::styled("─".repeat(w.min(30)), dim())]));
                }
                for l in turn_lines(state, r, w) {
                    let mut spans = vec![pad.clone()];
                    spans.extend(l.spans);
                    doc.push(Line::from(spans));
                }
            }
        }
        if cursor {
            let hint = if thread.last().is_some_and(|r| r.answer.is_some()) { "e follow up · v select · o fold · K zoom · Enter: item" } else { "o fold · K zoom · Enter: item" };
            hint_line(doc, indent, hint);
        }
        self.end(doc, focus, start, cursor);
    }

    // ---- sections --------------------------------------------------------

    /// A section header; returns whether its body should render.
    fn section(&mut self, doc: &mut Doc, width: usize, s: Section, name: &str, count: &str) -> bool {
        let focus = Focus::Section(s);
        self.nav.left.push(focus.clone());
        let open = self.nav.is_open(s);
        let cursor = self.nav.cursor.as_ref() == Some(&focus);
        let start = doc.lines.len();
        doc.hits.push((start, 0, usize::MAX, Hit::Focus(focus.clone())));
        doc.push(header_line(name, count, Some(open), cursor, width));
        doc.cards.push((focus, start, start + 1));
        open
    }
}

/// The caret's line and column for char `caret` in `cells` wrapped at `w`,
/// with the text starting at line `top`, column `indent`.
fn caret_pos(cells: &[Cell], w: usize, caret: usize, top: usize, indent: usize) -> (usize, usize) {
    let rows = text::wrap(cells, w);
    let row = rows.iter().rposition(|&(a, _)| a <= caret).unwrap_or(0);
    let (a, _) = rows[row];
    let col: usize = cells[a..caret.min(cells.len())].iter().map(|c| text::char_width(c.0)).sum();
    // A caret just past a full row goes to the start of the next one.
    if col >= w { (top + row + 1, indent) } else { (top + row, indent + col) }
}

/// Selection (reversed) and caret (reversed, underlined) over card text.
fn highlight(cells: &mut [Cell], range: Option<(usize, usize)>, caret: usize) {
    for (i, (_, s)) in cells.iter_mut().enumerate() {
        if range.is_some_and(|(a, b)| (a..=b).contains(&i)) {
            *s = s.add_modifier(Modifier::REVERSED);
        }
        if i == caret {
            *s = s.fg(ACCENT).add_modifier(Modifier::REVERSED | Modifier::UNDERLINED);
        }
    }
}

fn nav_title<'s>(state: &'s State, id: &str) -> &'s str {
    state
        .decision(id)
        .map(|d| d.title.as_str())
        .or_else(|| state.todo(id).map(|t| t.title.as_str()))
        .unwrap_or("")
}

pub fn item_title<'s>(state: &'s State, id: &str) -> &'s str {
    nav_title(state, id)
}

/// One turn of a thread: what the operator asked about, their note, and the
/// agent's answer. A follow-up on a whole answer doesn't repeat it.
pub fn turn_lines(state: &State, r: &Elab, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let parent_answer = r.parent.as_deref().and_then(|p| state.elab(p)).and_then(|p| p.answer.as_deref());
    let whole = parent_answer.is_some_and(|a| a.trim() == r.selection.trim());
    let mut first = Vec::new();
    if r.parent.is_some() {
        first.push(Span::styled(format!("{} ", r.rid), dim()));
    }
    let quote = if whole { "follow-up on the answer above".to_owned() } else { format!("“{}”", r.selection) };
    let style = if whole { dim().add_modifier(Modifier::ITALIC) } else { Style::new().add_modifier(Modifier::ITALIC) };
    out.extend(text::wrap_lines(&text::plain_cells(&quote, style), width.saturating_sub(first.iter().map(|s| s.width()).sum()), first, vec![]));
    if let Some(n) = &r.note {
        out.extend(text::wrap_lines(&text::plain_cells(n, bold()), width, vec![], vec![]));
    }
    match &r.answer {
        Some(a) => {
            out.push(Line::default());
            out.extend(markdown::render(a, width, Style::new()));
        }
        None => out.push(Line::styled("⋯ waiting for the agent's answer", Style::new().fg(ACCENT).add_modifier(Modifier::ITALIC))),
    }
    out
}

/// An item in the zoom overlay: title, status, context as markdown, options,
/// and the elaboration threads about it.
pub fn item_zoom(state: &State, id: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let title = text::plain_cells(item_title(state, id), bold().add_modifier(Modifier::UNDERLINED));
    out.extend(text::wrap_lines(&title, width, vec![], vec![]));
    let status = match (state.decision(id), state.todo(id)) {
        (Some(d), _) => match &d.status {
            DecisionStatus::Pending => "decision · pending".to_owned(),
            DecisionStatus::Resolved { text, .. } => format!("decision · resolved: {text}"),
            DecisionStatus::Withdrawn => "decision · withdrawn by the agent".to_owned(),
        },
        (_, Some(t)) => match &t.done {
            None => "todo · pending".to_owned(),
            Some(None) => "todo · done".to_owned(),
            Some(Some(n)) => format!("todo · done: {n}"),
        },
        _ => return out,
    };
    out.extend(text::wrap_lines(&text::plain_cells(&status, dim()), width, vec![], vec![]));
    let (context, options) = match (state.decision(id), state.todo(id)) {
        (Some(d), _) => (d.context.as_deref(), d.options.as_slice()),
        (_, Some(t)) => (t.context.as_deref(), &[][..]),
        _ => (None, &[][..]),
    };
    if let Some(c) = context {
        out.push(Line::default());
        out.extend(markdown::render(c, width, Style::new()));
    }
    if !options.is_empty() {
        out.push(Line::default());
        out.push(Line::styled("Options", bold()));
        for (i, o) in options.iter().enumerate() {
            let first = vec![Span::styled(format!("{}. ", i + 1), Style::new().fg(ACCENT))];
            out.extend(text::wrap_lines(&text::plain_cells(o, Style::new()), width.saturating_sub(3), first, vec![Span::raw("   ")]));
        }
    }
    let mut threads = state.threads();
    threads.retain(|t| t[0].target == id);
    if threads.is_empty() {
        return out;
    }
    out.push(Line::default());
    out.push(Line::styled("Elaborations", bold()));
    for thread in threads.iter().rev() {
        out.push(Line::styled(format!("── {} ", thread[0].rid), dim()));
        for (i, r) in thread.iter().enumerate() {
            if i > 0 {
                out.push(Line::styled("─".repeat(width.min(30)), dim()));
            }
            out.extend(turn_lines(state, r, width));
        }
    }
    out
}

/// "3" or "3 · 1 waiting".
pub fn elab_count(state: &State) -> String {
    let threads = state.threads().len();
    match state.elabs.iter().filter(|r| r.answer.is_none()).count() {
        0 => threads.to_string(),
        n => format!("{threads} · {n} waiting"),
    }
}

/// A section row: fold marker (when `open` is given), uppercase name, count
/// on the right. Reversed under the cursor.
pub fn header_line(name: &str, count: &str, open: Option<bool>, cursor: bool, width: usize) -> Line<'static> {
    let marker = match open {
        Some(true) => "▾ ",
        Some(false) => "▸ ",
        None => "",
    };
    let left = format!("{marker}{}", name.to_uppercase());
    let gap = width.saturating_sub(str_width(&left) + str_width(count) + 1).max(1);
    let style = if cursor { bold().fg(ACCENT).add_modifier(Modifier::REVERSED) } else { bold().add_modifier(Modifier::UNDERLINED) };
    Line::from(vec![Span::styled(format!("{left}{}{count} ", " ".repeat(gap)), style)])
}

fn subheading(doc: &mut Doc, s: &str) {
    doc.push(Line::from(vec![Span::raw("  "), Span::styled(s.to_owned(), bold())]));
}

fn note(doc: &mut Doc, s: &str) {
    doc.push(Line::from(vec![Span::raw("    "), Span::styled(s.to_owned(), dim().add_modifier(Modifier::ITALIC))]));
}

fn hint_line(doc: &mut Doc, indent: usize, hint: &str) {
    doc.push(Line::from(vec![Span::raw(" ".repeat(indent)), Span::styled(hint.to_owned(), dim().add_modifier(Modifier::ITALIC))]));
}
