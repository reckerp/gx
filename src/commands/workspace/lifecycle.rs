//! Workspace lifecycle commands beyond creation: remove, update (fetch+rebase),
//! setup, sync, move, lock/unlock, repair — plus the multi-worktree engines and
//! the shared removal kernel they (and `gx workspace clean`) build on.

use super::{
    WorkspaceError, create_from_picker, home_dir, load_worktrees, main_worktree_root,
    open_in_editor, paths_equal, pick_workspace, resolve_dest_path, resolve_target,
    resolve_worktree_root,
};
use crate::git::{self, GitError, worktree::Worktree};
use crate::output;
use crate::repo_setup::ScriptRun;
use crate::ui;
use crate::ui::workspace_picker::WorkspaceAction;
use crate::{config, repo_config, repo_setup};
use miette::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

pub fn run_remove(query: Option<String>, force: bool, delete_branch: bool) -> Result<()> {
    let worktrees = load_worktrees()?;

    let targets = match query {
        Some(q) => vec![resolve_target(&q, &worktrees)?],
        None => {
            let Some(action) = pick_workspace(&worktrees)? else {
                output::cancelled();
                return Ok(());
            };
            match action {
                // in remove context both Go and Remove target the selection
                WorkspaceAction::Go(w) => vec![w],
                WorkspaceAction::Remove {
                    worktrees: worktrees_to_remove,
                    delete_branches,
                    confirmed,
                    dirty_paths,
                } => {
                    return remove_worktrees(
                        &worktrees_to_remove,
                        &worktrees,
                        force,
                        delete_branches || delete_branch,
                        confirmed,
                        &dirty_paths,
                    );
                }
                WorkspaceAction::Update(worktrees_to_update) => {
                    return update_worktrees(&worktrees_to_update, None);
                }
                WorkspaceAction::Setup(worktrees_to_setup) => {
                    return setup_worktrees(&worktrees_to_setup, &worktrees);
                }
                WorkspaceAction::OpenEditor(worktree) => return open_in_editor(&worktree),
                WorkspaceAction::Create { name } => return create_from_picker(name),
            }
        }
    };

    remove_worktrees(
        &targets,
        &worktrees,
        force,
        delete_branch,
        false,
        &HashSet::new(),
    )
}

/// Bring a workspace up to date by fetching origin and rebasing its branch
/// onto a base (origin's default branch, e.g. 'origin/main', by default).
/// Updates the current workspace unless a query selects another one.
pub fn run_update(query: Option<String>, base: Option<String>) -> Result<()> {
    let worktrees = load_worktrees()?;

    let target = match query {
        Some(q) => resolve_target(&q, &worktrees)?,
        None => worktrees
            .iter()
            .find(|w| w.is_current)
            .cloned()
            .ok_or(WorkspaceError::GitError(GitError::NotInRepo))?,
    };

    update_worktrees(&[target], base)
}

/// Fetch origin (when it exists) so remote-tracking refs are current.
/// Failures (e.g. offline) are reported as a warning instead of aborting,
/// since the locally known refs may still be good enough.
pub(crate) fn fetch_origin() {
    match git::fetch::has_remote("origin") {
        Ok(true) => {
            eprintln!("Fetching origin...");
            if let Err(e) = git::fetch::fetch_remote("origin") {
                output::warn(format!("fetch failed ({}); using local refs", e));
            }
        }
        Ok(false) => {}
        Err(e) => output::warn(format!("could not check remotes ({})", e)),
    }
}

/// Re-run setup for the current workspace: copy setup files first, then run
/// the repo-specific setup script when one is configured.
pub fn run_setup() -> Result<()> {
    let worktrees = load_worktrees()?;
    let current = git::worktree::current_worktree_root().map_err(WorkspaceError::GitError)?;

    let current = worktrees
        .iter()
        .find(|w| paths_equal(&w.path, &current))
        .cloned()
        .ok_or(WorkspaceError::GitError(GitError::NotInRepo))?;

    setup_worktrees(&[current], &worktrees)
}

