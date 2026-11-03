//! Markdown (elaboration answers, item context in zoom) as wrapped terminal
//! lines: headings, paragraphs, lists, quotes, code blocks and tables. Board
//! text goes in as data only; nothing in it is interpreted beyond markdown.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::text::{self, ACCENT, CODE, Cell, str_width};

/// Renders `src` into lines at most `width` columns wide, with `base` as the
/// style of ordinary text.
pub fn render(src: &str, width: usize, base: Style) -> Vec<Line<'static>> {
    let mut r = Renderer {
        width: width.max(8),
        base,
        out: Vec::new(),
        cells: Vec::new(),
        styles: vec![base],
        lists: Vec::new(),
        quote: 0,
        pending_bullet: None,
        code: None,
        table: None,
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(src, opts) {
        r.event(ev);
    }
    r.flush();
    while r.out.last().is_some_and(|l| l.width() == 0) {
        r.out.pop();
    }
    r.out
}

struct Table {
    align: Vec<Alignment>,
    rows: Vec<Vec<String>>,
    cell: String,
    header_rows: usize,
}

struct Renderer {
    width: usize,
    base: Style,
    out: Vec<Line<'static>>,
    /// The paragraph, heading or list item being collected.
    cells: Vec<Cell>,
    styles: Vec<Style>,
    /// Next number of each open list, `None` for bullets.
    lists: Vec<Option<u64>>,
    quote: usize,
    /// The marker for the next text in a list item ("• ", "3. ").
    pending_bullet: Option<String>,
    code: Option<String>,
    table: Option<Table>,
}

impl Renderer {
    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let s = f(self.style());
        self.styles.push(s);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn indent(&self) -> usize {
        self.lists.len().saturating_sub(1) * 2
    }

