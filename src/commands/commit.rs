use crate::git;
use crate::git::GitError;
use crate::ui;
use miette::{Diagnostic, Result};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum CommitError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::commit::git_error), help("Are you in a git repository?"))]
    GitError(#[from] GitError),

    #[error("Commit aborted")]
    #[diagnostic(code(gx::commit::aborted))]
    Aborted,

    #[error("Nothing to commit")]
    #[diagnostic(
        code(gx::commit::nothing_to_commit),
        help("No staged or unstaged changes.")
    )]
    NothingToCommit,
}

pub fn run(message: Option<String>) -> Result<()> {
    let has_staged = git::status::has_staged_files().map_err(CommitError::GitError)?;

    if !has_staged {
        let (_, unstaged) = git::status::get_status_files().map_err(CommitError::GitError)?;

        if unstaged.is_empty() {
            return Err(CommitError::NothingToCommit.into());
        }

        let confirmed = ui::confirm::run("No staged files. Stage all changes?")?;

        if !confirmed {
            return Err(CommitError::Aborted.into());
        }

        git::staging::stage_all().map_err(CommitError::GitError)?;
    }

    git::commit::create_commit(message.as_deref()).map_err(CommitError::GitError)?;

    Ok(())
}
