use super::{GitError, get_repo};
use crate::git::time;

#[derive(Debug, Clone)]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub branch: String,
    pub time_relative: String,
}

pub fn list() -> Result<Vec<StashEntry>, GitError> {
    let mut repo = get_repo()?;
    let mut raw_entries: Vec<(usize, String, git2::Oid)> = Vec::new();

    repo.stash_foreach(|index, message, oid| {
        raw_entries.push((index, message.to_string(), *oid));
        true
    })?;

    let entries = raw_entries
        .into_iter()
        .map(|(index, message, oid)| {
            let time_relative = repo
                .find_commit(oid)
                .map(|c| time::format_relative(time::now_secs() - c.time().seconds()))
                .unwrap_or_else(|_| "unknown".to_string());

            let branch = extract_branch_from_message(&message);
            let description = extract_stash_description(&message);

            StashEntry {
                index,
                message: description,
                branch,
                time_relative,
            }
        })
        .collect();

    Ok(entries)
}

pub fn save(message: Option<&str>, include_untracked: bool) -> Result<git2::Oid, GitError> {
    let mut repo = get_repo()?;
    let signature = repo.signature()?;

    let mut flags = git2::StashFlags::DEFAULT;
    if include_untracked {
        flags |= git2::StashFlags::INCLUDE_UNTRACKED;
    }

    let oid = repo.stash_save(&signature, message.unwrap_or("WIP"), Some(flags))?;
    Ok(oid)
}

pub fn pop(index: usize) -> Result<(), GitError> {
    let mut repo = get_repo()?;
    repo.stash_pop(index, None)?;
    Ok(())
}

pub fn apply(index: usize) -> Result<(), GitError> {
    let mut repo = get_repo()?;
    repo.stash_apply(index, None)?;
    Ok(())
}

pub fn drop(index: usize) -> Result<(), GitError> {
    let mut repo = get_repo()?;
    repo.stash_drop(index)?;
    Ok(())
}

pub fn clear() -> Result<usize, GitError> {
    let entries = list()?;
    let count = entries.len();

    let mut repo = get_repo()?;
    for _ in 0..count {
        repo.stash_drop(0)?;
    }

    Ok(count)
}

pub fn show(index: usize) -> Result<String, GitError> {
    let mut repo = get_repo()?;

    let mut stash_oid: Option<git2::Oid> = None;
    repo.stash_foreach(|i, _, oid| {
        if i == index {
            stash_oid = Some(*oid);
            return false;
        }
        true
    })?;

    let oid = stash_oid.ok_or_else(|| GitError::CommandFailed("Stash not found".to_string()))?;
    let stash_commit = repo.find_commit(oid)?;
    let stash_tree = stash_commit.tree()?;

    let parent_commit = stash_commit.parent(0)?;
    let parent_tree = parent_commit.tree()?;

    let diff = repo.diff_tree_to_tree(Some(&parent_tree), Some(&stash_tree), None)?;

    let mut output = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        if let Ok(content) = std::str::from_utf8(line.content()) {
            let prefix = match line.origin() {
                '+' => "+",
                '-' => "-",
                ' ' => " ",
                _ => "",
            };
            output.push_str(prefix);
            output.push_str(content);
        }
        true
    })?;

    Ok(output)
}

pub fn branch(name: &str, index: usize) -> Result<(), GitError> {
    let mut repo = get_repo()?;

    let mut stash_oid: Option<git2::Oid> = None;
    repo.stash_foreach(|i, _, oid| {
        if i == index {
            stash_oid = Some(*oid);
            return false;
        }
        true
    })?;

    let oid = stash_oid.ok_or_else(|| GitError::CommandFailed("Stash not found".to_string()))?;

    let parent_oid = {
        let stash_commit = repo.find_commit(oid)?;
        stash_commit.parent(0)?.id()
    };

    {
        let parent_commit = repo.find_commit(parent_oid)?;
        repo.branch(name, &parent_commit, false)?;
    }

    let refname = format!("refs/heads/{}", name);
    repo.set_head(&refname)?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::default().safe()))?;

    repo.stash_pop(index, None)?;

    Ok(())
}

fn extract_branch_from_message(message: &str) -> String {
    let lower = message.to_lowercase();
    if let Some(start) = lower.find("on ") {
        let rest = &message[start + 3..];
        if let Some(end) = rest.find(':') {
            return rest[..end].to_string();
        }
    }
    "unknown".to_string()
}

fn extract_stash_description(message: &str) -> String {
    if let Some(colon_pos) = message.find(": ") {
        let desc = message[colon_pos + 2..].trim();
        if !desc.is_empty() {
            return desc.to_string();
        }
    }
    message.to_string()
}
