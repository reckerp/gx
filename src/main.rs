mod args;
mod commands;
mod config;
mod git;
mod ui;

use clap::Parser;
use miette::Result;

fn main() -> Result<()> {
    let cli = args::Cli::parse();
    cli.command.run()
}
