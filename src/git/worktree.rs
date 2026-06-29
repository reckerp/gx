use super::{GitError, get_repo};
use crate::git::git_exec::{self, ExecOptions};
use crate::git::pull_request::{PullRequestLookup, PullRequestStatus};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

const MAX_SUMMARY_WORKERS: usize = 4;

#[derive(Debug, Clone)]
pub struct Worktree {
    /// Directory name of the worktree (last path component)
    pub name: String,
    pub path: PathBuf,
    /// Checked-out branch, None when HEAD is detached or the worktree is bare
    pub branch: Option<String>,
    /// Short HEAD commit id
    pub head: Option<String>,
    /// The main worktree (the original repository checkout)
    pub is_main: bool,
    /// The worktree the command is currently running in
    pub is_current: bool,
    pub is_bare: bool,
    pub is_locked: bool,
}

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

/// Replace '/' with '-': branch names may contain '/', workspace directory
/// names may not, so the two are treated as interchangeable when matching.
pub fn flatten_slashes(name: &str) -> String {
    name.replace('/', "-")
}

impl Worktree {
    /// True when `query` is exactly the worktree's name or branch,
    /// treating '/' and '-' as interchangeable.
    pub fn matches_exactly(&self, query: &str) -> bool {
        let query = flatten_slashes(query);
        self.name.eq_ignore_ascii_case(&query)
            || self
                .branch
                .as_deref()
                .is_some_and(|b| flatten_slashes(b).eq_ignore_ascii_case(&query))
    }

    /// Fuzzy score of `query` against the worktree's name and branch, with
    /// '/' and '-' interchangeable: 'feat/x' matches directory 'feat-x' and
    /// 'feat-x' matches branch 'feat/x'.
    pub fn match_score(&self, matcher: &SkimMatcherV2, query: &str) -> Option<i64> {
        let flattened = flatten_slashes(query);

        let name_score = matcher
            .fuzzy_match(&self.name, query)
            .into_iter()
            .chain(matcher.fuzzy_match(&self.name, &flattened))
            .max();

        let branch_score = self.branch.as_deref().and_then(|b| {
            matcher
                .fuzzy_match(b, query)
                .into_iter()
                .chain(matcher.fuzzy_match(&flatten_slashes(b), &flattened))
                .max()
        });

        name_score.into_iter().chain(branch_score).max()
    }
}

