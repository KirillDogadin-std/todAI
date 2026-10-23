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
            Kind::ElabRequest { rid, target, selection, note } => {
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
