use crate::git::{
    GitError, get_repo,
    git_exec::{self, ExecOptions},
};

use miette::Result;

pub fn fetch() -> Result<(), GitError> {
    git_exec::exec(
        vec!["fetch".to_string()],
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Fetch a specific remote (e.g. "origin") to refresh its remote-tracking refs.
pub fn fetch_remote(remote: &str) -> Result<(), GitError> {
    git_exec::exec(
        vec!["fetch".to_string(), remote.to_string()],
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

pub fn has_remote(name: &str) -> Result<bool, GitError> {
    let repo = get_repo()?;
    Ok(repo.find_remote(name).is_ok())
}
