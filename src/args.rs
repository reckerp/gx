use crate::commands;
use clap::{Parser, Subcommand};
use miette::Result;

#[derive(Parser)]
#[command(name = "gx", about = "GX - Smart Git CLI", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Checkout/Switch a branch|commit|tag
    #[command(alias = "co", aliases = ["switch"])]
    Checkout { query: Option<String> },

    /// Show repository status
    #[command(alias = "s")]
    Status,

    /// Stage files for commit
    #[command(alias = "a")]
    Add {
        /// Interactive mode - select files to stage
        #[arg(short, long)]
        interactive: bool,

        /// Files/folders to stage (stages all if omitted)
        paths: Vec<String>,
    },

    /// Create a commit
    #[command(alias = "c")]
    Commit {
        /// Commit message (opens editor if omitted)
        message: Option<String>,
    },

    /// Pass-through to git for unrecognized commands
    #[command(external_subcommand)]
    External(Vec<String>),
}

impl Commands {
    pub fn run(self) -> Result<()> {
        match self {
            Self::Checkout { query } => commands::checkout::run(query),
            Self::External(args) => commands::external::run(args),
            Commands::Status => commands::status::run(),
            Commands::Add { interactive, paths } => commands::add::run(interactive, paths),
            Commands::Commit { message } => commands::commit::run(message),
        }
    }
}
