//! BERT's uncased WordPiece tokenizer, as HuggingFace `tokenizers` runs it
//! for `bert-base-uncased`-style vocabularies: normalise (clean text, space
//! out CJK ideographs, strip accents, lowercase), split on whitespace and
//! punctuation, then match each word greedily, longest piece first.
//! `reference.json` pins the output to the real tokenizer's.

use std::collections::HashMap;

use unicode_general_category::{GeneralCategory as Cat, get_general_category as category};
use unicode_normalization::UnicodeNormalization;

/// Words longer than this become `[UNK]`, as in BERT.
const MAX_WORD_CHARS: usize = 100;

pub struct WordPiece {
    ids: HashMap<&'static str, u32>,
    unk: u32,
}

impl WordPiece {
    /// `vocab`: one token per line, line number = id.
    pub fn new(vocab: &'static str) -> WordPiece {
        let ids: HashMap<&str, u32> = vocab.lines().enumerate().map(|(i, t)| (t, i as u32)).collect();
        let unk = ids["[UNK]"];
        WordPiece { ids, unk }
    }

    pub fn unk(&self) -> u32 {
        self.unk
    }

    pub fn ids(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for word in pre_tokenize(&normalize(text)) {
            self.word(&word, &mut out);
        }
        out
    }

    fn word(&self, word: &str, out: &mut Vec<u32>) {
        let chars: Vec<(usize, char)> = word.char_indices().collect();
        if chars.len() > MAX_WORD_CHARS {
            out.push(self.unk);
            return;
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let from = chars[start].0;
            // Longest piece first: shrink the end until the vocab has it.
            let found = (start + 1..=chars.len()).rev().find_map(|end| {
                let to = chars.get(end).map_or(word.len(), |c| c.0);
                let piece = &word[from..to];
                let id = if start == 0 { self.ids.get(piece) } else { self.ids.get(format!("##{piece}").as_str()) };
                id.map(|&id| (id, end))
            });
            match found {
                Some((id, end)) => {
                    pieces.push(id);
                    start = end;
                }
                None => {
                    // Any unmatched part makes the whole word unknown.
                    out.push(self.unk);
                    return;
                }
            }
        }
        out.extend(pieces);
    }
}

fn normalize(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\0' || c == '\u{fffd}' || is_control(c) {
            continue;
        }
        if is_whitespace(c) {
            cleaned.push(' ');
        } else if is_cjk(c) {
            cleaned.push(' ');
            cleaned.push(c);
            cleaned.push(' ');
        } else {
            cleaned.push(c);
        }
    }
    // Strip accents: decompose, drop the combining marks.
    let stripped: String = cleaned.nfd().filter(|&c| category(c) != Cat::NonspacingMark).collect();
    stripped.to_lowercase()
}

fn pre_tokenize(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            words.extend((!cur.is_empty()).then(|| std::mem::take(&mut cur)));
        } else if is_punctuation(c) {
            words.extend((!cur.is_empty()).then(|| std::mem::take(&mut cur)));
            words.push(c.to_string());
        } else {
            cur.push(c);
        }
    }
    words.extend((!cur.is_empty()).then_some(cur));
    words
}

fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r') || c.is_whitespace()
}

fn is_control(c: char) -> bool {
    !matches!(c, '\t' | '\n' | '\r')
        && matches!(category(c), Cat::Control | Cat::Format | Cat::Unassigned | Cat::PrivateUse | Cat::Surrogate)
}

/// ASCII symbols count as punctuation too (`$`, `+`, `^`), as in BERT.
fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            category(c),
            Cat::ConnectorPunctuation
                | Cat::DashPunctuation
                | Cat::OpenPunctuation
                | Cat::ClosePunctuation
                | Cat::InitialPunctuation
                | Cat::FinalPunctuation
                | Cat::OtherPunctuation
        )
}

/// The CJK Unified Ideograph blocks BERT spaces out (not Hangul or kana).
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2B73F
        | 0x2B740..=0x2B81F | 0x2B820..=0x2CEAF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F)
}
