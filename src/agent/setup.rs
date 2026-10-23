//! `todai setup`: installs agent integrations at a scope the user picks, shows
//! every change first, and records what it wrote so `--remove` can undo it.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{PI_EXTENSION, SKILL, repo_root};
use crate::config;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// Claude Code: skill + hooks.
    Claude,
    /// pi: skill + extension.
    Pi,
    /// Any agent reading `.agents/skills` (Codex, OpenCode, ...): skill only.
    Generic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// Only sessions in the current repository.
    Project,
    /// Every project for this user.
    User,
}

pub struct Args {
    pub agents: Vec<Agent>,
    pub scope: Option<Scope>,
    pub yes: bool,
    pub dry_run: bool,
    pub remove: bool,
}

/// One thing setup writes. Recorded in the manifest for removal.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Item {
    /// `agents` lists every agent relying on the file (the skill is shared).
    File { agents: Vec<Agent>, path: PathBuf, content: String },
    /// A relative symlink, e.g. Claude Code's skill dir pointing into `.agents`.
    Symlink { agent: Agent, path: PathBuf, target: PathBuf },
    ClaudeHooks { path: PathBuf, bin: String },
}

impl Item {
    fn path(&self) -> &Path {
        match self {
            Item::File { path, .. } | Item::Symlink { path, .. } | Item::ClaudeHooks { path, .. } => path,
        }
    }

    fn used_by(&self, agent: Agent) -> bool {
        match self {
            Item::File { agents, .. } => agents.contains(&agent),
            Item::Symlink { agent: a, .. } => *a == agent,
            Item::ClaudeHooks { .. } => agent == Agent::Claude,
        }
    }

    fn describe(&self, removing: bool) -> String {
        match (self, removing) {
            (Item::File { path, .. }, false) => format!("write   {}", path.display()),
            (Item::File { path, .. }, true) => format!("delete  {}", path.display()),
            (Item::Symlink { path, target, .. }, false) => {
                format!("link    {} -> {}", path.display(), target.display())
            }
            (Item::Symlink { path, .. }, true) => format!("delete  {} (link)", path.display()),
            (Item::ClaudeHooks { path, .. }, false) => {
                format!("update  {}  (+ PostToolUse and Stop hooks running `todai hook claude ...`)", path.display())
            }
            (Item::ClaudeHooks { path, .. }, true) => format!("update  {}  (- todai hooks)", path.display()),
        }
    }
}

pub fn run(args: Args) -> Result<()> {
    if args.remove {
        return remove(&args);
    }
    let interactive = !args.yes && std::io::stdin().is_terminal();
    let agents = if args.agents.is_empty() {
        if !interactive {
            bail!("pass --agent (claude, pi, generic), or run `todai setup` in a terminal");
        }
        ask_agents()?
    } else {
        args.agents.clone()
    };
    let project = std::env::current_dir().map(|cwd| repo_root(&cwd).unwrap_or(cwd))?;
    let scope = match args.scope {
        Some(s) => s,
        None if interactive => ask_scope(&project)?,
        None => bail!("pass --scope project or --scope user"),
    };
    let home = home()?;
    let base = match scope {
        Scope::Project => project.clone(),
        Scope::User => home.clone(),
    };
    let items = plan(&agents, scope, &base, &bin_command());

    println!("todai setup will:");
    for i in &items {
        println!("  {}", i.describe(false));
    }
    if args.dry_run {
        return Ok(());
    }
    if !args.yes && !(interactive && confirm("Proceed?")?) {
        println!("Nothing changed.");
        return Ok(());
    }
    // Fail before writing anything, so setup never leaves a half install.
    for item in &items {
        check(item)?;
    }
    let mut manifest = Manifest::load();
    for mut item in items {
        apply(&item)?;
        // Re-installing keeps agents recorded by earlier runs on a shared file.
        if let Some(Item::File { agents: old, .. }) = manifest.items.iter().find(|m| m.path() == item.path())
            && let Item::File { agents, .. } = &mut item
        {
            for a in old {
                if !agents.contains(a) {
                    agents.push(*a);
                }
            }
        }
        manifest.items.retain(|m| m.path() != item.path());
        manifest.items.push(item);
    }
    manifest.save()?;
    println!("Done. In a session, ask your agent to \"use todai\". Undo with `todai setup --remove`.");
    Ok(())
}

