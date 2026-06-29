use crate::commands::workspace::{delete_local_branch, main_worktree_root};
use crate::config::{self, Config};
use crate::git::time::now_secs;
use crate::git::worktree::{OrphanBranch, Worktree, WorktreeSummary};
use crate::git::{self, GitError};
use crate::ui;
use crate::ui::clean_picker::{CleanAction, CleanInputs};
use miette::{Diagnostic, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum WorkspaceCleanError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::workspace_clean::git_error))]
    GitError(#[from] GitError),

    #[error("TUI error: {0}")]
    #[diagnostic(code(gx::workspace_clean::tui_error))]
    TuiError(String),

    #[error("Cannot protect a detached HEAD; check out a branch first")]
    #[diagnostic(code(gx::workspace_clean::detached_head))]
    DetachedHead,

    #[error("No branch resolved: {0}")]
    #[diagnostic(code(gx::workspace_clean::no_branch))]
    NoBranch(String),
}

/// The set of branches cleanup must never remove: the configured list plus the
/// always-protected branches (default branch, `main`, `master`, the current
/// branch, and any branch checked out in an active worktree).
#[derive(Debug, Clone, Default)]
pub(crate) struct ProtectionSet {
    names: HashSet<String>,
}

impl ProtectionSet {
    pub(crate) fn is_protected(&self, branch: &str) -> bool {
        self.names.contains(branch)
    }

    pub(crate) fn names(&self) -> &HashSet<String> {
        &self.names
    }
}

/// Pure assembly of the protected-branch set from already-resolved inputs.
/// Extracted from [`resolve_protected`] so it is unit-testable without a repo.
fn build_protection_set(
    configured: &[String],
    default_branch: Option<&str>,
    current_branch: Option<&str>,
    checked_out: impl IntoIterator<Item = String>,
) -> ProtectionSet {
    let mut names: HashSet<String> = HashSet::new();
    names.extend(configured.iter().cloned());
    names.insert("main".to_string());
    names.insert("master".to_string());
    if let Some(default_branch) = default_branch {
        names.insert(default_branch.to_string());
    }
    if let Some(current) = current_branch {
        names.insert(current.to_string());
    }
    names.extend(checked_out);

    ProtectionSet { names }
}

/// Resolve the active protection set for the repository.
fn resolve_protected(cfg: &Config, worktrees: &[Worktree]) -> ProtectionSet {
    // origin's default branch comes back as "origin/main"; protect the local
    // branch name, so strip the remote prefix.
    let default_branch = git::branch::default_remote_branch()
        .ok()
        .flatten()
        .map(|remote| strip_remote_prefix(&remote).to_string());

    let current_branch = git::branch::get_current_branch()
        .ok()
        .filter(|b| !b.is_detached)
        .map(|b| b.name);

    let checked_out = worktrees
        .iter()
        .filter_map(|w| w.branch.clone())
        .collect::<Vec<_>>();

    build_protection_set(
        &cfg.workspace.protected_branches,
        default_branch.as_deref(),
        current_branch.as_deref(),
        checked_out,
    )
}

/// Strip a leading "<remote>/" component (e.g. "origin/main" -> "main").
fn strip_remote_prefix(remote_branch: &str) -> &str {
    remote_branch
        .split_once('/')
        .map(|(_, tail)| tail)
        .unwrap_or(remote_branch)
}

/// The outcome of evaluating a workspace against the safety rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SafetyResult {
    Safe,
    Unsafe(&'static str),
}

