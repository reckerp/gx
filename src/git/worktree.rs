use super::{GitError, get_repo};
use crate::git::git_exec::{self, ExecOptions};
use std::path::{Path, PathBuf};

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

/// Add a new worktree at `path` checking out `branch`.
/// When `create_branch` is true the branch is created (optionally from `base`).
pub fn add(
    path: &Path,
    branch: &str,
    create_branch: bool,
    base: Option<&str>,
) -> Result<(), GitError> {
    let mut args = vec!["worktree".to_string(), "add".to_string()];

    if create_branch {
        args.push("-b".to_string());
        args.push(branch.to_string());
        args.push(path.display().to_string());
        if let Some(base) = base {
            args.push(base.to_string());
        }
    } else {
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

pub fn remove(path: &Path, force: bool) -> Result<(), GitError> {
    let mut args = vec!["worktree".to_string(), "remove".to_string()];
    if force {
        args.push("--force".to_string());
    }
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

pub fn branch_exists(branch_name: &str) -> Result<bool, GitError> {
    let repo = get_repo()?;
    Ok(repo
        .find_branch(branch_name, git2::BranchType::Local)
        .is_ok())
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
}
