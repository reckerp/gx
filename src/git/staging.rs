use super::status::STAGED_FLAGS;
use super::{GitError, get_repo};
use git2::StatusOptions;
use std::path::Path;

pub fn stage_paths(paths: &[String]) -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    let mut index = repo.index()?;
    let mut staged = Vec::new();

    for path in paths {
        let p = Path::new(path);
        if p.exists() {
            if p.is_dir() {
                index.add_all([path], git2::IndexAddOption::DEFAULT, None)?;
            } else {
                index.add_path(p)?;
            }
        } else if index.get_path(p, 0).is_some() {
            // tracked file deleted from the working tree -> stage the deletion
            index.remove_path(p)?;
        } else {
            let dir_prefix = format!("{}/", path.trim_end_matches('/'));
            let has_entries_under = index.iter().any(|entry| {
                std::str::from_utf8(&entry.path)
                    .map(|entry_path| entry_path.starts_with(&dir_prefix))
                    .unwrap_or(false)
            });

            if has_entries_under {
                // tracked directory deleted from the working tree
                index.remove_all([path], None)?;
            } else {
                return Err(GitError::CommandFailed(format!(
                    "pathspec '{}' did not match any files",
                    path
                )));
            }
        }
        staged.push(path.clone());
    }

    index.write()?;
    Ok(staged)
}

pub fn unstage_paths(paths: &[String]) -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    match repo.head() {
        Ok(head_ref) => {
            let head = head_ref.peel_to_commit()?;
            repo.reset_default(Some(&head.into_object()), paths.iter().map(Path::new))?;
        }
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            let mut index = repo.index()?;
            for path in paths {
                let _ = index.remove_path(Path::new(path));
            }
            index.write()?;
        }
        Err(e) => return Err(GitError::Git2Error(e)),
    }
    Ok(paths.to_vec())
}

pub fn stage_all() -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    let mut index = repo.index()?;

    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;
    index.update_all(["*"], None)?;
    index.write()?;

    let mut opts = StatusOptions::new();
    opts.include_untracked(false);
    let statuses = repo.statuses(Some(&mut opts))?;

    let staged: Vec<String> = statuses
        .iter()
        .filter(|e| e.status().intersects(STAGED_FLAGS))
        .filter_map(|e| e.path().map(String::from))
        .collect();

    Ok(staged)
}

pub fn get_staged_diff() -> Result<String, GitError> {
    let repo = get_repo()?;
    let mut diff_options = git2::DiffOptions::new();

    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());

    let diff = repo.diff_tree_to_index(
        head_tree.as_ref(),
        Some(&repo.index()?),
        Some(&mut diff_options),
    )?;

    let mut diff_text = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let content = std::str::from_utf8(line.content()).unwrap_or("");
        match line.origin() {
            '+' | '-' | ' ' => diff_text.push_str(&format!("{}{}", line.origin(), content)),
            _ => diff_text.push_str(content),
        }
        true
    })?;

    Ok(diff_text)
}
