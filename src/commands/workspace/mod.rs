//! Workspace (git worktree) management, split by responsibility:
//! - [`create`] — `gx workspace new` and the shared creation pipeline
//! - [`navigate`] — go/list/interactive picker + editor launching
//! - [`lifecycle`] — remove/update/setup/sync/move/lock/repair + their engines
//!
//! This module root holds the error type, the creation options, and the small
//! cross-cutting resolution/path helpers the handlers share. The public API is
//! re-exported so callers (args.rs, sibling command modules) use
//! `commands::workspace::*` regardless of the internal split.

// The thiserror/miette-derived impls bind WorkspaceError's struct-variant
// fields; a recent rustc's `unused_assignments` lint misattributes a
// generated-code assignment to the field spans and reports them as "never
// read", even though they are used in the `#[error("…")]` messages. Silence the
// false positive for this module.
#![allow(unused_assignments)]

mod create;
mod lifecycle;
mod navigate;

pub use create::*;
pub use lifecycle::*;
pub use navigate::*;

use crate::git::{self, GitError, worktree::Worktree};
use fuzzy_matcher::skim::SkimMatcherV2;
use miette::{Diagnostic, Result};
use std::io;
use std::path::{Path, PathBuf};
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

    #[error("Cannot sync a workspace to itself")]
    #[diagnostic(
        code(gx::workspace::sync_same),
        help("Pass a different --from source or target")
    )]
    SameSourceAndTarget,

    #[error("Cannot remove the main worktree")]
    #[diagnostic(
        code(gx::workspace::remove_main),
        help("The main worktree is the original repository checkout and cannot be removed")
    )]
    RemoveMain,

    #[error("Cannot move the main worktree")]
    #[diagnostic(
        code(gx::workspace::move_main),
        help(
            "The main worktree is the original repository checkout; move it with 'git worktree move' manually if you must"
        )
    )]
    MoveMain,

    #[error("Destination '{0}' already exists")]
    #[diagnostic(
        code(gx::workspace::destination_exists),
        help("Choose a path that does not exist yet")
    )]
    DestinationExists(PathBuf),

    #[error("Cannot lock the main worktree")]
    #[diagnostic(
        code(gx::workspace::lock_main),
        help("The main worktree is the original repository checkout and cannot be locked")
    )]
    LockMain,

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

/// Options for [`run_new`] / `create_workspace`, gathered into one struct so
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
    /// Skip the repo policy's pre- and post-create hooks.
    pub no_hooks: bool,
    /// Create the workspace with a detached HEAD instead of a new branch.
    pub detach: bool,
    /// Set the base's remote branch as the new branch's upstream.
    pub track: bool,
}

// ----- shared resolution / path helpers --------------------------------------

/// List the repository's worktrees, mapping the git error into [`WorkspaceError`].
/// The common preamble of nearly every workspace handler.
pub(crate) fn load_worktrees() -> Result<Vec<Worktree>, WorkspaceError> {
    git::worktree::list().map_err(WorkspaceError::GitError)
}

/// Fuzzy-resolve `query` to a single worktree, erroring with
/// [`WorkspaceError::NoMatch`] when nothing matches.
pub(crate) fn resolve_target(
    query: &str,
    worktrees: &[Worktree],
) -> Result<Worktree, WorkspaceError> {
    fuzzy_match_worktree(query, worktrees).ok_or_else(|| WorkspaceError::NoMatch(query.to_string()))
}

pub(crate) fn main_worktree_root(worktrees: &[Worktree]) -> Result<PathBuf, WorkspaceError> {
    worktrees
        .iter()
        .find(|w| w.is_main)
        .map(|w| w.path.clone())
        .ok_or(WorkspaceError::GitError(GitError::NotInRepo))
}

pub(crate) fn workspace_path(
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

    let root_path = expand_home(&root, home);

    let base = if root_path.is_absolute() {
        root_path
    } else {
        main_root.join(root_path)
    };

    base.join(name)
}

