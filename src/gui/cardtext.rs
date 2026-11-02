//! Markdown in a card's title and context, rendered into styled runs of one
//! plain string. The card stays a single selectable text, and visual mode and
//! mouse selection index into what is shown, so markers (backticks, `**`,
//! fences) are dropped rather than drawn. Line breaks are kept as written:
//! agents often write plain lines, which markdown would join into one
//! paragraph.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::nav::Weight;

/// A run of card text with one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub text: String,
    pub weight: Weight,
    /// Inline code or a code block: monospace on the code background.
    pub code: bool,
    pub italic: bool,
}

impl Part {
    pub fn plain(text: impl Into<String>, weight: Weight) -> Part {
        Part { text: text.into(), weight, code: false, italic: false }
    }
}

/// Renders `src` with `base` as the weight of ordinary text.
pub fn render(src: &str, base: Weight) -> Vec<Part> {
    let mut r = Renderer { src, base, out: Vec::new(), strong: 0, italic: 0, heading: false, code_block: None,
        lists: Vec::new(), last_end: 0, block_start: None, after_prefix: false };
    for (event, range) in Parser::new(src).into_offset_iter() {
        r.event(event, range);
    }
    r.out
}

struct Renderer<'a> {
    src: &'a str,
    base: Weight,
    out: Vec<Part>,
    strong: usize,
    italic: usize,
    heading: bool,
    /// The text of the code block being read; emitted at its end without
    /// the final newline.
    code_block: Option<String>,
    /// Open lists: the next number for an ordered one.
    lists: Vec<Option<u64>>,
    /// Where the last block's content ended in `src`.
    last_end: usize,
    /// Where a block that has not emitted text yet starts in `src`.
    block_start: Option<usize>,
    /// The last thing emitted was a list marker; its paragraph follows on
    /// the same line.
    after_prefix: bool,
}

