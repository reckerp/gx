mod args;
mod commands;
mod git;

use clap::Parser;
use miette::Result;

fn main() -> Result<()> {
    let cli = args::Cli::parse();
    cli.command.run()
}