/// Expand a leading '~' (or '~/...') in `path` against `home`. When `home` is
/// unknown the '~' is left untouched and treated as a literal path component.
pub(crate) fn expand_home(path: &str, home: Option<&Path>) -> PathBuf {
    match home {
        Some(home) if path == "~" => home.to_path_buf(),
        Some(home) => match path.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

/// Resolve a user-supplied move destination to an absolute path: expand a
/// leading '~', then make relative paths absolute against the current working
/// directory (matching `git worktree move`'s own relative-path behavior).
pub(crate) fn resolve_dest_path(input: &str, home: Option<&Path>, cwd: &Path) -> PathBuf {
    let expanded = expand_home(input, home);
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Workspace names double as branch names, so '/' is allowed (e.g.
/// 'feat/expose-rationale'); the directory name flattens it via
/// `git::worktree::flatten_slashes`. Empty segments and '.'/'..'
/// segments are rejected.
pub(crate) fn validate_name(name: &str) -> Result<(), WorkspaceError> {
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

pub(crate) fn paths_equal(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Resolve a sync source/target query to a worktree root. Accepts an absolute
/// path to any worktree root, or to any existing directory; otherwise
/// fuzzy-matches workspace name/branch using the same matcher as
/// `gx workspace go`.
pub(crate) fn resolve_worktree_root(query: &str, worktrees: &[Worktree]) -> Result<PathBuf> {
    let p = Path::new(query);
    if p.is_absolute() {
        // An absolute path that equals a known worktree root resolves to it.
        if let Some(w) = worktrees.iter().find(|w| paths_equal(&w.path, p)) {
            return Ok(w.path.clone());
        }
        // Otherwise accept the path directly if it exists as a directory; the
        // spec lists absolute paths as a valid source/target form.
        if p.is_dir() {
            return Ok(p.to_path_buf());
        }
        return Err(WorkspaceError::NoMatch(query.to_string()).into());
    }

    fuzzy_match_worktree(query, worktrees)
        .map(|w| w.path)
        .ok_or_else(|| WorkspaceError::NoMatch(query.to_string()).into())
}

pub(crate) fn fuzzy_match_worktree(query: &str, worktrees: &[Worktree]) -> Option<Worktree> {
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
pub(crate) mod test_support {
    use crate::git::worktree::Worktree;
    use std::path::PathBuf;

    /// A bare [`Worktree`] fixture under `/ws/<name>` for the submodule tests.
    pub(crate) fn worktree(name: &str, branch: Option<&str>) -> Worktree {
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
}

#[cfg(test)]
mod tests {
    use super::test_support::worktree;
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
    fn test_expand_home() {
        let home = Path::new("/home/user");
        assert_eq!(expand_home("~", Some(home)), PathBuf::from("/home/user"));
        assert_eq!(
            expand_home("~/ws/feat", Some(home)),
            PathBuf::from("/home/user/ws/feat")
        );
        // A bare absolute path is untouched.
        assert_eq!(expand_home("/srv/ws", Some(home)), PathBuf::from("/srv/ws"));
        // Without a home dir, '~' stays literal.
        assert_eq!(expand_home("~/ws", None), PathBuf::from("~/ws"));
    }

    #[test]
    fn test_resolve_dest_path() {
        let home = Path::new("/home/user");
        let cwd = Path::new("/work/cwd");

        // Absolute destination is used as-is.
        assert_eq!(
            resolve_dest_path("/srv/ws/moved", Some(home), cwd),
            PathBuf::from("/srv/ws/moved")
        );
        // '~' expands to home and stays absolute.
        assert_eq!(
            resolve_dest_path("~/ws/moved", Some(home), cwd),
            PathBuf::from("/home/user/ws/moved")
        );
        // A relative destination resolves against the current working dir.
        assert_eq!(
            resolve_dest_path("moved", Some(home), cwd),
            PathBuf::from("/work/cwd/moved")
        );
        assert_eq!(
            resolve_dest_path("../sibling", Some(home), cwd),
            PathBuf::from("/work/cwd/../sibling")
        );
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
    fn test_resolve_worktree_root_fuzzy_matches_name_or_branch() {
        let worktrees = vec![
            worktree("repo", Some("main")),
            worktree("feat-expose-rationale", Some("feat/expose-rationale")),
        ];

        // Branch-form query resolves to the workspace root.
        let root = resolve_worktree_root("feat/expose-rationale", &worktrees).unwrap();
        assert_eq!(root, PathBuf::from("/ws/feat-expose-rationale"));

        // Name-form query resolves the same.
        let root = resolve_worktree_root("feat-expose-rationale", &worktrees).unwrap();
        assert_eq!(root, PathBuf::from("/ws/feat-expose-rationale"));
    }

    #[test]
    fn test_resolve_worktree_root_rejects_unknown_query() {
        let worktrees = vec![worktree("repo", Some("main"))];
        let err = resolve_worktree_root("nonexistent-xyz", &worktrees).unwrap_err();
        assert!(err.to_string().contains("nonexistent-xyz"));
    }

    #[test]
    fn test_resolve_worktree_root_accepts_absolute_worktree_path() {
        let tmp = std::env::temp_dir().join(format!(
            "gx-resolve-abs-known-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let mut wt = worktree("feature", Some("feature"));
        wt.path = tmp.clone();
        let worktrees = vec![wt];

        let root = resolve_worktree_root(tmp.to_str().unwrap(), &worktrees).unwrap();
        assert!(paths_equal(&root, &tmp));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_resolve_worktree_root_accepts_absolute_dir_not_registered() {
        let tmp = std::env::temp_dir().join(format!(
            "gx-resolve-abs-unknown-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        // No worktree registered at this path, but it is a real directory.
        let worktrees = vec![worktree("repo", Some("main"))];
        let root = resolve_worktree_root(tmp.to_str().unwrap(), &worktrees).unwrap();
        assert_eq!(root, tmp);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_resolve_worktree_root_rejects_absolute_nonexistent_path() {
        let worktrees = vec![worktree("repo", Some("main"))];
        let missing = "/this/path/should/not/exist/gx-test-xyz";
        let err = resolve_worktree_root(missing, &worktrees).unwrap_err();
        assert!(err.to_string().contains(missing));
    }

    #[test]
    fn test_self_sync_guard_via_paths_equal() {
        // run_sync refuses when resolved source and target are the same root;
        // this verifies the comparison primitive it relies on.
        let tmp =
            std::env::temp_dir().join(format!("gx-self-sync-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(paths_equal(&tmp, &tmp));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
