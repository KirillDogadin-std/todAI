//! The one text field being typed into: a decision's resolution, a todo's
//! note, or the elaboration message. The text itself lives where the window
//! keeps it (`Tab::drafts`, `Prompt::note`); this is only the caret.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A draft in `Tab::drafts`, by item id.
    Draft(String),
    /// `Nav::prompt`'s note.
    Prompt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub target: Target,
    /// Caret, in chars.
    pub caret: usize,
    /// Whether `Alt+Enter` and friends may insert a newline.
    pub multiline: bool,
}

/// Applies a key that navigation left alone. Returns whether it was used.
pub fn key(edit: &mut Edit, text: &mut String, k: &KeyEvent) -> bool {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    let n = text.chars().count();
    edit.caret = edit.caret.min(n);
    match k.code {
        // Terminals can't all send Shift+Enter: Alt+Enter and Ctrl+J too.
        KeyCode::Enter if alt || shift => newline(edit, text),
        KeyCode::Char('j') if ctrl => newline(edit, text),
        KeyCode::Char('u') if ctrl => {
            let start = line_start(text, edit.caret);
            remove(text, start, edit.caret);
            edit.caret = start;
        }
        KeyCode::Char('w') if alt => {
            let start = word_start(text, edit.caret);
            remove(text, start, edit.caret);
            edit.caret = start;
        }
        KeyCode::Char('a') if ctrl => edit.caret = line_start(text, edit.caret),
        KeyCode::Char('e') if ctrl => edit.caret = line_end(text, edit.caret),
        KeyCode::Char(c) if !ctrl && !alt => {
            insert(text, edit.caret, &c.to_string());
            edit.caret += 1;
        }
        KeyCode::Backspace if alt || ctrl => {
            let start = word_start(text, edit.caret);
            remove(text, start, edit.caret);
            edit.caret = start;
        }
        KeyCode::Backspace if edit.caret > 0 => {
            remove(text, edit.caret - 1, edit.caret);
            edit.caret -= 1;
        }
        KeyCode::Delete if edit.caret < n => remove(text, edit.caret, edit.caret + 1),
        KeyCode::Left => edit.caret = edit.caret.saturating_sub(1),
        KeyCode::Right => edit.caret = (edit.caret + 1).min(n),
        KeyCode::Home => edit.caret = line_start(text, edit.caret),
        KeyCode::End => edit.caret = line_end(text, edit.caret),
        KeyCode::Up | KeyCode::Down => edit.caret = vertical(text, edit.caret, k.code == KeyCode::Down),
        KeyCode::Backspace | KeyCode::Delete => {}
        _ => return false,
    }
    true
}

/// Pasted text, as one insertion.
pub fn paste(edit: &mut Edit, text: &mut String, s: &str) {
    let s = if edit.multiline { s.replace("\r\n", "\n") } else { s.replace(['\r', '\n'], " ") };
    edit.caret = edit.caret.min(text.chars().count());
    insert(text, edit.caret, &s);
    edit.caret += s.chars().count();
}

fn newline(edit: &mut Edit, text: &mut String) {
    if edit.multiline {
        insert(text, edit.caret, "\n");
        edit.caret += 1;
    }
}

fn byte(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map_or(text.len(), |(b, _)| b)
}

fn insert(text: &mut String, at: usize, s: &str) {
    let b = byte(text, at);
    text.insert_str(b, s);
}

fn remove(text: &mut String, from: usize, to: usize) {
    let (a, b) = (byte(text, from), byte(text, to));
    text.replace_range(a..b, "");
}

fn line_start(text: &str, caret: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    (0..caret).rev().find(|&i| chars[i] == '\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, caret: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    (caret..chars.len()).find(|&i| chars[i] == '\n').unwrap_or(chars.len())
}

fn word_start(text: &str, caret: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = caret;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// Up / down a logical line, keeping the column where possible.
fn vertical(text: &str, caret: usize, down: bool) -> usize {
    let start = line_start(text, caret);
    let col = caret - start;
    if down {
        let end = line_end(text, caret);
        if end >= text.chars().count() {
            return caret;
        }
        let next = end + 1;
        (next + col).min(line_end(text, next))
    } else {
        if start == 0 {
            return caret;
        }
        let prev = line_start(text, start - 1);
        (prev + col).min(start - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(e: &mut Edit, t: &mut String, code: KeyCode, m: KeyModifiers) {
        key(e, t, &KeyEvent::new(code, m));
    }

    #[test]
    fn types_moves_and_deletes() {
        let mut e = Edit { target: Target::Prompt, caret: 0, multiline: true };
        let mut t = String::new();
        for c in "héllo".chars() {
            press(&mut e, &mut t, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(&mut e, &mut t, KeyCode::Enter, KeyModifiers::ALT);
        press(&mut e, &mut t, KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!((t.as_str(), e.caret), ("héllo\nx", 7));
        press(&mut e, &mut t, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(e.caret, 1);
        press(&mut e, &mut t, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!((t.as_str(), e.caret), ("éllo\nx", 0));
        press(&mut e, &mut t, KeyCode::End, KeyModifiers::NONE);
        press(&mut e, &mut t, KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(t, "\nx");
    }

    #[test]
    fn single_line_fields_ignore_newlines() {
        let mut e = Edit { target: Target::Draft("t1".into()), caret: 0, multiline: false };
        let mut t = String::new();
        press(&mut e, &mut t, KeyCode::Char('j'), KeyModifiers::CONTROL);
        paste(&mut e, &mut t, "a\nb");
        assert_eq!(t, "a b");
    }
}
