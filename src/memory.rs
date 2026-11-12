//! `todai memory ...`: the decision memory from the command line. The store
//! is `crates/todai-memory`; this is argument parsing, finding the project's
//! database, and reading the card text agents pipe in.

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use todai_memory::{Actor, CardDraft, Class, Memory, Source, embed, format};

use crate::agent;

#[derive(Subcommand)]
pub enum Cmd {
    /// [agent] Remember a decision as a card; prints its id. Long fields come
    /// on stdin as markdown sections: `## Context` and `## Why` (required),
    /// `## Applies`, `## Not when`, `## Rejected`.
    Add {
        /// One line, at most 120 characters: the decision itself.
        #[arg(long)]
        gist: String,
        /// axiom (must always hold; an agent's is only proposed), context
        /// (holds in situations like this one), or one_off.
        #[arg(long, default_value = "context")]
        class: String,
        /// 1-5 short phrases to find it by (repeat, or comma separate).
        /// Reuse existing ones: `todai memory keys`.
        #[arg(long = "key", value_delimiter = ',', required = true)]
        keys: Vec<String>,
        /// The board item it was decided in: `d39`, or `BOARD/d39` when
        /// several boards have that id.
        #[arg(long)]
        from: Option<String>,
        /// Record it as the operator's (an axiom is then active at once).
        #[arg(long)]
        operator: bool,
        /// Keep this key even though it is close to an existing one (repeat,
        /// or comma separate). `add` names the close keys when it refuses.
        #[arg(long = "new-key", value_delimiter = ',')]
        new_keys: Vec<String>,
    },
    /// Print a card in full. A unique prefix of the id works.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// [agent] Cards whose keys are near any of these short phrases (2-6
    /// words each), best first. Try 2-3 phrasings of the same question.
    Find {
        #[arg(required = true)]
        phrases: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every key in use, with how many cards have it.
    Keys {
        #[arg(long)]
        json: bool,
    },
}

pub fn run(cmd: Cmd) -> Result<()> {
    let root = project_root()?;
    let path = todai_memory::path_for(&root).context("no state directory: set HOME or XDG_STATE_HOME")?;
    let mut memory = Memory::open(&path)?;
    match cmd {
        Cmd::Add { gist, class, keys, from, operator, new_keys } => {
            let mut draft = sections(&read_stdin()?)?;
            draft.class = Some(Class::parse(&class)?);
            draft.gist = gist;
            draft.keys = keys;
            let source = from.map(|f| source(&root, &f)).transpose()?;
            let actor = if operator { Actor::Operator } else { Actor::Agent };
            let (card, snaps) = memory.remember(draft, actor, source, embed::default(), &new_keys)?;
            for s in &snaps {
                eprintln!("key {:?} is the existing {:?} ({:.2}): used that", s.from, s.to, s.similarity);
            }
            println!("{}", card.id);
            if card.status == todai_memory::Status::Proposed {
                eprintln!("{}: proposed; an axiom holds once the operator confirms it", card.id);
            }
        }
        Cmd::Show { id, json } => {
            let Some(card) = memory.card(&id)? else { bail!("no card {id}") };
            if json {
                println!("{}", serde_json::to_string_pretty(&card)?);
            } else {
                println!("{}", format::full(&card));
            }
        }
        Cmd::Find { phrases, json } => {
            let hits = memory.recall(&phrases, embed::default())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else if hits.is_empty() {
                // Not an error: "nothing decided about this yet" is an answer.
                println!("no key near these phrases: nothing remembered about this; `todai memory keys` lists the keys");
            } else {
                let blocks: Vec<String> = hits
                    .iter()
                    .map(|h| {
                        let via: Vec<String> = h.via.iter().map(|(p, k, s)| format!("{p:?} → {k} ({s:.2})")).collect();
                        format!("{}\n  via: {}", format::hit_block(&h.card), via.join("; "))
                    })
                    .collect();
                println!("{}", blocks.join("\n\n"));
            }
        }
        Cmd::Keys { json } => {
            let keys = memory.keys()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&keys)?);
            } else {
                for (k, n) in keys {
                    println!("{n:>3}  {k}");
                }
            }
        }
    }
    Ok(())
}

/// The repo the current directory is in, else the directory itself: the
/// same root `todai attach` puts boards under.
fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(agent::repo_root(&cwd).unwrap_or(cwd))
}

fn read_stdin() -> Result<String> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    let mut s = String::new();
    stdin.read_to_string(&mut s)?;
    Ok(s)
}

