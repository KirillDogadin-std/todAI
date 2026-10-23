//! Vim-style motions over a char slice. Indices are char offsets; every
//! function returns a valid index into a non-empty slice.

fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn last(t: &[char]) -> usize {
    t.len().saturating_sub(1)
}

/// `w`: start of the next word.
pub fn word_forward(t: &[char], i: usize) -> usize {
    let n = t.len();
    if i + 1 >= n {
        return last(t);
    }
    let c = class(t[i]);
    let mut j = i;
    if c != 0 {
        while j < n && class(t[j]) == c {
            j += 1;
        }
    }
    while j < n && class(t[j]) == 0 {
        j += 1;
    }
    j.min(last(t))
}

/// `e`: end of the current or next word.
pub fn word_end(t: &[char], i: usize) -> usize {
    let n = t.len();
    let mut j = i + 1;
    while j < n && class(t[j]) == 0 {
        j += 1;
    }
    if j >= n {
        return last(t);
    }
    let c = class(t[j]);
    while j + 1 < n && class(t[j + 1]) == c {
        j += 1;
    }
    j
}

/// `b`: start of the current or previous word.
pub fn word_back(t: &[char], i: usize) -> usize {
    if i == 0 || t.is_empty() {
        return 0;
    }
    let mut j = i - 1;
    while j > 0 && class(t[j]) == 0 {
        j -= 1;
    }
    let c = class(t[j]);
    while j > 0 && class(t[j - 1]) == c {
        j -= 1;
    }
    j
}

/// `0`
pub fn line_start(t: &[char], i: usize) -> usize {
    let mut j = i.min(last(t));
    while j > 0 && t[j - 1] != '\n' {
        j -= 1;
    }
    j
}

/// `^`
pub fn first_non_blank(t: &[char], i: usize) -> usize {
    let mut j = line_start(t, i);
    while j + 1 < t.len() && t[j] != '\n' && t[j].is_whitespace() {
        j += 1;
    }
    j
}

/// `$`
pub fn line_end(t: &[char], i: usize) -> usize {
    let mut j = i.min(last(t));
    while j + 1 < t.len() && t[j] != '\n' && t[j + 1] != '\n' {
        j += 1;
    }
    j
}

/// Inclusive range of the word (or run of punctuation) under `i`.
pub fn word_at(t: &[char], i: usize) -> (usize, usize) {
    if t.is_empty() {
        return (0, 0);
    }
    let i = i.min(last(t));
    let c = class(t[i]);
    let (mut a, mut b) = (i, i);
    while a > 0 && class(t[a - 1]) == c {
        a -= 1;
    }
    while b + 1 < t.len() && class(t[b + 1]) == c {
        b += 1;
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn word_motions() {
        let t = chars("use egui, or iced?");
        assert_eq!(word_forward(&t, 0), 4);
        assert_eq!(word_forward(&t, 4), 8); // stops at ','
        assert_eq!(word_forward(&t, 8), 10);
        assert_eq!(word_end(&t, 0), 2);
        assert_eq!(word_end(&t, 2), 7);
        assert_eq!(word_back(&t, 10), 8);
        assert_eq!(word_back(&t, 8), 4);
        assert_eq!(word_forward(&t, 16), 17);
        assert_eq!(word_forward(&t, 17), 17);
    }

    #[test]
    fn line_motions() {
        let t = chars("title\n  context here");
        assert_eq!(line_end(&t, 0), 4);
        assert_eq!(line_start(&t, 10), 6);
        assert_eq!(first_non_blank(&t, 10), 8);
        assert_eq!(line_end(&t, 8), t.len() - 1);
    }

    #[test]
    fn word_under_cursor() {
        let t = chars("pick ~/.local/share now");
        assert_eq!(word_at(&t, 1), (0, 3));
        assert_eq!(word_at(&t, 5), (5, 7)); // "~/." punctuation run
    }
}
