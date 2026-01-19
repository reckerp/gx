use crate::git;
use crate::git::GitError;
use crate::git::commit::CommitOptions;
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

    #[error("--no-edit can only be used with --amend")]
    #[diagnostic(
        code(gx::commit::no_edit_without_amend),
        help("Use --amend flag when using --no-edit")
    )]
    NoEditWithoutAmend,
}

pub fn run(message: Option<String>, amend: bool, no_edit: bool) -> Result<()> {
    if no_edit && !amend {
        return Err(CommitError::NoEditWithoutAmend.into());
    }

    if !amend {
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
    }

    let options = CommitOptions {
        message: message.as_deref(),
        amend,
        no_edit,
    };

    git::commit::create_commit(options).map_err(CommitError::GitError)?;

    Ok(())
}
