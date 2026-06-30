//! Branch-state queries and the age / orphan-branch / gone-tracking heuristics
//! that back `gx workspace clean` and `gx workspace prune`.

use super::Worktree;
use crate::git::git_exec::{self, ExecOptions};
use crate::git::{GitError, get_repo};

pub fn branch_exists(branch_name: &str) -> Result<bool, GitError> {
    let repo = get_repo()?;
    Ok(repo
        .find_branch(branch_name, git2::BranchType::Local)
        .is_ok())
}

/// True when the local branch `branch` has a configured upstream.
pub fn has_upstream(branch: &str) -> Result<bool, GitError> {
    let repo = get_repo()?;
    let local = repo.find_branch(branch, git2::BranchType::Local)?;
    Ok(local.upstream().is_ok())
}

/// Number of commits the local branch `branch` is ahead of its upstream.
/// Returns `Ok(None)` when the branch has no upstream configured (so callers
/// can distinguish "no upstream" from "0 ahead"), sharing the single git2
/// ahead/behind impl in [`crate::git::branch::get_ahead_behind`].
pub fn unpushed_count(branch: &str) -> Result<Option<usize>, GitError> {
    Ok(crate::git::branch::get_ahead_behind(branch)?.map(|(ahead, _behind)| ahead))
}

/// True when `branch` has an upstream and is ahead of it. A branch with no
/// upstream is *not* reported as unpushed here; callers that need to treat a
/// missing upstream as unsafe should consult [`has_upstream`] separately.
pub fn has_unpushed(branch: &str) -> Result<bool, GitError> {
    Ok(unpushed_count(branch)?
        .map(|ahead| ahead > 0)
        .unwrap_or(false))
}

/// Committer epoch seconds of the tip commit of `branch`.
pub fn branch_last_commit_secs(branch: &str) -> Result<i64, GitError> {
    let repo = get_repo()?;
    let obj = repo.revparse_single(branch)?;
    let commit = obj.peel_to_commit()?;
    Ok(commit.time().seconds())
}

/// Conservative age of a workspace in whole days. The smaller of:
/// - days since the branch's last commit (when the worktree is on a branch), and
/// - days since the worktree's `.git` file (the gitdir pointer) was last modified.
///
/// Using the `.git` pointer's mtime means a freshly-created workspace from an
/// old branch reads as fresh, so cleanup never deletes it just for pointing at
/// stale commits.
pub fn workspace_age_days(worktree: &Worktree, now_secs: i64) -> Result<u64, GitError> {
    let branch_secs = match worktree.branch.as_deref() {
        Some(branch) => Some(branch_last_commit_secs(branch)?),
        None => None,
    };

    let git_file = worktree.path.join(".git");
    let git_file_secs = std::fs::metadata(&git_file)
        .and_then(|m| m.modified())
        .map(system_time_to_secs)
        // A missing/unreadable .git pointer should not make a workspace look
        // ancient (which could lead to deletion); fall back to "now".
        .unwrap_or(now_secs);

    Ok(age_days_from(branch_secs, git_file_secs, now_secs))
}

