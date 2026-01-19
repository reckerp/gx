use crate::git::{GitError, status};
use crate::ui::status::render_status;
use miette::{Diagnostic, Result};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum StatusError {
    #[error("Could not read git branches")]
    #[diagnostic(code(gx::git::read_error), help("Are you in a git repository?"))]
    GitError(#[from] GitError),
}

pub fn run() -> Result<()> {
    let status = status::get_repo_status().map_err(StatusError::GitError)?;
    render_status(&status);
    Ok(())
}
