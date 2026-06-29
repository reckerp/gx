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

        // Check out the stash's base commit before moving HEAD so the index
        // and working tree match the new branch when the stash is re-applied.
        repo.checkout_tree(
            parent_commit.as_object(),
            Some(git2::build::CheckoutBuilder::default().safe()),
        )?;
    }

    let refname = format!("refs/heads/{}", name);
    repo.set_head(&refname)?;

    repo.stash_pop(index, None)?;

    Ok(())
}

fn extract_branch_from_message(message: &str) -> String {
    // Stash messages look like "WIP on <branch>: ..." or "On <branch>: ...".
    // Locate "on " case-insensitively via byte windows on the ORIGINAL string:
    // lowercasing first can change byte lengths for non-ASCII text, so an index
    // from the lowercased string can land mid-char and either return the wrong
    // slice or panic. The needle is ASCII, so the matched offset is a valid
    // char boundary in the original.
    let needle = b"on ";
    if let Some(start) = message
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
    {
        let rest = &message[start + needle.len()..];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_branch_from_message_standard_formats() {
        assert_eq!(
            extract_branch_from_message("WIP on main: 1a2b3c msg"),
            "main"
        );
        assert_eq!(
            extract_branch_from_message("On feature/x: 1a2b3c msg"),
            "feature/x"
        );
    }

    #[test]
    fn test_extract_branch_from_message_non_ascii_does_not_panic() {
        // A multibyte char before "on " used to shift the lowercased byte index
        // and could slice mid-char (panic) or return a wrong substring.
        let msg = "WIP on naïve-café: 1a2b3c work";
        assert_eq!(extract_branch_from_message(msg), "naïve-café");

        let msg = "On 日本語-branch: deadbee did things";
        assert_eq!(extract_branch_from_message(msg), "日本語-branch");
    }

    #[test]
    fn test_extract_branch_from_message_unknown_when_no_match() {
        assert_eq!(
            extract_branch_from_message("garbage without marker"),
            "unknown"
        );
    }
}
