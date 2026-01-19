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

        /// Amend the previous commit
        #[arg(long)]
        amend: bool,

        /// Use the existing commit message without editing
        #[arg(long)]
        no_edit: bool,
    },

    /// Push commits to remote
    #[command(alias = "p")]
    Push {
        /// Force push with lease (safer)
        #[arg(short, long)]
        force: bool,

        /// Force push without lease (dangerous)
        #[arg(long)]
        force_dangerously: bool,
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
            Commands::Commit {
                message,
                amend,
                no_edit,
            } => commands::commit::run(message, amend, no_edit),
            Commands::Push {
                force,
                force_dangerously,
            } => commands::push::run(force, force_dangerously),
        }
    }
}
