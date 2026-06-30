//! `gx workspace new` and the shared creation pipeline (also used by the PR
//! dashboard's open-in-workspace / troubleshoot via [`ensure_workspace_for_branch`]).

use super::{
    NewWorkspaceOptions, WorkspaceError, display_exit_status, fetch_origin, home_dir,
    load_worktrees, main_worktree_root, paths_equal, print_setup_report, validate_name,
    workspace_path,
};
use crate::git::{self, worktree::Worktree};
use crate::{config, output, repo_config, repo_setup};
use miette::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Create a new workspace (worktree) and print its path to stdout so the
/// shell wrapper from `gx setup` can cd into it.
pub fn run_new(name: String, mut opts: NewWorkspaceOptions) -> Result<()> {
    // A GitHub pull-request/branch URL (or '#123') resolves to a branch; the
    // workspace is named after that branch.
    let name = match git::github::parse_ref(&name) {
        Some(gh_ref) => {
            let resolved = git::github::resolve_branch(&gh_ref)?;
            if let Some(explicit) = &opts.branch
                && explicit != &resolved
            {
                eprintln!(
                    "warning: ignoring --branch '{}'; using '{}' from the GitHub reference",
                    explicit, resolved
                );
            }
            opts.branch = Some(resolved.clone());
            resolved
        }
        None => name,
    };

    let no_cd = opts.no_cd;
    let path = create_workspace(&name, &opts)?;
    if no_cd {
        // stdout stays empty for scripts/batch creation; the workspace path was
        // already reported on stderr by create_workspace.
        eprintln!("Staying in the current workspace (--no-cd)");
    } else {
        output::nav_path(&path);
    }
    Ok(())
}

/// Return the path of the workspace whose checked-out branch is `branch`,
/// creating one if none exists. Used by the PR dashboard's open-in-workspace and
/// troubleshoot actions. No stdout side effects, so the caller controls the cd.
pub fn ensure_workspace_for_branch(branch: &str) -> Result<PathBuf> {
    let worktrees = load_worktrees()?;
    if let Some(existing) = worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(branch))
    {
        return Ok(existing.path.clone());
    }
    create_workspace(branch, &NewWorkspaceOptions::default())
}

