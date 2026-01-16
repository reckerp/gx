use crate::git::git_exec::{self, ExecOptions};

use super::{GitError, get_repo};

pub fn get_branches() -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;

    let names = repo
        .branches(None)?
        .filter_map(|res| res.ok())
        .filter_map(|(branch, branch_type)| {
            let shorthand = branch.get().shorthand()?;

            match branch_type {
                git2::BranchType::Local => Some(shorthand.to_string()),
                git2::BranchType::Remote => {
                    // get rid of remote name
                    let parts: Vec<&str> = shorthand.splitn(2, '/').collect();
                    parts.get(1).map(|&s| s.to_string())
                }
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
