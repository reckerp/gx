# Workspaces

[← Docs index](README.md)

A **workspace** is a [git worktree](https://git-scm.com/docs/git-worktree): an
isolated checkout of the same repository, each on its own branch. Workspaces let
you keep your main checkout stable while you work on (or jump between) several
branches at once — no stashing, no context-switching churn.

By default workspaces live in `~/gx/workspaces/<repo>/<name>` (configurable via
[`[workspace] root`](configuration.md#workspace-configuration)).

- [Quick reference](#quick-reference)
- [Creating a workspace](#creating-a-workspace)
- [Switching, listing, and updating](#switching-listing-and-updating)
- [Removing a workspace](#removing-a-workspace)
- [Setup files: `setup` vs `sync`](#setup-files-setup-vs-sync)
- [Utility commands](#utility-commands)
- [Cleaning up](#cleaning-up)
- [The interactive picker](#the-interactive-picker)
- [Changing directories](#changing-directories)

## Quick reference

```bash
gx workspace               # Interactive workspace picker (TUI)
gx ws                      # Same, shorter
gx workspace new <name>    # Create a workspace + branch and switch to it
gx workspace go [query]    # Switch to a workspace (fuzzy match, picker if omitted)
gx workspace list          # List all workspaces
gx workspace update        # Fetch origin and rebase the branch onto the default
gx workspace remove [query] # Remove a workspace
gx workspace clean         # Interactive cleanup of stale workspaces and branches
```

## Creating a workspace

```bash
gx workspace new <name>            # Create workspace + branch <name>, run setup
gx workspace new <name> <base>     # Create the new branch from <base>
gx workspace new <name> -b <branch> # Check out an existing/specific branch
```

Origin is fetched first, then: if `<name>` matches a remote branch (e.g.
`origin/<name>`), the new branch is created from it and set up to track it.
Otherwise the new branch is created from origin's default branch (e.g.
`origin/main`).

Names may contain `/` (e.g. `feat/expose-rationale`); the branch keeps the `/`
while the workspace directory uses `-` (`feat-expose-rationale`).

**Create from a GitHub reference** — resolve a PR/branch URL (or `#13`) to a
branch and name the workspace after it:

```bash
gx workspace new <github-url>
gx workspace new '#13'
```

**Creation flags**

```bash
gx workspace new <name> --no-setup # Skip copying setup files and the setup script
gx workspace new <name> --no-cd    # Create the workspace but stay put (no shell
                                   # navigation; stdout stays empty for scripts)
gx workspace new <name> --no-fetch # Offline: resolve the base from local refs
                                   # only (skips fetching origin). If the base
                                   # can't be resolved locally, gx says so and
                                   # suggests retrying without --no-fetch.
gx workspace new <name> --no-hooks # Skip the repo's pre/post-create hooks for
                                   # this creation (see Repo Onboarding)
gx workspace new <name> --detach   # Detached HEAD instead of a new branch
                                   # (mirrors 'git worktree add --detach'; pass
                                   # a <base> to detach at a specific commit)
gx workspace new <name> --track    # Set the base's remote branch as the new
                                   # branch's upstream (mirrors git tracking)
```

**Extract work already in progress** — copy staged file contents from the
current workspace into a new one, leaving the source untouched:

```bash
gx workspace new feat/x --from-staged           # all staged files
gx workspace new feat/x --from-staged a.rs b.rs # ...limited to specific paths
                                                # (staged deletions are skipped)
```

> If a branch name collides with an existing one in git's ref namespace (e.g.
> `foo/bar` when branch `foo` exists), gx explains the conflict instead of
> letting `git worktree add` fail cryptically. If the target path is already a
> worktree on a clean, different branch, gx safely switches it (or navigates to
> wherever the branch is already checked out).

## Switching, listing, and updating

```bash
gx workspace go [query]            # Switch to a workspace (fuzzy match, picker if omitted)
gx workspace go <github-url>       # Switch to the workspace on a PR/branch's branch
gx workspace list                  # List all workspaces
gx workspace update [query]        # Fetch origin and rebase the workspace's branch
                                   # onto origin's default branch (current workspace
                                   # if no query)
gx workspace update [query] <base> # Rebase onto <base> instead
```

## Removing a workspace

```bash
gx workspace remove [query]                # Remove a workspace (asks for confirmation)
gx workspace remove <name> --force         # Remove even with uncommitted changes
gx workspace remove <name> --delete-branch # Also delete the local branch
```

Removing the workspace you are currently in moves you to the main workspace
first. For bulk cleanup, see [Cleaning up](#cleaning-up).

## Setup files: `setup` vs `sync`

Files like `.env` are usually gitignored, so a fresh worktree doesn't have them.
When creating a workspace, gx copies the files configured in
[`workspace.copy_files`](configuration.md#workspace-configuration) and the
current repo's onboarding config from the main worktree into the new one
(missing files are skipped), then runs the repo-specific setup script when one is
configured. The setup script runs from the workspace root. If it fails, gx warns
and still switches into the workspace.

```bash
gx workspace setup         # Re-run setup for the current workspace: copy files,
                           # then run the setup script

gx workspace sync          # Copy configured setup files from main into the
                           # current workspace (manual copy tool)
gx workspace sync <target> # Copy into <target> (workspace name, branch, fuzzy
                           # query, or absolute path) instead of the current one
gx workspace sync <target> .env config/local.toml   # Copy explicit paths
gx workspace sync <target> --from staging .env.local # Copy from another source
gx workspace sync <target> --dry-run                 # Print what would be copied
```

- **`gx workspace setup`** applies the configured policy (copy files plus the
  setup script) for the current workspace.
- **`gx workspace sync`** is the manual copy tool: it copies arbitrary paths
  (defaulting to the configured `copy_files`) from a source workspace (defaulting
  to the main worktree) into a target workspace (defaulting to the current one).
  Directories are copied recursively, parent directories are created as needed,
  and missing source paths are reported without aborting the rest of the sync.

See [Repo Onboarding](onboarding.md) to configure which files are copied and what
the setup script does.

## Utility commands

```bash
gx workspace root          # Print the main worktree root, e.g. cd "$(gx workspace root)"
gx workspace move <query> <new-path> # Move a workspace to a new path (refuses the
                                     # main worktree and existing destinations)
gx workspace lock <query> [--reason <reason>]  # Lock a workspace so cleanup and
                                               # 'git worktree prune' skip it
gx workspace unlock <query> # Clear a workspace lock
gx workspace repair [query] # Repair worktree admin files after a move (all if omitted)
```

## Cleaning up

Over time you accumulate finished or abandoned workspaces and orphan branches.
These commands clear them out safely.

```bash
gx workspace clean         # Interactive multi-section cleanup picker: choose
                           # workspaces to remove and orphan branches to delete
gx workspace clean --auto  # Remove every workspace that passes the safety
                           # checks (still asks for one final confirmation).
                           # Also enabled by [workspace.clean] auto = true
gx workspace clean --auto --use-threshold # In --auto mode, only consider
                           # workspaces older than [workspace.clean]
                           # threshold_days (default 7)
gx workspace clean --dry-run # Show what would be removed without deleting
gx workspace clean --force   # Bypass the dirty/untracked/unpushed checks (still
                           # protects main, current, locked, and protected ones)

gx workspace prune         # Prune stale worktree metadata, then delete safe
                           # orphan branches (asks for confirmation)
gx workspace prune --dry-run     # Show what would be pruned/deleted
gx workspace prune --no-branches # Only prune metadata; leave branches alone

gx workspace protect [branch]    # Protect a branch from cleanup (current branch
                                 # if omitted); adds to [workspace] protected_branches
gx workspace unprotect [branch]  # Remove a branch from the protected list
```

**Cleanup safety.** `gx workspace clean` and `gx workspace prune` never remove
the main worktree, the current worktree, locked worktrees, or protected
branches. By default they also skip workspaces with uncommitted changes,
untracked files, unpushed commits, or a branch that was never pushed (no
upstream). `--force` relaxes only those content checks — the structural
protections always hold. Protect a branch you want to keep with
`gx workspace protect <branch>` (stored in
[`[workspace] protected_branches`](configuration.md#workspace-configuration)),
and lock an in-progress workspace with `gx workspace lock <query>` so cleanup and
`git worktree prune` skip it. Use `--dry-run` to preview either command before it
deletes anything.

## The interactive picker

Run `gx workspace` (or `gx ws`) with no subcommand for a fuzzy-searchable TUI
across workspace names and branches:

- `enter` switch to the highlighted workspace
- `ctrl+n` create a workspace named after the current query
- `space` toggle selection, `ctrl+a` toggle all visible, `ctrl+u` clear selection
- `ctrl+d` remove selected workspaces; `ctrl+b` remove them **and** delete their
  local branches (after an inline confirmation)
- `ctrl+r` update/rebase selected workspaces; `ctrl+t` re-copy setup files
- `?` open the full help screen

When the GitHub CLI (`gh`) is available, workspace rows show PR badges for open,
draft, merged, and closed pull requests.

## Changing directories

A child process can't change your shell's directory, so `cd`-on-switch is handled
by the shell wrapper emitted by [`gx setup`](shell-integration.md). With
`eval "$(gx setup)"` in your shell config, `gx workspace go`, `gx workspace new`,
and the TUI land you directly in the workspace. Without it, the workspace path is
printed so you can `cd "$(gx workspace go <query>)"` yourself.

---

[← Core Commands](commands.md) · [Docs index](README.md) · [Pull Requests →](pull-requests.md)
