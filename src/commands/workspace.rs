use crate::config;
use crate::git::{self, GitError, worktree::Worktree};
use crate::ui;
use crate::ui::workspace_picker::WorkspaceAction;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use miette::{Diagnostic, Result};
use std::io::{self, Write};
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

    #[error("Cannot remove the main worktree")]
    #[diagnostic(
        code(gx::workspace::remove_main),
        help("The main worktree is the original repository checkout and cannot be removed")
    )]
    RemoveMain,

    #[error("Cannot remove the workspace you are currently in")]
    #[diagnostic(
        code(gx::workspace::remove_current),
        help("Switch to another workspace first, e.g. 'gx workspace go'")
    )]
    RemoveCurrent,

    #[error("Invalid workspace name: {0}")]
    #[diagnostic(code(gx::workspace::invalid_name))]
    InvalidName(String),

    #[error("Failed to copy setup file: {0}")]
    #[diagnostic(code(gx::workspace::copy_failed))]
    CopyFailed(#[from] io::Error),

    #[error("Failed to create workspace directory: {0}")]
    #[diagnostic(code(gx::workspace::create_dir_failed))]
    CreateDirFailed(io::Error),
}

/// Create a new workspace (worktree) and print its path to stdout so the
/// shell wrapper from `gx setup` can cd into it.
pub fn run_new(
    name: String,
    base: Option<String>,
    branch: Option<String>,
    no_setup: bool,
) -> Result<()> {
    validate_name(&name)?;

    // Branch names may contain '/' (e.g. 'feat/expose-rationale'), but the
    // workspace directory must be a single path component.
    let dir_name = directory_name(&name);

    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;
    if let Some(existing) = worktrees.iter().find(|w| w.name == dir_name) {
        return Err(WorkspaceError::AlreadyExists(dir_name, existing.path.clone()).into());
    }

    let cfg = config::load()?;
    let main_root = main_worktree_root(&worktrees)?;
    let path = workspace_path(
        &main_root,
        home_dir().as_deref(),
        &cfg.workspace.root,
        &dir_name,
    );

    if path.exists() {
        return Err(WorkspaceError::AlreadyExists(dir_name, path).into());
    }

    let branch_name = branch.unwrap_or_else(|| name.clone());
    let branch_exists_locally =
        git::worktree::branch_exists(&branch_name).map_err(WorkspaceError::GitError)?;

    // Resolve what the new branch should start from: an explicit base wins,
    // then a matching remote branch (like plain 'git checkout'), otherwise
    // the default branch of origin (e.g. 'origin/main') instead of HEAD.
    let mut tracking_remote: Option<String> = None;
    let mut default_base: Option<String> = None;
    let (create_branch, base) = if branch_exists_locally {
        if let Some(base) = &base {
            eprintln!(
                "warning: ignoring base '{}'; branch '{}' already exists",
                base, branch_name
            );
        }
        (false, None)
    } else {
        let base = match base {
            Some(base) => Some(base),
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
        (true, base)
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(WorkspaceError::CreateDirFailed)?;
    }

    // When branching off the default remote branch, don't set it as upstream:
    // the new branch should push to its own name, not e.g. origin/main.
    let no_track = default_base.is_some();
    git::worktree::add(
        &path,
        &branch_name,
        create_branch,
        base.as_deref(),
        no_track,
    )
    .map_err(WorkspaceError::GitError)?;

    let path = path.canonicalize().unwrap_or(path);

    if let Some(remote) = &tracking_remote {
        eprintln!(
            "Created workspace '{}' on new branch '{}' (tracking '{}')",
            dir_name, branch_name, remote
        );
    } else if let Some(default_base) = &default_base {
        eprintln!(
            "Created workspace '{}' on new branch '{}' (from '{}')",
            dir_name, branch_name, default_base
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

    if !no_setup {
        let copied = copy_setup_files(&main_root, &path, &cfg.workspace.copy_files)?;
        for file in &copied {
            eprintln!("  copied {}", file);
        }
    }

    print_go_path(&path);
    Ok(())
}

/// Resolve a workspace by query (or interactively) and print its path to
/// stdout for the shell wrapper to cd into.
pub fn run_go(query: Option<String>) -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    let target = match query {
        Some(q) => fuzzy_match_worktree(&q, &worktrees)
            .ok_or_else(|| WorkspaceError::NoMatch(q.clone()))?,
        None => {
            let Some(action) = pick_workspace(&worktrees)? else {
                eprintln!("Cancelled");
                return Ok(());
            };
            match action {
                WorkspaceAction::Go(w) => w,
                WorkspaceAction::Remove(w) => return remove_worktree(&w, false),
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

pub fn run_remove(query: Option<String>, force: bool) -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;

    let target = match query {
        Some(q) => fuzzy_match_worktree(&q, &worktrees)
            .ok_or_else(|| WorkspaceError::NoMatch(q.clone()))?,
        None => {
            let Some(action) = pick_workspace(&worktrees)? else {
                eprintln!("Cancelled");
                return Ok(());
            };
            match action {
                // in remove context both Go and Remove target the selection
                WorkspaceAction::Go(w) | WorkspaceAction::Remove(w) => w,
                WorkspaceAction::Create { name } => return create_from_picker(name),
            }
        }
    };

    remove_worktree(&target, force)
}

/// Re-copy setup files (e.g. .env) from the main worktree into the current
/// workspace.
pub fn run_setup() -> Result<()> {
    let worktrees = git::worktree::list().map_err(WorkspaceError::GitError)?;
    let main_root = main_worktree_root(&worktrees)?;
    let current = git::worktree::current_worktree_root().map_err(WorkspaceError::GitError)?;

    if paths_equal(&main_root, &current) {
        eprintln!("Already in the main worktree; nothing to copy");
        return Ok(());
    }

    let cfg = config::load()?;
    let copied = copy_setup_files(&main_root, &current, &cfg.workspace.copy_files)?;

    if copied.is_empty() {
        eprintln!(
            "No setup files to copy (configured: {:?})",
            cfg.workspace.copy_files
        );
    } else {
        for file in &copied {
            eprintln!("copied {}", file);
        }
    }

    Ok(())
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
        WorkspaceAction::Remove(w) => remove_worktree(&w, false),
        WorkspaceAction::Create { name } => create_from_picker(name),
    }
}

fn pick_workspace(worktrees: &[Worktree]) -> Result<Option<WorkspaceAction>> {
    let mut terminal = ui::terminal::setup_terminal_stderr()
        .map_err(|e| WorkspaceError::TuiError(e.to_string()))?;
    let result = ui::workspace_picker::run(&mut terminal, worktrees);
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

    run_new(name, None, None, false)
}

fn remove_worktree(worktree: &Worktree, force: bool) -> Result<()> {
    if worktree.is_main {
        return Err(WorkspaceError::RemoveMain.into());
    }
    if worktree.is_current {
        return Err(WorkspaceError::RemoveCurrent.into());
    }

    let confirmed = ui::confirm::run_on_stderr(&format!(
        "Remove workspace '{}' ({})?",
        worktree.name,
        worktree.path.display()
    ))?;
    if !confirmed {
        eprintln!("Cancelled");
        return Ok(());
    }

    match git::worktree::remove(&worktree.path, force) {
        Ok(()) => {}
        // copied setup files (e.g. .env) are untracked, so offer a force
        // removal instead of failing outright
        Err(GitError::CommandFailed(msg))
            if !force && msg.contains("contains modified or untracked files") =>
        {
            let confirmed = ui::confirm::run_on_stderr(&format!(
                "Workspace '{}' has modified or untracked files. Remove anyway?",
                worktree.name
            ))?;
            if !confirmed {
                eprintln!("Cancelled");
                return Ok(());
            }
            git::worktree::remove(&worktree.path, true).map_err(WorkspaceError::GitError)?;
        }
        Err(e) => return Err(WorkspaceError::GitError(e).into()),
    }

    match &worktree.branch {
        Some(branch) => eprintln!(
            "Removed workspace '{}' (branch '{}' kept)",
            worktree.name, branch
        ),
        None => eprintln!("Removed workspace '{}'", worktree.name),
    }
    Ok(())
}

/// Print the path the shell wrapper should cd into. This is the only thing
/// workspace commands write to stdout.
fn print_go_path(path: &Path) {
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
/// [`directory_name`]. Empty segments and '.'/'..' segments are rejected.
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

/// Directory name for a workspace: '/' in the name (valid in branch names)
/// is replaced with '-' so the workspace stays a single path component.
fn directory_name(name: &str) -> String {
    name.replace('/', "-")
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
            if w.name.eq_ignore_ascii_case(query) {
                return Some((i64::MAX, w)); // prioritize exact name matches
            }

            let name_score = matcher.fuzzy_match(&w.name, query);
            let branch_score = w
                .branch
                .as_deref()
                .and_then(|b| matcher.fuzzy_match(b, query));

            name_score
                .into_iter()
                .chain(branch_score)
                .max()
                .map(|score| (score, w))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, w)| w.clone())
}

/// Copy configured setup files from `src_root` to `dst_root`.
/// Patterns are relative paths; the filename component may contain `*`/`?`
/// wildcards. Directories are copied recursively. Missing sources are
/// skipped. Returns the relative paths that were copied.
fn copy_setup_files(
    src_root: &Path,
    dst_root: &Path,
    patterns: &[String],
) -> Result<Vec<String>, WorkspaceError> {
    let mut copied = Vec::new();

    for pattern in patterns {
        let pattern = pattern.trim_matches('/');
        if pattern.is_empty() {
            continue;
        }

        let (dir_part, file_pattern) = match pattern.rsplit_once('/') {
            Some((dir, file)) => (Some(dir), file),
            None => (None, pattern),
        };

        let src_dir = match dir_part {
            Some(dir) => src_root.join(dir),
            None => src_root.to_path_buf(),
        };

        if !src_dir.is_dir() {
            continue;
        }

        let has_wildcard = file_pattern.contains('*') || file_pattern.contains('?');

        // never copy .git: in a worktree it is a file pointing at the main
        // repository, and overwriting it would corrupt the workspace
        let is_git_meta = |name: &str| dir_part.is_none() && name == ".git";

        let matched_names: Vec<String> = if has_wildcard {
            let mut names: Vec<String> = std::fs::read_dir(&src_dir)
                .map_err(WorkspaceError::CopyFailed)?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| wildcard_match(file_pattern, name) && !is_git_meta(name))
                .collect();
            names.sort();
            names
        } else if !is_git_meta(file_pattern) && src_dir.join(file_pattern).exists() {
            vec![file_pattern.to_string()]
        } else {
            vec![]
        };

        for name in matched_names {
            let rel_path = match dir_part {
                Some(dir) => format!("{}/{}", dir, name),
                None => name.clone(),
            };
            let src = src_root.join(&rel_path);
            let dst = dst_root.join(&rel_path);

            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(WorkspaceError::CopyFailed)?;
            }

            if src.is_dir() {
                copy_dir_recursive(&src, &dst)?;
            } else {
                std::fs::copy(&src, &dst).map_err(WorkspaceError::CopyFailed)?;
            }

            copied.push(rel_path);
        }
    }

    Ok(copied)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), WorkspaceError> {
    std::fs::create_dir_all(dst).map_err(WorkspaceError::CopyFailed)?;

    for entry in std::fs::read_dir(src).map_err(WorkspaceError::CopyFailed)? {
        let entry = entry.map_err(WorkspaceError::CopyFailed)?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(WorkspaceError::CopyFailed)?;
        }
    }

    Ok(())
}

