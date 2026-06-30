//! Integration tests for `gx workspace new` creation ergonomics (Section 4).
//!
//! Each test builds a throwaway git repository in a temp dir and drives the
//! compiled `gx` binary against it. The workspace root is configured (via an
//! isolated config home) to a path *inside* the temp repo so every created
//! worktree is cleaned up with the temp dir and never touches the real machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A temp git repo plus an isolated config home, wired so `gx` creates
/// workspaces inside the temp dir and reads only our test config.
struct Fixture {
    _tmp: TempDir,
    repo: PathBuf,
    config_home: PathBuf,
    workspaces_root: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let repo = root.join("repo");
        let config_home = root.join("config");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&config_home).unwrap();

        // Workspaces live in a sibling dir of the repo, addressed relative to
        // the main worktree root so they stay inside the temp dir.
        let workspaces_root = "../workspaces".to_string();

        // confy resolves its config dir from XDG_CONFIG_HOME/HOME; point it at
        // our isolated dir and write a config with the relative workspace root.
        let cfg_dir = config_home.join("gx");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("config.toml"),
            format!(
                "[workspace]\nroot = \"{}\"\ncopy_files = []\n",
                workspaces_root
            ),
        )
        .unwrap();

        let fixture = Fixture {
            _tmp: tmp,
            repo,
            config_home,
            workspaces_root,
        };
        fixture.init_repo();
        fixture
    }

    fn git(&self, args: &[&str]) -> Output {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn init_repo(&self) {
        self.git(&["init", "-b", "main"]);
        self.git(&["config", "user.email", "test@example.com"]);
        self.git(&["config", "user.name", "Test"]);
        std::fs::write(self.repo.join("README.md"), "hello\n").unwrap();
        self.git(&["add", "."]);
        self.git(&["commit", "-m", "init"]);
    }

    /// Run `gx <args>` inside the repo with the isolated config home.
    fn gx(&self, args: &[&str]) -> Output {
        let bin = env!("CARGO_BIN_EXE_gx");
        Command::new(bin)
            .args(args)
            .current_dir(&self.repo)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("HOME", &self.config_home)
            // No network; keep gh/pr lookups from interfering.
            .env("GH_TOKEN", "")
            .output()
            .unwrap()
    }

    fn workspace_path(&self, dir_name: &str) -> PathBuf {
        // workspaces_root is relative to the main worktree root (the repo).
        self.repo.join(&self.workspaces_root).join(dir_name)
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn no_cd_does_not_print_navigation_target() {
    let fx = Fixture::new();
    let out = fx.gx(&["workspace", "new", "feat-x", "--no-cd"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    // stdout is reserved for the navigation path; --no-cd must leave it empty.
    assert!(
        stdout(&out).trim().is_empty(),
        "stdout should be empty, got: {:?}",
        stdout(&out)
    );
    // The workspace is still created and reported on stderr.
    assert!(fx.workspace_path("feat-x").exists());
    assert!(stderr(&out).contains("--no-cd"));
}

#[test]
fn default_prints_navigation_target_to_stdout() {
    let fx = Fixture::new();
    let out = fx.gx(&["workspace", "new", "feat-y"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let printed = stdout(&out);
    let expected = fx.workspace_path("feat-y").canonicalize().unwrap();
    assert_eq!(printed.trim(), expected.to_string_lossy());
}

#[test]
fn no_fetch_does_not_fetch() {
    let fx = Fixture::new();
    // An origin remote exists, so without --no-fetch gx would try to fetch it.
    fx.git(&[
        "remote",
        "add",
        "origin",
        "https://invalid.invalid/repo.git",
    ]);

    let out = fx.gx(&["workspace", "new", "feat-offline", "--no-fetch", "main"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // The fetch helper prints "Fetching origin..." on stderr; --no-fetch skips it.
    assert!(
        !stderr(&out).contains("Fetching origin"),
        "--no-fetch should not fetch; stderr: {}",
        stderr(&out)
    );
    assert!(fx.workspace_path("feat-offline").exists());
}

#[test]
fn no_fetch_unresolvable_base_gives_offline_hint() {
    let fx = Fixture::new();
    let out = fx.gx(&[
        "workspace",
        "new",
        "feat-z",
        "--no-fetch",
        "origin/does-not-exist",
    ]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains("--no-fetch was used"),
        "expected offline hint, got: {}",
        err
    );
}

#[test]
fn from_staged_copies_added_and_modified_files() {
    let fx = Fixture::new();
    // Modify a tracked file and add a new file, then stage both.
    std::fs::write(fx.repo.join("README.md"), "changed\n").unwrap();
    std::fs::write(fx.repo.join("new.rs"), "fn main() {}\n").unwrap();
    fx.git(&["add", "README.md", "new.rs"]);

    let out = fx.gx(&["workspace", "new", "extracted", "--no-cd", "--from-staged"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let ws = fx.workspace_path("extracted");
    assert_eq!(
        std::fs::read_to_string(ws.join("README.md")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("new.rs")).unwrap(),
        "fn main() {}\n"
    );
    // The source workspace is unchanged: its index still has the staged change.
    assert_eq!(
        std::fs::read_to_string(fx.repo.join("README.md")).unwrap(),
        "changed\n"
    );
}

#[test]
fn from_staged_preserves_nested_paths() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.repo.join("src/deep/nest")).unwrap();
    std::fs::write(fx.repo.join("src/deep/nest/mod.rs"), "// nested\n").unwrap();
    fx.git(&["add", "src/deep/nest/mod.rs"]);

    let out = fx.gx(&["workspace", "new", "nested", "--no-cd", "--from-staged"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let ws = fx.workspace_path("nested");
    assert_eq!(
        std::fs::read_to_string(ws.join("src/deep/nest/mod.rs")).unwrap(),
        "// nested\n"
    );
}

#[test]
fn from_staged_handles_renamed_files() {
    let fx = Fixture::new();
    // Create and commit a file, then rename it and stage the rename.
    std::fs::write(fx.repo.join("old_name.txt"), "rename me\n").unwrap();
    fx.git(&["add", "old_name.txt"]);
    fx.git(&["commit", "-m", "add old_name"]);
    std::fs::rename(fx.repo.join("old_name.txt"), fx.repo.join("new_name.txt")).unwrap();
    fx.git(&["add", "-A"]);

    let out = fx.gx(&["workspace", "new", "renamed", "--no-cd", "--from-staged"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let ws = fx.workspace_path("renamed");
    // A renamed entry is copied under its new (target) path with the staged
    // contents. (The new worktree starts from HEAD, which still carries the
    // pre-rename old_name.txt; --from-staged only copies the rename target, it
    // does not replay the deletion of the old path.)
    assert_eq!(
        std::fs::read_to_string(ws.join("new_name.txt")).unwrap(),
        "rename me\n"
    );
    // gx reports it copied the rename target, not the old path.
    assert!(stderr(&out).contains("new_name.txt"));
}

#[test]
fn from_staged_skips_deleted_files() {
    let fx = Fixture::new();
    std::fs::write(fx.repo.join("doomed.txt"), "bye\n").unwrap();
    fx.git(&["add", "doomed.txt"]);
    fx.git(&["commit", "-m", "add doomed"]);
    fx.git(&["rm", "doomed.txt"]);

    let out = fx.gx(&["workspace", "new", "withdelete", "--no-cd", "--from-staged"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    // A staged deletion has no content to copy, so gx skips it with a warning
    // rather than touching the new workspace's copy of the file (which still
    // exists there because the worktree starts from HEAD, pre-deletion).
    assert!(stderr(&out).contains("skipping deleted file 'doomed.txt'"));
}

#[test]
fn from_staged_filters_to_requested_paths() {
    let fx = Fixture::new();
    std::fs::write(fx.repo.join("wanted.rs"), "keep\n").unwrap();
    std::fs::write(fx.repo.join("other.rs"), "drop\n").unwrap();
    fx.git(&["add", "wanted.rs", "other.rs"]);

    let out = fx.gx(&[
        "workspace",
        "new",
        "filtered",
        "--no-cd",
        "--from-staged",
        "wanted.rs",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let ws = fx.workspace_path("filtered");
    assert!(ws.join("wanted.rs").exists());
    assert!(!ws.join("other.rs").exists());
}

#[test]
fn ref_conflict_detected_before_worktree_add() {
    let fx = Fixture::new();
    // Existing branch 'foo' (a ref file) blocks creating 'foo/bar' (a dir).
    fx.git(&["branch", "foo"]);

    let out = fx.gx(&["workspace", "new", "foo/bar", "--no-cd"]);
    assert!(!out.status.success());
    // miette wraps the rendered diagnostic across lines, so match on the
    // stable diagnostic code plus key fragments rather than a full sentence.
    let err = stderr(&out);
    assert!(
        err.contains("ref_conflict") && err.contains("conflicts with existing branch"),
        "expected ref-conflict diagnostic, got: {}",
        err
    );
    // The workspace path must not have been created.
    assert!(!fx.workspace_path("foo-bar").exists());
}

#[test]
fn existing_clean_workspace_can_switch_branch() {
    let fx = Fixture::new();
    // Create a workspace on branch 'topic'.
    let create = fx.gx(&["workspace", "new", "topic", "--no-cd"]);
    assert!(create.status.success(), "stderr: {}", stderr(&create));
    let ws = fx.workspace_path("topic");
    assert!(ws.exists());

    // Create another local branch we can switch the workspace onto.
    fx.git(&["branch", "other"]);

    // Re-running new for the same path but branch 'other' switches it.
    let out = fx.gx(&["workspace", "new", "topic", "--no-cd", "-b", "other"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Switched workspace"),
        "stderr: {}",
        stderr(&out)
    );

    // The worktree now reports branch 'other'.
    let head = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&ws)
        .output()
        .unwrap();
    assert_eq!(stdout(&head).trim(), "other");
}

#[test]
fn existing_dirty_workspace_refuses_branch_switch() {
    let fx = Fixture::new();
    let create = fx.gx(&["workspace", "new", "topic2", "--no-cd"]);
    assert!(create.status.success(), "stderr: {}", stderr(&create));
    let ws = fx.workspace_path("topic2");

    // Dirty the workspace: modify a tracked file.
    std::fs::write(ws.join("README.md"), "dirty\n").unwrap();
    fx.git(&["branch", "other2"]);

    let out = fx.gx(&["workspace", "new", "topic2", "--no-cd", "-b", "other2"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("uncommitted changes"),
        "stderr: {}",
        stderr(&out)
    );

    // The workspace stays on its original branch.
    let head = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&ws)
        .output()
        .unwrap();
    assert_eq!(stdout(&head).trim(), "topic2");
}

#[test]
fn existing_path_navigates_when_branch_checked_out_elsewhere() {
    let fx = Fixture::new();
    // Workspace A on its own branch.
    let a = fx.gx(&["workspace", "new", "wsA", "--no-cd"]);
    assert!(a.status.success(), "stderr: {}", stderr(&a));
    // Workspace B on branch 'shared'.
    let b = fx.gx(&["workspace", "new", "shared", "--no-cd"]);
    assert!(b.status.success(), "stderr: {}", stderr(&b));
    let shared_ws = fx.workspace_path("shared").canonicalize().unwrap();

    // Asking to put branch 'shared' onto workspace A's path must not steal the
    // checkout; instead gx navigates to wherever 'shared' already lives.
    let out = fx.gx(&["workspace", "new", "wsA", "-b", "shared"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("already checked out in workspace 'shared'"),
        "stderr: {}",
        stderr(&out)
    );
    // stdout is the navigation target: the existing 'shared' workspace.
    assert_eq!(stdout(&out).trim(), shared_ws.to_string_lossy());
}

#[test]
fn detach_creates_detached_head() {
    let fx = Fixture::new();
    let out = fx.gx(&["workspace", "new", "detached", "--no-cd", "--detach"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let ws = fx.workspace_path("detached");
    assert!(ws.exists());

    let head = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .current_dir(&ws)
        .output()
        .unwrap();
    // symbolic-ref fails (non-zero) on a detached HEAD.
    assert!(
        !head.status.success(),
        "expected detached HEAD, but HEAD is symbolic: {}",
        stdout(&head)
    );
}

#[test]
fn detach_conflicts_with_branch_flag() {
    let fx = Fixture::new();
    let out = fx.gx(&[
        "workspace",
        "new",
        "bad",
        "--no-cd",
        "--detach",
        "-b",
        "whatever",
    ]);
    assert!(!out.status.success());
}

/// Sanity: the relative-root fixture actually places worktrees where we expect.
#[test]
fn fixture_places_workspace_relative_to_repo() {
    let fx = Fixture::new();
    let out = fx.gx(&["workspace", "new", "sanity", "--no-cd"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(fx.workspace_path("sanity").exists());
    // ...and the path is under the temp dir, never the real machine.
    assert!(
        fx.workspace_path("sanity")
            .starts_with(Path::new(fx.repo.parent().unwrap()))
    );
}