/// Manually copy paths from one workspace to another. Defaults: target =
/// current workspace, source = main worktree, paths = configured copy files.
/// All output goes to stderr; nothing is printed to stdout (this command does
/// not participate in shell navigation).
pub fn run_sync(
    target: Option<String>,
    from: Option<String>,
    paths: Vec<String>,
    dry_run: bool,
) -> Result<()> {
    let worktrees = load_worktrees()?;
    let main_root = main_worktree_root(&worktrees)?;

    // Resolve target: explicit query, else the current workspace.
    let target_root = match &target {
        Some(query) => resolve_worktree_root(query, &worktrees)?,
        None => git::worktree::current_worktree_root().map_err(WorkspaceError::GitError)?,
    };

    // Resolve source: explicit query, else the main worktree.
    let source_root = match &from {
        Some(query) => resolve_worktree_root(query, &worktrees)?,
        None => main_root.clone(),
    };

    if paths_equal(&source_root, &target_root) {
        return Err(WorkspaceError::SameSourceAndTarget.into());
    }

    // Determine paths: explicit list, else the configured default copy files
    // (global config + the repo profile's copy files, deduped) — the same set
    // workspace creation/setup uses.
    let patterns = if !paths.is_empty() {
        paths
    } else {
        let cfg = config::load()?;
        let mut patterns = cfg.workspace.copy_files.clone();
        patterns.extend(
            repo_setup::profile_for_repo(&main_root)?
                .config
                .copy_files
                .iter()
                .cloned(),
        );
        let mut seen = HashSet::new();
        patterns.retain(|p| seen.insert(p.clone()));
        patterns
    };

    let outcome = repo_setup::sync_paths(&source_root, &target_root, &patterns, dry_run)?;

    let src_name = worktree_display_name(&source_root, &worktrees);
    let dst_name = worktree_display_name(&target_root, &worktrees);
    let suffix = if dry_run { " (dry run)" } else { "" };
    eprintln!(
        "Syncing {} path(s) from '{}' to '{}'{}",
        patterns.len(),
        src_name,
        dst_name,
        suffix
    );

    for path in &outcome.copied {
        if dry_run {
            eprintln!("  would copy {}", path);
        } else {
            eprintln!("  copied {}", path);
        }
    }

    if outcome.copied.is_empty() {
        eprintln!("  nothing to copy");
    }

    // Missing sources are reported but never abort the sync.
    if !outcome.missing.is_empty() {
        eprintln!(
            "warning: {} path(s) not found in source: {}",
            outcome.missing.len(),
            outcome.missing.join(", ")
        );
    }

    Ok(())
}

/// Print the main worktree root to STDOUT, for use in scripts and aliases:
/// `cd "$(gx workspace root)"`. Unlike the navigation commands this is a plain
/// value command, so it emits the canonical path on stdout with no stderr hint.
pub fn run_root() -> Result<()> {
    let worktrees = load_worktrees()?;
    let main_root = main_worktree_root(&worktrees)?;
    let canonical = main_root.canonicalize().unwrap_or(main_root);
    println!("{}", canonical.display());
    Ok(())
}

/// Move a workspace to a new path via `git worktree move`. Refuses the main
/// worktree and refuses an existing destination. Human-readable output goes to
/// stderr; nothing is written to stdout unless the CURRENT workspace is moved,
/// in which case the new path is emitted via [`crate::output::nav_path`] so the
/// shell wrapper follows the user into it (mirroring the current-workspace
/// handling in [`remove_worktrees`]).
pub fn run_move(workspace: String, new_path: String) -> Result<()> {
    let worktrees = load_worktrees()?;
    let target = resolve_target(&workspace, &worktrees)?;

    if target.is_main {
        return Err(WorkspaceError::MoveMain.into());
    }

    let cwd = std::env::current_dir().map_err(WorkspaceError::CopyFailed)?;
    let dest = resolve_dest_path(&new_path, home_dir().as_deref(), &cwd);

    if dest.exists() {
        return Err(WorkspaceError::DestinationExists(dest).into());
    }

    let main_root = main_worktree_root(&worktrees)?;
    git::worktree::move_worktree(&main_root, &target.path, &dest)
        .map_err(WorkspaceError::GitError)?;

    eprintln!("Moved workspace '{}' to {}", target.name, dest.display());

    if target.is_current {
        eprintln!("Switching to moved workspace");
        output::nav_path(&dest);
    }

    Ok(())
}

/// Best-effort human-readable name for a worktree root, used only in messages.
/// Falls back to the final path component when the root is not a registered
/// worktree (e.g. a bare absolute path).
fn worktree_display_name(root: &Path, worktrees: &[Worktree]) -> String {
    worktrees
        .iter()
        .find(|w| paths_equal(&w.path, root))
        .map(|w| w.name.clone())
        .unwrap_or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string())
        })
}

