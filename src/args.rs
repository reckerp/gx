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

    /// Manage workspaces (git worktrees)
    #[command(alias = "ws")]
    Workspace {
        #[command(subcommand)]
        action: Option<WorkspaceCommands>,
    },

    /// Dashboard of your open pull requests
    #[command(aliases = ["prs", "pullrequest", "pullrequests"])]
    Pr {
        #[command(subcommand)]
        action: Option<PrCommands>,
    },

    /// Configure repo-specific workspace setup
    #[command(alias = "onboard")]
    Onboarding,

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

#[derive(Subcommand)]
pub enum PrCommands {
    /// Print open PRs grouped by state (non-interactive)
    List,
}

#[derive(Subcommand)]
pub enum WorkspaceCommands {
    /// Create a new workspace
    #[command(alias = "create", alias = "add")]
    New {
        /// Name of the workspace (also used as the branch name by default;
        /// '/' is replaced with '-' in the directory name)
        name: String,

        /// Base branch/commit/tag to create the new branch from (defaults to
        /// the matching remote branch, then origin's default branch, then HEAD)
        base: Option<String>,

        /// Branch to check out in the workspace (created if it doesn't exist)
        #[arg(short, long)]
        branch: Option<String>,

        /// Skip copying setup files (e.g. .env) into the new workspace
        #[arg(long)]
        no_setup: bool,

        /// Create the workspace but do not request shell navigation
        #[arg(long)]
        no_cd: bool,

        /// Skip fetching origin; resolve the base from local refs only
        #[arg(long)]
        no_fetch: bool,

        /// Copy staged file contents from the current workspace into the new
        /// one (optionally limited to PATHS)
        #[arg(long, num_args = 0.., value_name = "PATH")]
        from_staged: Option<Vec<String>>,

        /// Skip workspace creation hooks
        #[arg(long)]
        no_hooks: bool,

        /// Create the workspace with a detached HEAD instead of a new branch
        #[arg(long, conflicts_with_all = ["branch", "track"])]
        detach: bool,

        /// Set the base's remote branch as the new branch's upstream
        #[arg(long)]
        track: bool,
    },

    /// Switch to a workspace (prints its path; cd handled by 'gx setup' shell wrapper)
    #[command(alias = "switch", alias = "cd")]
    Go {
        /// Workspace to switch to (supports fuzzy matching, picker if omitted)
        query: Option<String>,
    },

    /// List all workspaces
    #[command(alias = "ls")]
    List,

    /// Update a workspace: fetch origin and rebase its branch onto
    /// origin's default branch (e.g. origin/main)
    #[command(alias = "up", alias = "sync")]
    Update {
        /// Workspace to update (defaults to the current one)
        query: Option<String>,

        /// Base to rebase onto (defaults to origin's default branch)
        base: Option<String>,
    },

    /// Remove a workspace
    #[command(alias = "rm", alias = "delete")]
    Remove {
        /// Workspace to remove (supports fuzzy matching, picker if omitted)
        query: Option<String>,

        /// Remove even if the workspace has uncommitted changes
        #[arg(short, long)]
        force: bool,

        /// Delete the associated local branch after removing the workspace
        #[arg(long)]
        delete_branch: bool,
    },

    /// Copy setup files (e.g. .env) from the main worktree into this workspace
    Setup,
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
            Commands::Onboarding => commands::onboarding::run(),
            Commands::Workspace { action } => match action {
                None => commands::workspace::run_interactive(),
                Some(WorkspaceCommands::New {
                    name,
                    base,
                    branch,
                    no_setup,
                    no_cd,
                    no_fetch,
                    from_staged,
                    no_hooks,
                    detach,
                    track,
                }) => commands::workspace::run_new(
                    name,
                    commands::workspace::NewWorkspaceOptions {
                        base,
                        branch,
                        no_setup,
                        no_cd,
                        no_fetch,
                        from_staged,
                        no_hooks,
                        detach,
                        track,
                    },
                ),
                Some(WorkspaceCommands::Go { query }) => commands::workspace::run_go(query),
                Some(WorkspaceCommands::List) => commands::workspace::run_list(),
                Some(WorkspaceCommands::Update { query, base }) => {
                    commands::workspace::run_update(query, base)
                }
                Some(WorkspaceCommands::Remove {
                    query,
                    force,
                    delete_branch,
                }) => commands::workspace::run_remove(query, force, delete_branch),
                Some(WorkspaceCommands::Setup) => commands::workspace::run_setup(),
            },
            Commands::Pr { action } => match action {
                None => commands::pr::run_interactive(),
                Some(PrCommands::List) => commands::pr::run_list(),
            },
            Commands::Setup => commands::setup::run(),
        }
    }
}