impl Renderer<'_> {
    fn event(&mut self, event: Event, range: std::ops::Range<usize>) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph if self.after_prefix => {}
                Tag::Paragraph | Tag::BlockQuote(_) | Tag::HtmlBlock => self.block(range.start),
                Tag::Heading { .. } => {
                    self.block(range.start);
                    self.heading = true;
                }
                Tag::CodeBlock(_) => {
                    self.block(range.start);
                    self.code_block = Some(String::new());
                }
                Tag::Item => {
                    self.block(range.start);
                    let depth = self.lists.len().saturating_sub(1);
                    let marker = match self.lists.last_mut() {
                        Some(Some(n)) => {
                            *n += 1;
                            format!("{}. ", *n - 1)
                        }
                        _ => "• ".to_owned(),
                    };
                    self.emit(&format!("{}{marker}", "  ".repeat(depth)), self.base, false);
                    self.after_prefix = true;
                }
                Tag::List(start) => self.lists.push(start),
                Tag::Strong => self.strong += 1,
                Tag::Emphasis => self.italic += 1,
                _ => {}
            },
            Event::End(end) => {
                match end {
                    TagEnd::CodeBlock => {
                        if let Some(code) = self.code_block.take() {
                            self.emit(code.trim_end_matches(['\n', '\r']), self.base, true);
                        }
                    }
                    TagEnd::List(_) => {
                        self.lists.pop();
                    }
                    TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
                    TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
                    TagEnd::Heading(_) => self.heading = false,
                    _ => {}
                }
                if matches!(end, TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item | TagEnd::HtmlBlock) {
                    self.last_end = self.src[..range.end].trim_end_matches(['\n', '\r']).len().max(self.last_end);
                }
            }
            Event::Text(t) => match &mut self.code_block {
                Some(code) => code.push_str(&t),
                None => self.emit(&t, self.weight(), false),
            },
            Event::Code(t) => self.emit(&t, self.weight(), true),
            Event::Html(t) | Event::InlineHtml(t) => self.emit(t.trim_end_matches('\n'), self.weight(), false),
            Event::SoftBreak | Event::HardBreak => self.emit("\n", self.weight(), false),
            Event::Rule => {
                self.block(range.start);
                self.emit("———", Weight::Weak, false);
                self.last_end = self.src[..range.end].trim_end_matches(['\n', '\r']).len();
            }
            Event::TaskListMarker(done) => self.emit(if done { "[x] " } else { "[ ] " }, self.base, false),
            _ => {}
        }
    }

    /// A block starts at `start`; the next text is separated from what came
    /// before by the line breaks between them in the source (at most one
    /// blank line).
    fn block(&mut self, start: usize) {
        self.block_start.get_or_insert(start);
    }

    fn weight(&self) -> Weight {
        match self.base {
            Weight::Struck => Weight::Struck,
            _ if self.strong > 0 || self.heading => Weight::Strong,
            b => b,
        }
    }

    fn emit(&mut self, text: &str, weight: Weight, code: bool) {
        if let Some(start) = self.block_start.take()
            && !self.out.is_empty()
        {
            let gap = &self.src[self.last_end.min(start)..start];
            let blank = gap.split('\n').count() > 2;
            self.push(if blank { "\n\n" } else { "\n" }, self.base, false);
        }
        self.after_prefix = false;
        self.push(text, weight, code);
    }

    fn push(&mut self, text: &str, weight: Weight, code: bool) {
        if text.is_empty() {
            return;
        }
        let italic = self.italic > 0;
        let weight = if self.base == Weight::Struck { Weight::Struck } else { weight };
        match self.out.last_mut() {
            Some(p) if p.weight == weight && p.code == code && p.italic == italic => p.text.push_str(text),
            _ => self.out.push(Part { text: text.to_owned(), weight, code, italic }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(src: &str) -> String {
        render(src, Weight::Weak).into_iter().map(|p| p.text).collect()
    }

    fn code(src: &str) -> Vec<String> {
        render(src, Weight::Weak).into_iter().filter(|p| p.code).map(|p| p.text).collect()
    }

    #[test]
    fn plain_lines_keep_their_breaks() {
        assert_eq!(text("Run this:\nthen that\n\nlast"), "Run this:\nthen that\n\nlast");
        assert_eq!(text("a\n\n\n\nb"), "a\n\nb", "at most one blank line");
    }

    #[test]
    fn markers_are_dropped_and_code_is_marked() {
        assert_eq!(text("Run `cargo test` **now**"), "Run cargo test now");
        assert_eq!(code("Run `cargo test` **now**"), ["cargo test"]);
        let bold = render("a **b**", Weight::Weak);
        assert_eq!(bold.last().unwrap().weight, Weight::Strong);
    }

    #[test]
    fn fenced_blocks_lose_their_fences() {
        let src = "Steps:\n```sh\ncd x\nmake\n```\nDone = it builds.";
        assert_eq!(text(src), "Steps:\ncd x\nmake\nDone = it builds.");
        assert_eq!(code(src), ["cd x\nmake"]);
        assert_eq!(text("a\n\n```\nb\n```\n\nc"), "a\n\nb\n\nc");
    }

    #[test]
    fn lists_get_markers() {
        assert_eq!(text("- one\n- two"), "• one\n• two");
        assert_eq!(text("1. Build:\n   cargo build\n2. Run"), "1. Build:\ncargo build\n2. Run");
        assert_eq!(text("- a\n  - b"), "• a\n  • b");
    }

    #[test]
    fn plain_text_that_looks_like_markup_survives() {
        assert_eq!(text("rm *.tmp and 2 * 3"), "rm *.tmp and 2 * 3");
        assert_eq!(text("A) yes\nB) no"), "A) yes\nB) no");
    }

    #[test]
    fn withdrawn_items_are_struck_throughout() {
        assert!(render("**x** `y`", Weight::Struck).iter().all(|p| p.weight == Weight::Struck));
    }
}
