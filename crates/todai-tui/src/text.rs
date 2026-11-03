//! Styled text as cells, wrapped into rows. Card text keeps a char index
//! per cell so visual mode (which indexes into the item's text) can draw
//! its selection and move by displayed rows.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use todai_client::cardtext::Part;
use todai_client::nav::{Rows, Weight};
use unicode_width::UnicodeWidthChar;

pub type Cell = (char, Style);

/// Display width of a char; tabs count as 4, control chars as nothing.
pub fn char_width(c: char) -> usize {
    if c == '\t' { 4 } else { c.width().unwrap_or(0) }
}

pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Accent for the cursor, the active tab and links: the terminal's own cyan.
pub const ACCENT: Color = Color::Cyan;
/// Inline code and code blocks.
pub const CODE: Color = Color::Yellow;

pub fn weight_style(w: Weight) -> Style {
    match w {
        Weight::Strong => Style::new().add_modifier(Modifier::BOLD),
        Weight::Normal => Style::new(),
        Weight::Weak => Style::new().add_modifier(Modifier::DIM),
        Weight::Struck => Style::new().add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
    }
}

/// Card parts as cells, styled like the window draws them.
pub fn part_cells(parts: &[Part]) -> Vec<Cell> {
    let mut out = Vec::new();
    for p in parts {
        let mut style = weight_style(p.weight);
        if p.code {
            // Code stays readable inside weak context text.
            style = style.remove_modifier(Modifier::DIM).fg(CODE);
        }
        if p.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        out.extend(p.text.chars().map(|c| (c, style)));
    }
    out
}

pub fn plain_cells(s: &str, style: Style) -> Vec<Cell> {
    s.chars().map(|c| (c, style)).collect()
}

/// Wraps at `width` columns, preferring to break after a space. Returns
/// each row as a char range `[start, end)`; a `'\n'` ends its row and
/// belongs to no row's range (it sits at `end`).
pub fn wrap(cells: &[Cell], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let (mut start, mut i, mut w) = (0, 0, 0);
    let mut after_space = None;
    while i < cells.len() {
        let c = cells[i].0;
        if c == '\n' {
            rows.push((start, i));
            i += 1;
            (start, w, after_space) = (i, 0, None);
            continue;
        }
        let cw = char_width(c);
        if w + cw > width && i > start {
            let brk = after_space.filter(|&b| b > start).unwrap_or(i);
            rows.push((start, brk));
            (start, i, w, after_space) = (brk, brk, 0, None);
            continue;
        }
        w += cw;
        if c == ' ' {
            after_space = Some(i + 1);
        }
        i += 1;
    }
    rows.push((start, cells.len()));
    rows
}

/// One row of cells as a line, merging runs of the same style.
pub fn row_line(cells: &[Cell], prefix: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = prefix;
    let mut run = String::new();
    let mut style = None;
    for &(c, s) in cells {
        if style != Some(s) && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), style.unwrap_or_default()));
        }
        style = Some(s);
        run.push(if c == '\t' { ' ' } else { c });
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, style.unwrap_or_default()));
    }
    Line::from(spans)
}

/// Wraps `cells` into lines of at most `width` columns after the prefixes:
/// `first` on the first row, `rest` on the others (a hanging indent).
pub fn wrap_lines(cells: &[Cell], width: usize, first: Vec<Span<'static>>, rest: Vec<Span<'static>>) -> Vec<Line<'static>> {
    wrap(cells, width)
        .into_iter()
        .enumerate()
        .map(|(i, (a, b))| row_line(&cells[a..b], if i == 0 { first.clone() } else { rest.clone() }))
        .collect()
}

/// Cuts `s` to `width` columns, ending in `…` if it was longer.
pub fn truncate(s: &str, width: usize) -> String {
    if str_width(s) <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Visual mode's `j` / `k`: rows as wrapped last frame, positions in columns.
pub struct WrapRows {
    pub chars: Vec<char>,
    pub rows: Vec<(usize, usize)>,
}

impl WrapRows {
    fn row_of(&self, caret: usize) -> usize {
        self.rows.iter().rposition(|&(a, _)| a <= caret).unwrap_or(0)
    }

    fn col_of(&self, row: usize, caret: usize) -> usize {
        let (a, _) = self.rows[row];
        self.chars[a..caret.max(a)].iter().copied().map(char_width).sum()
    }

    /// The char in `row` at display column `col`, or the row's last char.
    fn at_col(&self, row: usize, col: usize) -> usize {
        let (a, b) = self.rows[row];
        let mut w = 0;
        for i in a..b {
            w += char_width(self.chars[i]);
            if w > col {
                return i;
            }
        }
        // Past the end: the row's newline if it has one, else its last char.
        if b < self.chars.len() || b == a { b } else { b - 1 }
    }

    fn step(&self, caret: usize, h: Option<f32>, d: isize) -> (usize, Option<f32>) {
        let row = self.row_of(caret);
        let col = h.map_or_else(|| self.col_of(row, caret), |h| h as usize);
        let target = row as isize + d;
        if target < 0 || target as usize >= self.rows.len() {
            return (caret, Some(col as f32));
        }
        (self.at_col(target as usize, col), Some(col as f32))
    }
}

impl Rows for WrapRows {
    fn down(&self, caret: usize, h: Option<f32>) -> (usize, Option<f32>) {
        self.step(caret, h, 1)
    }

    fn up(&self, caret: usize, h: Option<f32>) -> (usize, Option<f32>) {
        self.step(caret, h, -1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(s: &str) -> Vec<Cell> {
        plain_cells(s, Style::new())
    }

    fn rows(s: &str, w: usize) -> Vec<String> {
        let c = cells(s);
        wrap(&c, w).into_iter().map(|(a, b)| c[a..b].iter().map(|x| x.0).collect()).collect()
    }

    #[test]
    fn wraps_after_spaces_and_at_newlines() {
        assert_eq!(rows("use egui or iced", 9), ["use egui ", "or iced"]);
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(rows("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(rows("", 10), [""]);
    }

    #[test]
    fn rows_move_by_column() {
        let text = "one two\nthree";
        let r = WrapRows { chars: text.chars().collect(), rows: wrap(&cells(text), 20) };
        let (c, h) = r.down(2, None); // 'e' in "one", column 2
        assert_eq!((c, h), (10, Some(2.0))); // 'r' in "three"
        assert_eq!(r.up(c, h).0, 2);
        assert_eq!(r.down(12, None).0, 12, "last row stays");
        // A short row clamps to its newline.
        let r = WrapRows { chars: "ab\nlonger".chars().collect(), rows: wrap(&cells("ab\nlonger"), 20) };
        assert_eq!(r.up(8, None).0, 2);
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
    }
}
