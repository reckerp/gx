use crate::git;
use crate::git::GitError;
use crate::git::push::PushOptions;
use miette::{Diagnostic, Result};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum PushError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::push::git_error), help("Are you in a git repository?"))]
    GitError(#[from] GitError),
}

pub fn run(force: bool, force_dangerously: bool) -> Result<()> {
    let options = PushOptions {
        force,
        force_dangerously,
    };

    git::push::push(options).map_err(PushError::GitError)?;

    Ok(())
}