/// Decide whether a workspace is safe to remove. Pure over its inputs so the
/// dirty/unpushed/locked/protected/main/current rules are unit-testable.
///
/// `force` bypasses ONLY the dirty/untracked/unpushed checks. The main worktree,
/// current worktree, locked worktrees, and protected branches are never removed,
/// even with `--force`.
pub(crate) fn is_safe_to_remove(
    worktree: &Worktree,
    summary: &WorktreeSummary,
    protection: &ProtectionSet,
    force: bool,
) -> SafetyResult {
    if worktree.is_main {
        return SafetyResult::Unsafe("main worktree");
    }
    if worktree.is_current {
        return SafetyResult::Unsafe("current worktree");
    }
    if worktree.is_locked {
        return SafetyResult::Unsafe("locked");
    }
    if worktree
        .branch
        .as_deref()
        .is_some_and(|b| protection.is_protected(b))
    {
        return SafetyResult::Unsafe("protected branch");
    }

    if force {
        return SafetyResult::Safe;
    }

    if summary.tracked_changes > 0 {
        return SafetyResult::Unsafe("uncommitted changes");
    }
    if summary.untracked_changes > 0 {
        return SafetyResult::Unsafe("untracked files");
    }
    if summary.has_unpushed_commits() {
        return SafetyResult::Unsafe("unpushed commits");
    }
    // Mirror the orphan-branch rule: a branch with no configured upstream has
    // never been pushed, so its commits exist only here. `ahead == None` (with a
    // branch present) means the upstream comparison could not be made; treat that
    // as unsafe rather than assuming "0 unpushed". A detached worktree (no
    // branch) has no branch to lose, so it is exempt.
    if worktree.branch.is_some() && summary.ahead.is_none() {
        return SafetyResult::Unsafe("no upstream (never pushed)");
    }

    SafetyResult::Safe
}

/// `gx workspace clean [--auto --dry-run --use-threshold --force]`.
pub fn run_clean(auto: bool, dry_run: bool, use_threshold: bool, force: bool) -> Result<()> {
    let cfg = config::load()?;

    // CLI flag OR config: an explicit --auto flag or `clean.auto = true` both
    // select the non-interactive path.
    let auto = auto || cfg.workspace.clean.auto;

    if auto {
        run_clean_auto(&cfg, dry_run, use_threshold, force)
    } else {
        run_clean_interactive(&cfg)
    }
}

fn run_clean_auto(cfg: &Config, dry_run: bool, use_threshold: bool, force: bool) -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceCleanError::GitError)?;
    let protection = resolve_protected(cfg, &worktrees);
    let main_root = main_worktree_root(&worktrees)?;
    let now = now_secs();
    let threshold = cfg.workspace.clean.threshold_days;

    let mut safe: Vec<Worktree> = Vec::new();

    for worktree in &worktrees {
        if worktree.is_main || worktree.is_current {
            continue;
        }

        if use_threshold {
            let age = git::worktree::workspace_age_days(worktree, now)
                .map_err(WorkspaceCleanError::GitError)?;
            if age < threshold {
                eprintln!(
                    "Skipping '{}': below threshold ({} < {} days)",
                    worktree.name, age, threshold
                );
                continue;
            }
        }

        let summary = git::worktree::summarize(worktree).map_err(WorkspaceCleanError::GitError)?;
        match is_safe_to_remove(worktree, &summary, &protection, force) {
            SafetyResult::Safe => safe.push(worktree.clone()),
            SafetyResult::Unsafe(reason) => {
                eprintln!("Skipping '{}': {}", worktree.name, reason);
            }
        }
    }

    if safe.is_empty() {
        eprintln!("No workspaces are safe to remove.");
        return Ok(());
    }

    if dry_run {
        eprintln!("Would remove {} workspace(s):", safe.len());
        for worktree in &safe {
            eprintln!("  - {} ({})", worktree.name, worktree.path.display());
        }
        eprintln!("(dry run; nothing was deleted)");
        return Ok(());
    }

    let prompt = clean_confirm_prompt(&safe);
    if !ui::confirm::run_on_stderr(&prompt)? {
        eprintln!("Cancelled");
        return Ok(());
    }

    for worktree in &safe {
        eprintln!("Removing workspace '{}'...", worktree.name);
        // Safety was already enforced above; `force` here only relaxes git's own
        // dirty-file guard so a --force run can actually proceed.
        git::worktree::remove(&main_root, &worktree.path, force)
            .map_err(WorkspaceCleanError::GitError)?;
        eprintln!("Removed workspace '{}'", worktree.name);
    }

    Ok(())
}

fn clean_confirm_prompt(safe: &[Worktree]) -> String {
    if let [worktree] = safe {
        return format!(
            "Remove workspace '{}' ({})?",
            worktree.name,
            worktree.path.display()
        );
    }

    let mut lines = vec![format!("Remove {} workspace(s)?", safe.len()), String::new()];
    lines.extend(
        safe.iter()
            .map(|w| format!("  - {} ({})", w.name, w.path.display())),
    );
    lines.join("\n")
}

