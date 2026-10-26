mod agent;
mod cli;
mod config;
mod gui;
mod ipc;
mod session;

use todai_core::{inbox, store};

use clap::Parser;

fn main() -> anyhow::Result<()> {
    cli::run(cli::Cli::parse())
}
