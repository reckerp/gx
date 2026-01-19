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

    if let Ok(head) = repo.head() {
        if let Some(name) = head.shorthand() {
            return Ok(name == branch_name);
        }
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
    let head = repo.head()?;

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
    let head = repo.head()?;

    if !head.is_branch() {
        return Err(GitError::NotOnBranch);
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