/// Lock a workspace so cleanup and `git worktree prune` skip it. Refuses the
/// main worktree (git itself errors on locking it). Output goes to stderr only.
pub fn run_lock(workspace: String, reason: Option<String>) -> Result<()> {
    let worktrees = load_worktrees()?;
    let target = resolve_target(&workspace, &worktrees)?;

    if target.is_main {
        return Err(WorkspaceError::LockMain.into());
    }

    let main_root = main_worktree_root(&worktrees)?;
    git::worktree::lock(&main_root, &target.path, reason.as_deref())
        .map_err(WorkspaceError::GitError)?;

    eprintln!(
        "Locked workspace '{}'{}",
        target.name,
        reason.map(|r| format!(" ({})", r)).unwrap_or_default()
    );
    Ok(())
}

/// Clear the lock on a previously locked workspace. Output goes to stderr only.
pub fn run_unlock(workspace: String) -> Result<()> {
    let worktrees = load_worktrees()?;
    let target = resolve_target(&workspace, &worktrees)?;

    let main_root = main_worktree_root(&worktrees)?;
    git::worktree::unlock(&main_root, &target.path).map_err(WorkspaceError::GitError)?;

    eprintln!("Unlocked workspace '{}'", target.name);
    Ok(())
}

/// Repair worktree administrative files. With a workspace query, repairs only
/// that worktree; with no query, repairs all of them. Recovery command, so
/// output goes to stderr only. Mostly useful after a worktree directory or the
/// main repository has been moved on disk.
pub fn run_repair(workspace: Option<String>) -> Result<()> {
    let worktrees = load_worktrees()?;
    let main_root = main_worktree_root(&worktrees)?;

    let paths: Vec<PathBuf> = match &workspace {
        Some(q) => {
            let target = resolve_target(q, &worktrees)?;
            vec![target.path]
        }
        None => Vec::new(),
    };

    git::worktree::repair(&main_root, &paths).map_err(WorkspaceError::GitError)?;

    match paths.as_slice() {
        [path] => eprintln!(
            "Repaired worktree administrative files for {}",
            path.display()
        ),
        _ => eprintln!("Repaired worktree administrative files"),
    }
    Ok(())
}

pub(crate) fn update_worktrees(
    worktrees_to_update: &[Worktree],
    base: Option<String>,
) -> Result<()> {
    let mut seen = HashSet::new();
    let targets: Vec<Worktree> = worktrees_to_update
        .iter()
        .filter(|w| seen.insert(w.path.clone()))
        .cloned()
        .collect();

    if targets.is_empty() {
        eprintln!("No workspaces selected");
        return Ok(());
    }

    for target in &targets {
        if target.branch.is_none() {
            return Err(WorkspaceError::DetachedHead(target.name.clone()).into());
        }

        if git::worktree::has_tracked_changes(&target.path).map_err(WorkspaceError::GitError)? {
            return Err(WorkspaceError::Dirty(target.name.clone()).into());
        }
    }

    fetch_origin();

    let base = match base {
        Some(base) => base,
        None => git::branch::default_remote_branch()
            .map_err(WorkspaceError::GitError)?
            .ok_or(WorkspaceError::NoBase)?,
    };

    for target in &targets {
        let Some(branch) = target.branch.as_deref() else {
            continue;
        };
        eprintln!("Rebasing '{}' onto '{}'...", branch, base);
        git::worktree::rebase_onto(&target.path, &base)
            .map_err(|e| WorkspaceError::RebaseFailed(e.to_string()))?;

        eprintln!(
            "Updated workspace '{}': '{}' is now up to date with '{}'",
            target.name, branch, base
        );
    }

    Ok(())
}

pub(crate) fn setup_worktrees(
    worktrees_to_setup: &[Worktree],
    all_worktrees: &[Worktree],
) -> Result<()> {
    let mut seen = HashSet::new();
    let targets: Vec<Worktree> = worktrees_to_setup
        .iter()
        .filter(|w| seen.insert(w.path.clone()))
        .cloned()
        .collect();

    if targets.is_empty() {
        eprintln!("No workspaces selected");
        return Ok(());
    }

    let main_root = main_worktree_root(all_worktrees)?;
    // `gx workspace setup` applies the configured policy, so it honors the
    // shared repo config too. Hooks are only run on create, not setup.
    let policy = repo_config::resolve_for_repo(&main_root)?;

    for target in &targets {
        if paths_equal(&main_root, &target.path) {
            eprintln!(
                "Skipping main worktree '{}'; nothing to set up",
                target.name
            );
            continue;
        }

        let report =
            repo_setup::run_setup_pipeline(&main_root, &target.path, &policy.copy_files, true)?;

        eprintln!("Setup for '{}':", target.name);
        print_setup_report(&report, "  ", &policy.copy_files);
    }

    Ok(())
}

