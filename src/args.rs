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
    Checkout {
        /// Create a new branch and switch to it
        #[arg(short = 'b', long)]
        create_branch: Option<String>,

        /// Branch, commit, or tag to checkout
        query: Option<String>,
    },

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

        /// Generate commit message using AI
        #[arg(long)]
        ai: bool,
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

    /// Stash changes
    #[command(alias = "st")]
    Stash {
        #[command(subcommand)]
        action: Option<StashCommands>,
    },

    /// View commit history
    #[command(alias = "l")]
    Log {
        /// Maximum number of commits to show
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },

    /// Generate shell aliases from config
    Setup,

    /// Pass-through to git for unrecognized commands
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
pub enum StashCommands {
    /// Save changes to stash (default action)
    Push {
        /// Stash message
        #[arg(short, long)]
        message: Option<String>,

        /// Include untracked files
        #[arg(short, long)]
        untracked: bool,
    },

    /// List all stashes
    List,

    /// Apply and remove a stash
    Pop {
        /// Stash reference (e.g., 0 or stash@{0})
        stash: Option<String>,
    },

    /// Apply a stash without removing it
    Apply {
        /// Stash reference (e.g., 0 or stash@{0})
        stash: Option<String>,
    },

    /// Delete a stash
    Drop {
        /// Stash reference (e.g., 0 or stash@{0})
        stash: Option<String>,
    },

    /// Remove all stashes
    Clear,

    /// Show the diff of a stash
    Show {
        /// Stash reference (e.g., 0 or stash@{0})
        stash: Option<String>,
    },

    /// Create a branch from a stash
    Branch {
        /// Name for the new branch
        name: String,

        /// Stash reference (e.g., 0 or stash@{0})
        stash: Option<String>,
    },
}

impl Commands {
    pub fn run(self) -> Result<()> {
        match self {
            Self::Checkout {
                create_branch,
                query,
            } => commands::checkout::run(create_branch, query),
            Self::External(args) => commands::external::run(args),
            Commands::Status => commands::status::run(),
            Commands::Add { interactive, paths } => commands::add::run(interactive, paths),
            Commands::Commit {
                message,
                amend,
                no_edit,
                ai,
            } => commands::commit::run(message, amend, no_edit, ai),
            Commands::Push {
                force,
                force_dangerously,
            } => commands::push::run(force, force_dangerously),
            Commands::Stash { action } => match action {
                None => commands::stash::run_interactive(),
                Some(StashCommands::Push { message, untracked }) => {
                    commands::stash::run_push(message, untracked)
                }
                Some(StashCommands::List) => commands::stash::run_list(),
                Some(StashCommands::Pop { stash }) => commands::stash::run_pop(stash),
                Some(StashCommands::Apply { stash }) => commands::stash::run_apply(stash),
                Some(StashCommands::Drop { stash }) => commands::stash::run_drop(stash),
                Some(StashCommands::Clear) => commands::stash::run_clear(),
                Some(StashCommands::Show { stash }) => commands::stash::run_show(stash),
                Some(StashCommands::Branch { name, stash }) => {
                    commands::stash::run_branch(name, stash)
                }
            },
            Commands::Log { limit } => commands::log::run(limit),
            Commands::Setup => commands::setup::run(),
        }
    }
}
