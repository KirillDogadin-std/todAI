mod cli;
mod config;
mod gui;
mod inbox;
mod ipc;
mod session;
mod store;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    cli::run(cli::Cli::parse())
}
