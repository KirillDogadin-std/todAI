//! Text for agents, in the layers of the token budget: a gist line
//! (~20 tokens), a hit block (~100: enough to decide "follow it / doesn't
//! apply / ask" without opening the card), and the full card.

use std::fmt::Write;

use crate::{Card, Class, Status};

/// One field's share of a hit block.
const HIT_FIELD: usize = 160;

/// `m3f9a1c02 · axiom (proposed) · egui over iced: memory`
pub fn gist_line(c: &Card) -> String {
    let class = match c.class {
        Class::Axiom => "axiom",
        Class::Context => "context",
        Class::OneOff => "one-off",
    };
    let status = if c.status == Status::Proposed { " (proposed)" } else { "" };
    format!("{} · {class}{status} · {}", c.id, c.gist)
}

/// The gist line plus each field's start, one line each.
pub fn hit_block(c: &Card) -> String {
    let mut s = gist_line(c);
    let mut field = |name: &str, text: &str| {
        if !text.is_empty() {
            let _ = write!(s, "\n  {name}: {}", clip(text, HIT_FIELD));
        }
    };
    field("context", &c.context);
    field("applies", &c.applies);
    field("not when", &c.not_when);
    field("why", &c.why);
    let _ = write!(s, "\n  keys: {}", c.keys.join(", "));
    if let Some(from) = from(c) {
        let _ = write!(s, " · from {from}");
    }
    s
}

/// Everything, as markdown.
pub fn full(c: &Card) -> String {
    let mut s = gist_line(c);
    let _ = write!(s, "\nkeys: {}\ncreated: {} by {}", c.keys.join(", "), date(c.created), c.created_by.as_str());
    if let Some(from) = from(c) {
        let _ = write!(s, ", from {from}");
    }
    for (name, text) in
        [("Context", &c.context), ("Applies", &c.applies), ("Not when", &c.not_when), ("Why", &c.why), ("Rejected", &c.rejected)]
    {
        if !text.is_empty() {
            let _ = write!(s, "\n\n## {name}\n{text}");
        }
    }
    s
}

fn from(c: &Card) -> Option<String> {
    let src = c.source.as_ref()?;
    match (&src.board, &src.item) {
        (Some(b), Some(i)) => Some(format!("{b}/{i}")),
        (Some(b), None) => Some(b.clone()),
        (None, Some(i)) => Some(i.clone()),
        (None, None) => None,
    }
}

/// The text on one line, cut at a word boundary to at most `max` chars.
fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.']))
}

/// `2026-11-09` (UTC) from milliseconds since the epoch.
fn date(ms: i64) -> String {
    // Howard Hinnant's days-to-civil.
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_clipping() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_794_258_201_740), "2026-11-09");
        assert_eq!(date(951_782_400_000), "2000-02-29");
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("one two three four", 12), "one two…");
        assert_eq!(clip("a\n  b", 10), "a b");
    }
}
