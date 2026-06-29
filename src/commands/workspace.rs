use crate::git::{self, GitError, worktree::Worktree};
use crate::repo_setup::ScriptRun;
use crate::ui;
use crate::ui::workspace_picker::WorkspaceAction;
use crate::{config, repo_setup};
use fuzzy_matcher::skim::SkimMatcherV2;
use miette::{Diagnostic, Result};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum WorkspaceError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::workspace::git_error))]
    GitError(#[from] GitError),

    #[error("TUI error: {0}")]
    #[diagnostic(code(gx::workspace::tui_error))]
    TuiError(String),

    #[error("No workspace matches query: {0}")]
    #[diagnostic(
        code(gx::workspace::no_match),
        help("Use 'gx workspace list' to see available workspaces")
    )]
    NoMatch(String),

    #[error("Workspace '{0}' already exists at {1}")]
    #[diagnostic(
        code(gx::workspace::already_exists),
        help("Use 'gx workspace go {0}' to switch to it")
    )]
    AlreadyExists(String, PathBuf),

    #[error("Cannot remove the main worktree")]
    #[diagnostic(
        code(gx::workspace::remove_main),
        help("The main worktree is the original repository checkout and cannot be removed")
    )]
    RemoveMain,

    #[error("Invalid workspace name: {0}")]
    #[diagnostic(code(gx::workspace::invalid_name))]
    InvalidName(String),

    #[error("Workspace '{0}' has uncommitted changes")]
    #[diagnostic(
        code(gx::workspace::dirty),
        help("Commit or stash your changes before updating")
    )]
    Dirty(String),

    #[error("Workspace '{0}' is not on a branch (detached HEAD)")]
    #[diagnostic(code(gx::workspace::detached_head))]
    DetachedHead(String),

    #[error("Could not determine a base branch to update against")]
    #[diagnostic(
        code(gx::workspace::no_base),
        help("Pass a base explicitly, e.g. 'gx workspace update <workspace> origin/main'")
    )]
    NoBase,

    #[error("Rebase failed: {0}")]
    #[diagnostic(
        code(gx::workspace::rebase_failed),
        help(
            "Resolve the conflicts and run 'git rebase --continue', or 'git rebase --abort' to undo"
        )
    )]
    RebaseFailed(String),

    #[error("Failed to copy setup file: {0}")]
    #[diagnostic(code(gx::workspace::copy_failed))]
    CopyFailed(#[from] io::Error),

    #[error("Failed to create workspace directory: {0}")]
    #[diagnostic(code(gx::workspace::create_dir_failed))]
    CreateDirFailed(io::Error),

    #[error("Could not open editor: {0}")]
    #[diagnostic(
        code(gx::workspace::editor_failed),
        help("Set $EDITOR (or $VISUAL) to your editor, e.g. 'export EDITOR=nvim'")
    )]
    EditorFailed(String),

    #[error(
        "Cannot create branch '{wanted}' because it conflicts with existing branch '{existing}'."
    )]
    #[diagnostic(
        code(gx::workspace::ref_conflict),
        help(
            "Options:\n  1. Use a different branch name\n  2. Delete or rename the conflicting branch\n  3. Check out the existing branch instead"
        )
    )]
    RefConflict { wanted: String, existing: String },

    #[error("Could not resolve base '{base}' from local refs")]
    #[diagnostic(
        code(gx::workspace::base_unresolved_offline),
        help("--no-fetch was used; retry without it to refresh remote refs")
    )]
    BaseUnresolvedOffline { base: String },

    #[error("Conflicting flags: {0}")]
    #[diagnostic(code(gx::workspace::conflicting_flags))]
    ConflictingFlags(String),
}

