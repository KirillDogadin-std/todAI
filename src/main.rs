mod agent;
mod cli;
mod gui;
mod memory;

use todai_client::{config, ipc, remote, session};
use todai_core::{inbox, store};

use clap::Parser;

fn main() -> anyhow::Result<()> {
    cli::run(cli::Cli::parse())
}