pub(crate) fn print_setup_report(
    report: &repo_setup::SetupReport,
    prefix: &str,
    configured: &[String],
) {
    if report.copied.is_empty() {
        if matches!(report.script, ScriptRun::Skipped) {
            eprintln!(
                "{}No setup files to copy (configured: {:?})",
                prefix, configured
            );
        }
    } else {
        for file in &report.copied {
            eprintln!("{}copied {}", prefix, file);
        }
    }

    match &report.script {
        ScriptRun::Skipped => {}
        ScriptRun::Succeeded(path) => {
            eprintln!("{}ran setup script {}", prefix, path.display());
        }
        ScriptRun::Failed { path, status } => {
            eprintln!(
                "{}warning: setup script {} failed with status {}; continuing",
                prefix,
                path.display(),
                display_exit_status(status)
            );
        }
    }
}

pub(crate) fn display_exit_status(status: &ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".to_string())
}

pub(crate) fn remove_worktrees(
    worktrees_to_remove: &[Worktree],
    all_worktrees: &[Worktree],
    force: bool,
    delete_branches: bool,
    already_confirmed: bool,
    known_dirty_paths: &HashSet<PathBuf>,
) -> Result<()> {
    let mut seen = HashSet::new();
    let mut targets: Vec<Worktree> = worktrees_to_remove
        .iter()
        .filter(|w| seen.insert(w.path.clone()))
        .cloned()
        .collect();

    if targets.is_empty() {
        eprintln!("No workspaces selected");
        return Ok(());
    }

    if targets.iter().any(|w| w.is_main) {
        return Err(WorkspaceError::RemoveMain.into());
    }

    // If the current workspace is selected, remove it last so failures in
    // other workspaces do not leave the user's shell inside a deleted path.
    targets.sort_by_key(|w| w.is_current);

    let main_root = main_worktree_root(all_worktrees)?;

    if !already_confirmed {
        let prompt = remove_prompt(&targets, delete_branches);
        let confirmed = ui::confirm::run_on_stderr(&prompt)?;
        if !confirmed {
            output::cancelled();
            return Ok(());
        }
    }

    let removes_current = targets.iter().any(|w| w.is_current);

    for worktree in &targets {
        eprintln!("Removing workspace '{}'...", worktree.name);

        // Known-dirty (from the picker's summary) and unlocked: confirm the
        // force up front to skip a guaranteed-failing plain removal.
        let use_force = if force {
            true
        } else if known_dirty_paths.contains(&worktree.path) && !worktree.is_locked {
            if !confirm_force_remove(worktree)? {
                output::cancelled();
                return Ok(());
            }
            true
        } else {
            false
        };

        match remove_one_worktree(&main_root, worktree, use_force)? {
            RemoveOutcome::Removed => {}
            // `gx workspace remove` aborts the whole operation if the user
            // declines to force-remove a dirty workspace.
            RemoveOutcome::SkippedDirty => {
                output::cancelled();
                return Ok(());
            }
        }

        match &worktree.branch {
            Some(branch) if delete_branches => {
                delete_local_branch(&main_root, branch)?;
                eprintln!(
                    "Removed workspace '{}' and deleted branch '{}'",
                    worktree.name, branch
                );
            }
            Some(branch) => eprintln!(
                "Removed workspace '{}' (branch '{}' kept)",
                worktree.name, branch
            ),
            None => eprintln!("Removed workspace '{}'", worktree.name),
        }
    }

    // The user's shell is inside the removed directory; send them to the
    // main workspace via the shell wrapper.
    if removes_current {
        eprintln!("Switching to main workspace");
        output::nav_path(&main_root);
    }
    Ok(())
}

/// Outcome of [`remove_one_worktree`].
pub(crate) enum RemoveOutcome {
    Removed,
    /// The worktree had modified/untracked files and the user declined the
    /// force-removal prompt. The caller decides whether to abort or skip it.
    SkippedDirty,
}

/// Remove a single worktree, recovering from git's "contains modified or
/// untracked files" refusal by prompting for a force removal. `force` skips the
/// dirty check (the caller already decided to force). Other git errors
/// propagate. Shared by `gx workspace remove` and `gx workspace clean` so the
/// dirty-recovery path lives in exactly one place.
pub(crate) fn remove_one_worktree(
    main_root: &Path,
    worktree: &Worktree,
    force: bool,
) -> Result<RemoveOutcome> {
    match git::worktree::remove(main_root, &worktree.path, force) {
        Ok(()) => Ok(RemoveOutcome::Removed),
        // copied setup files (e.g. .env) are untracked, so offer a force
        // removal instead of failing outright.
        Err(GitError::CommandFailed { stderr: msg, .. })
            if !force && msg.contains("contains modified or untracked files") =>
        {
            if !confirm_force_remove(worktree)? {
                return Ok(RemoveOutcome::SkippedDirty);
            }
            git::worktree::remove(main_root, &worktree.path, true)
                .map_err(WorkspaceError::GitError)?;
            Ok(RemoveOutcome::Removed)
        }
        Err(e) => Err(WorkspaceError::GitError(e).into()),
    }
}