/// Create a workspace and return its canonical path, with no stdout side
/// effects (except the shell-navigation path printed by the existing-path
/// switch flow). Shared core of [`run_new`] and [`ensure_workspace_for_branch`].
fn create_workspace(name: &str, opts: &NewWorkspaceOptions) -> Result<PathBuf> {
    validate_name(name)?;

    // --detach replaces the new branch with a detached HEAD, so a branch name
    // makes no sense alongside it.
    if opts.detach && opts.branch.is_some() {
        return Err(WorkspaceError::ConflictingFlags(
            "--detach cannot be combined with --branch".to_string(),
        )
        .into());
    }
    if opts.detach && opts.track {
        return Err(WorkspaceError::ConflictingFlags(
            "--detach cannot be combined with --track".to_string(),
        )
        .into());
    }

    // Branch names may contain '/' (e.g. 'feat/expose-rationale'), but the
    // workspace directory must be a single path component.
    let dir_name = git::worktree::flatten_slashes(name);

    let worktrees = load_worktrees()?;

    let cfg = config::load()?;
    let main_root = main_worktree_root(&worktrees)?;

    // Resolve the full workspace policy: built-in defaults < global config <
    // personal profile < shared .gx/workspace.toml < local override. CLI flags
    // (e.g. a future `--no-hooks`) are applied on top by the caller / via the
    // `run_hooks` gate below.
    let personal = repo_setup::profile_for_repo(&main_root)?;
    let (shared, local) = repo_config::load_repo_layers(&main_root)?;
    let policy = repo_config::resolve(&cfg, &personal, shared.as_ref(), local.as_ref(), &main_root);

    let path = workspace_path(
        &main_root,
        home_dir().as_deref(),
        &cfg.workspace.root,
        &dir_name,
    );

    let branch_name = opts.branch.clone().unwrap_or_else(|| name.to_string());

    // The path may already host a worktree on another branch; offer a safe
    // switch before treating the existing path/name as a hard conflict.
    if let Some(handled) = try_switch_existing(&worktrees, &path, &branch_name, opts)? {
        return Ok(handled);
    }

    if let Some(existing) = worktrees.iter().find(|w| w.name == dir_name) {
        return Err(WorkspaceError::AlreadyExists(dir_name, existing.path.clone()).into());
    }
    if path.exists() {
        return Err(WorkspaceError::AlreadyExists(dir_name, path).into());
    }

    let branch_exists_locally =
        git::worktree::branch_exists(&branch_name).map_err(WorkspaceError::GitError)?;

    // When creating a new branch, a ref-namespace conflict (e.g. existing 'foo'
    // vs wanted 'foo/bar') would make `git worktree add` fail with an opaque
    // message; detect it first and explain the options.
    if !opts.detach
        && !branch_exists_locally
        && let Some(existing) =
            git::worktree::conflicting_branch(&branch_name).map_err(WorkspaceError::GitError)?
    {
        return Err(WorkspaceError::RefConflict {
            wanted: branch_name,
            existing,
        }
        .into());
    }

    // Resolve what the new branch should start from: an explicit base wins,
    // then a matching remote branch (like plain 'git checkout'), otherwise
    // the default branch of origin (e.g. 'origin/main') instead of HEAD.
    let mut tracking_remote: Option<String> = None;
    let mut default_base: Option<String> = None;
    let (create_branch, base) = if branch_exists_locally && !opts.detach {
        if let Some(base) = &opts.base {
            eprintln!(
                "warning: ignoring base '{}'; branch '{}' already exists",
                base, branch_name
            );
        }
        (false, None)
    } else {
        // Refresh origin's remote-tracking refs first so the new branch starts
        // from the actual state of origin (e.g. origin/main), not a stale
        // local snapshot from the last fetch. --no-fetch skips this for offline
        // use.
        if !opts.no_fetch {
            fetch_origin();
        }

        let base = match &opts.base {
            Some(base) => Some(base.clone()),
            None => {
                tracking_remote = git::branch::find_remote_branch(&branch_name)
                    .map_err(WorkspaceError::GitError)?;
                match tracking_remote.clone() {
                    Some(remote) => Some(remote),
                    None => {
                        default_base = git::branch::default_remote_branch()
                            .map_err(WorkspaceError::GitError)?;
                        default_base.clone()
                    }
                }
            }
        };
        (!opts.detach, base)
    };

    // When offline, a remote base may not exist locally; give a targeted hint
    // instead of letting `git worktree add` fail with a raw "invalid reference".
    if opts.no_fetch
        && let Some(base) = &base
        && !git::worktree::ref_resolvable(base).map_err(WorkspaceError::GitError)?
    {
        return Err(WorkspaceError::BaseUnresolvedOffline { base: base.clone() }.into());
    }

    // Pre-create hooks run before the worktree is added so a failed check
    // (e.g. `test -f package.json`) aborts creation and leaves nothing behind.
    // The workspace does not exist yet, so they run from the main worktree.
    // `--no-hooks` skips them.
    if !opts.no_hooks && !policy.pre_create_hooks.is_empty() {
        let vars = repo_config::HookVars {
            workspace: dir_name.clone(),
            workspace_path: path.clone(),
            main_root: main_root.clone(),
            branch: branch_name.clone(),
        };
        repo_config::run_hooks(&policy.pre_create_hooks, &vars, &main_root, true)?;
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(WorkspaceError::CreateDirFailed)?;
    }

    // When branching off the default remote branch, don't set it as upstream:
    // the new branch should push to its own name, not e.g. origin/main.
    // --track overrides this so the remote base becomes the upstream.
    let no_track = default_base.is_some() && !opts.track;
    git::worktree::add(
        &path,
        &branch_name,
        create_branch,
        base.as_deref(),
        no_track,
        opts.detach,
    )
    .map_err(WorkspaceError::GitError)?;

    let path = path.canonicalize().unwrap_or(path);

    if opts.detach {
        eprintln!(
            "Created workspace '{}' with detached HEAD{}",
            dir_name,
            base.as_deref()
                .map(|b| format!(" at '{}'", b))
                .unwrap_or_default()
        );
    } else if let Some(remote) = &tracking_remote {
        eprintln!(
            "Created workspace '{}' on new branch '{}' (tracking '{}')",
            dir_name, branch_name, remote
        );
    } else if let Some(default_base) = &default_base {
        eprintln!(
            "Created workspace '{}' on new branch '{}' (from '{}'{})",
            dir_name,
            branch_name,
            default_base,
            if opts.track { ", tracking" } else { "" }
        );
    } else if create_branch {
        eprintln!(
            "Created workspace '{}' on new branch '{}'",
            dir_name, branch_name
        );
    } else {
        eprintln!(
            "Created workspace '{}' on existing branch '{}'",
            dir_name, branch_name
        );
    }
    eprintln!("  {}", path.display());

    // Copy staged work from the source worktree into the new one. Runs
    // independently of --no-setup: setup copies repo policy files, --from-staged
    // copies the user's own staged changes.
    if let Some(filter) = &opts.from_staged
        && let Err(e) = copy_staged_into(filter, &path)
    {
        // No --keep-on-error flag yet: roll back the freshly created workspace
        // so a failed extraction doesn't leave a half-built one.
        let _ = git::worktree::remove(&main_root, &path, true);
        return Err(e.into());
    }

    if !opts.no_setup {
        // Use the resolved policy's copy_files so repo-shared/local copy files
        // take effect alongside the global and personal sets. The pipeline also
        // runs the personal profile's setup_script (unchanged).
        let report = repo_setup::run_setup_pipeline(&main_root, &path, &policy.copy_files, true)?;
        print_setup_report(&report, "  ", &policy.copy_files);

        // The repo-config setup_script (from .gx) is resolved against main_root
        // and runs with post-create semantics: a failure warns but keeps the
        // workspace. The personal profile's script (run above) is left
        // untouched. Avoid running the same script twice when both sources
        // happen to point at the same file.
        if let Some(repo_script) = repo_config_setup_script(&policy, &personal) {
            run_repo_config_setup_script(&repo_script, &path, &main_root);
        }
    }

    // Post-create hooks run after creation and setup, from inside the new
    // workspace. A failure only warns and keeps the workspace. `--no-hooks`
    // skips them.
    if !opts.no_hooks && !policy.post_create_hooks.is_empty() {
        let vars = repo_config::HookVars {
            workspace: dir_name.clone(),
            workspace_path: path.clone(),
            main_root: main_root.clone(),
            branch: branch_name.clone(),
        };
        repo_config::run_hooks(&policy.post_create_hooks, &vars, &path, false)?;
    }

    Ok(path)
}