fn run_clean_interactive(cfg: &Config) -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceCleanError::GitError)?;
    let protection = resolve_protected(cfg, &worktrees);
    let main_root = main_worktree_root(&worktrees)?;

    let gone_branches =
        git::worktree::remote_gone_branches().map_err(WorkspaceCleanError::GitError)?;
    let gone_set: HashSet<&str> = gone_branches.iter().map(String::as_str).collect();

    let orphans =
        git::worktree::orphan_branches(&worktrees).map_err(WorkspaceCleanError::GitError)?;
    // A `[gone]` branch with no worktree would otherwise appear in BOTH this
    // section and the "remote tracking branch is gone" section below, sharing
    // the same `branch:<name>` selection key and producing a duplicate delete.
    // Surface each branch in exactly one section.
    let orphan_branches: Vec<(String, bool, bool)> = orphans
        .into_iter()
        .filter(|o: &OrphanBranch| !gone_set.contains(o.name.as_str()))
        .map(|o: OrphanBranch| (o.name, o.has_unpushed, o.has_upstream))
        .collect();

    // Compute each workspace's age once here instead of on every TUI frame: the
    // render loop redraws ~20x/sec and the age lookup does repository discovery
    // plus a revparse per row, which is wasted git I/O if recomputed live.
    let now = now_secs();
    let ages: HashMap<PathBuf, u64> = worktrees
        .iter()
        .filter_map(|w| git::worktree::workspace_age_days(w, now).ok().map(|d| (w.path.clone(), d)))
        .collect();

    let inputs = CleanInputs {
        worktrees: worktrees.clone(),
        orphan_branches,
        gone_branches,
        protected: protection.names().clone(),
        ages,
    };

    let Some(action) = pick_clean(inputs, &worktrees)? else {
        eprintln!("Cancelled");
        return Ok(());
    };

    if !action.confirmed
        || (action.remove_worktrees.is_empty() && action.delete_branches.is_empty())
    {
        eprintln!("Nothing selected");
        return Ok(());
    }

    apply_clean_action(&action, &main_root)
}

fn pick_clean(inputs: CleanInputs, worktrees: &[Worktree]) -> Result<Option<CleanAction>> {
    let summary_lookup = git::worktree::spawn_summary_lookup(worktrees);
    ui::terminal::with_terminal_stderr(|t| ui::clean_picker::run(t, inputs, summary_lookup))
        .map_err(|e| WorkspaceCleanError::TuiError(e.to_string()))?
}

fn apply_clean_action(action: &CleanAction, main_root: &Path) -> Result<()> {
    // Remove the current workspace last so a failure elsewhere does not strand
    // the user's shell inside a deleted directory.
    let mut targets = action.remove_worktrees.clone();
    targets.sort_by_key(|w| w.is_current);
    let removes_current = targets.iter().any(|w| w.is_current);

    for worktree in &targets {
        eprintln!("Removing workspace '{}'...", worktree.name);
        match git::worktree::remove(main_root, &worktree.path, false) {
            Ok(()) => {}
            Err(GitError::CommandFailed { stderr: msg, .. })
                if msg.contains("contains modified or untracked files") =>
            {
                let confirmed = ui::confirm::run_on_stderr(&format!(
                    "Workspace '{}' has modified or untracked files. Remove anyway?",
                    worktree.name
                ))?;
                if !confirmed {
                    eprintln!("Skipped '{}'", worktree.name);
                    continue;
                }
                git::worktree::remove(main_root, &worktree.path, true)
                    .map_err(WorkspaceCleanError::GitError)?;
            }
            Err(e) => return Err(WorkspaceCleanError::GitError(e).into()),
        }

        match &worktree.branch {
            Some(branch) => eprintln!(
                "Removed workspace '{}' (branch '{}' kept)",
                worktree.name, branch
            ),
            None => eprintln!("Removed workspace '{}'", worktree.name),
        }
    }

    for branch in &action.delete_branches {
        eprintln!("Deleting branch '{}'...", branch);
        delete_local_branch(main_root, branch)?;
    }

    if removes_current {
        eprintln!("Switching to main workspace");
        crate::commands::workspace::print_go_path(main_root);
    }

    Ok(())
}

