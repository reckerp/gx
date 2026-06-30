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
                return Err(GitError::PathspecNotFound(path.clone()));
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
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    diff_index_against(&repo, head_tree.as_ref())
}

/// Diff for the commit produced by `--amend`: the index against HEAD's *parent*,
/// so it reflects the full content of the amended commit (the original change
/// plus anything newly staged). `get_staged_diff` would compare against HEAD and
/// therefore be empty on a plain reword, which is why amend needs its own diff.
pub fn get_amend_diff() -> Result<String, GitError> {
    let repo = get_repo()?;
    let head_commit = repo.head()?.peel_to_commit()?;
    let parent_tree = match head_commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        // Amending the root commit: diff against the empty tree.
        Err(_) => None,
    };
    diff_index_against(&repo, parent_tree.as_ref())
}

fn diff_index_against(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree>,
) -> Result<String, GitError> {
    let mut diff_options = git2::DiffOptions::new();
    let diff = repo.diff_tree_to_index(old_tree, Some(&repo.index()?), Some(&mut diff_options))?;

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
