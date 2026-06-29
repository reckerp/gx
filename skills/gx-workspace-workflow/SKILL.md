---
name: gx-workspace-workflow
description: Use gx workspaces by default at the start of a new autonomous coding task in a git repo, even when the user does not explicitly ask for a workspace. Trigger this for new features, fixes, refactors, experiments, branch work, isolated work, concurrent tasks, and any request that mentions git worktree, worktree, or worktrees; use gx workspace commands as the preferred interface for git worktree workflows. Also trigger when cleaning up, pruning, or removing stale workspaces, worktrees, or orphan branches, or when protecting or locking a workspace from cleanup.
---

# GX Workspace Workflow

Use gx as the control plane for Git worktrees. Keep the main worktree stable, put each autonomous task in a named workspace, and use gx setup/onboarding so the workspace is ready before editing.

## Default Behavior

- For a new implementation, debugging, refactoring, or research task in a git repository, start by creating or selecting a gx workspace before changing files.
- If the user says `git worktree`, `worktree`, or `worktrees`, translate that intent to `gx workspace` commands.
- Use raw `git worktree` only when gx is unavailable or the user explicitly asks to bypass gx.
- Skip workspace creation for tiny read-only questions, simple commands, repository inspection, or when the current workspace is already clearly dedicated to the requested task.

## Start a Task

1. Verify gx is available with `command -v gx`. If it is unavailable, fall back to plain git and say gx could not be used.
2. Check context with `git rev-parse --show-toplevel`, `git branch --show-current`, and `gx workspace list`.
3. Choose a short, branch-safe workspace name based on the task, such as `fix-login-timeout` or `feat-pr-dashboard-filter`.
4. Create the workspace with `gx workspace new <name>` or `gx workspace new <name> <base>`. Use `-b <branch>` only when the branch already exists.
5. Enter the printed workspace path if your shell wrapper did not change directories for you. In non-interactive shells, prefer `cd "$(gx workspace new <name>)"` or `cd "$(gx workspace go <query>)"` when running a single shell command.

Useful creation flags:

- `gx workspace new feat/x --from-staged [paths...]` when you have already begun work in the current workspace: it copies the staged file contents into the new workspace and leaves the source untouched, so you can split work onto its own branch.
- `gx workspace new <name> --no-cd` when scripting and you do not want shell navigation (stdout stays empty).
- `gx workspace new <name> --no-hooks` to skip the repo's pre/post-create hooks for one creation.
- `gx workspace new <name> --no-fetch` when offline; gx resolves the base from local refs only.

## Resume or Switch

- Use `gx workspace go [query]` to resume an existing workspace.
- Use `gx workspace list` before creating a new workspace to avoid duplicating active work.
- Use GitHub PR or branch references directly when supplied by the user, for example `gx workspace new '#123'` or `gx workspace go https://github.com/owner/repo/tree/branch-name`.

## Keep Current

- Run `gx workspace update [query]` before substantial work when the branch may be stale.
- Pass an explicit base with `gx workspace update [query] <base>` when the user or repo policy names a base branch.
- Run `gx workspace setup` after switching if expected local files, dependencies, or repo setup are missing.

## Work Safely

- Inspect `gx status` before editing and before handing off.
- Keep unrelated work out of the workspace. If unrelated local changes are present, preserve them and ask before moving or stashing them.
- Do not remove a workspace with uncommitted changes unless the user explicitly requests it.
- Use `gx workspace remove <name>` only after the task is committed, pushed, abandoned by request, or otherwise clearly safe to discard.
- Lock long-running or important work with `gx workspace lock <query>` so cleanup and `git worktree prune` skip it; clear it later with `gx workspace unlock <query>`. Protect a branch from cleanup with `gx workspace protect <branch>`.

## Clean Up

- Prefer gx for cleanup requests instead of raw `git worktree remove`/`prune` or `git branch -d`.
- Use `gx workspace clean` for the interactive multi-section picker (workspaces to remove plus orphan branches to delete), or `gx workspace clean --auto` to remove every workspace that passes the safety checks after one confirmation.
- Always preview with `gx workspace clean --dry-run` (or `gx workspace prune --dry-run`) before deleting, and report what would be removed.
- `gx workspace prune` drops stale worktree metadata and deletes safe orphan branches; add `--no-branches` to touch only metadata.
- Cleanup never removes the main worktree, the current worktree, locked worktrees, or protected branches, and by default skips workspaces with uncommitted changes, untracked files, unpushed commits, or no upstream. Do not pass `--force` unless the user explicitly accepts discarding those changes.

## Handoff

- Report the workspace name, branch, current status, and validation commands run.
- Include the exact command needed to return to the workspace, usually `gx workspace go <name>`.
