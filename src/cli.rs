use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::{agent, inbox};
use crate::ipc::{self, Bind, Request};
use crate::store::{Actor, DecisionStatus, Kind, Log, State, unprocessed};

#[derive(Parser)]
#[command(version, about = "Decision & operator-todo board for agent workflows")]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Shorthand for `todai open FILE...`.
    files: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open FILEs in the board window: new tabs if it is running, otherwise
    /// start it in the background. No FILE just raises (or starts) the window.
    Open {
        files: Vec<PathBuf>,
        /// Run the window in this process instead of detaching.
        #[arg(long)]
        foreground: bool,
    },
    /// List the tabs open in the running window.
    Tabs,

    /// [agent] Start using a board: creates `.agents/todai/NAME.jsonl` at the repo
    /// root (NAME defaults to `board`), opens it, and prints its path.
    Attach {
        name: Option<String>,
        /// Don't open the window.
        #[arg(long)]
        no_window: bool,
    },
    /// [agent] Print how an agent should use todai.
    Guide,
    /// Install todai into your coding agents, at the scope you choose.
    /// Interactive without flags; shows every change before making it.
    Setup {
        /// Agents to set up (repeat or comma separate).
        #[arg(long = "agent", value_enum, value_delimiter = ',')]
        agents: Vec<crate::agent::setup::Agent>,
        #[arg(long, value_enum)]
        scope: Option<crate::agent::setup::Scope>,
        /// Don't ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Only show what would change.
        #[arg(long)]
        dry_run: bool,
        /// Undo what setup installed (all, or only the given --agent).
        #[arg(long)]
        remove: bool,
    },
    /// Internal: agent hook entry points, e.g. `todai hook claude stop`.
    #[command(hide = true)]
    Hook { agent: String, event: String },
    /// Internal: the detached window process.
    #[command(name = "__gui", hide = true)]
    Gui { files: Vec<PathBuf> },

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
    /// [operator] Remove finished items (and their elaborations) from the board.
    Trash {
        file: PathBuf,
        #[arg(required = true)]
        ids: Vec<String>,
    },
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
    let cmd = cli.cmd.unwrap_or(Cmd::Open { files: cli.files, foreground: false });
    match cmd {
        Cmd::Open { files, foreground } => {
            let files = files.iter().map(std::path::absolute).collect::<std::io::Result<Vec<_>>>()?;
            if foreground {
                gui_main(files)?;
            } else {
                open(files)?;
            }
        }
        Cmd::Gui { files } => gui_main(files)?,
        Cmd::Attach { name, no_window } => {
            let path = agent::board_path(name.as_deref().unwrap_or("board"))?;
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
            if !no_window && let Err(e) = open(vec![path.clone()]) {
                eprintln!("todai: could not open the window ({e:#}); the board still works");
            }
            println!("{}{}", agent::ATTACHED_MARKER, path.display());
            println!("Pass this path as BOARD to todai commands. How to use it: `todai guide`.");
        }
        Cmd::Guide => print!("{}", agent::GUIDE),
        Cmd::Setup { agents, scope, yes, dry_run, remove } => {
            agent::setup::run(agent::setup::Args { agents, scope, yes, dry_run, remove })?
        }
        Cmd::Hook { agent: a, event } => {
            // Hooks must never break the agent: report, but always succeed.
            let result = match a.as_str() {
                "claude" => agent::claude::run(&event),
                other => Err(anyhow::anyhow!("unknown agent {other}")),
            };
            if let Err(e) = result {
                eprintln!("todai hook: {e:#}");
            }
        }
        Cmd::Tabs => match ipc::send(&ipc::socket_path(), &Request::List)? {
            Some(r) => {
                for (i, t) in r.tabs.iter().enumerate() {
                    println!("{} {}", if i == r.active { "*" } else { " " }, t.display());
                }
            }
            None => println!("todai: window not running"),
        },

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
            let registered = agent::claude::register_waiter(log.path());
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
                    drop(registered); // exit() skips destructors
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
        Cmd::Trash { file, ids } => {
            log(&file)?.append(Actor::Operator, |_| Ok(Kind::Trash { ids }))?;
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

/// Hands `files` to the running window, or starts one in the background.
fn open(files: Vec<PathBuf>) -> Result<()> {
    let socket = ipc::socket_path();
    let first = files.first().map_or(Request::Focus, |f| Request::Open { path: f.clone() });
    if ipc::send(&socket, &first)?.is_some() {
        for f in files.iter().skip(1) {
            ipc::send(&socket, &Request::Open { path: f.clone() })?;
        }
        return Ok(());
    }
    spawn_window(&socket, &files)
}

/// Starts `todai __gui` in its own session so it outlives the caller (a
/// terminal or an agent's shell), then waits until it answers on the socket.
fn spawn_window(socket: &std::path::Path, files: &[PathBuf]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let log_path = crate::config::state_dir().map(|d| d.join("gui.log"));
    let stderr = match &log_path {
        Some(p) => {
            std::fs::create_dir_all(p.parent().unwrap())?;
            Stdio::from(std::fs::OpenOptions::new().create(true).append(true).open(p)?)
        }
        None => Stdio::null(),
    };
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("__gui").args(files).stdin(Stdio::null()).stdout(Stdio::null()).stderr(stderr);
    // SAFETY: setsid is async-signal-safe and touches no Rust state.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            let hint = log_path.map(|p| format!("; see {}", p.display())).unwrap_or_default();
            bail!("window exited at startup ({status}){hint}");
        }
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("window did not start within 5s");
}

/// The window process: claims the socket, or forwards to whoever has it.
fn gui_main(files: Vec<PathBuf>) -> Result<()> {
    let socket = ipc::socket_path();
    match ipc::bind(&socket)? {
        Bind::Listening(listener) => crate::gui::run(listener, socket, files),
        // Lost a startup race to another window; hand it our files.
        Bind::Taken => {
            for f in &files {
                ipc::send(&socket, &Request::Open { path: f.clone() })?;
            }
            Ok(())
        }
    }
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
    print!("{}", inbox::summarize(log.path(), &events, &pending));
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
            DecisionStatus::Withdrawn => made.push(format!("{}  {}  (withdrawn)", d.id, d.title)),
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
