//! Per-worktree status summaries (dirty/ahead/behind + PR state) and the
//! background, interruptible lookup that computes them in parallel so the
//! workspace picker can render before the `git status` scans finish.

use super::Worktree;
use crate::git::GitError;
use crate::git::git_exec::{self, ExecOptions};
use crate::git::pull_request::{PullRequestLookup, PullRequestStatus};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

const MAX_SUMMARY_WORKERS: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeSummary {
    pub tracked_changes: usize,
    pub untracked_changes: usize,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub pull_request: PullRequestStatus,
    pub status_loaded: bool,
    pub status_error: bool,
}

impl WorktreeSummary {
    pub fn has_changes(&self) -> bool {
        self.tracked_changes > 0 || self.untracked_changes > 0
    }

    pub fn has_unpushed_commits(&self) -> bool {
        self.ahead.unwrap_or(0) > 0
    }

    pub fn pending_for(worktree: &Worktree) -> Self {
        Self {
            pull_request: if worktree.branch.is_some() {
                PullRequestStatus::Loading
            } else {
                PullRequestStatus::None
            },
            ..Default::default()
        }
    }

    fn status_error_for(worktree: &Worktree) -> Self {
        Self {
            status_loaded: true,
            status_error: true,
            ..Self::pending_for(worktree)
        }
    }
}

pub struct SummaryLookup {
    rx: Receiver<HashMap<PathBuf, WorktreeSummary>>,
    cancelled: Arc<AtomicBool>,
}

impl SummaryLookup {
    pub fn try_recv(&self) -> Result<HashMap<PathBuf, WorktreeSummary>, TryRecvError> {
        self.rx.try_recv()
    }
}

impl Drop for SummaryLookup {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// Start the local status lookup on a background thread so the workspace picker
/// can render before expensive `git status` scans finish.
pub fn spawn_summary_lookup(worktrees: &[Worktree]) -> SummaryLookup {
    let (tx, rx) = mpsc::channel();
    let worktrees = worktrees.to_vec();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);

    std::thread::spawn(move || {
        let summaries = summarize_all_interruptible(&worktrees, &worker_cancelled);
        if !worker_cancelled.load(Ordering::Relaxed) {
            let _ = tx.send(summaries);
        }
    });

    SummaryLookup { rx, cancelled }
}

pub fn pending_summaries(worktrees: &[Worktree]) -> HashMap<PathBuf, WorktreeSummary> {
    worktrees
        .iter()
        .map(|worktree| (worktree.path.clone(), WorktreeSummary::pending_for(worktree)))
        .collect()
}

/// Compute the local (network-free) status for every worktree, in parallel.
/// Each summary shells out to git (status + ahead/behind) against an independent
/// working tree. Worker count is capped because many simultaneous filesystem
/// scans can make disk-heavy repositories slower, not faster.
fn summarize_all_interruptible(
    worktrees: &[Worktree],
    cancelled: &AtomicBool,
) -> HashMap<PathBuf, WorktreeSummary> {
    if worktrees.is_empty() {
        return HashMap::new();
    }

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(PathBuf, WorktreeSummary)>> =
        Mutex::new(Vec::with_capacity(worktrees.len()));

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(MAX_SUMMARY_WORKERS)
        .min(worktrees.len());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(worktree) = worktrees.get(i) else {
                        break;
                    };

                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let summary = summarize(worktree)
                        .unwrap_or_else(|_| WorktreeSummary::status_error_for(worktree));

                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    results
                        .lock()
                        .unwrap()
                        .push((worktree.path.clone(), summary));
                }
            });
        }
    });

    results.into_inner().unwrap().into_iter().collect()
}

/// Merge completed local status into the map used by the picker, preserving any
/// PR lookup result that may have arrived first.
pub fn apply_local_summaries(
    summaries: &mut HashMap<PathBuf, WorktreeSummary>,
    lookup: HashMap<PathBuf, WorktreeSummary>,
) {
    for (path, mut local_summary) in lookup {
        if let Some(existing) = summaries.get(&path) {
            local_summary.pull_request = existing.pull_request.clone();
        }
        summaries.insert(path, local_summary);
    }
}

/// Merge a completed pull-request lookup into existing summaries. On success
/// each branch resolves to `Found` (when a PR matched) or `None`; on failure
/// every summary is marked `Error`.
pub fn apply_pull_requests(
    summaries: &mut HashMap<PathBuf, WorktreeSummary>,
    worktrees: &[Worktree],
    lookup: PullRequestLookup,
) {
    match lookup {
        Ok(mut pull_requests) => {
            for worktree in worktrees {
                let Some(summary) = summaries.get_mut(&worktree.path) else {
                    continue;
                };
                summary.pull_request = match worktree
                    .branch
                    .as_deref()
                    .and_then(|branch| pull_requests.remove(branch))
                {
                    Some(pull_request) => PullRequestStatus::Found(pull_request),
                    None => PullRequestStatus::None,
                };
            }
        }
        Err(_) => {
            for summary in summaries.values_mut() {
                summary.pull_request = PullRequestStatus::Error;
            }
        }
    }
}