/// Splits `## Heading` sections into the card's long fields.
fn sections(md: &str) -> Result<CardDraft> {
    let mut draft = CardDraft::default();
    let mut current: Option<&mut String> = None;
    let mut preamble = String::new();
    for line in md.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            let field = match h.trim().to_lowercase().replace(['-', '_'], " ").as_str() {
                "context" => &mut draft.context,
                "applies" | "applies when" => &mut draft.applies,
                "not when" => &mut draft.not_when,
                "why" => &mut draft.why,
                "rejected" | "alternatives" => &mut draft.rejected,
                other => bail!("unknown section `## {other}`: use Context, Why, Applies, Not when, Rejected"),
            };
            if !field.is_empty() {
                bail!("section `## {}` appears twice", h.trim());
            }
            // Nonempty marks the section as seen; trimmed by validation.
            field.push('\n');
            current = Some(field);
            continue;
        }
        let target = match current.as_deref_mut() {
            Some(f) => f,
            None => &mut preamble,
        };
        target.push_str(line);
        target.push('\n');
    }
    if !preamble.trim().is_empty() {
        bail!("text before the first section; start with `## Context`");
    }
    Ok(draft)
}

/// `d39` or `BOARD/d39` → where the card came from. A bare id is looked up
/// in the project's boards; item ids restart on every board, so the one
/// where it changed last wins (the decision just made).
fn source(root: &Path, from: &str) -> Result<Source> {
    let (board, item) = match from.rsplit_once('/') {
        Some((b, i)) => (Some(b.to_owned()), i.to_owned()),
        None => (None, from.to_owned()),
    };
    if !item.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) || item.len() < 2 {
        bail!("--from {from}: expected a board item like d39 or BOARD/d39");
    }
    let board = match board {
        Some(b) => {
            if !board_file(root, &b).is_file() {
                bail!("--from {from}: no board {b} in {}", root.join(".agents/todai").display());
            }
            b
        }
        None => infer_board(root, &item)?.with_context(|| format!("--from {item}: no board in this project has {item}"))?,
    };
    Ok(Source { board: Some(board), item: Some(item), session: None })
}

fn board_file(root: &Path, name: &str) -> PathBuf {
    root.join(".agents/todai").join(format!("{name}.jsonl"))
}

fn infer_board(root: &Path, item: &str) -> Result<Option<String>> {
    let Ok(dir) = std::fs::read_dir(root.join(".agents/todai")) else { return Ok(None) };
    let mut best: Option<(u64, String)> = None;
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Ok(events) = crate::store::Log::new(path.clone()).load() else { continue };
        let last = events
            .iter()
            .filter(|e| serde_json::to_value(&e.kind).is_ok_and(|v| v.get("id").and_then(|i| i.as_str()) == Some(item)))
            .map(|e| e.ts)
            .max();
        if let Some(ts) = last
            && best.as_ref().is_none_or(|(b, _)| ts > *b)
        {
            let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            best = Some((ts, name));
        }
    }
    Ok(best.map(|(_, name)| name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_fill_the_long_fields() {
        let d = sections("## Context\nA board app.\n\n## Why\nMemory.\n## Not-when\nOn a server.\n").unwrap();
        assert_eq!((d.context.trim(), d.why.trim(), d.not_when.trim()), ("A board app.", "Memory.", "On a server."));
        assert!(sections("hello\n## Context\nx").is_err(), "preamble");
        assert!(sections("## Context\nx\n## Context\ny").is_err(), "twice");
        assert!(format!("{:#}", sections("## Pros\nx").unwrap_err()).contains("unknown section"));
        assert_eq!(sections("").unwrap(), CardDraft::default());
    }

    #[test]
    fn from_finds_the_board_where_the_item_changed_last() {
        use crate::store::{Actor as By, Kind, Log};
        let dir = tempfile::tempdir().unwrap();
        let add = |board: &str, id: &str| {
            Log::new(board_file(dir.path(), board))
                .append(By::Agent, |_| Ok(Kind::TodoAdd { id: id.into(), title: "x".into(), context: None }))
                .unwrap();
        };
        std::fs::create_dir_all(dir.path().join(".agents/todai")).unwrap();
        add("old", "t1");
        std::thread::sleep(std::time::Duration::from_millis(5));
        add("new", "t1");
        add("old", "t2");
        let src = |f: &str| source(dir.path(), f).map(|s| (s.board.unwrap(), s.item.unwrap()));
        assert_eq!(src("t1").unwrap(), ("new".into(), "t1".into()));
        assert_eq!(src("t2").unwrap(), ("old".into(), "t2".into()));
        assert_eq!(src("old/t1").unwrap(), ("old".into(), "t1".into()));
        assert!(src("t9").is_err());
        assert!(src("nope/t1").is_err());
        assert!(src("39").is_err());
    }
}