/// The repo-config (`.gx`) setup script to run, if any, distinct from the
/// personal profile's script (which `run_setup_pipeline` already runs). Returns
/// `None` when the policy's script came from the personal profile or when the
/// resolved path does not exist.
fn repo_config_setup_script(
    policy: &repo_config::WorkspacePolicy,
    personal: &repo_setup::RepoSetupProfile,
) -> Option<PathBuf> {
    let resolved = policy.resolved_setup_script()?;

    // If the resolved script lives under the personal profile dir, it was
    // already run by run_setup_pipeline; don't run it again.
    if resolved.starts_with(&personal.dir) {
        return None;
    }

    if resolved.exists() {
        Some(resolved)
    } else {
        None
    }
}

/// Run a repo-config setup script with post-create semantics (warn on failure,
/// keep workspace). Mirrors the env/stdio convention of the personal profile's
/// script runner so stdout stays clean for the cd target.
fn run_repo_config_setup_script(script: &Path, workspace_root: &Path, main_root: &Path) {
    let stdout = match repo_setup::stderr_stdio() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "  warning: could not run setup script {}: {}",
                script.display(),
                e
            );
            return;
        }
    };

    eprintln!("  running setup script {}", script.display());
    let status = std::process::Command::new("sh")
        .arg(script)
        .current_dir(workspace_root)
        .env("GX_WORKSPACE_ROOT", workspace_root)
        .env("GX_MAIN_ROOT", main_root)
        .stdin(std::process::Stdio::inherit())
        .stdout(stdout)
        .stderr(std::process::Stdio::inherit())
        .status();

    match status {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "  warning: setup script {} failed with status {}; continuing",
            script.display(),
            display_exit_status(&status)
        ),
        Err(e) => eprintln!(
            "  warning: could not run setup script {}: {}",
            script.display(),
            e
        ),
    }
}