fn plan(agents: &[Agent], scope: Scope, base: &Path, bin: &str) -> Vec<Item> {
    let mut items = Vec::new();
    // One real skill in the agent-agnostic `.agents/skills`, shared by everyone.
    let skill = base.join(".agents/skills/todai/SKILL.md");
    let add_skill = |items: &mut Vec<Item>, agent| {
        match items.iter_mut().find(|i| i.path() == skill) {
            Some(Item::File { agents, .. }) => agents.push(agent),
            _ => items.push(Item::File { agents: vec![agent], path: skill.clone(), content: SKILL.to_owned() }),
        }
    };
    for agent in agents {
        match agent {
            Agent::Claude => {
                // Claude Code only reads `.claude/skills`; point it at the shared copy.
                add_skill(&mut items, Agent::Claude);
                items.push(Item::Symlink {
                    agent: Agent::Claude,
                    path: base.join(".claude/skills/todai"),
                    target: PathBuf::from("../../.agents/skills/todai"),
                });
                // Project scope uses the personal, uncommitted settings file.
                let settings = match scope {
                    Scope::Project => base.join(".claude/settings.local.json"),
                    Scope::User => base.join(".claude/settings.json"),
                };
                items.push(Item::ClaudeHooks { path: settings, bin: bin.to_owned() });
            }
            Agent::Pi => {
                add_skill(&mut items, Agent::Pi);
                let ext = match scope {
                    Scope::Project => base.join(".pi/extensions/todai.ts"),
                    Scope::User => base.join(".pi/agent/extensions/todai.ts"),
                };
                let content = PI_EXTENSION.replace("__TODAI_BIN__", bin);
                items.push(Item::File { agents: vec![Agent::Pi], path: ext, content });
            }
            Agent::Generic => add_skill(&mut items, Agent::Generic),
        }
    }
    items
}

/// Refuses to replace anything setup didn't create.
fn check(item: &Item) -> Result<()> {
    if let Item::Symlink { path, target, .. } = item
        && let Ok(m) = std::fs::symlink_metadata(path)
        && !(m.is_symlink() && std::fs::read_link(path)? == *target)
    {
        bail!("{} already exists; move it away and run setup again", path.display());
    }
    Ok(())
}

fn apply(item: &Item) -> Result<()> {
    match item {
        Item::File { path, content, .. } => {
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
        }
        Item::Symlink { path, target, .. } => {
            if std::fs::symlink_metadata(path).is_ok() {
                return Ok(()); // `check` confirmed it is already our link
            }
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::os::unix::fs::symlink(target, path).with_context(|| format!("linking {}", path.display()))
        }
        Item::ClaudeHooks { path, bin } => edit_json(path, |v| add_claude_hooks(v, bin)),
    }
}

fn remove(args: &Args) -> Result<()> {
    let mut manifest = Manifest::load();
    let mut gone = Vec::new();
    for mut item in std::mem::take(&mut manifest.items) {
        if args.agents.is_empty() {
            gone.push(item);
            continue;
        }
        // A shared file goes only when no remaining agent uses it.
        if let Item::File { agents, .. } = &mut item {
            agents.retain(|a| !args.agents.contains(a));
            if agents.is_empty() {
                gone.push(item);
            } else {
                manifest.items.push(item);
            }
        } else if args.agents.iter().any(|a| item.used_by(*a)) {
            gone.push(item);
        } else {
            manifest.items.push(item);
        }
    }
    if gone.is_empty() {
        println!("todai setup: nothing recorded to remove.");
        return Ok(());
    }
    println!("todai setup --remove will:");
    for i in &gone {
        println!("  {}", i.describe(true));
    }
    let interactive = !args.yes && std::io::stdin().is_terminal();
    if args.dry_run || (!args.yes && !(interactive && confirm("Proceed?")?)) {
        println!("Nothing changed.");
        return Ok(());
    }
    for item in &gone {
        match item {
            Item::File { path, content, .. } => match std::fs::read_to_string(path) {
                // Leave files the user has since edited.
                Ok(now) if now != *content => println!("  kept {} (modified since install)", path.display()),
                Ok(_) => {
                    std::fs::remove_file(path)?;
                    // Tidy the `skills/todai` directory we created.
                    if let Some(dir) = path.parent().filter(|d| d.ends_with("skills/todai")) {
                        let _ = std::fs::remove_dir(dir);
                    }
                }
                Err(_) => {}
            },
            Item::Symlink { path, target, .. } => {
                let ours = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_symlink())
                    && std::fs::read_link(path).is_ok_and(|t| t == *target);
                if ours {
                    std::fs::remove_file(path)?;
                }
            }
            Item::ClaudeHooks { path, .. } if path.exists() => {
                edit_json(path, remove_claude_hooks)?;
                // Don't leave behind a settings file that only existed for us.
                if std::fs::read_to_string(path).is_ok_and(|s| s.trim() == "{}") {
                    std::fs::remove_file(path)?;
                }
            }
            Item::ClaudeHooks { .. } => {}
        }
    }
    manifest.save()?;
    println!("Removed.");
    Ok(())
}