/// Prompt (on stderr) to force-remove a worktree with modified/untracked files.
/// The caller reports the decline (abort vs skip), so this only asks.
fn confirm_force_remove(worktree: &Worktree) -> Result<bool> {
    ui::confirm::run_on_stderr(&format!(
        "Workspace '{}' has modified or untracked files. Remove anyway?",
        worktree.name
    ))
}

pub(crate) fn delete_local_branch(main_root: &Path, branch: &str) -> Result<()> {
    match git::worktree::delete_branch(main_root, branch, false) {
        Ok(()) => Ok(()),
        // The branch is already gone (e.g. removed earlier in the same cleanup
        // run): treat that as success rather than aborting the whole operation.
        Err(GitError::CommandFailed { stderr: msg, .. }) if msg.contains("not found") => {
            eprintln!("Branch '{}' was already deleted", branch);
            Ok(())
        }
        Err(GitError::CommandFailed { stderr: msg, .. }) if msg.contains("not fully merged") => {
            let confirmed = ui::confirm::run_on_stderr(&format!(
                "Branch '{}' is not fully merged. Force delete it?",
                branch
            ))?;
            if confirmed {
                git::worktree::delete_branch(main_root, branch, true)
                    .map_err(WorkspaceError::GitError)?;
            } else {
                eprintln!("Kept branch '{}'", branch);
            }
            Ok(())
        }
        Err(e) => Err(WorkspaceError::GitError(e).into()),
    }
}

fn remove_prompt(worktrees_to_remove: &[Worktree], delete_branches: bool) -> String {
    if let [worktree] = worktrees_to_remove {
        return if worktree.is_current {
            if delete_branches {
                format!(
                    "Remove current workspace '{}' ({}) and delete its local branch? You will be moved to the main workspace.",
                    worktree.name,
                    worktree.path.display()
                )
            } else {
                format!(
                    "Remove current workspace '{}' ({})? You will be moved to the main workspace.",
                    worktree.name,
                    worktree.path.display()
                )
            }
        } else if delete_branches {
            format!(
                "Remove workspace '{}' ({}) and delete its local branch?",
                worktree.name,
                worktree.path.display()
            )
        } else {
            format!(
                "Remove workspace '{}' ({})?",
                worktree.name,
                worktree.path.display()
            )
        };
    }

    let mut header = if delete_branches {
        format!(
            "Remove {} workspaces and delete their local branches?",
            worktrees_to_remove.len()
        )
    } else {
        format!("Remove {} workspaces?", worktrees_to_remove.len())
    };
    if worktrees_to_remove.iter().any(|w| w.is_current) {
        header.push_str(
            "\nThe current workspace is selected; you will be moved to the main workspace.",
        );
    }
    output::bulleted_prompt(
        header,
        worktrees_to_remove
            .iter()
            .map(|w| format!("{} ({})", w.name, w.path.display())),
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_support::worktree;
    use super::*;

    #[test]
    fn test_remove_prompt_lists_multiple_workspaces() {
        let feature = worktree("feature", Some("feature"));
        let fix = worktree("fix", Some("fix"));

        let prompt = remove_prompt(&[feature, fix], false);

        assert!(prompt.contains("Remove 2 workspaces?"));
        assert!(prompt.contains("  - feature (/ws/feature)"));
        assert!(prompt.contains("  - fix (/ws/fix)"));
    }

    #[test]
    fn test_remove_prompt_notes_current_workspace() {
        let feature = worktree("feature", Some("feature"));
        let mut current = worktree("current", Some("current"));
        current.is_current = true;

        let prompt = remove_prompt(&[feature, current], false);

        assert!(prompt.contains(
            "The current workspace is selected; you will be moved to the main workspace."
        ));
    }

    #[test]
    fn test_remove_prompt_mentions_branch_delete() {
        let feature = worktree("feature", Some("feature"));
        let prompt = remove_prompt(&[feature], true);

        assert!(prompt.contains("delete its local branch"));
    }
}
