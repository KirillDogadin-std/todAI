//! Renders unprocessed operator events as short lines an agent can act on.
//! Shared by `todai inbox`, `todai wait`, and the Claude Code hooks.

use std::path::Path;

use crate::store::{Event, Kind};

/// `log` is the whole board; titles come from its add events, so items the
/// operator already trashed still read properly.
pub fn summarize(file: &Path, log: &[Event], events: &[&Event]) -> String {
    let f = file.display();
    let title = |id: &str| {
        log.iter()
            .find_map(|e| match &e.kind {
                Kind::DecisionAdd { id: i, title, .. } | Kind::TodoAdd { id: i, title, .. } if i == id => {
                    Some(title.as_str())
                }
                _ => None,
            })
            .unwrap_or("?")
    };
    let answer = |rid: &str| {
        log.iter().rev().find_map(|e| match &e.kind {
            Kind::ElabAdd { rid: r, text_md } if r == rid => Some(text_md.as_str()),
            _ => None,
        })
    };
    let parent = |rid: &str| {
        log.iter().find_map(|e| match &e.kind {
            Kind::ElabRequest { rid: r, parent, .. } if r == rid => Some(parent.clone()),
            _ => None,
        })?
    };
    let root = |rid: &str| {
        let mut cur = rid.to_owned();
        while let Some(p) = parent(&cur) {
            cur = p;
        }
        cur
    };
    let mut out = format!("todai: operator updates in {f}\n");
    for e in events {
        let line = match &e.kind {
            Kind::DecisionResolve { id, text } => {
                format!("- {id} \"{}\" resolved: {text}", title(id))
            }
            Kind::DecisionReopen { id } => {
                format!("- {id} \"{}\" reopened; treat its previous resolution as void", title(id))
            }
            Kind::TodoDone { id, note } => match note {
                Some(n) => format!("- {id} \"{}\" done; note: {n}", title(id)),
                None => format!("- {id} \"{}\" done", title(id)),
            },
            Kind::TodoUndone { id } => format!("- {id} \"{}\" marked NOT done again", title(id)),
            Kind::ElabRequest { rid, target, selection, note, parent: Some(p) } => {
                let root = root(p);
                // Replying to the thread's first answer needs no extra pointer.
                let to = if root == *p { String::new() } else { format!(", replying to {p}") };
                let mut l = format!(
                    "- {rid}: operator follows up in thread {root} on {target} \"{}\"{to}; {}",
                    title(target),
                    quote(selection, answer(p), p)
                );
                if let Some(n) = note {
                    l.push_str(&format!("; note: {n}"));
                }
                l.push_str(&format!("\n  answer with: todai elaborate {f} {rid} - <<'EOF' ... EOF"));
                l
            }
            Kind::ElabRequest { rid, target, selection, note, parent: None } => {
                let mut l = format!(
                    "- {rid}: operator asks to elaborate on {target} \"{}\"; selected text: \"{selection}\"",
                    title(target)
                );
                if let Some(n) = note {
                    l.push_str(&format!("; note: {n}"));
                }
                l.push_str(&format!("\n  answer with: todai elaborate {f} {rid} - <<'EOF' ... EOF"));
                l
            }
            _ => continue,
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// How a follow-up's selection is shown: the agent wrote the parent answer, so
/// quoting all of it back is noise.
pub fn quote(selection: &str, parent_answer: Option<&str>, parent: &str) -> String {
    const MAX: usize = 120;
    if parent_answer.is_some_and(|a| a.trim() == selection.trim()) {
        return format!("about the whole of {parent}'s answer");
    }
    let short: String = selection.chars().take(MAX).collect();
    let more = if selection.chars().count() > MAX { "…" } else { "" };
    format!("selected text: \"{short}{more}\"")
}
