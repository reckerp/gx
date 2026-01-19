use crate::git::branch::{
    BranchStatus, RemoteTrackingInfo, get_current_branch, get_remote_tracking_info,
};

use git2::{Status, StatusOptions};

use super::{GitError, get_repo};

#[derive(Debug, Clone)]
pub struct StatusFile {
    pub path: String,
    pub status: FileStatus,
}

pub const STAGED_FLAGS: Status = Status::INDEX_NEW
    .union(Status::INDEX_MODIFIED)
    .union(Status::INDEX_DELETED)
    .union(Status::INDEX_RENAMED)
    .union(Status::INDEX_TYPECHANGE);

pub const UNSTAGED_FLAGS: Status = Status::WT_NEW
    .union(Status::WT_MODIFIED)
    .union(Status::WT_DELETED)
    .union(Status::WT_RENAMED)
    .union(Status::WT_TYPECHANGE);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileStatus {
    New,
    Modified,
    Deleted,
    Renamed,
    Typechange,
}

impl FileStatus {
    pub fn from_staged(status: Status) -> Self {
        if status.contains(Status::INDEX_NEW) {
            FileStatus::New
        } else if status.contains(Status::INDEX_MODIFIED) {
            FileStatus::Modified
        } else if status.contains(Status::INDEX_DELETED) {
            FileStatus::Deleted
        } else if status.contains(Status::INDEX_RENAMED) {
            FileStatus::Renamed
        } else {
            FileStatus::Typechange
        }
    }

    pub fn from_unstaged(status: Status) -> Self {
        if status.contains(Status::WT_NEW) {
            FileStatus::New
        } else if status.contains(Status::WT_MODIFIED) {
            FileStatus::Modified
        } else if status.contains(Status::WT_DELETED) {
            FileStatus::Deleted
        } else if status.contains(Status::WT_RENAMED) {
            FileStatus::Renamed
        } else {
            FileStatus::Typechange
        }
    }
}

#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub branch: BranchStatus,
    pub remote: Option<RemoteTrackingInfo>,
    pub staged_files: Vec<StatusFile>,
    pub unstaged_files: Vec<StatusFile>,
    pub stash_count: usize,
    pub last_commit_message: Option<String>,
    pub last_commit_time: Option<String>,
}

pub fn get_repo_status() -> Result<RepoStatus, GitError> {
    let branch = get_current_branch()?;
    let remote = get_remote_tracking_info(branch.name.as_str())?;
    let (staged_files, unstaged_files) = get_status_files()?;
    let stash_count = count_stashes()?;
    let (last_commit_message, last_commit_time) = get_last_commit_info()?;
    Ok(RepoStatus {
        branch,
        remote,
        staged_files,
        unstaged_files,
        stash_count,
        last_commit_message,
        last_commit_time,
    })
}

pub fn get_status_files() -> Result<(Vec<StatusFile>, Vec<StatusFile>), GitError> {
    let repo = get_repo()?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();

    for entry in statuses.iter() {
        let Some(path) = entry.path() else { continue };
        let status = entry.status();

        if status.intersects(STAGED_FLAGS) {
            staged.push(StatusFile {
                path: path.to_string(),
                status: FileStatus::from_staged(status),
            });
        }

        if status.intersects(UNSTAGED_FLAGS) {
            unstaged.push(StatusFile {
                path: path.to_string(),
                status: FileStatus::from_unstaged(status),
            });
        }
    }

    staged.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));

    Ok((staged, unstaged))
}

fn count_stashes() -> Result<usize, GitError> {
    let mut repo = get_repo()?;
    let mut count = 0;
    repo.stash_foreach(|_, _, _| {
        count += 1;
        true
    })?;
    Ok(count)
}

fn get_last_commit_info() -> Result<(Option<String>, Option<String>), GitError> {
    let repo = get_repo()?;
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return Ok((None, None)),
    };

    let commit = match head.peel_to_commit() {
        Ok(c) => c,
        Err(_) => return Ok((None, None)),
    };

    let message = commit
        .message()
        .map(|m| m.lines().next().unwrap_or("").to_string());

    let secs = commit.time().seconds();
    let time_str = crate::git::time::format_relative(crate::git::time::now_secs() - secs);

    Ok((message, Some(time_str)))
}
