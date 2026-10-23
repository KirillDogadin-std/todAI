mod cli;
mod inbox;
mod store;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    cli::run(cli::Cli::parse())
}