// ---- Claude settings.json hooks ----

const HOOK_EVENTS: [(&str, Option<&str>, &str); 2] =
    [("PostToolUse", Some("*"), "post-tool-use"), ("Stop", None, "stop")];

fn is_ours(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .is_some_and(|hs| hs.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains(" hook claude "))))
}

fn add_claude_hooks(v: &mut Value, bin: &str) {
    remove_claude_hooks(v); // replace, so a changed binary path is picked up
    let hooks = v.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    for (event, matcher, sub) in HOOK_EVENTS {
        let mut entry = json!({ "hooks": [{ "type": "command", "command": format!("{bin} hook claude {sub}") }] });
        if let Some(m) = matcher {
            entry["matcher"] = json!(m);
        }
        let list = hooks.as_object_mut().unwrap().entry(event).or_insert_with(|| json!([]));
        if let Some(a) = list.as_array_mut() {
            a.push(entry);
        }
    }
}

fn remove_claude_hooks(v: &mut Value) {
    let Some(hooks) = v.get_mut("hooks").and_then(Value::as_object_mut) else { return };
    for (event, _, _) in HOOK_EVENTS {
        if let Some(list) = hooks.get_mut(event).and_then(Value::as_array_mut) {
            list.retain(|e| !is_ours(e));
            if list.is_empty() {
                hooks.remove(event);
            }
        }
    }
    if hooks.is_empty() {
        v.as_object_mut().unwrap().remove("hooks");
    }
}

/// Reads a JSON object file (missing = `{}`), applies `f`, writes it back.
fn edit_json(path: &Path, f: impl FnOnce(&mut Value)) -> Result<()> {
    let mut v: Value = match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => {
            serde_json::from_str(&s).with_context(|| format!("{} is not valid JSON", path.display()))?
        }
        _ => json!({}),
    };
    if !v.is_object() {
        bail!("{} is not a JSON object", path.display());
    }
    f(&mut v);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}

// ---- manifest ----

#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    items: Vec<Item>,
}

impl Manifest {
    fn path() -> Option<PathBuf> {
        Some(config::state_dir()?.join("installs.json"))
    }

