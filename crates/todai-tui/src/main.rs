//! `todai-tui`: the todai board in a terminal. Same board, keys and
//! behaviour as the desktop window, for SSH sessions and machines without
//! a GL stack. While it runs it serves the window socket (if no window has
//! it), so `todai attach` and `todai FILE` open boards here.

mod app;
mod board;
mod edit;
mod markdown;
mod text;
mod ui;

#[cfg(test)]
mod tests;

use std::io::stdout;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use anyhow::{Context, Result, bail};
use clap::Parser;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange,
    EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};
use todai_client::ipc::{self, Bind};
use todai_client::tab::{Location, Wake};

use app::App;

/// The todai board in a terminal.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Boards to open (with --server: board names, PROJECT/NAME or ids;
    /// all of the server's boards if none). Last run's tabs come back too.
    files: Vec<PathBuf>,
    /// Open boards from a todai server, e.g. http://host:7341.
    #[arg(long)]
    server: Option<String>,
    /// The server's token (`todai-server token` on it); saved for next
    /// time. Defaults to the saved one, or $TODAI_TOKEN.
    #[arg(long, requires = "server")]
    token: Option<String>,
}

enum Msg {
    Term(event::Event),
    Wake,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (remote, follow) = match &args.server {
        Some(server) => remote_boards(server, args.token.as_deref(), &args.files)?,
        None => (Vec::new(), None),
    };
    let files = if args.server.is_some() {
        Vec::new()
    } else {
        args.files.iter().map(std::path::absolute).collect::<std::io::Result<Vec<_>>>()?
    };
    for f in &files {
        todai_client::remote::register_local(f);
    }

    let (tx, rx) = mpsc::channel::<Msg>();
    let wake: Wake = {
        let tx = tx.clone();
        Arc::new(move || {
            let _ = tx.send(Msg::Wake);
        })
    };

    // Serve the socket unless a window (or another TUI) already does.
    let socket = ipc::socket_path();
    let (requests, owns_socket, notice) = match ipc::bind(&socket) {
        Ok(Bind::Listening(listener)) => {
            let (req_tx, req_rx) = mpsc::channel();
            let w = wake.clone();
            ipc::serve(listener, req_tx, move || w());
            (Some(req_rx), true, None)
        }
        Ok(Bind::Taken) => (None, false, Some("another todai window has the socket: boards agents attach open there".to_owned())),
        Err(e) => (None, false, Some(format!("no socket ({e:#}): boards agents attach won't open here"))),
    };

    let mut app = App::new(wake, requests, true);
    app.notice = notice;
    app.restore();
    for f in files {
        app.open_file(f);
    }
    let known = remote.iter().filter_map(|l| match l {
        Location::Remote { board, .. } => Some(board.clone()),
        Location::File(_) => None,
    });
    let known: Vec<String> = known.collect();
    for loc in remote {
        app.open_or_focus(loc);
    }
    if let Some(client) = follow {
        app.follow(client, known);
    }

    let result = run(&mut app, tx, rx);
    if owns_socket {
        let _ = std::fs::remove_file(&socket);
    }
    result
}

fn run(app: &mut App, tx: mpsc::Sender<Msg>, rx: mpsc::Receiver<Msg>) -> Result<()> {
    let mut terminal = ratatui::try_init().context("starting the terminal UI")?;
    // Shift+Enter and unambiguous Esc where the terminal supports it (kitty protocol).
    let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
    let _ = execute!(stdout(), EnableMouseCapture, EnableBracketedPaste, EnableFocusChange);
    if enhanced {
        let _ = execute!(stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Msg::Term(ev)).is_err() {
                break;
            }
        }
    });

    let result = (|| -> Result<()> {
        loop {
            app.tick();
            terminal.draw(|f| ui::draw(f, app))?;
            if app.quit {
                return Ok(());
            }
            match rx.recv() {
                Ok(Msg::Term(ev)) => app.on_event(ev),
                Ok(Msg::Wake) => {}
                Err(_) => return Ok(()),
            }
        }
    })();

    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
    ratatui::restore();
    result
}

/// Resolves `--server` boards and saves the token, before the screen is
/// taken. Without board names, also the client whose server to follow.
fn remote_boards(
    server: &str,
    token: Option<&str>,
    wanted: &[PathBuf],
) -> Result<(Vec<Location>, Option<todai_client::remote::Client>)> {
    let server = server.trim_end_matches('/');
    let token = token.map(str::to_owned).or_else(|| std::env::var("TODAI_TOKEN").ok().filter(|t| !t.is_empty()));
    let client = match &token {
        Some(t) => todai_client::remote::Client::new(server, t),
        None => match todai_client::remote::client_for(server) {
            Some(c) => c,
            None => bail!("no token for {server}: pass --token (run `todai-server token` there)"),
        },
    };
    client.check()?;
    let wanted: Vec<String> = wanted.iter().map(|w| w.to_string_lossy().into_owned()).collect();
    let picked = todai_client::remote::pick_boards(&client, &wanted)?;
    if let Some(t) = &token {
        todai_client::remote::save_token(server, t)?;
    }
    let locations = picked
        .into_iter()
        .map(|b| Location::Remote { server: client.base().to_owned(), board: b.id, name: b.name })
        .collect();
    Ok((locations, wanted.is_empty().then_some(client)))
}