/// `gx workspace prune [--dry-run --no-branches]`.
pub fn run_prune(dry_run: bool, no_branches: bool) -> Result<()> {
    let cfg = config::load()?;
    let worktrees = git::worktree::list().map_err(WorkspaceCleanError::GitError)?;
    let main_root = main_worktree_root(&worktrees)?;

    // Step 1: prune stale worktree metadata.
    if dry_run {
        eprintln!("Would run 'git worktree prune' to drop stale worktree metadata");
    } else {
        git::worktree::prune_metadata(&main_root).map_err(WorkspaceCleanError::GitError)?;
        eprintln!("Pruned stale worktree metadata");
    }

    if no_branches {
        eprintln!("--no-branches: skipping branch cleanup");
        return Ok(());
    }

    // Re-list after pruning so removed worktrees no longer pin their branches.
    let worktrees = git::worktree::list().map_err(WorkspaceCleanError::GitError)?;
    let protection = resolve_protected(&cfg, &worktrees);

    let orphans =
        git::worktree::orphan_branches(&worktrees).map_err(WorkspaceCleanError::GitError)?;

    let deletable = prunable_branches(&orphans, &protection);

    if deletable.is_empty() {
        eprintln!("No orphan branches are safe to delete.");
        return Ok(());
    }

    if dry_run {
        eprintln!("Would delete {} orphan branch(es):", deletable.len());
        for branch in &deletable {
            eprintln!("  - {}", branch);
        }
        eprintln!("(dry run; nothing was deleted)");
        return Ok(());
    }

    let prompt = prune_confirm_prompt(&deletable);
    if !ui::confirm::run_on_stderr(&prompt)? {
        eprintln!("Cancelled");
        return Ok(());
    }

    for branch in &deletable {
        eprintln!("Deleting branch '{}'...", branch);
        delete_local_branch(&main_root, branch)?;
    }

    Ok(())
}

/// Select orphan branches that are safe to delete: not protected, no unpushed
/// commits, and (per spec) not lacking an upstream. Pure for unit testing.
fn prunable_branches(orphans: &[OrphanBranch], protection: &ProtectionSet) -> Vec<String> {
    orphans
        .iter()
        .filter(|o| !protection.is_protected(&o.name))
        .filter(|o| !o.has_unpushed)
        // A branch with no upstream is treated as unsafe: it has never been
        // pushed, so deleting it could discard the only copy of the work.
        .filter(|o| o.has_upstream)
        .map(|o| o.name.clone())
        .collect()
}

fn prune_confirm_prompt(branches: &[String]) -> String {
    let mut lines = vec![
        format!("Delete {} orphan branch(es)?", branches.len()),
        String::new(),
    ];
    lines.extend(branches.iter().map(|b| format!("  - {}", b)));
    lines.join("\n")
}

/// `gx workspace protect [branch]`.
pub fn run_protect(branch: Option<String>) -> Result<()> {
    let branch = resolve_branch_arg(branch)?;
    let mut cfg = config::load()?;

    if cfg.workspace.protected_branches.iter().any(|b| b == &branch) {
        eprintln!("Branch '{}' is already protected", branch);
        return Ok(());
    }

    cfg.workspace.protected_branches.push(branch.clone());
    config::store(&cfg)?;

    eprintln!("Protected branch '{}'", branch);
    if is_implicitly_protected(&branch) {
        eprintln!(
            "note: '{}' was already protected by default rules; it is now listed explicitly too",
            branch
        );
    }
    Ok(())
}

/// `gx workspace unprotect [branch]`.
pub fn run_unprotect(branch: Option<String>) -> Result<()> {
    let branch = resolve_branch_arg(branch)?;
    let mut cfg = config::load()?;

    let before = cfg.workspace.protected_branches.len();
    cfg.workspace.protected_branches.retain(|b| b != &branch);

    if cfg.workspace.protected_branches.len() == before {
        eprintln!("'{}' was not in the protected list", branch);
    } else {
        config::store(&cfg)?;
        eprintln!("Removed '{}' from the protected list", branch);
    }

    // Even after removal, the always-protected rules may still cover the branch.
    if is_implicitly_protected(&branch) {
        eprintln!(
            "note: '{}' remains protected by default rules (default/main/master/current/checked-out)",
            branch
        );
    }

    Ok(())
}

