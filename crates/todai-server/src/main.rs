use clap::Parser;

/// Serves todai boards over HTTP to operator frontends (the desktop window's
/// remote mode, or a web app). See docs/server.md.
#[derive(Parser)]
#[command(name = "todai-server", version)]
struct Cli {
    #[command(flatten)]
    args: todai_server::Args,
}

fn main() -> anyhow::Result<()> {
    todai_server::run(Cli::parse().args)
}
