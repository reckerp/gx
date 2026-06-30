//! Worktree lifecycle operations: listing, creating, switching, removing,
//! moving, locking, and repairing worktrees. These shell out to `git` (often via
//! `git -C <dir>` so they work from any worktree). The pure `*_args` builders are
//! split out so the argument construction is unit-testable without spawning git.

use super::Worktree;
use super::model::parse_porcelain;
use crate::git::git_exec::{self, ExecOptions};
use crate::git::{GitError, get_repo};
use std::path::{Path, PathBuf};

/// List all worktrees of the repository. The main worktree is always first.
pub fn list() -> Result<Vec<Worktree>, GitError> {
    let output = git_exec::exec(["worktree", "list", "--porcelain"], ExecOptions::capture())?;

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
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ExecOptions::capture(),
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

    git_exec::exec(args, ExecOptions::silent())?;

    Ok(())
}

/// Switch the worktree at `path` to an existing local `branch`
/// (`git -C <path> switch <branch>`). Used by the existing-path branch
/// switch flow when the requested branch differs from what's checked out.
/// `switch` (rather than `checkout <branch>`) only ever resolves a branch,
/// never a pathspec, so a `--` separator is unnecessary and would misparse.
pub fn switch_branch(path: &Path, branch: &str) -> Result<(), GitError> {
    git_exec::exec_in(path, &["switch", branch], ExecOptions::silent())?;
    Ok(())
}

/// True when `base` resolves to a commit using only local refs
/// (`git rev-parse --verify --quiet <base>^{commit}`). Used to give a targeted
/// offline diagnostic when `--no-fetch` is set before calling [`add`].
pub fn ref_resolvable(base: &str) -> Result<bool, GitError> {
    let spec = format!("{}^{{commit}}", base);
    match git_exec::exec(
        ["rev-parse", "--verify", "--quiet", &spec],
        ExecOptions::capture(),
    ) {
        Ok(out) => Ok(!out.trim().is_empty()),
        // rev-parse --verify --quiet exits non-zero with no stderr when the ref
        // is unknown; treat that (non-zero exit + empty stderr) as "not
        // resolvable" rather than a hard error.
        Err(GitError::CommandFailed {
            stderr,
            code: Some(_),
        }) if stderr.is_empty() => Ok(false),
        Err(e) => Err(e),
    }
}

/// Detect a git ref-namespace conflict for a branch we are about to create.
/// Git stores branches as files under `refs/heads`, so `refs/heads/foo` (a
/// file) and `refs/heads/foo/bar` (a directory) cannot coexist. Returns the
/// first existing branch that conflicts with `branch_name`, if any.
pub fn conflicting_branch(branch_name: &str) -> Result<Option<String>, GitError> {
    let output = git_exec::exec(
        ["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        ExecOptions::capture(),
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
    name.len() > prefix.len() && name.starts_with(prefix) && name.as_bytes()[prefix.len()] == b'/'
}

/// Rebase the branch checked out in the worktree at `path` onto `base`
/// (e.g. 'origin/main'). Runs in the worktree via 'git -C'.
pub fn rebase_onto(path: &Path, base: &str) -> Result<(), GitError> {
    git_exec::exec_in(path, &["rebase", "--", base], ExecOptions::silent())?;
    Ok(())
}

/// True when the worktree at `path` has staged or unstaged changes to
/// tracked files (untracked files don't block a rebase, so they're ignored).
pub fn has_tracked_changes(path: &Path) -> Result<bool, GitError> {
    let output = git_exec::exec_in(
        path,
        &["status", "--porcelain", "--untracked-files=no"],
        ExecOptions::capture(),
    )?;
    Ok(!output.trim().is_empty())
}

/// Remove the worktree at `path`. Runs git from `from` (the main worktree)
/// so removal works even when the process is inside the worktree being
/// removed (git refuses to remove its own current worktree).
pub fn remove(from: &Path, path: &Path, force: bool) -> Result<(), GitError> {
    let path = path.display().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push("--");
    args.push(&path);

    git_exec::exec_in(from, &args, ExecOptions::silent())?;

    Ok(())
}

pub fn delete_branch(from: &Path, branch_name: &str, force: bool) -> Result<(), GitError> {
    let delete_flag = if force { "-D" } else { "-d" };
    git_exec::exec_in(
        from,
        &["branch", delete_flag, "--", branch_name],
        ExecOptions::silent(),
    )?;
    Ok(())
}

/// Move the worktree at `path` to `new_path`. Runs git from `from` (the main
/// worktree) and prefers `git worktree move` over a manual filesystem move so
/// Git's administrative files (`.git` pointer, gitdir link) stay consistent.
pub fn move_worktree(from: &Path, path: &Path, new_path: &Path) -> Result<(), GitError> {
    git_exec::exec(
        move_worktree_args(from, path, new_path),
        ExecOptions::silent(),
    )?;
    Ok(())
}

/// Lock the worktree at `path` so cleanup and `git worktree prune` skip it.
/// Runs git from `from` (the main worktree). An optional `reason` is recorded
/// by git and shown in `git worktree list --verbose`.
pub fn lock(from: &Path, path: &Path, reason: Option<&str>) -> Result<(), GitError> {
    git_exec::exec(lock_args(from, path, reason), ExecOptions::silent())?;
    Ok(())
}

/// Clear the lock on the worktree at `path`. Runs git from `from` (the main
/// worktree).
pub fn unlock(from: &Path, path: &Path) -> Result<(), GitError> {
    git_exec::exec(unlock_args(from, path), ExecOptions::silent())?;
    Ok(())
}

/// Repair worktree administrative files (the two-way links between the main
/// repository and its linked worktrees). With no `paths`, git repairs every
/// worktree; otherwise it repairs only the listed ones. Mostly for recovery
/// after a worktree directory or the main repo has been moved. Runs git from
/// `from` (the main worktree).
pub fn repair(from: &Path, paths: &[PathBuf]) -> Result<(), GitError> {
    git_exec::exec(repair_args(from, paths), ExecOptions::silent())?;
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

/// Run `git worktree prune` from `from` (the main worktree) to drop metadata for
/// worktrees whose directories no longer exist.
pub fn prune_metadata(from: &Path) -> Result<(), GitError> {
    git_exec::exec_in(from, &["worktree", "prune"], ExecOptions::silent())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_ref_conflict() {
        let existing = vec!["foo".to_string(), "feat/a".to_string(), "main".to_string()];

        // existing 'foo' (a file) blocks creating 'foo/bar' (a directory)
        assert_eq!(
            find_ref_conflict("foo/bar", &existing),
            Some("foo".to_string())
        );
        // existing 'feat/a' (a directory) blocks creating 'feat' (a file)
        assert_eq!(
            find_ref_conflict("feat", &existing),
            Some("feat/a".to_string())
        );

        // exact matches are not conflicts (handled as "already exists")
        assert_eq!(find_ref_conflict("foo", &existing), None);
        assert_eq!(find_ref_conflict("main", &existing), None);

        // unrelated names and shared text prefixes (not path prefixes) are fine
        assert_eq!(find_ref_conflict("bar", &existing), None);
        assert_eq!(find_ref_conflict("foobar", &existing), None);
        assert_eq!(find_ref_conflict("feat-b", &existing), None);
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
