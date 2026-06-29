mod ai;
mod args;
mod browser;
mod clipboard;
mod commands;
mod config;
mod git;
mod repo_config;
mod repo_setup;
mod ui;

use clap::Parser;
use clap::ValueEnum;
use miette::Result;

fn main() -> Result<()> {
    // Intercept the internal `__complete <kind>` backend before clap parses.
    // It deliberately is not a clap subcommand: clap_complete would otherwise
    // emit it as a visible `gx <TAB>` candidate (it ignores `hide`). Handling it
    // here keeps it out of the command tree while still backing the generated
    // dynamic completion helpers.
    let mut raw = std::env::args().skip(1);
    if raw.next().as_deref() == Some("__complete") {
        // Unknown/missing kinds print nothing and exit 0 so completion never
        // breaks the user's shell.
        if let Some(kind) = raw
            .next()
            .and_then(|k| args::CompleteKind::from_str(&k, true).ok())
        {
            return commands::setup::run_complete(kind);
        }
        return Ok(());
    }

    let cli = args::Cli::parse();
    cli.command.run()
}