/// List all worktrees of the repository. The main worktree is always first.
pub fn list() -> Result<Vec<Worktree>, GitError> {
    let output = git_exec::exec(
        vec![
            "worktree".to_string(),
            "list".to_string(),
            "--porcelain".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )?;

    let current_root = current_worktree_root().ok();
    Ok(parse_porcelain(&output, current_root.as_deref()))
}

/// Root directory of the worktree the command is running in.
pub fn current_worktree_root() -> Result<PathBuf, GitError> {
    let repo = get_repo()?;
    repo.workdir()
        .map(|p| p.to_path_buf())
        .ok_or(GitError::NotInRepo)
}

/// Path to Git's shared metadata directory for this repository. Linked
/// worktrees from the same repository return the same common directory.
pub fn common_git_dir() -> Result<PathBuf, GitError> {
    let output = git_exec::exec(
        vec![
            "rev-parse".to_string(),
            "--path-format=absolute".to_string(),
            "--git-common-dir".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )?;

    Ok(PathBuf::from(output))
}

/// Add a new worktree at `path` checking out `branch`.
/// When `create_branch` is true the branch is created (optionally from `base`).
/// `no_track` prevents the new branch from tracking a remote `base` (useful
/// when branching off e.g. 'origin/main' without wanting it as upstream).
/// When `detach` is true, no branch is created or checked out; the worktree is
/// created with a detached HEAD at `base` (which is then required), mirroring
/// `git worktree add --detach <path> <base>`.
pub fn add(
    path: &Path,
    branch: &str,
    create_branch: bool,
    base: Option<&str>,
    no_track: bool,
    detach: bool,
) -> Result<(), GitError> {
    let mut args = vec!["worktree".to_string(), "add".to_string()];

    if detach {
        args.push("--detach".to_string());
        // End option parsing so a path or base beginning with '-' is never read
        // as a flag.
        args.push("--".to_string());
        args.push(path.display().to_string());
        if let Some(base) = base {
            args.push(base.to_string());
        }
    } else if create_branch {
        if no_track {
            args.push("--no-track".to_string());
        }
        args.push("-b".to_string());
        args.push(branch.to_string());
        args.push("--".to_string());
        args.push(path.display().to_string());
        if let Some(base) = base {
            args.push(base.to_string());
        }
    } else {
        args.push("--".to_string());
        args.push(path.display().to_string());
        args.push(branch.to_string());
    }

    git_exec::exec(
        args,
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;

    Ok(())
}

/// A file staged in the index, as reported by `git diff --cached --name-status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedEntry {
    /// The current (post-rename) path of the staged file.
    pub path: String,
    pub status: StagedStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedStatus {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    Copied { from: String },
    Other(char),
}

/// List the files staged in the index of the worktree at `root`.
/// Runs `git -C <root> diff --cached --name-status -z` and parses the
/// NUL-delimited porcelain output (NUL-delimited keeps paths unambiguous and
/// gives rename/copy entries their old+new path as separate fields).
pub fn staged_entries(root: &Path) -> Result<Vec<StagedEntry>, GitError> {
    let output = git_exec::exec(
        vec![
            "-C".to_string(),
            root.display().to_string(),
            "diff".to_string(),
            "--cached".to_string(),
            "--name-status".to_string(),
            "-z".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )?;
    Ok(parse_name_status(&output))
}

/// Parse the NUL-delimited output of `git diff --cached --name-status -z`.
/// Each record is a status field (e.g. "A", "M", "R100") followed by one path,
/// or two paths for renames/copies (old then new). Fields are NUL-separated.
fn parse_name_status(out: &str) -> Vec<StagedEntry> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut entries = Vec::new();

    while let Some(status_field) = fields.next() {
        let code = status_field.chars().next().unwrap_or(' ');
        match code {
            'R' | 'C' => {
                let Some(from) = fields.next() else { break };
                let Some(to) = fields.next() else { break };
                let status = if code == 'R' {
                    StagedStatus::Renamed {
                        from: from.to_string(),
                    }
                } else {
                    StagedStatus::Copied {
                        from: from.to_string(),
                    }
                };
                entries.push(StagedEntry {
                    path: to.to_string(),
                    status,
                });
            }
            other => {
                let Some(path) = fields.next() else { break };
                let status = match other {
                    'A' => StagedStatus::Added,
                    'M' => StagedStatus::Modified,
                    'D' => StagedStatus::Deleted,
                    c => StagedStatus::Other(c),
                };
                entries.push(StagedEntry {
                    path: path.to_string(),
                    status,
                });
            }
        }
    }

    entries
}

/// Read the staged (index) contents of `path` in the worktree at `root` via
/// `git -C <root> show :<path>`. Returns raw bytes so binary and
/// whitespace-significant files round-trip exactly.
pub fn show_staged(root: &Path, path: &str) -> Result<Vec<u8>, GitError> {
    git_exec::exec_bytes(
        vec![
            "-C".to_string(),
            root.display().to_string(),
            "show".to_string(),
            format!(":{}", path),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )
}

/// Switch the worktree at `path` to an existing local `branch`
/// (`git -C <path> switch <branch>`). Used by the existing-path branch
/// switch flow when the requested branch differs from what's checked out.
/// `switch` (rather than `checkout <branch>`) only ever resolves a branch,
/// never a pathspec, so a `--` separator is unnecessary and would misparse.
pub fn switch_branch(path: &Path, branch: &str) -> Result<(), GitError> {
    git_exec::exec(
        vec![
            "-C".to_string(),
            path.display().to_string(),
            "switch".to_string(),
            branch.to_string(),
        ],
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// True when `base` resolves to a commit using only local refs
/// (`git rev-parse --verify --quiet <base>^{commit}`). Used to give a targeted
/// offline diagnostic when `--no-fetch` is set before calling [`add`].
pub fn ref_resolvable(base: &str) -> Result<bool, GitError> {
    match git_exec::exec(
        vec![
            "rev-parse".to_string(),
            "--verify".to_string(),
            "--quiet".to_string(),
            format!("{}^{{commit}}", base),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    ) {
        Ok(out) => Ok(!out.trim().is_empty()),
        // rev-parse --verify --quiet exits non-zero with no stderr when the ref
        // is unknown; treat that as "not resolvable" rather than a hard error.
        Err(GitError::CommandFailed(msg)) if msg.trim().is_empty() => Ok(false),
        Err(e) => Err(e),
    }
}

/// Detect a git ref-namespace conflict for a branch we are about to create.
/// Git stores branches as files under `refs/heads`, so `refs/heads/foo` (a
/// file) and `refs/heads/foo/bar` (a directory) cannot coexist. Returns the
/// first existing branch that conflicts with `branch_name`, if any.
pub fn conflicting_branch(branch_name: &str) -> Result<Option<String>, GitError> {
    let output = git_exec::exec(
        vec![
            "for-each-ref".to_string(),
            "--format=%(refname:short)".to_string(),
            "refs/heads".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )?;

    let existing: Vec<String> = output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(find_ref_conflict(branch_name, &existing))
}

/// Pure conflict check over a list of existing branch names. `wanted` conflicts
/// with an existing branch when one is a strict path-prefix of the other:
/// existing `foo` blocks `foo/bar` (file vs directory), and existing `foo/bar`
/// blocks `foo` (directory vs file). An exact match is not a conflict here
/// (that case is handled separately as "branch already exists").
fn find_ref_conflict(wanted: &str, existing: &[String]) -> Option<String> {
    existing
        .iter()
        .find(|name| is_path_prefix(name, wanted) || is_path_prefix(wanted, name))
        .cloned()
}

/// True when `prefix` is a strict slash-delimited path prefix of `name`
/// (e.g. "foo" is a prefix of "foo/bar", but not of "foobar" or "foo").
fn is_path_prefix(prefix: &str, name: &str) -> bool {
    name.len() > prefix.len()
        && name.starts_with(prefix)
        && name.as_bytes()[prefix.len()] == b'/'
}

/// Rebase the branch checked out in the worktree at `path` onto `base`
/// (e.g. 'origin/main'). Runs in the worktree via 'git -C'.
pub fn rebase_onto(path: &Path, base: &str) -> Result<(), GitError> {
    git_exec::exec(
        vec![
            "-C".to_string(),
            path.display().to_string(),
            "rebase".to_string(),
            "--".to_string(),
            base.to_string(),
        ],
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// True when the worktree at `path` has staged or unstaged changes to
/// tracked files (untracked files don't block a rebase, so they're ignored).
pub fn has_tracked_changes(path: &Path) -> Result<bool, GitError> {
    let output = git_exec::exec(
        vec![
            "-C".to_string(),
            path.display().to_string(),
            "status".to_string(),
            "--porcelain".to_string(),
            "--untracked-files=no".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )?;
    Ok(!output.trim().is_empty())
}

/// Remove the worktree at `path`. Runs git from `from` (the main worktree)
/// so removal works even when the process is inside the worktree being
/// removed (git refuses to remove its own current worktree).
pub fn remove(from: &Path, path: &Path, force: bool) -> Result<(), GitError> {
    let mut args = vec![
        "-C".to_string(),
        from.display().to_string(),
        "worktree".to_string(),
        "remove".to_string(),
    ];
    if force {
        args.push("--force".to_string());
    }
    args.push("--".to_string());
    args.push(path.display().to_string());

    git_exec::exec(
        args,
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;

    Ok(())
}

pub fn delete_branch(from: &Path, branch_name: &str, force: bool) -> Result<(), GitError> {
    let delete_flag = if force { "-D" } else { "-d" };
    git_exec::exec(
        vec![
            "-C".to_string(),
            from.display().to_string(),
            "branch".to_string(),
            delete_flag.to_string(),
            "--".to_string(),
            branch_name.to_string(),
        ],
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Move the worktree at `path` to `new_path`. Runs git from `from` (the main
/// worktree) and prefers `git worktree move` over a manual filesystem move so
/// Git's administrative files (`.git` pointer, gitdir link) stay consistent.
pub fn move_worktree(from: &Path, path: &Path, new_path: &Path) -> Result<(), GitError> {
    git_exec::exec(
        move_worktree_args(from, path, new_path),
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Lock the worktree at `path` so cleanup and `git worktree prune` skip it.
/// Runs git from `from` (the main worktree). An optional `reason` is recorded
/// by git and shown in `git worktree list --verbose`.
pub fn lock(from: &Path, path: &Path, reason: Option<&str>) -> Result<(), GitError> {
    git_exec::exec(
        lock_args(from, path, reason),
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Clear the lock on the worktree at `path`. Runs git from `from` (the main
/// worktree).
pub fn unlock(from: &Path, path: &Path) -> Result<(), GitError> {
    git_exec::exec(
        unlock_args(from, path),
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Repair worktree administrative files (the two-way links between the main
/// repository and its linked worktrees). With no `paths`, git repairs every
/// worktree; otherwise it repairs only the listed ones. Mostly for recovery
/// after a worktree directory or the main repo has been moved. Runs git from
/// `from` (the main worktree).
pub fn repair(from: &Path, paths: &[PathBuf]) -> Result<(), GitError> {
    git_exec::exec(
        repair_args(from, paths),
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

// The arg builders are split out as pure functions so they can be unit tested
// without spawning git; `--` always ends option parsing so paths beginning with
// '-' are treated as paths, never flags.

fn move_worktree_args(from: &Path, path: &Path, new_path: &Path) -> Vec<String> {
    vec![
        "-C".to_string(),
        from.display().to_string(),
        "worktree".to_string(),
        "move".to_string(),
        "--".to_string(),
        path.display().to_string(),
        new_path.display().to_string(),
    ]
}

fn lock_args(from: &Path, path: &Path, reason: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "-C".to_string(),
        from.display().to_string(),
        "worktree".to_string(),
        "lock".to_string(),
    ];
    if let Some(reason) = reason {
        args.push("--reason".to_string());
        args.push(reason.to_string());
    }
    args.push("--".to_string());
    args.push(path.display().to_string());
    args
}

fn unlock_args(from: &Path, path: &Path) -> Vec<String> {
    vec![
        "-C".to_string(),
        from.display().to_string(),
        "worktree".to_string(),
        "unlock".to_string(),
        "--".to_string(),
        path.display().to_string(),
    ]
}

fn repair_args(from: &Path, paths: &[PathBuf]) -> Vec<String> {
    let mut args = vec![
        "-C".to_string(),
        from.display().to_string(),
        "worktree".to_string(),
        "repair".to_string(),
    ];
    if !paths.is_empty() {
        args.push("--".to_string());
        args.extend(paths.iter().map(|p| p.display().to_string()));
    }
    args
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
        .map(|worktree| {
            (
                worktree.path.clone(),
                WorktreeSummary::pending_for(worktree),
            )
        })
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
    let status_output = git_exec::exec(
        vec![
            "-C".to_string(),
            worktree.path.display().to_string(),
            "status".to_string(),
            "--porcelain".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )?;

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

fn ahead_behind(path: &Path) -> Result<(Option<usize>, Option<usize>), GitError> {
    // rev-list itself fails when the branch has no upstream, and the caller
    // treats any error here as "no ahead/behind info", so a separate
    // rev-parse @{upstream} existence check would just be a wasted git spawn.
    let output = git_exec::exec(
        vec![
            "-C".to_string(),
            path.display().to_string(),
            "rev-list".to_string(),
            "--left-right".to_string(),
            "--count".to_string(),
            "@{upstream}...HEAD".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
    )?;

    let mut counts = output.split_whitespace();
    let behind = counts.next().and_then(|n| n.parse::<usize>().ok());
    let ahead = counts.next().and_then(|n| n.parse::<usize>().ok());
    Ok((ahead, behind))
}

pub fn branch_exists(branch_name: &str) -> Result<bool, GitError> {
    let repo = get_repo()?;
    Ok(repo
        .find_branch(branch_name, git2::BranchType::Local)
        .is_ok())
}

/// Find a remote-tracking branch matching `branch_name` (e.g. "origin/feature"
/// for "feature"), preferring "origin" when multiple remotes have it.
pub fn find_remote_branch(branch_name: &str) -> Result<Option<String>, GitError> {
    let repo = get_repo()?;

    let mut found: Vec<String> = repo
        .branches(Some(git2::BranchType::Remote))?
        .filter_map(|res| res.ok())
        .filter_map(|(branch, _)| branch.get().shorthand().map(|s| s.to_string()))
        .filter(|shorthand| {
            shorthand
                .split_once('/')
                .is_some_and(|(_, tail)| tail == branch_name)
        })
        .collect();

    found.sort_by_key(|r| (!r.starts_with("origin/"), r.clone()));
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

/// Enumerate local branch shorthands (full names, including any '/').
pub fn list_local_branches() -> Result<Vec<String>, GitError> {
    let repo = get_repo()?;

    let branches_iter = match repo.branches(Some(git2::BranchType::Local)) {
        Ok(iter) => iter,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };

    let names = branches_iter
        .filter_map(|res| res.ok())
        .filter_map(|(branch, _)| branch.get().shorthand().map(|s| s.to_string()))
        .collect();

    Ok(names)
}

/// True when the local branch `branch` has a configured upstream.
pub fn has_upstream(branch: &str) -> Result<bool, GitError> {
    let repo = get_repo()?;
    let local = repo.find_branch(branch, git2::BranchType::Local)?;
    Ok(local.upstream().is_ok())
}

/// Number of commits the local branch `branch` is ahead of its upstream.
/// Returns `Ok(None)` when the branch has no upstream configured (so callers
/// can distinguish "no upstream" from "0 ahead"). Branch-name variant of the
/// path-based [`ahead_behind`].
pub fn unpushed_count(branch: &str) -> Result<Option<usize>, GitError> {
    let repo = get_repo()?;

    let local = match repo.find_branch(branch, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };

    let upstream = match local.upstream() {
        Ok(u) => u,
        Err(_) => return Ok(None),
    };

    let local_oid = local.get().peel_to_commit()?.id();
    let upstream_oid = upstream.get().peel_to_commit()?.id();

    let (ahead, _behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;
    Ok(Some(ahead))
}

/// True when `branch` has an upstream and is ahead of it. A branch with no
/// upstream is *not* reported as unpushed here; callers that need to treat a
/// missing upstream as unsafe should consult [`has_upstream`] separately.
pub fn has_unpushed(branch: &str) -> Result<bool, GitError> {
    Ok(unpushed_count(branch)?.map(|ahead| ahead > 0).unwrap_or(false))
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
    let all_local = list_local_branches()?;
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
        vec![
            "for-each-ref".to_string(),
            "--format=%(refname:short) %(upstream:track)".to_string(),
            "refs/heads".to_string(),
        ],
        ExecOptions {
            capture: true,
            ..Default::default()
        },
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

/// Run `git worktree prune` from `from` (the main worktree) to drop metadata for
/// worktrees whose directories no longer exist.
pub fn prune_metadata(from: &Path) -> Result<(), GitError> {
    git_exec::exec(
        vec![
            "-C".to_string(),
            from.display().to_string(),
            "worktree".to_string(),
            "prune".to_string(),
        ],
        ExecOptions {
            silent: true,
            ..Default::default()
        },
    )?;
    Ok(())
}

fn parse_porcelain(output: &str, current_root: Option<&Path>) -> Vec<Worktree> {
    let current_canonical = current_root.and_then(|p| p.canonicalize().ok());
    let mut worktrees = Vec::new();

    for entry in output.split("\n\n") {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        let mut path: Option<PathBuf> = None;
        let mut branch: Option<String> = None;
        let mut head: Option<String> = None;
        let mut is_bare = false;
        let mut is_locked = false;

        for line in entry.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                head = Some(h.chars().take(7).collect());
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if line == "bare" {
                is_bare = true;
            } else if line == "locked" || line.starts_with("locked ") {
                is_locked = true;
            }
        }

        let Some(path) = path else { continue };

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        let is_current = match (&current_canonical, path.canonicalize()) {
            (Some(current), Ok(canonical)) => canonical == *current,
            _ => current_root == Some(path.as_path()),
        };

        worktrees.push(Worktree {
            name,
            path,
            branch,
            head,
            is_main: worktrees.is_empty(),
            is_current,
            is_bare,
            is_locked,
        });
    }

    worktrees
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_porcelain_basic() {
        let output = "worktree /home/user/proj\nHEAD 1234567890abcdef\nbranch refs/heads/main\n\nworktree /home/user/proj-workspaces/feature\nHEAD fedcba0987654321\nbranch refs/heads/feature";
        let worktrees = parse_porcelain(output, Some(Path::new("/home/user/proj")));

        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].name, "proj");
        assert_eq!(worktrees[0].path, PathBuf::from("/home/user/proj"));
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[0].head.as_deref(), Some("1234567"));
        assert!(worktrees[0].is_main);
        assert!(worktrees[0].is_current);

        assert_eq!(worktrees[1].name, "feature");
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature"));
        assert!(!worktrees[1].is_main);
        assert!(!worktrees[1].is_current);
    }

    #[test]
    fn test_parse_porcelain_detached_and_locked() {
        let output = "worktree /repo\nHEAD aaaaaaa1111111\nbranch refs/heads/main\n\nworktree /repo-ws/hotfix\nHEAD bbbbbbb2222222\ndetached\nlocked reason here";
        let worktrees = parse_porcelain(output, None);

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[1].branch, None);
        assert!(worktrees[1].is_locked);
        assert!(!worktrees[1].is_main);
    }

    #[test]
    fn test_parse_porcelain_bare() {
        let output = "worktree /repo.git\nbare\n\nworktree /repo-ws/dev\nHEAD ccccccc3333333\nbranch refs/heads/dev";
        let worktrees = parse_porcelain(output, None);

        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[0].is_bare);
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].branch, None);
        assert!(!worktrees[1].is_bare);
    }

    #[test]
    fn test_parse_porcelain_empty() {
        assert!(parse_porcelain("", None).is_empty());
    }

    #[test]
    fn test_parse_status_counts() {
        let output = " M src/main.rs\nA  src/new.rs\n?? scratch.txt\n?? notes/todo.md\n";
        let (tracked, untracked) = parse_status_counts(output);

        assert_eq!(tracked, 2);
        assert_eq!(untracked, 2);
    }

    fn worktree(name: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            name: name.to_string(),
            path: PathBuf::from(format!("/ws/{}", name)),
            branch: branch.map(|b| b.to_string()),
            head: None,
            is_main: false,
            is_current: false,
            is_bare: false,
            is_locked: false,
        }
    }

    fn pr_summary(number: usize) -> crate::git::pull_request::PullRequestSummary {
        crate::git::pull_request::PullRequestSummary {
            number,
            state: crate::git::pull_request::PullRequestState::Open,
            url: format!("https://example.com/pull/{number}"),
        }
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

    #[test]
    fn test_flatten_slashes() {
        assert_eq!(flatten_slashes("feature-x"), "feature-x");
        assert_eq!(
            flatten_slashes("feat/expose-rationale"),
            "feat-expose-rationale"
        );
        assert_eq!(flatten_slashes("a/b/c"), "a-b-c");
    }

    #[test]
    fn test_matches_exactly() {
        let w = worktree("feat-expose-rationale", Some("feat/expose-rationale"));
        assert!(w.matches_exactly("feat-expose-rationale"));
        assert!(w.matches_exactly("feat/expose-rationale"));
        assert!(w.matches_exactly("FEAT/EXPOSE-RATIONALE"));
        assert!(!w.matches_exactly("feat/expose"));

        // workspace dir differs from branch: both still match exactly
        let w = worktree("custom-dir", Some("fix/null-check"));
        assert!(w.matches_exactly("custom-dir"));
        assert!(w.matches_exactly("fix/null-check"));
        assert!(w.matches_exactly("fix-null-check"));

        let detached = worktree("hotfix", None);
        assert!(detached.matches_exactly("hotfix"));
        assert!(!detached.matches_exactly("fix/null-check"));
    }

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

        assert_eq!(orphans, vec!["experiment".to_string(), "old-fix".to_string()]);
    }

    #[test]
    fn test_parse_gone_branches() {
        let output = "main [ahead 1]\nfeat/x [gone]\nrelease \nold-thing [gone]\nahead-only [behind 2]";
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

    #[test]
    fn test_find_ref_conflict() {
        let existing = vec!["foo".to_string(), "feat/a".to_string(), "main".to_string()];

        // existing 'foo' (a file) blocks creating 'foo/bar' (a directory)
        assert_eq!(find_ref_conflict("foo/bar", &existing), Some("foo".to_string()));
        // existing 'feat/a' (a directory) blocks creating 'feat' (a file)
        assert_eq!(find_ref_conflict("feat", &existing), Some("feat/a".to_string()));

        // exact matches are not conflicts (handled as "already exists")
        assert_eq!(find_ref_conflict("foo", &existing), None);
        assert_eq!(find_ref_conflict("main", &existing), None);

        // unrelated names and shared text prefixes (not path prefixes) are fine
        assert_eq!(find_ref_conflict("bar", &existing), None);
        assert_eq!(find_ref_conflict("foobar", &existing), None);
        assert_eq!(find_ref_conflict("feat-b", &existing), None);
    }

    #[test]
    fn test_parse_name_status_added_modified_deleted() {
        // NUL-delimited: each record is "<status>\0<path>\0"
        let out = "A\0src/new.rs\0M\0src/main.rs\0D\0old.rs\0";
        let entries = parse_name_status(out);

        assert_eq!(
            entries,
            vec![
                StagedEntry {
                    path: "src/new.rs".to_string(),
                    status: StagedStatus::Added,
                },
                StagedEntry {
                    path: "src/main.rs".to_string(),
                    status: StagedStatus::Modified,
                },
                StagedEntry {
                    path: "old.rs".to_string(),
                    status: StagedStatus::Deleted,
                },
            ]
        );
    }

    #[test]
    fn test_parse_name_status_rename_has_from_and_to() {
        // Renames carry a similarity score (R100) then old\0new.
        let out = "R100\0src/old.rs\0src/new.rs\0M\0other.rs\0";
        let entries = parse_name_status(out);

        assert_eq!(
            entries,
            vec![
                StagedEntry {
                    path: "src/new.rs".to_string(),
                    status: StagedStatus::Renamed {
                        from: "src/old.rs".to_string(),
                    },
                },
                StagedEntry {
                    path: "other.rs".to_string(),
                    status: StagedStatus::Modified,
                },
            ]
        );
    }

    #[test]
    fn test_parse_name_status_copy_has_from_and_to() {
        let out = "C75\0src/template.rs\0src/copy.rs\0";
        let entries = parse_name_status(out);

        assert_eq!(
            entries,
            vec![StagedEntry {
                path: "src/copy.rs".to_string(),
                status: StagedStatus::Copied {
                    from: "src/template.rs".to_string(),
                },
            }]
        );
    }

    #[test]
    fn test_parse_name_status_empty() {
        assert!(parse_name_status("").is_empty());
        assert!(parse_name_status("\0").is_empty());
    }

    #[test]
    fn test_match_score_slash_dash_interchangeable() {
        let matcher = SkimMatcherV2::default();
        let w = worktree("feat-expose-rationale", Some("feat/expose-rationale"));

        assert!(w.match_score(&matcher, "feat/expo").is_some());
        assert!(w.match_score(&matcher, "feat-expo").is_some());
        assert!(w.match_score(&matcher, "expose").is_some());
        assert!(w.match_score(&matcher, "zzz-no-match").is_none());

        // dash query against a slash-only branch on a differently named dir
        let w = worktree("custom-dir", Some("fix/null-check"));
        assert!(w.match_score(&matcher, "fix-null").is_some());
        assert!(w.match_score(&matcher, "fix/null").is_some());
    }

    #[test]
    fn test_move_worktree_args() {
        let args = move_worktree_args(
            Path::new("/repo"),
            Path::new("/ws/feature"),
            Path::new("/ws/moved"),
        );
        assert_eq!(
            args,
            vec![
                "-C",
                "/repo",
                "worktree",
                "move",
                "--",
                "/ws/feature",
                "/ws/moved"
            ]
        );
    }

    #[test]
    fn test_lock_args_with_and_without_reason() {
        let with_reason = lock_args(
            Path::new("/repo"),
            Path::new("/ws/feature"),
            Some("long review"),
        );
        assert_eq!(
            with_reason,
            vec![
                "-C",
                "/repo",
                "worktree",
                "lock",
                "--reason",
                "long review",
                "--",
                "/ws/feature"
            ]
        );

        let without_reason = lock_args(Path::new("/repo"), Path::new("/ws/feature"), None);
        assert_eq!(
            without_reason,
            vec!["-C", "/repo", "worktree", "lock", "--", "/ws/feature"]
        );
    }

    #[test]
    fn test_unlock_args() {
        let args = unlock_args(Path::new("/repo"), Path::new("/ws/feature"));
        assert_eq!(
            args,
            vec!["-C", "/repo", "worktree", "unlock", "--", "/ws/feature"]
        );
    }

    #[test]
    fn test_repair_args_forwards_paths() {
        // No paths: git repairs every worktree, so no `--` / path args.
        let all = repair_args(Path::new("/repo"), &[]);
        assert_eq!(all, vec!["-C", "/repo", "worktree", "repair"]);

        // Targeted: paths are forwarded after `--`.
        let targeted = repair_args(
            Path::new("/repo"),
            &[PathBuf::from("/ws/a"), PathBuf::from("/ws/b")],
        );
        assert_eq!(
            targeted,
            vec!["-C", "/repo", "worktree", "repair", "--", "/ws/a", "/ws/b"]
        );
    }
}