/// Copy staged file contents from the current (source) worktree's index into
/// the new workspace at `dest_root`. When `filter` is non-empty, only the
/// listed paths are copied (and any requested path not staged is warned about).
/// Deleted entries are skipped with a warning since there is no content to copy.
fn copy_staged_into(filter: &[String], dest_root: &Path) -> Result<(), WorkspaceError> {
    use crate::git::worktree::StagedStatus;

    let source_root = git::worktree::current_worktree_root().map_err(WorkspaceError::GitError)?;
    let entries = git::worktree::staged_entries(&source_root).map_err(WorkspaceError::GitError)?;

    let filtering = !filter.is_empty();
    let mut requested: HashSet<&str> = filter.iter().map(|s| s.as_str()).collect();

    let selected: Vec<_> = entries
        .iter()
        .filter(|entry| {
            if !filtering {
                return true;
            }
            // A filtered path matches either the current path or, for renames,
            // the old path the user may still be thinking in terms of.
            requested.remove(entry.path.as_str())
                || match &entry.status {
                    StagedStatus::Renamed { from } | StagedStatus::Copied { from } => {
                        requested.remove(from.as_str())
                    }
                    _ => false,
                }
        })
        .collect();

    if filtering {
        for missing in &requested {
            output::warn(format!("'{}' is not staged; skipping", missing));
        }
    }

    let mut copied = 0usize;
    for entry in selected {
        if matches!(entry.status, StagedStatus::Deleted) {
            eprintln!(
                "warning: skipping deleted file '{}' (no content to copy)",
                entry.path
            );
            continue;
        }

        let contents = git::worktree::show_staged(&source_root, &entry.path)
            .map_err(WorkspaceError::GitError)?;
        let dest = dest_root.join(&entry.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(WorkspaceError::CopyFailed)?;
        }
        std::fs::write(&dest, &contents).map_err(WorkspaceError::CopyFailed)?;
        eprintln!("  staged {}", entry.path);
        copied += 1;
    }

    if copied == 0 {
        eprintln!("  no staged file contents to copy");
    }

    Ok(())
}

/// When the target `path` already hosts a registered worktree on a different
/// branch, offer a safe branch switch instead of erroring on the existing path:
/// - target branch checked out in another worktree -> navigate there instead
///   (doesn't touch the existing worktree, so its dirty state is irrelevant);
/// - dirty worktree -> refuse the switch;
/// - target branch exists locally and is free -> switch this worktree to it.
///
/// Returns `Some(path)` when the situation was handled (and shell navigation,
/// if any, was already emitted), or `None` to fall through to normal creation.
fn try_switch_existing(
    worktrees: &[Worktree],
    path: &Path,
    branch_name: &str,
    opts: &NewWorkspaceOptions,
) -> Result<Option<PathBuf>> {
    // Only relevant when the exact target path is already a registered worktree.
    let Some(existing) = worktrees.iter().find(|w| paths_equal(&w.path, path)) else {
        return Ok(None);
    };

    // Detached HEAD, or already on the requested branch: not a switch case.
    if existing.branch.as_deref() == Some(branch_name) {
        return Ok(None);
    }
    // --detach has no target branch to switch to; let normal handling run.
    if opts.detach {
        return Ok(None);
    }

    // The target branch is checked out elsewhere: git won't allow a second
    // checkout of it, so send the user to that workspace instead. This is
    // checked first because navigation leaves the existing worktree untouched,
    // so its dirty state doesn't matter here. Navigation is emitted by the
    // single caller (`run_new`) using the returned path; this helper only
    // performs the git-side switch and reports it on stderr.
    if let Some(other) = worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(branch_name) && !paths_equal(&w.path, path))
    {
        eprintln!(
            "Branch '{}' is already checked out in workspace '{}'",
            branch_name, other.name
        );
        let target = other.path.clone();
        return Ok(Some(target.canonicalize().unwrap_or(target)));
    }

    // The target branch must already exist locally to switch to it; otherwise
    // fall through so creation can make it.
    if !git::worktree::branch_exists(branch_name).map_err(WorkspaceError::GitError)? {
        return Ok(None);
    }

    // Switching would carry over the working tree, so refuse on uncommitted
    // changes to avoid surprising the user.
    if git::worktree::has_tracked_changes(&existing.path).map_err(WorkspaceError::GitError)? {
        return Err(WorkspaceError::Dirty(existing.name.clone()).into());
    }

    git::worktree::switch_branch(&existing.path, branch_name).map_err(WorkspaceError::GitError)?;
    eprintln!(
        "Switched workspace '{}' to branch '{}'",
        existing.name, branch_name
    );
    let target = existing.path.clone();
    Ok(Some(target.canonicalize().unwrap_or(target)))
}
