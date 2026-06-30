//! Resolving the diff range a `gx review` session targets.
//!
//! A review compares a "from" side (the tree of some commit, or the empty tree
//! for a root commit) against a "to" side (another commit, or the working
//! tree). Three modes are supported:
//!
//! - **Branch** (default): `merge-base(base, HEAD) → HEAD`, i.e. what this
//!   branch adds over its base (origin's default branch unless overridden).
//! - **Commit**: a single commit (`<commit>^..<commit>`) or an explicit
//!   `A..B` / `A...B` range.
//! - **Uncommitted**: the working tree against `HEAD`.
//!
//! Each resolved range also carries a `scope_id` used as the per-branch
//! persistence key (see the `state` unit): `branch:<name>`, `commit:<oid…>`, or
//! `worktree`.

use crate::git::{GitError, branch, get_repo};
use git2::Oid;

/// Which kind of comparison a review targets. Drives the header label and the
/// in-TUI range switcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeMode {
    Branch,
    Commit,
    Uncommitted,
}

/// The "to" side of a review range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// Compare against this commit's tree.
    Commit(Oid),
    /// Compare against the current working tree (index + unstaged).
    WorkingTree,
}

/// A fully resolved review range, ready for the diff builder to consume.
#[derive(Debug, Clone)]
pub struct ReviewRange {
    pub mode: RangeMode,
    /// Commit whose tree is the "old" side. `None` means the empty tree (a root
    /// commit has no parent to diff against).
    pub from: Option<Oid>,
    pub to: Endpoint,
    /// Human-readable description for the header (e.g. `origin/main...HEAD`).
    pub label: String,
    /// Stable per-review key fragment, combined with the clone identity for
    /// persistence: `branch:<name>`, `commit:<oid>`, or `worktree`.
    pub scope_id: String,
}

/// How a positional `target` argument is interpreted.
enum Target {
    /// An explicit `A..B` / `A...B` range.
    Range { from: String, to: String },
    /// A single branch/commit/tag reference.
    Single(String),
}

/// Split a positional target into a range or a single ref. `...` is checked
/// before `..` so a three-dot range is not mis-split on its leading two dots.
fn parse_target(target: &str) -> Target {
    if let Some((a, b)) = target.split_once("...") {
        Target::Range {
            from: a.to_string(),
            to: b.to_string(),
        }
    } else if let Some((a, b)) = target.split_once("..") {
        Target::Range {
            from: a.to_string(),
            to: b.to_string(),
        }
    } else {
        Target::Single(target.to_string())
    }
}

/// Resolve CLI arguments into a concrete range.
///
/// - no `target` → branch mode (`base` overrides the default-branch base);
/// - `target` containing `..`/`...` → an explicit range;
/// - any other `target` → single-commit mode.
pub fn resolve(target: Option<String>, base: Option<String>) -> Result<ReviewRange, GitError> {
    match target {
        None => resolve_branch(base),
        Some(t) => match parse_target(&t) {
            Target::Range { from, to } => resolve_explicit_range(&from, &to),
            Target::Single(s) => resolve_commit(&s),
        },
    }
}

/// Branch-vs-base: `merge-base(base, HEAD) → HEAD`.
pub fn resolve_branch(base: Option<String>) -> Result<ReviewRange, GitError> {
    let repo = get_repo()?;

    let base_ref = match base {
        Some(b) => b,
        None => branch::default_remote_branch()?.ok_or_else(|| GitError::CommandFailed {
            stderr: "no base branch found (no 'origin' remote); pass --base <ref>".to_string(),
            code: None,
        })?,
    };

    let base_oid = repo.revparse_single(&base_ref)?.peel_to_commit()?.id();
    let head_oid = repo.head()?.peel_to_commit()?.id();
    let merge_base = repo.merge_base(base_oid, head_oid)?;

    Ok(ReviewRange {
        mode: RangeMode::Branch,
        from: Some(merge_base),
        to: Endpoint::Commit(head_oid),
        label: format!("{base_ref}...HEAD"),
        scope_id: branch_scope_id(&repo, head_oid)?,
    })
}

