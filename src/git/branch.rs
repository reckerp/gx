use crate::git::git_exec::{self, ExecOptions};
use std::collections::HashSet;

use super::{GitError, get_repo};

pub fn get_branches() -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    let mut seen = HashSet::new();

    let names = repo
        .branches(None)?
        .filter_map(|res| res.ok())
        .filter_map(|(branch, branch_type)| {
            let shorthand = branch.get().shorthand()?;

            let name = match branch_type {
                git2::BranchType::Local => shorthand.to_string(),
                git2::BranchType::Remote => shorthand
                    .split_once('/')
                    .map(|(_, tail)| tail.to_string())
                    .unwrap_or_else(|| shorthand.to_string()),
            };

            if seen.insert(name.clone()) {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    Ok(names)
}

pub fn checkout_branch(branch_name: &str) -> Result<(), GitError> {
    git_exec::exec(
        vec!["checkout".to_string(), branch_name.to_string()],
        ExecOptions::default(),
    )?;

    Ok(())
}
