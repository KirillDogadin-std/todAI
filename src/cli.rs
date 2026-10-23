use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::inbox;
use crate::store::{Actor, DecisionStatus, Kind, Log, State, unprocessed};

#[derive(Parser)]
#[command(version, about = "Decision & operator-todo board for agent workflows")]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Shorthand for `todai open FILE`.
    file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open FILE in the board window (new tab if already running).
    Open { file: PathBuf },

    /// [agent] Ask the operator to decide something. Prints the new id.
    AddDecision {
        file: PathBuf,
        /// Short form, ideally under 80 chars.
        title: String,
        #[arg(short, long)]
        context: Option<String>,
        /// Quick-pick answer; repeatable.
        #[arg(short, long = "option")]
        options: Vec<String>,
    },
    /// [agent] Ask the operator to do something by hand. Prints the new id.
    AddTodo {
        file: PathBuf,
        title: String,
        #[arg(short, long)]
        context: Option<String>,
    },
    /// [agent] Answer an elaboration request. TEXT is markdown; `-` or omitted reads stdin.
    Elaborate { file: PathBuf, rid: String, text: Option<String> },
    /// [agent] Retract a decision that is no longer needed.
    Withdraw { file: PathBuf, id: String },
    /// [agent] Print unprocessed operator updates and ack them.
    Inbox {
        file: PathBuf,
        #[arg(long)]
        no_ack: bool,
    },
    /// [agent] Mark operator updates as processed up to SEQ (default: latest).
    Ack {
        file: PathBuf,
        #[arg(long)]
        upto: Option<u64>,
    },
    /// [agent] Block until the operator updates FILE, then print like `inbox`.
    /// Exits 3 on timeout.
    Wait {
        file: PathBuf,
        /// Seconds; waits forever if omitted.
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long)]
        no_ack: bool,
    },
    /// Print the current board.
    Show {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// [operator] Resolve a decision.
    Resolve { file: PathBuf, id: String, text: String },
    /// [operator] Reopen a resolved decision.
    Reopen { file: PathBuf, id: String },
    /// [operator] Mark a todo done.
    Done {
        file: PathBuf,
        id: String,
        #[arg(short, long)]
        note: Option<String>,
    },
    /// [operator] Mark a todo not done.
    Undone { file: PathBuf, id: String },
    /// [operator] Request an elaboration on part of an item.
    Ask {
        file: PathBuf,
        target: String,
        selection: String,
        #[arg(short, long)]
        note: Option<String>,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    let cmd = match (cli.cmd, cli.file) {
        (Some(c), _) => c,
        (None, Some(file)) => Cmd::Open { file },
        (None, None) => bail!("usage: todai FILE | todai <command> (see --help)"),
    };
    match cmd {
        Cmd::Open { file } => bail!("GUI not implemented yet ({})", file.display()),

        Cmd::AddDecision { file, title, context, options } => {
            let e = log(&file)?.append(Actor::Agent, |s| {
                Ok(Kind::DecisionAdd { id: s.next_decision_id(), title, context, options })
            })?;
            print_id(&e.kind);
        }
        Cmd::AddTodo { file, title, context } => {
            let e = log(&file)?.append(Actor::Agent, |s| {
                Ok(Kind::TodoAdd { id: s.next_todo_id(), title, context })
            })?;
            print_id(&e.kind);
        }
        Cmd::Elaborate { file, rid, text } => {
            let text_md = match text.as_deref() {
                None | Some("-") => {
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s)?;
                    s
                }
                Some(t) => t.to_owned(),
            };
            if text_md.trim().is_empty() {
                bail!("empty elaboration");
            }
            log(&file)?.append(Actor::Agent, |_| Ok(Kind::ElabAdd { rid, text_md }))?;
        }
        Cmd::Withdraw { file, id } => {
            log(&file)?.append(Actor::Agent, |_| Ok(Kind::DecisionWithdraw { id }))?;
        }
        Cmd::Inbox { file, no_ack } => {
            if !print_inbox(&log(&file)?, !no_ack)? {
                println!("todai: no operator updates");
            }
        }
        Cmd::Ack { file, upto } => {
            log(&file)?.append(Actor::Agent, |s| Ok(Kind::Ack { upto_seq: upto.unwrap_or(s.last_seq) }))?;
        }
        Cmd::Wait { file, timeout, no_ack } => {
            let log = log(&file)?;
            let deadline = timeout.map(|t| Instant::now() + Duration::from_secs(t));
            // Polling the file size is cheap and avoids inotify edge cases with
            // editors and network filesystems; the agent is not latency-bound here.
            let mut last_len = None;
            loop {
                let len = std::fs::metadata(log.path()).map(|m| m.len()).ok();
                if len != last_len {
                    last_len = len;
                    if print_inbox(&log, !no_ack)? {
                        return Ok(());
                    }
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    println!("todai: wait timed out");
                    std::process::exit(3);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        Cmd::Show { file, json } => {
            let (_, state) = log(&file)?.load_state()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&state)?);
            } else {
                print!("{}", render(&state));
            }
        }

        Cmd::Resolve { file, id, text } => {
            log(&file)?.append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id, text }))?;
        }
        Cmd::Reopen { file, id } => {
            log(&file)?.append(Actor::Operator, |_| Ok(Kind::DecisionReopen { id }))?;
        }
        Cmd::Done { file, id, note } => {
            log(&file)?.append(Actor::Operator, |_| Ok(Kind::TodoDone { id, note }))?;
        }
        Cmd::Undone { file, id } => {
            log(&file)?.append(Actor::Operator, |_| Ok(Kind::TodoUndone { id }))?;
        }
        Cmd::Ask { file, target, selection, note } => {
            let e = log(&file)?.append(Actor::Operator, |s| {
                Ok(Kind::ElabRequest { rid: s.next_elab_id(), target, selection, note })
            })?;
            print_id(&e.kind);
        }
    }
    Ok(())
}