/// Resolve a `[branch]` argument: an explicit value is used as-is; otherwise the
/// current branch is resolved, erroring on a detached HEAD.
fn resolve_branch_arg(branch: Option<String>) -> Result<String> {
    match branch {
        Some(branch) => Ok(branch),
        None => {
            let status =
                git::branch::get_current_branch().map_err(WorkspaceCleanError::GitError)?;
            if status.is_detached {
                return Err(WorkspaceCleanError::DetachedHead.into());
            }
            if status.name == "(no commits)" {
                return Err(WorkspaceCleanError::NoBranch(
                    "the repository has no commits yet".to_string(),
                )
                .into());
            }
            Ok(status.name)
        }
    }
}

/// True when a branch is protected by the always-protected rules regardless of
/// the configured list (best-effort: a non-repo context just answers `main`/
/// `master`). Used only for advisory messaging.
fn is_implicitly_protected(branch: &str) -> bool {
    if branch == "main" || branch == "master" {
        return true;
    }

    let default_branch = git::branch::default_remote_branch()
        .ok()
        .flatten()
        .map(|remote| strip_remote_prefix(&remote).to_string());
    if default_branch.as_deref() == Some(branch) {
        return true;
    }

    let current = git::branch::get_current_branch()
        .ok()
        .filter(|b| !b.is_detached)
        .map(|b| b.name);
    if current.as_deref() == Some(branch) {
        return true;
    }

    git::worktree::list()
        .map(|wts| {
            wts.iter()
                .any(|w| w.branch.as_deref() == Some(branch))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    /// A clean, fully-pushed branch: no changes and an upstream that HEAD is
    /// level with (`ahead: Some(0)`, i.e. upstream exists but nothing unpushed).
    fn clean_summary() -> WorktreeSummary {
        WorktreeSummary {
            ahead: Some(0),
            status_loaded: true,
            ..Default::default()
        }
    }

    fn protection(branches: &[&str]) -> ProtectionSet {
        ProtectionSet {
            names: branches.iter().map(|b| b.to_string()).collect(),
        }
    }

    #[test]
    fn test_build_protection_set_includes_always_protected() {
        let set = build_protection_set(
            &["staging".to_string(), "release".to_string()],
            Some("develop"),
            Some("feature/x"),
            vec!["checked-out".to_string()],
        );

        for branch in [
            "staging",
            "release",
            "main",
            "master",
            "develop",
            "feature/x",
            "checked-out",
        ] {
            assert!(set.is_protected(branch), "{} should be protected", branch);
        }
        assert!(!set.is_protected("random-branch"));
    }

    #[test]
    fn test_strip_remote_prefix() {
        assert_eq!(strip_remote_prefix("origin/main"), "main");
        assert_eq!(strip_remote_prefix("upstream/feature/x"), "feature/x");
        assert_eq!(strip_remote_prefix("main"), "main");
    }

    #[test]
    fn test_main_worktree_never_removed_even_with_force() {
        let mut main = worktree("repo", Some("main"));
        main.is_main = true;
        let protection = protection(&[]);

        assert_eq!(
            is_safe_to_remove(&main, &clean_summary(), &protection, true),
            SafetyResult::Unsafe("main worktree")
        );
        assert_eq!(
            is_safe_to_remove(&main, &clean_summary(), &protection, false),
            SafetyResult::Unsafe("main worktree")
        );
    }

    #[test]
    fn test_current_worktree_never_removed() {
        let mut current = worktree("ws", Some("feature"));
        current.is_current = true;
        assert_eq!(
            is_safe_to_remove(&current, &clean_summary(), &protection(&[]), true),
            SafetyResult::Unsafe("current worktree")
        );
    }

    #[test]
    fn test_locked_worktree_is_skipped() {
        let mut locked = worktree("ws", Some("feature"));
        locked.is_locked = true;
        // Even with --force a locked worktree is never removed.
        assert_eq!(
            is_safe_to_remove(&locked, &clean_summary(), &protection(&[]), true),
            SafetyResult::Unsafe("locked")
        );
    }

    #[test]
    fn test_protected_branch_is_skipped() {
        let ws = worktree("ws", Some("staging"));
        assert_eq!(
            is_safe_to_remove(&ws, &clean_summary(), &protection(&["staging"]), true),
            SafetyResult::Unsafe("protected branch")
        );
    }

    #[test]
    fn test_dirty_workspace_skipped_without_force() {
        let ws = worktree("ws", Some("feature"));
        let dirty = WorktreeSummary {
            tracked_changes: 2,
            status_loaded: true,
            ..Default::default()
        };

        assert_eq!(
            is_safe_to_remove(&ws, &dirty, &protection(&[]), false),
            SafetyResult::Unsafe("uncommitted changes")
        );
        // --force bypasses the dirty check.
        assert_eq!(
            is_safe_to_remove(&ws, &dirty, &protection(&[]), true),
            SafetyResult::Safe
        );
    }

    #[test]
    fn test_untracked_workspace_skipped_without_force() {
        let ws = worktree("ws", Some("feature"));
        let untracked = WorktreeSummary {
            untracked_changes: 1,
            status_loaded: true,
            ..Default::default()
        };
        assert_eq!(
            is_safe_to_remove(&ws, &untracked, &protection(&[]), false),
            SafetyResult::Unsafe("untracked files")
        );
        assert_eq!(
            is_safe_to_remove(&ws, &untracked, &protection(&[]), true),
            SafetyResult::Safe
        );
    }

    #[test]
    fn test_unpushed_workspace_skipped_without_force() {
        let ws = worktree("ws", Some("feature"));
        let unpushed = WorktreeSummary {
            ahead: Some(3),
            status_loaded: true,
            ..Default::default()
        };
        assert_eq!(
            is_safe_to_remove(&ws, &unpushed, &protection(&[]), false),
            SafetyResult::Unsafe("unpushed commits")
        );
        // --force bypasses the unpushed check too.
        assert_eq!(
            is_safe_to_remove(&ws, &unpushed, &protection(&[]), true),
            SafetyResult::Safe
        );
    }

    #[test]
    fn test_clean_workspace_is_safe() {
        let ws = worktree("ws", Some("feature"));
        assert_eq!(
            is_safe_to_remove(&ws, &clean_summary(), &protection(&[]), false),
            SafetyResult::Safe
        );
    }

    #[test]
    fn test_workspace_with_no_upstream_is_unsafe_without_force() {
        // `ahead: None` with a branch present means the upstream comparison
        // could not be made (never pushed). Mirrors the orphan-branch rule:
        // treat it as unsafe rather than assuming nothing is unpushed.
        let ws = worktree("ws", Some("feature"));
        let no_upstream = WorktreeSummary {
            ahead: None,
            status_loaded: true,
            ..Default::default()
        };
        assert_eq!(
            is_safe_to_remove(&ws, &no_upstream, &protection(&[]), false),
            SafetyResult::Unsafe("no upstream (never pushed)")
        );
        // --force bypasses the never-pushed guard, like the other content checks.
        assert_eq!(
            is_safe_to_remove(&ws, &no_upstream, &protection(&[]), true),
            SafetyResult::Safe
        );
    }

    #[test]
    fn test_detached_workspace_not_flagged_for_missing_upstream() {
        // A detached worktree (no branch) has `ahead: None` but no branch to
        // lose, so the never-pushed guard must not fire.
        let detached = worktree("detached", None);
        let summary = WorktreeSummary {
            ahead: None,
            status_loaded: true,
            ..Default::default()
        };
        assert_eq!(
            is_safe_to_remove(&detached, &summary, &protection(&[]), false),
            SafetyResult::Safe
        );
    }

    fn orphan(name: &str, has_unpushed: bool, has_upstream: bool) -> OrphanBranch {
        OrphanBranch {
            name: name.to_string(),
            has_unpushed,
            has_upstream,
        }
    }

    #[test]
    fn test_prunable_branches_filters_protected_unpushed_and_no_upstream() {
        let orphans = vec![
            orphan("safe", false, true),         // deletable
            orphan("protected", false, true),    // skipped: protected
            orphan("ahead", true, true),         // skipped: unpushed
            orphan("no-upstream", false, false), // skipped: no upstream (unsafe)
        ];
        let deletable = prunable_branches(&orphans, &protection(&["protected"]));
        assert_eq!(deletable, vec!["safe".to_string()]);
    }

    #[test]
    fn test_prunable_branches_orphan_without_unpushed_is_deletable() {
        let orphans = vec![orphan("feature-merged", false, true)];
        let deletable = prunable_branches(&orphans, &protection(&[]));
        assert_eq!(deletable, vec!["feature-merged".to_string()]);
    }
}