pub fn summarize(worktree: &Worktree) -> Result<WorktreeSummary, GitError> {
    let status_output =
        git_exec::exec_in(&worktree.path, &["status", "--porcelain"], ExecOptions::capture())?;

    let (tracked_changes, untracked_changes) = parse_status_counts(&status_output);
    let (ahead, behind) = match worktree.branch {
        Some(_) => ahead_behind(&worktree.path).unwrap_or((None, None)),
        None => (None, None),
    };

    Ok(WorktreeSummary {
        tracked_changes,
        untracked_changes,
        ahead,
        behind,
        pull_request: if worktree.branch.is_some() {
            PullRequestStatus::Loading
        } else {
            PullRequestStatus::None
        },
        status_loaded: true,
        status_error: false,
    })
}

fn parse_status_counts(output: &str) -> (usize, usize) {
    output.lines().fold((0, 0), |(tracked, untracked), line| {
        if line.starts_with("??") {
            (tracked, untracked + 1)
        } else if line.trim().is_empty() {
            (tracked, untracked)
        } else {
            (tracked + 1, untracked)
        }
    })
}

/// Path-based ahead/behind of `path`'s HEAD vs its upstream, via `git -C <path>
/// rev-list --left-right --count`. Worktrees address by path (the git2
/// name-based variant lives in [`crate::git::branch`]).
fn ahead_behind(path: &Path) -> Result<(Option<usize>, Option<usize>), GitError> {
    // rev-list itself fails when the branch has no upstream, and the caller
    // treats any error here as "no ahead/behind info", so a separate
    // rev-parse @{upstream} existence check would just be a wasted git spawn.
    let output = git_exec::exec_in(
        path,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
        ExecOptions::capture(),
    )?;

    let mut counts = output.split_whitespace();
    let behind = counts.next().and_then(|n| n.parse::<usize>().ok());
    let ahead = counts.next().and_then(|n| n.parse::<usize>().ok());
    Ok((ahead, behind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::worktree::test_support::{pr_summary, worktree};

    #[test]
    fn test_parse_status_counts() {
        let output = " M src/main.rs\nA  src/new.rs\n?? scratch.txt\n?? notes/todo.md\n";
        let (tracked, untracked) = parse_status_counts(output);

        assert_eq!(tracked, 2);
        assert_eq!(untracked, 2);
    }

    #[test]
    fn test_pending_summaries_seed_pr_loading_state() {
        let branched = worktree("feature", Some("feature"));
        let detached = worktree("detached", None);
        let summaries = pending_summaries(&[branched.clone(), detached.clone()]);

        assert_eq!(
            summaries[&branched.path].pull_request,
            PullRequestStatus::Loading
        );
        assert_eq!(
            summaries[&detached.path].pull_request,
            PullRequestStatus::None
        );
        assert!(!summaries[&branched.path].status_loaded);
    }

    #[test]
    fn test_apply_local_summaries_preserves_pr_status() {
        let feature = worktree("feature", Some("feature"));
        let pr = pr_summary(7);
        let mut summaries = HashMap::from([(
            feature.path.clone(),
            WorktreeSummary {
                pull_request: PullRequestStatus::Found(pr.clone()),
                ..Default::default()
            },
        )]);
        let lookup = HashMap::from([(
            feature.path.clone(),
            WorktreeSummary {
                tracked_changes: 2,
                status_loaded: true,
                pull_request: PullRequestStatus::None,
                ..Default::default()
            },
        )]);

        apply_local_summaries(&mut summaries, lookup);

        assert_eq!(summaries[&feature.path].tracked_changes, 2);
        assert!(summaries[&feature.path].status_loaded);
        assert_eq!(
            summaries[&feature.path].pull_request,
            PullRequestStatus::Found(pr)
        );
    }

    #[test]
    fn test_apply_pull_requests_resolves_matched_and_unmatched_branches() {
        let feature = worktree("feature", Some("feature"));
        let solo = worktree("solo", Some("no-pr"));
        let detached = worktree("detached", None);
        let worktrees = [feature.clone(), solo.clone(), detached.clone()];

        let mut summaries: HashMap<PathBuf, WorktreeSummary> = worktrees
            .iter()
            .map(|w| (w.path.clone(), WorktreeSummary::default()))
            .collect();

        let lookup = Ok(HashMap::from([("feature".to_string(), pr_summary(7))]));
        apply_pull_requests(&mut summaries, &worktrees, lookup);

        assert_eq!(
            summaries[&feature.path].pull_request,
            PullRequestStatus::Found(pr_summary(7))
        );
        assert_eq!(summaries[&solo.path].pull_request, PullRequestStatus::None);
        // A worktree with no branch can never match a PR.
        assert_eq!(
            summaries[&detached.path].pull_request,
            PullRequestStatus::None
        );
    }

    #[test]
    fn test_apply_pull_requests_marks_all_errored_on_failure() {
        let feature = worktree("feature", Some("feature"));
        let worktrees = [feature.clone()];
        let mut summaries: HashMap<PathBuf, WorktreeSummary> =
            HashMap::from([(feature.path.clone(), WorktreeSummary::default())]);

        apply_pull_requests(
            &mut summaries,
            &worktrees,
            Err(crate::git::pull_request::PullRequestLookupError::CommandFailed),
        );

        assert_eq!(
            summaries[&feature.path].pull_request,
            PullRequestStatus::Error
        );
    }
}