fn log(file: &PathBuf) -> Result<Log> {
    Ok(Log::new(std::path::absolute(file)?))
}

fn print_id(kind: &Kind) {
    match kind {
        Kind::DecisionAdd { id, .. } | Kind::TodoAdd { id, .. } => println!("{id}"),
        Kind::ElabRequest { rid, .. } => println!("{rid}"),
        _ => {}
    }
}

/// Prints unprocessed operator events, acking exactly the ones shown.
/// Returns whether there were any.
fn print_inbox(log: &Log, ack: bool) -> Result<bool> {
    let (events, state) = log.load_state()?;
    let pending = unprocessed(&events, &state);
    let Some(upto) = pending.last().map(|e| e.seq) else {
        return Ok(false);
    };
    print!("{}", inbox::summarize(log.path(), &pending, &state));
    if ack {
        log.append(Actor::Agent, |_| Ok(Kind::Ack { upto_seq: upto }))?;
    }
    Ok(true)
}

fn render(s: &State) -> String {
    let mut out = String::new();
    let mut section = |name: &str, lines: Vec<String>| {
        out.push_str(name);
        out.push('\n');
        if lines.is_empty() {
            out.push_str("  (none)\n");
        }
        for l in lines {
            out.push_str("  ");
            out.push_str(&l);
            out.push('\n');
        }
    };
    let elabs_for = |id: &str| -> Vec<String> {
        s.elabs
            .iter()
            .filter(|r| r.target == id)
            .map(|r| match &r.answer {
                Some(a) => format!("    > [{}] \"{}\": {}", r.rid, r.selection, a.trim()),
                None => format!("    > [{}] \"{}\": (waiting for agent)", r.rid, r.selection),
            })
            .collect()
    };

    let mut pending = Vec::new();
    let mut made = Vec::new();
    for d in &s.decisions {
        match &d.status {
            DecisionStatus::Pending => {
                let mut l = format!("{}  {}", d.id, d.title);
                if !d.options.is_empty() {
                    l.push_str(&format!("  [{}]", d.options.join(" | ")));
                }
                pending.push(l);
                pending.extend(elabs_for(&d.id));
            }
            DecisionStatus::Resolved { text, .. } => made.push(format!("{}  {}: {text}", d.id, d.title)),
            DecisionStatus::Withdrawn => {}
        }
    }
    let mut todos = Vec::new();
    let mut done = Vec::new();
    for t in &s.todos {
        match &t.done {
            None => {
                todos.push(format!("{}  {}", t.id, t.title));
                todos.extend(elabs_for(&t.id));
            }
            Some(note) => done.push(match note {
                Some(n) => format!("{}  {} ({n})", t.id, t.title),
                None => format!("{}  {}", t.id, t.title),
            }),
        }
    }
    section("Decisions", pending);
    section("Todos", todos);
    section("Decisions made", made);
    section("Done", done);
    out
}
