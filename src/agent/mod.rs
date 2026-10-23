//! Everything todai tells an agent, plus the per-agent integrations that
//! `todai setup` installs. The protocol text lives here, in the binary, so an
//! upgrade never needs a reinstall: installed skills just say "run `todai guide`".

pub mod claude;
pub mod setup;

use std::path::{Path, PathBuf};

use anyhow::Result;

pub const GUIDE: &str = include_str!("guide.md");

/// The Agent Skills file installed for every agent. Kept tiny on purpose.
pub const SKILL: &str = r#"---
name: todai
description: Route questions for the user (decisions) and hands-on tasks they must do (todos) through the todai board window instead of asking in chat. That includes clarifying questions (in plan mode too) and anything you leave open at the end of a turn, such as offers ("want me to…?"), suggested next steps, and unresolved choices. Use only when the user asks to use todai, or when a todai board is already attached in this session.
---

Run `todai guide` and follow it. Start with `todai attach`.
"#;

/// pi extension source, installed by `todai setup --agent pi`.
pub const PI_EXTENSION: &str = include_str!("pi-extension.ts");

/// Printed by `todai attach`; agent adapters look for this line in tool
/// output to learn which board a session uses.
pub const ATTACHED_MARKER: &str = "todai: attached ";

/// `<repo root or cwd>/.agents/todai/<name>.jsonl`, or `name` itself if it is a path.
pub fn board_path(name: &str) -> Result<PathBuf> {
    if name.contains('/') || name.ends_with(".jsonl") {
        return Ok(std::path::absolute(name)?);
    }
    let cwd = std::env::current_dir()?;
    let root = repo_root(&cwd).unwrap_or(cwd);
    Ok(root.join(".agents/todai").join(format!("{name}.jsonl")))
}

pub fn repo_root(from: &Path) -> Option<PathBuf> {
    from.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

/// Finds the board a piece of tool output attached to, if any.
pub fn attached_board(output: &str) -> Option<PathBuf> {
    output.lines().rev().find_map(|l| {
        let path = l.trim().strip_prefix(ATTACHED_MARKER)?;
        Some(PathBuf::from(path.trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_attached_marker_in_noisy_output() {
        let out = "building...\ntodai: attached /r/.agents/todai/board.jsonl\nPost decisions ...\n";
        assert_eq!(attached_board(out), Some(PathBuf::from("/r/.agents/todai/board.jsonl")));
        assert_eq!(attached_board("nothing here"), None);
    }

    #[test]
    fn skill_frontmatter_is_valid_for_agent_skills() {
        let fm = SKILL.strip_prefix("---\n").unwrap().split("\n---").next().unwrap();
        assert!(fm.lines().any(|l| l == "name: todai"));
        let desc = fm.lines().find_map(|l| l.strip_prefix("description: ")).unwrap();
        assert!(desc.len() <= 1024, "Agent Skills caps description at 1024 chars");
    }
}