    /// The prefix every line of the current block gets: quote bars, list indent.
    fn prefix(&self, first: bool) -> Vec<Span<'static>> {
        let mut v = Vec::new();
        if self.quote > 0 {
            v.push(Span::styled("▎ ".repeat(self.quote), Style::new().add_modifier(Modifier::DIM)));
        }
        if !self.lists.is_empty() {
            let bullet_w = self.pending_bullet.as_deref().map_or(2, str_width);
            v.push(Span::raw(" ".repeat(self.indent())));
            match (&self.pending_bullet, first) {
                (Some(b), true) => v.push(Span::styled(b.clone(), Style::new().fg(ACCENT))),
                _ => v.push(Span::raw(" ".repeat(bullet_w))),
            }
        }
        v
    }

    fn blank(&mut self) {
        if self.out.last().is_some_and(|l| l.width() > 0) {
            if self.quote > 0 {
                self.out.push(Line::from(self.prefix(false)));
            } else {
                self.out.push(Line::default());
            }
        }
    }

    /// Emits the collected inline text as a wrapped block.
    fn flush(&mut self) {
        if self.cells.is_empty() {
            return;
        }
        let cells = std::mem::take(&mut self.cells);
        let (first, rest) = (self.prefix(true), self.prefix(false));
        let w = self.width.saturating_sub(first.iter().map(|s| s.width()).sum());
        self.out.extend(text::wrap_lines(&cells, w, first, rest));
        self.pending_bullet = None;
    }

    fn text(&mut self, s: &str) {
        if let Some(t) = &mut self.table {
            t.cell.push_str(s);
        } else if let Some(c) = &mut self.code {
            c.push_str(s);
        } else {
            let st = self.style();
            self.cells.extend(s.chars().map(|c| (c, st)));
        }
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                if let Some(tb) = &mut self.table {
                    tb.cell.push_str(&t);
                } else {
                    let st = self.style().remove_modifier(Modifier::DIM).fg(CODE);
                    self.cells.extend(t.chars().map(|c| (c, st)));
                }
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => {
                let st = self.style();
                self.cells.push(('\n', st));
            }
            Event::Rule => {
                self.flush();
                self.blank();
                self.out.push(Line::styled("─".repeat(self.width.min(40)), Style::new().add_modifier(Modifier::DIM)));
                self.out.push(Line::default());
            }
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
            Event::Html(h) | Event::InlineHtml(h) => self.text(&h),
            Event::FootnoteReference(f) => self.text(&format!("[{f}]")),
            Event::InlineMath(m) | Event::DisplayMath(m) => self.text(&m),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.blank();
                let underline = matches!(level, HeadingLevel::H1 | HeadingLevel::H2);
                self.push_style(|s| {
                    let s = s.add_modifier(Modifier::BOLD).remove_modifier(Modifier::DIM);
                    if underline { s.add_modifier(Modifier::UNDERLINED) } else { s }
                });
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.blank();
                self.quote += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.blank();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) if !l.is_empty() => l.to_string(),
                    _ => String::new(),
                };
                if !lang.is_empty() {
                    self.out.push(Line::styled(format!("  {lang}"), Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC)));
                }
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                if self.lists.is_empty() {
                    self.blank();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_owned(),
                };
                self.pending_bullet = Some(marker);
            }
            Tag::Table(align) => {
                self.flush();
                self.blank();
                self.table = Some(Table { align, rows: Vec::new(), cell: String::new(), header_rows: 0 });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {}
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } => self.push_style(|s| s.fg(ACCENT).add_modifier(Modifier::UNDERLINED)),
            Tag::Image { .. } => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.pop_style();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                let style = Style::new().fg(CODE);
                let bar = Span::styled("│ ", Style::new().add_modifier(Modifier::DIM));
                let w = self.width.saturating_sub(2 + self.indent());
                for l in code.trim_end_matches('\n').split('\n') {
                    let cells = text::plain_cells(l, style);
                    let pad = Span::raw(" ".repeat(self.indent()));
                    self.out.extend(text::wrap_lines(&cells, w, vec![pad.clone(), bar.clone()], vec![pad, bar.clone()]));
                }
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => self.flush(),
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.header_rows = t.rows.len();
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = &mut self.table {
                    let cell = std::mem::take(&mut t.cell);
                    if let Some(r) = t.rows.last_mut() {
                        r.push(cell.trim().to_owned());
                    }
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.table(t);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image => self.pop_style(),
            _ => {}
        }
    }

    /// Columns sized to their widest cell, shrunk (widest first) to fit;
    /// cells that still don't fit are cut with `…`.
    fn table(&mut self, t: Table) {
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![1; cols];
        for r in &t.rows {
            for (i, c) in r.iter().enumerate() {
                widths[i] = widths[i].max(str_width(c));
            }
        }
        let sep = 3; // " │ "
        let avail = self.width.saturating_sub(sep * (cols - 1));
        while widths.iter().sum::<usize>() > avail {
            let (i, &w) = widths.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
            if w <= 3 {
                break;
            }
            widths[i] = w - 1;
        }
        let dim = Style::new().add_modifier(Modifier::DIM);
        for (ri, r) in t.rows.iter().enumerate() {
            let header = ri < t.header_rows;
            let style = if header { self.base.add_modifier(Modifier::BOLD) } else { self.base };
            let mut spans = Vec::new();
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" │ ", dim));
                }
                let c = text::truncate(r.get(i).map_or("", String::as_str), *w);
                let pad = w.saturating_sub(str_width(&c));
                let (l, rt) = match t.align.get(i) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                spans.push(Span::styled(format!("{}{c}{}", " ".repeat(l), " ".repeat(rt)), style));
            }
            self.out.push(Line::from(spans));
            if header && ri + 1 == t.header_rows {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                self.out.push(Line::styled(rule.join("─┼─"), dim));
            }
        }
    }
}

/// The first couple of lines of an answer as plain text, for collapsed cards.
pub fn preview(src: &str) -> String {
    src.lines()
        .map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' ']).replace(['*', '`'], ""))
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(src: &str, w: usize) -> Vec<String> {
        render(src, w, Style::new()).iter().map(|l| l.to_string().trim_end().to_owned()).collect()
    }

    #[test]
    fn blocks_render_readably() {
        let src = "# Short\n\nPostgres. It fits.\n\n- one\n- two\n  1. nested\n\n```sql\nSELECT 1;\n```\n\n> quoted";
        assert_eq!(
            plain(src, 40),
            ["Short", "", "Postgres. It fits.", "", "• one", "• two", "  1. nested", "", "  sql", "│ SELECT 1;", "", "▎ quoted"]
        );
    }

    #[test]
    fn tables_align_and_fit() {
        let src = "| a | long header |\n|---|--:|\n| x | 1 |";
        assert_eq!(plain(src, 40), ["a │ long header", "──┼────────────", "x │           1"]);
        let narrow = plain("| aaaaaaaa | bbbbbbbb |\n|-|-|\n| c | d |", 12);
        assert!(narrow.iter().all(|l| str_width(l) <= 12), "{narrow:?}");
    }

    #[test]
    fn long_paragraphs_wrap() {
        let lines = plain("alpha beta gamma delta", 12);
        assert_eq!(lines, ["alpha beta", "gamma delta"]);
    }
}