fn system_time_to_secs(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Pure age computation in whole days, used by [`workspace_age_days`]. Returns
/// the smaller of the branch age and the gitdir-pointer age; a future timestamp
/// (clock skew) clamps to 0.
fn age_days_from(branch_secs: Option<i64>, git_file_secs: i64, now_secs: i64) -> u64 {
    const DAY: i64 = 86_400;

    let to_days = |secs: i64| -> u64 {
        let diff = (now_secs - secs).max(0);
        (diff / DAY) as u64
    };

    let git_file_days = to_days(git_file_secs);
    match branch_secs {
        Some(branch_secs) => git_file_days.min(to_days(branch_secs)),
        None => git_file_days,
    }
}

/// A local branch that is not checked out in any worktree, annotated with the
/// upstream/unpushed state cleanup needs to decide whether it is safe to delete.
#[derive(Debug, Clone)]
pub struct OrphanBranch {
    pub name: String,
    pub has_unpushed: bool,
    pub has_upstream: bool,
}

/// Local branches with no associated worktree, each annotated with its
/// unpushed/upstream state.
pub fn orphan_branches(worktrees: &[Worktree]) -> Result<Vec<OrphanBranch>, GitError> {
    let all_local = crate::git::branch::get_local_branches()?;
    let candidates = branches_without_worktree(&all_local, worktrees);

    candidates
        .into_iter()
        .map(|name| {
            let has_upstream = has_upstream(&name)?;
            let has_unpushed = has_unpushed(&name)?;
            Ok(OrphanBranch {
                name,
                has_unpushed,
                has_upstream,
            })
        })
        .collect()
}

/// Local branches that are not checked out in any worktree. Pure set difference,
/// extracted for unit testing.
fn branches_without_worktree(all_local: &[String], worktrees: &[Worktree]) -> Vec<String> {
    let checked_out: std::collections::HashSet<&str> = worktrees
        .iter()
        .filter_map(|w| w.branch.as_deref())
        .collect();

    all_local
        .iter()
        .filter(|b| !checked_out.contains(b.as_str()))
        .cloned()
        .collect()
}

/// Local branches whose configured upstream no longer resolves (`[gone]`), for
/// the interactive cleaner's "remote tracking branch is gone" section.
pub fn remote_gone_branches() -> Result<Vec<String>, GitError> {
    let output = git_exec::exec(
        [
            "for-each-ref",
            "--format=%(refname:short) %(upstream:track)",
            "refs/heads",
        ],
        ExecOptions::capture(),
    )?;

    Ok(parse_gone_branches(&output))
}

/// Parse `git for-each-ref --format='%(refname:short) %(upstream:track)'`
/// output, selecting branches whose tracking state is `[gone]`.
fn parse_gone_branches(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            // The track field is the last whitespace-separated token, but the
            // branch name comes first and never contains spaces, so split once.
            let (name, track) = line.split_once(' ')?;
            if track.trim() == "[gone]" {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::worktree::test_support::worktree;

    #[test]
    fn test_age_days_from_uses_smaller_of_branch_and_gitfile() {
        const DAY: i64 = 86_400;
        let now = 100 * DAY;

        // branch tip 30 days ago, gitdir pointer 2 days ago -> 2 (the smaller).
        let branch_secs = now - 30 * DAY;
        let git_file_secs = now - 2 * DAY;
        assert_eq!(age_days_from(Some(branch_secs), git_file_secs, now), 2);

        // ...and the other way round: branch newer than the gitdir pointer.
        assert_eq!(age_days_from(Some(now - 2 * DAY), now - 30 * DAY, now), 2);
    }

    #[test]
    fn test_age_days_from_fresh_workspace_from_old_branch_is_fresh() {
        // The spec's headline guarantee: a workspace created today from a
        // branch whose tip is 200 days old reads as fresh (0 days).
        const DAY: i64 = 86_400;
        let now = 365 * DAY;
        let branch_secs = now - 200 * DAY;
        let git_file_secs = now; // .git pointer created just now

        assert_eq!(age_days_from(Some(branch_secs), git_file_secs, now), 0);
    }

    #[test]
    fn test_age_days_from_detached_uses_gitfile_only() {
        const DAY: i64 = 86_400;
        let now = 100 * DAY;
        assert_eq!(age_days_from(None, now - 5 * DAY, now), 5);
    }

    #[test]
    fn test_age_days_from_future_timestamp_clamps_to_zero() {
        const DAY: i64 = 86_400;
        let now = 100 * DAY;
        // Clock skew puts the branch/gitfile in the future; age never goes negative.
        assert_eq!(age_days_from(Some(now + 10 * DAY), now + 3 * DAY, now), 0);
    }

    #[test]
    fn test_branches_without_worktree() {
        let worktrees = vec![
            worktree("repo", Some("main")),
            worktree("feat", Some("feat/x")),
            worktree("detached", None),
        ];
        let all_local = vec![
            "main".to_string(),
            "feat/x".to_string(),
            "old-fix".to_string(),
            "experiment".to_string(),
        ];

        let mut orphans = branches_without_worktree(&all_local, &worktrees);
        orphans.sort();

        assert_eq!(
            orphans,
            vec!["experiment".to_string(), "old-fix".to_string()]
        );
    }

    #[test]
    fn test_parse_gone_branches() {
        let output =
            "main [ahead 1]\nfeat/x [gone]\nrelease \nold-thing [gone]\nahead-only [behind 2]";
        let gone = parse_gone_branches(output);
        assert_eq!(gone, vec!["feat/x".to_string(), "old-thing".to_string()]);
    }

    #[test]
    fn test_parse_gone_branches_empty() {
        assert!(parse_gone_branches("").is_empty());
        // A branch with no upstream has an empty track field, not "[gone]".
        assert!(parse_gone_branches("solo ").is_empty());
        assert!(parse_gone_branches("solo").is_empty());
    }
}
