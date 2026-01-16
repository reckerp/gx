use crate::git::{
    GitError,
    git_exec::{self, ExecOptions},
};

use miette::Result;

#[allow(dead_code)]
pub fn fetch() -> Result<(), GitError> {
    git_exec::exec(vec!["fetch".to_string()], ExecOptions { silent: true })?;
    Ok(())
}