/// Options for [`run_new`] / [`create_workspace`], gathered into one struct so
/// the function signatures stay readable as creation flags accumulate.
#[derive(Debug, Default, Clone)]
pub struct NewWorkspaceOptions {
    /// Explicit base branch/commit/tag for the new branch.
    pub base: Option<String>,
    /// Branch to check out (created if it doesn't exist).
    pub branch: Option<String>,
    /// Skip copying setup files / running the setup script.
    pub no_setup: bool,
    /// Create the workspace but do not request shell navigation.
    pub no_cd: bool,
    /// Skip fetching origin; resolve the base from local refs only.
    pub no_fetch: bool,
    /// Copy staged file contents from the current workspace. `None` means the
    /// flag was absent; `Some(vec)` means copy all staged files (empty vec) or
    /// only the listed paths (non-empty vec).
    pub from_staged: Option<Vec<String>>,
    /// Skip workspace creation hooks. Threaded through for forward
    /// compatibility; Section 3's hook runner consumes it once merged, so it
    /// currently controls nothing (no hook engine exists yet).
    #[allow(dead_code)]
    pub no_hooks: bool,
    /// Create the workspace with a detached HEAD instead of a new branch.
    pub detach: bool,
    /// Set the base's remote branch as the new branch's upstream.
    pub track: bool,
}

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
        print_go_path(&path);
    }
    Ok(())
}