/// Single commit: `<commit>^..<commit>` (the empty tree when it is a root commit).
pub fn resolve_commit(reference: &str) -> Result<ReviewRange, GitError> {
    let repo = get_repo()?;
    let commit = repo.revparse_single(reference)?.peel_to_commit()?;
    let to_oid = commit.id();
    let from = commit.parent(0).ok().map(|p| p.id());
    let sh = short_oid(&repo, to_oid);

    Ok(ReviewRange {
        mode: RangeMode::Commit,
        from,
        to: Endpoint::Commit(to_oid),
        label: format!("{sh}^..{sh}"),
        scope_id: format!("commit:{to_oid}"),
    })
}

/// Explicit `A..B` range between two committish refs.
pub fn resolve_explicit_range(from_ref: &str, to_ref: &str) -> Result<ReviewRange, GitError> {
    let repo = get_repo()?;
    let from_oid = repo.revparse_single(from_ref)?.peel_to_commit()?.id();
    let to_oid = repo.revparse_single(to_ref)?.peel_to_commit()?.id();

    Ok(ReviewRange {
        mode: RangeMode::Commit,
        from: Some(from_oid),
        to: Endpoint::Commit(to_oid),
        label: format!("{from_ref}..{to_ref}"),
        scope_id: format!("commit:{from_oid}-{to_oid}"),
    })
}

/// Working tree (index + unstaged) against `HEAD`.
pub fn resolve_uncommitted() -> Result<ReviewRange, GitError> {
    let repo = get_repo()?;
    let head_oid = repo.head()?.peel_to_commit()?.id();

    Ok(ReviewRange {
        mode: RangeMode::Uncommitted,
        from: Some(head_oid),
        to: Endpoint::WorkingTree,
        label: "HEAD..<working tree>".to_string(),
        scope_id: "worktree".to_string(),
    })
}

/// Branch-mode scope: the current branch name, or `commit:<HEAD>` on a detached
/// HEAD where there is no branch name to key on.
fn branch_scope_id(repo: &git2::Repository, head_oid: Oid) -> Result<String, GitError> {
    let _ = repo;
    let current = branch::get_current_branch()?;
    if current.is_detached {
        Ok(format!("commit:{head_oid}"))
    } else {
        Ok(format!("branch:{}", current.name))
    }
}

/// Abbreviated object id for display, falling back to a 7-char prefix.
fn short_oid(repo: &git2::Repository, oid: Oid) -> String {
    repo.find_object(oid, None)
        .ok()
        .and_then(|o| o.short_id().ok())
        .and_then(|s| s.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| oid.to_string().chars().take(7).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_target_detects_two_dot_range() {
        match parse_target("main..feature") {
            Target::Range { from, to } => {
                assert_eq!(from, "main");
                assert_eq!(to, "feature");
            }
            Target::Single(_) => panic!("expected a range"),
        }
    }

    #[test]
    fn parse_target_detects_three_dot_range_without_mis_split() {
        match parse_target("main...feature") {
            Target::Range { from, to } => {
                assert_eq!(from, "main");
                assert_eq!(to, "feature");
            }
            Target::Single(_) => panic!("expected a range"),
        }
    }

    #[test]
    fn parse_target_treats_plain_ref_as_single() {
        match parse_target("abc123") {
            Target::Single(s) => assert_eq!(s, "abc123"),
            Target::Range { .. } => panic!("expected a single ref"),
        }
    }

    #[test]
    fn uncommitted_resolves_against_working_tree() {
        // Runs in the repo's own working tree; HEAD always has a commit here.
        let range = resolve_uncommitted().expect("resolve uncommitted");
        assert_eq!(range.mode, RangeMode::Uncommitted);
        assert_eq!(range.to, Endpoint::WorkingTree);
        assert_eq!(range.scope_id, "worktree");
        assert!(range.from.is_some());
    }
}
