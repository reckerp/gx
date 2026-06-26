mod args;
mod browser;
mod clipboard;
mod commands;
mod config;
mod git;
mod repo_setup;
mod ui;

use clap::Parser;
use miette::Result;

fn main() -> Result<()> {
    let cli = args::Cli::parse();
    cli.command.run()
}