/// Return the path of the workspace whose checked-out branch is `branch`,
/// creating one if none exists. Used by the PR dashboard's open-in-workspace and
/// troubleshoot actions. No stdout side effects, so the caller controls the cd.
pub fn ensure_workspace_for_branch(branch: &str) -> Result<PathBuf> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;
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

    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    let cfg = config::load()?;
    let main_root = main_worktree_root(&worktrees)?;
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
                tracking_remote = git::worktree::find_remote_branch(&branch_name)
                    .map_err(WorkspaceError::GitError)?;
                match tracking_remote.clone() {
                    Some(remote) => Some(remote),
                    None => {
                        default_base = git::worktree::default_remote_branch()
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

    // Note: hook gating via opts.no_hooks is consumed by Section 3's hook
    // runner once merged; for now the flag is threaded through but controls
    // nothing because no hook engine exists yet.
    if !opts.no_setup {
        let report =
            repo_setup::run_setup_pipeline(&main_root, &path, &cfg.workspace.copy_files, true)?;
        print_setup_report(&report, "  ", &cfg.workspace.copy_files);
    }

    Ok(path)
}

/// Copy staged file contents from the current (source) worktree's index into
/// the new workspace at `dest_root`. When `filter` is non-empty, only the
/// listed paths are copied (and any requested path not staged is warned about).
/// Deleted entries are skipped with a warning since there is no content to copy.
fn copy_staged_into(filter: &[String], dest_root: &Path) -> Result<(), WorkspaceError> {
    use crate::git::worktree::StagedStatus;

    let source_root =
        git::worktree::current_worktree_root().map_err(WorkspaceError::GitError)?;
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
            eprintln!("warning: '{}' is not staged; skipping", missing);
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

/// Resolve a workspace by query (or interactively) and print its path to
/// stdout for the shell wrapper to cd into.
pub fn run_go(query: Option<String>) -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    let target = match query {
        Some(q) => {
            // A GitHub pull-request/branch URL (or '#123') resolves to a branch;
            // switch to the workspace checked out on it.
            let q = match git::github::parse_ref(&q) {
                Some(gh_ref) => git::github::resolve_branch(&gh_ref)?,
                None => q,
            };
            fuzzy_match_worktree(&q, &worktrees)
                .ok_or_else(|| WorkspaceError::NoMatch(q.clone()))?
        }
        None => {
            let Some(action) = pick_workspace(&worktrees)? else {
                eprintln!("Cancelled");
                return Ok(());
            };
            match action {
                WorkspaceAction::Go(w) => w,
                WorkspaceAction::Remove {
                    worktrees: worktrees_to_remove,
                    delete_branches,
                    confirmed,
                    dirty_paths,
                } => {
                    return remove_worktrees(
                        &worktrees_to_remove,
                        &worktrees,
                        false,
                        delete_branches,
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

    eprintln!("Switching to workspace '{}'", target.name);
    print_go_path(&target.path);
    Ok(())
}

pub fn run_list() -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    if worktrees.is_empty() {
        eprintln!("No workspaces found");
        return Ok(());
    }

    let name_width = worktrees.iter().map(|w| w.name.len()).max().unwrap_or(0);
    let branch_width = worktrees
        .iter()
        .map(|w| w.branch.as_deref().unwrap_or("(detached)").len())
        .max()
        .unwrap_or(0);

    for w in &worktrees {
        let branch = w.branch.as_deref().unwrap_or("(detached)");
        let mut markers = Vec::new();
        if w.is_main {
            markers.push("main");
        }
        if w.is_current {
            markers.push("current");
        }
        if w.is_locked {
            markers.push("locked");
        }
        let markers = if markers.is_empty() {
            String::new()
        } else {
            format!(" ({})", markers.join(", "))
        };

        println!(
            "{:<name_width$}  [{:<branch_width$}]  {}{}",
            w.name,
            branch,
            w.path.display(),
            markers,
        );
    }

    Ok(())
}

pub fn run_remove(query: Option<String>, force: bool, delete_branch: bool) -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    let targets = match query {
        Some(q) => vec![
            fuzzy_match_worktree(&q, &worktrees)
                .ok_or_else(|| WorkspaceError::NoMatch(q.clone()))?,
        ],
        None => {
            let Some(action) = pick_workspace(&worktrees)? else {
                eprintln!("Cancelled");
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
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    let target = match query {
        Some(q) => fuzzy_match_worktree(&q, &worktrees)
            .ok_or_else(|| WorkspaceError::NoMatch(q.clone()))?,
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
fn fetch_origin() {
    match git::fetch::has_remote("origin") {
        Ok(true) => {
            eprintln!("Fetching origin...");
            if let Err(e) = git::fetch::fetch_remote("origin") {
                eprintln!("warning: fetch failed ({}); using local refs", e);
            }
        }
        Ok(false) => {}
        Err(e) => eprintln!("warning: could not check remotes ({})", e),
    }
}

/// Re-run setup for the current workspace: copy setup files first, then run
/// the repo-specific setup script when one is configured.
pub fn run_setup() -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;
    let current = git::worktree::current_worktree_root().map_err(WorkspaceError::GitError)?;

    let current = worktrees
        .iter()
        .find(|w| paths_equal(&w.path, &current))
        .cloned()
        .ok_or(WorkspaceError::GitError(GitError::NotInRepo))?;

    setup_worktrees(&[current], &worktrees)
}

/// Interactive workspace manager (default when no subcommand is given).
pub fn run_interactive() -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    let Some(action) = pick_workspace(&worktrees)? else {
        eprintln!("Cancelled");
        return Ok(());
    };

    match action {
        WorkspaceAction::Go(w) => {
            eprintln!("Switching to workspace '{}'", w.name);
            print_go_path(&w.path);
            Ok(())
        }
        WorkspaceAction::Remove {
            worktrees: worktrees_to_remove,
            delete_branches,
            confirmed,
            dirty_paths,
        } => remove_worktrees(
            &worktrees_to_remove,
            &worktrees,
            false,
            delete_branches,
            confirmed,
            &dirty_paths,
        ),
        WorkspaceAction::Update(worktrees_to_update) => {
            update_worktrees(&worktrees_to_update, None)
        }
        WorkspaceAction::Setup(worktrees_to_setup) => {
            setup_worktrees(&worktrees_to_setup, &worktrees)
        }
        WorkspaceAction::OpenEditor(worktree) => open_in_editor(&worktree),
        WorkspaceAction::Create { name } => create_from_picker(name),
    }
}

fn pick_workspace(worktrees: &[Worktree]) -> Result<Option<WorkspaceAction>> {
    let mut terminal = ui::terminal::setup_terminal_stderr()
        .map_err(|e| WorkspaceError::TuiError(e.to_string()))?;
    let summary_lookup = git::worktree::spawn_summary_lookup(worktrees);
    let pull_requests = git::pull_request::spawn_lookup(worktrees);
    let result = ui::workspace_picker::run(&mut terminal, worktrees, summary_lookup, pull_requests);
    ui::terminal::restore_terminal_stderr(terminal)
        .map_err(|e| WorkspaceError::TuiError(e.to_string()))?;
    result
}

fn create_from_picker(name: String) -> Result<()> {
    let name = if name.is_empty() {
        eprint!("Workspace name: ");
        io::stderr().flush().ok();
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| WorkspaceError::TuiError(e.to_string()))?;
        input.trim().to_string()
    } else {
        name
    };

    if name.is_empty() {
        eprintln!("Cancelled");
        return Ok(());
    }

    run_new(name, NewWorkspaceOptions::default())
}

/// Open a workspace in the user's editor, resolved from `$VISUAL`, then
/// `$EDITOR`, with a platform fallback. The editor runs attached to the
/// terminal and gx waits for it, which is correct for terminal editors
/// (vim, nano, …) and for GUI editors invoked with a blocking flag
/// (e.g. `code --wait`).
fn open_in_editor(worktree: &Worktree) -> Result<()> {
    let editor = editor_command();
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| WorkspaceError::EditorFailed("no editor configured".to_string()))?;
    let args: Vec<&str> = parts.collect();

    eprintln!("Opening '{}' in {}", worktree.name, program);

    let mut command = std::process::Command::new(program);
    command
        .args(&args)
        .arg(&worktree.path)
        .current_dir(&worktree.path);

    // The picker renders to stderr because gx's stdout may be captured by the
    // `gx setup` shell wrapper's `$(...)`. A terminal editor needs a real tty
    // on stdout, so point it at the controlling terminal when ours is not one;
    // stdin and stderr are already the terminal, so they stay inherited.
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        if !io::stdout().is_terminal()
            && let Ok(tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty")
        {
            command.stdout(tty);
        }
    }

    let status = command.status().map_err(|e| {
        WorkspaceError::EditorFailed(format!("could not launch '{}': {}", program, e))
    })?;

    if !status.success() {
        return Err(WorkspaceError::EditorFailed(format!(
            "{} exited with {}",
            program,
            display_exit_status(&status)
        ))
        .into());
    }

    Ok(())
}

/// The editor command to use, from `$VISUAL` then `$EDITOR`, falling back to a
/// platform default. May contain arguments (e.g. "code --wait").
fn editor_command() -> String {
    resolve_editor(std::env::var("VISUAL").ok(), std::env::var("EDITOR").ok())
}

fn resolve_editor(visual: Option<String>, editor: Option<String>) -> String {
    visual
        .filter(|s| !s.trim().is_empty())
        .or_else(|| editor.filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(default_editor)
}

fn default_editor() -> String {
    if cfg!(windows) {
        "notepad".to_string()
    } else {
        "vi".to_string()
    }
}

fn update_worktrees(worktrees_to_update: &[Worktree], base: Option<String>) -> Result<()> {
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
        None => git::worktree::default_remote_branch()
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

fn setup_worktrees(worktrees_to_setup: &[Worktree], all_worktrees: &[Worktree]) -> Result<()> {
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
    let cfg = config::load()?;

    for target in &targets {
        if paths_equal(&main_root, &target.path) {
            eprintln!(
                "Skipping main worktree '{}'; nothing to set up",
                target.name
            );
            continue;
        }

        let report = repo_setup::run_setup_pipeline(
            &main_root,
            &target.path,
            &cfg.workspace.copy_files,
            true,
        )?;

        eprintln!("Setup for '{}':", target.name);
        print_setup_report(&report, "  ", &cfg.workspace.copy_files);
    }

    Ok(())
}

fn print_setup_report(report: &repo_setup::SetupReport, prefix: &str, configured: &[String]) {
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

fn display_exit_status(status: &ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".to_string())
}

fn remove_worktrees(
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
            eprintln!("Cancelled");
            return Ok(());
        }
    }

    let removes_current = targets.iter().any(|w| w.is_current);

    for worktree in &targets {
        eprintln!("Removing workspace '{}'...", worktree.name);

        let use_force = if force {
            true
        } else if known_dirty_paths.contains(&worktree.path) && !worktree.is_locked {
            if !confirm_force_remove(worktree)? {
                return Ok(());
            }
            true
        } else {
            false
        };

        match git::worktree::remove(&main_root, &worktree.path, use_force) {
            Ok(()) => {}
            // copied setup files (e.g. .env) are untracked, so offer a force
            // removal instead of failing outright
            Err(GitError::CommandFailed(msg))
                if !use_force && msg.contains("contains modified or untracked files") =>
            {
                if !confirm_force_remove(worktree)? {
                    return Ok(());
                }
                git::worktree::remove(&main_root, &worktree.path, true)
                    .map_err(WorkspaceError::GitError)?;
            }
            Err(e) => return Err(WorkspaceError::GitError(e).into()),
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
        print_go_path(&main_root);
    }
    Ok(())
}

fn confirm_force_remove(worktree: &Worktree) -> Result<bool> {
    let confirmed = ui::confirm::run_on_stderr(&format!(
        "Workspace '{}' has modified or untracked files. Remove anyway?",
        worktree.name
    ))?;
    if !confirmed {
        eprintln!("Cancelled");
    }
    Ok(confirmed)
}

fn delete_local_branch(main_root: &Path, branch: &str) -> Result<()> {
    match git::worktree::delete_branch(main_root, branch, false) {
        Ok(()) => Ok(()),
        Err(GitError::CommandFailed(msg)) if msg.contains("not fully merged") => {
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

    let mut lines = vec![if delete_branches {
        format!(
            "Remove {} workspaces and delete their local branches?",
            worktrees_to_remove.len()
        )
    } else {
        format!("Remove {} workspaces?", worktrees_to_remove.len())
    }];
    if worktrees_to_remove.iter().any(|w| w.is_current) {
        lines.push(
            "The current workspace is selected; you will be moved to the main workspace."
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.extend(
        worktrees_to_remove
            .iter()
            .map(|w| format!("  - {} ({})", w.name, w.path.display())),
    );
    lines.join("\n")
}

/// Print the path the shell wrapper should cd into. This is the only thing
/// workspace commands (and the PR dashboard's workspace actions) write to stdout.
pub(crate) fn print_go_path(path: &Path) {
    use std::io::IsTerminal;

    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    println!("{}", path.display());

    // When stdout is a terminal, the output is not being captured by the
    // shell wrapper from 'gx setup', so gx cannot cd for the user.
    if io::stdout().is_terminal() {
        eprintln!();
        eprintln!("hint: gx printed the path but could not cd for you.");
        eprintln!(
            "      Add 'eval \"$(gx setup)\"' to your shell config (and use 'gx' from your PATH,"
        );
        eprintln!("      not './gx') to switch workspaces automatically.");
    }
}

fn main_worktree_root(worktrees: &[Worktree]) -> Result<PathBuf, WorkspaceError> {
    worktrees
        .iter()
        .find(|w| w.is_main)
        .map(|w| w.path.clone())
        .ok_or(WorkspaceError::GitError(GitError::NotInRepo))
}

fn workspace_path(
    main_root: &Path,
    home: Option<&Path>,
    root_template: &str,
    name: &str,
) -> PathBuf {
    let repo_name = main_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());

    let root = root_template.replace("{repo}", &repo_name);

    let root_path = if let Some(home) = home {
        if root == "~" {
            home.to_path_buf()
        } else if let Some(rest) = root.strip_prefix("~/") {
            home.join(rest)
        } else {
            PathBuf::from(&root)
        }
    } else {
        PathBuf::from(&root)
    };

    let base = if root_path.is_absolute() {
        root_path
    } else {
        main_root.join(root_path)
    };

    base.join(name)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Workspace names double as branch names, so '/' is allowed (e.g.
/// 'feat/expose-rationale'); the directory name flattens it via
/// [`git::worktree::flatten_slashes`]. Empty segments and '.'/'..'
/// segments are rejected.
fn validate_name(name: &str) -> Result<(), WorkspaceError> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains('\\')
        || name
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(WorkspaceError::InvalidName(name.to_string()));
    }
    Ok(())
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn fuzzy_match_worktree(query: &str, worktrees: &[Worktree]) -> Option<Worktree> {
    let matcher = SkimMatcherV2::default();

    worktrees
        .iter()
        .filter_map(|w| {
            if w.matches_exactly(query) {
                return Some((i64::MAX, w)); // prioritize exact name/branch matches
            }
            w.match_score(&matcher, query).map(|score| (score, w))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, w)| w.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_path_default_template() {
        let path = workspace_path(
            Path::new("/home/user/myrepo"),
            Some(Path::new("/home/user")),
            "~/gx/workspaces/{repo}",
            "feature-x",
        );
        assert_eq!(
            path,
            PathBuf::from("/home/user/gx/workspaces/myrepo/feature-x")
        );
    }

    #[test]
    fn test_workspace_path_relative_template() {
        let path = workspace_path(
            Path::new("/home/user/myrepo"),
            Some(Path::new("/home/user")),
            "../{repo}-workspaces",
            "feature-x",
        );
        assert_eq!(
            path,
            PathBuf::from("/home/user/myrepo/../myrepo-workspaces/feature-x")
        );
    }

    #[test]
    fn test_workspace_path_custom_template() {
        let path = workspace_path(Path::new("/repo"), None, ".worktrees", "fix");
        assert_eq!(path, PathBuf::from("/repo/.worktrees/fix"));
    }

    #[test]
    fn test_workspace_path_absolute_template() {
        let path = workspace_path(
            Path::new("/repo"),
            Some(Path::new("/home/user")),
            "/srv/workspaces/{repo}",
            "fix",
        );
        assert_eq!(path, PathBuf::from("/srv/workspaces/repo/fix"));
    }

    #[test]
    fn test_workspace_path_tilde_without_home() {
        // without a home dir, "~/..." is treated as a relative path
        let path = workspace_path(Path::new("/repo"), None, "~/ws", "fix");
        assert_eq!(path, PathBuf::from("/repo/~/ws/fix"));
    }

    #[test]
    fn test_validate_name() {
        assert!(validate_name("feature-x").is_ok());
        assert!(validate_name("feat/expose-rationale").is_ok());
        assert!(validate_name("a/b/c").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("--force").is_err());
        assert!(validate_name("/a").is_err());
        assert!(validate_name("a/").is_err());
        assert!(validate_name("a//b").is_err());
        assert!(validate_name("a/../b").is_err());
        assert!(validate_name("a/./b").is_err());
        assert!(validate_name("a\\b").is_err());
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

    #[test]
    fn test_fuzzy_match_worktree_slash_and_dash() {
        let worktrees = vec![
            worktree("repo", Some("main")),
            worktree("feat-expose-rationale", Some("feat/expose-rationale")),
            worktree("custom-dir", Some("fix/null-check")),
        ];

        // slash and dash forms both resolve a workspace exactly,
        // whether they match the directory name or the branch
        for (query, expected) in [
            ("feat/expose-rationale", "feat-expose-rationale"),
            ("feat-expose-rationale", "feat-expose-rationale"),
            ("fix/null-check", "custom-dir"),
            ("fix-null-check", "custom-dir"),
        ] {
            let m = fuzzy_match_worktree(query, &worktrees).unwrap();
            assert_eq!(m.name, expected, "query '{}'", query);
        }

        // partial queries fuzzy match across the '/'-'-' boundary
        let m = fuzzy_match_worktree("feat/expo", &worktrees).unwrap();
        assert_eq!(m.name, "feat-expose-rationale");
        let m = fuzzy_match_worktree("fix-null", &worktrees).unwrap();
        assert_eq!(m.name, "custom-dir");

        assert!(fuzzy_match_worktree("nonexistent-xyz", &worktrees).is_none());
    }

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

    #[test]
    fn test_resolve_editor_prefers_visual_then_editor() {
        assert_eq!(
            resolve_editor(Some("code --wait".to_string()), Some("vim".to_string())),
            "code --wait"
        );
        assert_eq!(resolve_editor(None, Some("vim".to_string())), "vim");
    }

    #[test]
    fn test_resolve_editor_ignores_blank_and_falls_back_to_default() {
        // A blank $VISUAL is skipped in favour of $EDITOR.
        assert_eq!(
            resolve_editor(Some("   ".to_string()), Some("nano".to_string())),
            "nano"
        );
        // Nothing usable set -> platform default.
        assert_eq!(resolve_editor(None, None), default_editor());
        assert_eq!(resolve_editor(Some(String::new()), None), default_editor());
    }
}
