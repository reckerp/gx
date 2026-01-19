pub mod branch;
pub mod commit;
pub mod fetch;
pub mod git_exec;
pub mod push;
pub mod staging;
pub mod status;
pub mod time;

use git2::Repository;
use miette::Diagnostic;
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum GitError {
    #[error("Git executable not found.")]
    #[diagnostic(
        code(gx::git::not_found),
        help("Ensure that 'git' is installed and available in your PATH.")
    )]
    NotFound(#[source] std::io::Error),

    #[error("Failed to execute git command.")]
    #[diagnostic(code(gx::git::execution_failed))]
    IoError(#[from] std::io::Error),

    #[error("Not in git repository")]
    #[diagnostic(code(gx::git::not_in_repo))]
    NotInRepo,

    #[error("Not on branch")]
    #[diagnostic(code(gx::git::not_on_branch))]
    NotOnBranch,

    #[error("Git command failed: {0}")]
    #[diagnostic(code(gx::git::command_failed))]
    CommandFailed(String),

    #[error("{0}")]
    #[diagnostic(code(gx::git::git2_error))]
    Git2Error(#[from] git2::Error),
}

fn get_repo() -> Result<git2::Repository, GitError> {
    Repository::discover(".").map_err(|e| {
        if e.code() == git2::ErrorCode::NotFound {
            GitError::NotInRepo
        } else {
            GitError::Git2Error(e)
        }
    })
}
