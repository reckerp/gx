use super::{GitError, get_repo};
use crate::git::git_exec::{self, ExecOptions};
use crate::git::pull_request::{PullRequestLookup, PullRequestStatus};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    pub status_error: bool,
}

impl WorktreeSummary {
    pub fn has_changes(&self) -> bool {
        self.tracked_changes > 0 || self.untracked_changes > 0
    }

    pub fn has_unpushed_commits(&self) -> bool {
        self.ahead.unwrap_or(0) > 0
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
pub fn add(
    path: &Path,
    branch: &str,
    create_branch: bool,
    base: Option<&str>,
    no_track: bool,
) -> Result<(), GitError> {
    let mut args = vec!["worktree".to_string(), "add".to_string()];

    if create_branch {
        if no_track {
            args.push("--no-track".to_string());
        }
        args.push("-b".to_string());
        args.push(branch.to_string());
        // End option parsing so a path or base beginning with '-' is never read
        // as a flag.
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

/// Compute the local (network-free) status for every worktree, in parallel.
/// Each summary shells out to git (status + ahead/behind) against an independent
/// working tree, so the work is I/O-bound and embarrassingly parallel; doing it
/// serially makes opening the workspace picker scale linearly with the number of
/// worktrees. Pull-request status is left as `Loading`; resolve it separately
/// via [`crate::git::pull_request::spawn_lookup`] and [`apply_pull_requests`] so
/// the caller never blocks on the network.
pub fn summarize_all(worktrees: &[Worktree]) -> HashMap<PathBuf, WorktreeSummary> {
    if worktrees.is_empty() {
        return HashMap::new();
    }

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(PathBuf, WorktreeSummary)>> =
        Mutex::new(Vec::with_capacity(worktrees.len()));

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(worktrees.len());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(worktree) = worktrees.get(i) else {
                        break;
                    };
                    let summary = summarize(worktree).unwrap_or_else(|_| WorktreeSummary {
                        status_error: true,
                        ..Default::default()
                    });
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
}