/// Simple wildcard matcher supporting `*` (any sequence) and `?` (any char).
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    fn matches(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => matches(&p[1..], t) || (!t.is_empty() && matches(p, &t[1..])),
            (Some('?'), Some(_)) => matches(&p[1..], &t[1..]),
            (Some(pc), Some(tc)) if pc == tc => matches(&p[1..], &t[1..]),
            _ => false,
        }
    }

    matches(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_match() {
        assert!(wildcard_match(".env", ".env"));
        assert!(wildcard_match(".env*", ".env"));
        assert!(wildcard_match(".env*", ".env.local"));
        assert!(wildcard_match("*.local", ".env.local"));
        assert!(wildcard_match("?env", ".env"));
        assert!(!wildcard_match(".env", ".env.local"));
        assert!(!wildcard_match(".env?", ".env"));
        assert!(!wildcard_match("*.toml", ".env"));
    }

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

    #[test]
    fn test_directory_name() {
        assert_eq!(directory_name("feature-x"), "feature-x");
        assert_eq!(
            directory_name("feat/expose-rationale"),
            "feat-expose-rationale"
        );
        assert_eq!(directory_name("a/b/c"), "a-b-c");
    }

    #[test]
    fn test_copy_setup_files_never_copies_git() {
        let tmp = std::env::temp_dir().join(format!("gx-ws-git-test-{}", std::process::id()));
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        std::fs::write(src.join(".gitignore"), "target").unwrap();

        let patterns = vec![".*".to_string(), ".git".to_string()];
        let copied = copy_setup_files(&src, &dst, &patterns).unwrap();

        assert_eq!(copied, vec![".gitignore".to_string()]);
        assert!(!dst.join(".git").exists());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_copy_setup_files() {
        let tmp = std::env::temp_dir().join(format!("gx-ws-test-{}", std::process::id()));
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(src.join("config")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        std::fs::write(src.join(".env"), "SECRET=1").unwrap();
        std::fs::write(src.join(".env.local"), "LOCAL=1").unwrap();
        std::fs::write(src.join("config/dev.toml"), "[dev]").unwrap();

        let patterns = vec![
            ".env*".to_string(),
            "config/dev.toml".to_string(),
            "missing.txt".to_string(),
        ];
        let copied = copy_setup_files(&src, &dst, &patterns).unwrap();

        assert_eq!(
            copied,
            vec![
                ".env".to_string(),
                ".env.local".to_string(),
                "config/dev.toml".to_string()
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dst.join(".env")).unwrap(),
            "SECRET=1"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("config/dev.toml")).unwrap(),
            "[dev]"
        );
        assert!(!dst.join("missing.txt").exists());

        std::fs::remove_dir_all(&tmp).ok();
    }
}