    fn load() -> Manifest {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<()> {
        let path = Self::path().context("no state dir")?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

// ---- prompts and environment ----

fn ask_agents() -> Result<Vec<Agent>> {
    let home = home()?;
    let detected = |dir: &str, exe: &str| home.join(dir).exists() || on_path(exe);
    let rows = [
        (Agent::Claude, "Claude Code", "skill + hooks: answers arrive between tool calls; it won't stop with unread answers", detected(".claude", "claude")),
        (Agent::Pi, "pi", "skill + extension: answers steer the running agent, or wake it when idle", detected(".pi", "pi")),
        (Agent::Generic, "Other agents", "skill in .agents/skills (Codex, OpenCode, ...); the agent polls with inbox/wait", false),
    ];
    println!("Which agents should todai integrate with?");
    for (i, (_, name, what, found)) in rows.iter().enumerate() {
        let mark = if *found { "  (detected)" } else { "" };
        println!("  [{}] {name}{mark}\n      {what}", i + 1);
    }
    let default: Vec<String> = rows.iter().enumerate().filter(|r| r.1.3).map(|(i, _)| (i + 1).to_string()).collect();
    let default = if default.is_empty() { "3".to_owned() } else { default.join(",") };
    let answer = prompt(&format!("Choose, comma separated [{default}]: "))?;
    let answer = if answer.is_empty() { default } else { answer };
    let mut agents = Vec::new();
    for part in answer.split([',', ' ']).filter(|s| !s.is_empty()) {
        match part.parse::<usize>().ok().and_then(|n| rows.get(n.wrapping_sub(1))) {
            Some(r) if !agents.contains(&r.0) => agents.push(r.0),
            Some(_) => {}
            None => bail!("not a choice: {part}"),
        }
    }
    Ok(agents)
}

fn ask_scope(project: &Path) -> Result<Scope> {
    println!("Install for:");
    println!("  [1] this project  {}  (only sessions in this repository)", project.display());
    println!("  [2] your user     (every project)");
    match prompt("Choose [1]: ")?.as_str() {
        "" | "1" => Ok(Scope::Project),
        "2" => Ok(Scope::User),
        other => bail!("not a choice: {other}"),
    }
}

fn confirm(q: &str) -> Result<bool> {
    Ok(matches!(prompt(&format!("{q} [y/N] "))?.to_lowercase().as_str(), "y" | "yes"))
}

fn prompt(q: &str) -> Result<String> {
    print!("{q}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")
}

fn on_path(exe: &str) -> bool {
    which(exe).is_some()
}

fn which(exe: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.is_file())
}

/// How installed hooks and extensions invoke todai: plain `todai` when that
/// resolves to this binary, otherwise its absolute path.
fn bin_command() -> String {
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    match (which("todai").and_then(|p| p.canonicalize().ok()), me) {
        (Some(found), Some(me)) if found == me => "todai".to_owned(),
        (_, Some(me)) => me.display().to_string(),
        _ => "todai".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_hooks_merge_and_remove_cleanly() {
        let mut v = json!({
            "model": "x",
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }] }
        });
        add_claude_hooks(&mut v, "todai");
        add_claude_hooks(&mut v, "/opt/todai"); // re-install replaces, never duplicates
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "*");
        assert_eq!(v["hooks"]["PostToolUse"][0]["hooks"][0]["command"], "/opt/todai hook claude post-tool-use");

        remove_claude_hooks(&mut v);
        assert_eq!(v, json!({
            "model": "x",
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }] }
        }));
        let mut only_ours = json!({});
        add_claude_hooks(&mut only_ours, "todai");
        remove_claude_hooks(&mut only_ours);
        assert_eq!(only_ours, json!({}));
    }

    #[test]
    fn plan_paths_per_scope() {
        let items = plan(&[Agent::Pi, Agent::Generic, Agent::Claude], Scope::Project, Path::new("/p"), "todai");
        let paths: Vec<_> = items.iter().map(|i| i.path().display().to_string()).collect();
        assert_eq!(paths, [
            "/p/.agents/skills/todai/SKILL.md",
            "/p/.pi/extensions/todai.ts",
            "/p/.claude/skills/todai",
            "/p/.claude/settings.local.json",
        ]);
        // One shared skill, recorded for every agent that needs it.
        assert!(matches!(&items[0], Item::File { agents, .. } if *agents == [Agent::Pi, Agent::Generic, Agent::Claude]));
        assert!(matches!(&items[2], Item::Symlink { target, .. } if target == Path::new("../../.agents/skills/todai")));

        let user = plan(&[Agent::Pi, Agent::Claude], Scope::User, Path::new("/h"), "todai");
        assert!(user.iter().any(|i| i.path().ends_with(".pi/agent/extensions/todai.ts")));
        assert!(user.iter().any(|i| matches!(i, Item::ClaudeHooks { path, .. } if path.ends_with(".claude/settings.json"))));
        let ext = items.iter().find_map(|i| match i {
            Item::File { path, content, .. } if path.ends_with("todai.ts") => Some(content),
            _ => None,
        });
        assert!(ext.unwrap().contains(r#"const TODAI = "todai";"#));
    }
}
