use crate::git::git_exec::{self, ExecOptions};
use std::collections::HashSet;

use super::{GitError, get_repo};

/// `repo.branches(kind)` with the successful entries collected, treating an
/// unborn HEAD (a repo with no commits yet) as "no branches" rather than an
/// error. Shared by every branch-enumeration helper below.
fn branches_or_empty(
    repo: &git2::Repository,
    kind: Option<git2::BranchType>,
) -> Result<Vec<(git2::Branch<'_>, git2::BranchType)>, GitError> {
    match repo.branches(kind) {
        Ok(iter) => Ok(iter.filter_map(Result::ok).collect()),
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn get_branches() -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    let mut seen = HashSet::new();

    let names = branches_or_empty(&repo, None)?
        .into_iter()
        .filter_map(|(branch, branch_type)| {
            let shorthand = branch.get().shorthand()?;

            let name = match branch_type {
                git2::BranchType::Local => shorthand.to_string(),
                git2::BranchType::Remote => {
                    let tail = shorthand
                        .split_once('/')
                        .map(|(_, tail)| tail.to_string())
                        .unwrap_or_else(|| shorthand.to_string());

                    // skip symbolic refs like origin/HEAD, they are not real branches
                    if tail == "HEAD" {
                        return None;
                    }

                    tail
                }
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

/// Remote-tracking branches as `remote/branch` shorthands (e.g.
/// "origin/main"), excluding symbolic refs like `origin/HEAD`.
pub fn get_remote_branches() -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    let mut seen = HashSet::new();

    let names = branches_or_empty(&repo, Some(git2::BranchType::Remote))?
        .into_iter()
        .filter_map(|(branch, _)| {
            let shorthand = branch.get().shorthand()?;
            // skip symbolic refs like origin/HEAD, they are not real branches
            if shorthand.ends_with("/HEAD") {
                return None;
            }
            let name = shorthand.to_string();
            if seen.insert(name.clone()) {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    Ok(names)
}

/// Enumerate local branch shorthands (full names, including any '/').
pub fn get_local_branches() -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    let names = branches_or_empty(&repo, Some(git2::BranchType::Local))?
        .into_iter()
        .filter_map(|(branch, _)| branch.get().shorthand().map(|s| s.to_string()))
        .collect();
    Ok(names)
}

/// Find a remote-tracking branch matching `branch_name` (e.g. "origin/feature"
/// for "feature"), preferring "origin" when multiple remotes have it.
pub fn find_remote_branch(branch_name: &str) -> Result<Option<String>, GitError> {
    let repo = get_repo()?;

    let mut found: Vec<String> = branches_or_empty(&repo, Some(git2::BranchType::Remote))?
        .into_iter()
        .filter_map(|(branch, _)| branch.get().shorthand().map(|s| s.to_string()))
        .filter(|shorthand| {
            shorthand
                .split_once('/')
                .is_some_and(|(_, tail)| tail == branch_name)
        })
        .collect();

    // origin first, then alphabetical — compared by borrow to avoid cloning keys.
    found.sort_by(|a, b| {
        (!a.starts_with("origin/"), a.as_str()).cmp(&(!b.starts_with("origin/"), b.as_str()))
    });
    Ok(found.into_iter().next())
}

/// Default branch of the 'origin' remote (e.g. "origin/main"), resolved from
/// 'refs/remotes/origin/HEAD' with a fallback to origin/main or origin/master.
/// Returns None when there is no origin remote.
pub fn default_remote_branch() -> Result<Option<String>, GitError> {
    let repo = get_repo()?;

    if let Ok(head) = repo.find_reference("refs/remotes/origin/HEAD")
        && let Some(target) = head.symbolic_target()
        && let Some(branch) = target.strip_prefix("refs/remotes/")
    {
        return Ok(Some(branch.to_string()));
    }

    // origin/HEAD is only set on clone; fall back to common default names
    for candidate in ["origin/main", "origin/master"] {
        if repo
            .find_branch(candidate, git2::BranchType::Remote)
            .is_ok()
        {
            return Ok(Some(candidate.to_string()));
        }
    }

    Ok(None)
}

pub fn checkout_branch(branch_name: &str) -> Result<(), GitError> {
    git_exec::exec(
        vec!["checkout".to_string(), branch_name.to_string()],
        ExecOptions::default(),
    )?;

    Ok(())
}

pub fn create_branch(branch_name: &str, start_point: Option<&str>) -> Result<(), GitError> {
    let repo = get_repo()?;

    let commit = if let Some(sp) = start_point {
        let obj = repo.revparse_single(sp)?;
        obj.peel_to_commit()?
    } else {
        let head = repo.head()?;
        head.peel_to_commit()?
    };

    repo.branch(branch_name, &commit, false)?;

    Ok(())
}

// aggregated branch information
pub struct BranchInfo {
    pub name: String,
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub commit_time: i64,
    pub ahead_behind: Option<(usize, usize)>,
    pub is_current: bool,
    pub recent_commits: Vec<String>,
}

impl BranchInfo {
    pub fn fetch(branch_name: &str) -> Result<Self, GitError> {
        let tip = get_branch_tip(branch_name)?;
        let ahead_behind = get_ahead_behind(branch_name)?;
        let is_current = is_current_branch(branch_name)?;
        let recent_commits = get_recent_commits(branch_name, 5)?;

        Ok(Self {
            name: branch_name.to_string(),
            short_id: tip.short_id,
            summary: tip.summary,
            author_name: tip.author_name,
            author_email: tip.author_email,
            commit_time: tip.commit_time,
            ahead_behind,
            is_current,
            recent_commits,
        })
    }
}

pub struct BranchTipInfo {
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub commit_time: i64,
}

fn resolve_branch_commit<'a>(
    repo: &'a git2::Repository,
    branch_name: &str,
) -> Result<git2::Commit<'a>, GitError> {
    let obj = repo.revparse_single(branch_name)?;
    Ok(obj.peel_to_commit()?)
}

pub fn get_branch_tip(branch_name: &str) -> Result<BranchTipInfo, GitError> {
    let repo = get_repo()?;
    let commit = resolve_branch_commit(&repo, branch_name)?;

    let short_id = commit
        .as_object()
        .short_id()?
        .as_str()
        .unwrap_or("")
        .to_string();

    let summary = commit.summary().unwrap_or("").to_string();
    let author = commit.author();
    let author_name = author.name().unwrap_or("Unknown").to_string();
    let author_email = author.email().unwrap_or("").to_string();
    let commit_time = commit.time().seconds();

    Ok(BranchTipInfo {
        short_id,
        summary,
        author_name,
        author_email,
        commit_time,
    })
}

pub fn get_ahead_behind(branch_name: &str) -> Result<Option<(usize, usize)>, GitError> {
    let repo = get_repo()?;

    let local = match repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return Ok(None), // remote only branch
    };

    let upstream = match local.upstream() {
        Ok(u) => u,
        Err(_) => return Ok(None), // no upstream configured
    };

    let local_oid = local.get().peel_to_commit()?.id();
    let upstream_oid = upstream.get().peel_to_commit()?.id();

    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;
    Ok(Some((ahead, behind)))
}

pub fn get_recent_commits(branch_name: &str, limit: usize) -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;
    let commit = resolve_branch_commit(&repo, branch_name)?;

    let mut revwalk = repo.revwalk()?;
    // Without an explicit sort the walk order is unspecified, so `take(limit)`
    // could miss the actual most-recent commits; TIME order yields newest first.
    revwalk.set_sorting(git2::Sort::TIME)?;
    revwalk.push(commit.id())?;

    let messages: Vec<String> = revwalk
        .take(limit)
        .filter_map(|oid| oid.ok())
        .filter_map(|oid| repo.find_commit(oid).ok())
        .filter_map(|c| c.summary().map(|s| s.to_string()))
        .collect();

    Ok(messages)
}

pub fn is_current_branch(branch_name: &str) -> Result<bool, GitError> {
    let repo = get_repo()?;

    if let Ok(head) = repo.head()
        && let Some(name) = head.shorthand()
    {
        return Ok(name == branch_name);
    }

    Ok(false)
}

#[derive(Debug, Clone)]
pub struct BranchStatus {
    pub name: String,
    pub is_detached: bool,
}
pub fn get_current_branch() -> Result<BranchStatus, GitError> {
    let repo = get_repo()?;
    let head = match repo.head() {
        Ok(h) => h,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            return Ok(BranchStatus {
                name: "(no commits)".to_string(),
                is_detached: false,
            });
        }
        Err(e) => return Err(e.into()),
    };

    if head.is_branch() {
        Ok(BranchStatus {
            name: head.shorthand().unwrap_or("unknown").to_string(),
            is_detached: false,
        })
    } else {
        let commit = head.peel_to_commit()?;
        let short_id = commit.as_object().short_id()?;
        Ok(BranchStatus {
            name: short_id.as_str().unwrap_or("HEAD").to_string(),
            is_detached: true,
        })
    }
}

pub fn get_remote_name() -> Result<Option<String>, GitError> {
    let repo = get_repo()?;
    let head = match repo.head() {
        Ok(h) => h,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };

    // detached HEAD has no upstream
    if !head.is_branch() {
        return Ok(None);
    }

    let branch_name = head.shorthand().ok_or(GitError::NotOnBranch)?;
    let local_branch = repo.find_branch(branch_name, git2::BranchType::Local)?;

    let remote_name = local_branch.upstream().ok().and_then(|upstream| {
        upstream
            .name()
            .ok()
            .flatten()
            .map(|name| name.split('/').next().unwrap_or("origin").to_string())
    });

    Ok(remote_name)
}

#[derive(Debug, Clone)]
pub struct RemoteTrackingInfo {
    pub remote: String,
    pub ahead: usize,
    pub behind: usize,
}
pub fn get_remote_tracking_info(branch_name: &str) -> Result<Option<RemoteTrackingInfo>, GitError> {
    let Some(remote_name) = get_remote_name()? else {
        return Ok(None);
    };

    let (ahead, behind) = get_ahead_behind(branch_name)?.unwrap_or((0, 0));

    Ok(Some(RemoteTrackingInfo {
        remote: remote_name,
        ahead,
        behind,
    }))
}
