# GX - Smart Git CLI

GX is a smart Git CLI that provides a streamlined interface for common Git operations. It offers interactive pickers, fuzzy matching, and intuitive commands for checkout, status, staging, committing, pushing, stashing, and viewing log history.

## Installation

**Homebrew:**

```bash
brew install reckerp/tap/gx
```

**Source:**

```bash
cargo install --path .
```

## Agent Skills

GX ships agent skills in the top-level `skills/` directory. They are designed for autonomous workflows that use gx as the guide rail for workspaces and repo onboarding.

Included skills:

- `gx-workspace-workflow` - automatically use gx workspaces for new autonomous coding tasks and for any `git worktree` request
- `gx-onboarding-workflow` - configure repeatable workspace setup for autonomous agents

Install them with the Vercel skills CLI:

```bash
# List skills available from this repo
npx skills add reckerp/gx --list

# Install the main workspace skill globally
npx skills add reckerp/gx --skill gx-workspace-workflow --global

# Install both gx skills globally
npx skills add reckerp/gx --skill gx-workspace-workflow --skill gx-onboarding-workflow --global
```

The CLI detects supported agents automatically. To target one explicitly, add `--agent opencode`, `--agent claude-code`, `--agent cursor`, or another supported agent. Omit `--global` to install into the current project instead of the user-level agent skills directory.

When developing this repo locally:

```bash
npx skills add . --list
npx skills add . --skill gx-workspace-workflow --global
```

You can also use a skill once without installing it:

```bash
npx skills use reckerp/gx --skill gx-workspace-workflow
```

## Development

### Building

```bash
# Build in debug mode
cargo build

# Run the CLI
cargo run -- <arguments>
```

## Commands

| Command    | Aliases        | Description                         |
| ---------- | -------------- | ----------------------------------- |
| `checkout` | `co`, `switch` | Checkout/Switch a branch/commit/tag |
| `status`   | `s`            | Show repository status              |
| `add`      | `a`            | Stage files for commit              |
| `commit`   | `c`            | Create a commit                     |
| `push`     | `p`            | Push commits to remote              |
| `stash`    | `st`           | Stash changes                       |
| `log`      | `l`            | View commit history                 |
| `workspace`| `ws`           | Manage workspaces (git worktrees)   |
| `pr`       | `prs`, `pullrequest`, `pullrequests` | Dashboard of your open pull requests|
| `onboarding`| `onboard`    | Configure repo-specific setup       |
| `setup`    | -              | Generate shell aliases, cd wrapper, and completions |

### Checkout

Switch to a branch, commit, or tag.

```bash
gx checkout <query>
gx co <query>
gx switch <query>

# GitHub references resolve to a branch in the current repo, then check it out:
gx checkout https://github.com/<owner>/<repo>/pull/13   # PR -> its head branch
gx checkout https://github.com/<owner>/<repo>/tree/<branch>
gx checkout '#13'                                        # shorthand for a PR (quote it)
```

**Arguments:**

- `query` (optional): Branch/commit/tag to checkout (supports fuzzy matching)

**GitHub references:** in place of a query you can pass a GitHub pull-request URL, a branch (`/tree/...`) URL, or the `#<number>` PR shorthand. gx verifies the reference belongs to this repository's `origin` remote (erroring otherwise), resolves pull requests to their head branch via the GitHub CLI (`gh`), and checks it out. Pull requests opened from a fork are not supported. This also works for `gx workspace new` (the workspace is named after the resolved branch) and `gx workspace go`.

### Status

Show the current repository status with an interactive TUI.

```bash
gx status
gx s
```

### Add

Stage files for commit.

```bash
gx add                    # Stage all files
gx add <paths...>         # Stage specific files
gx add -i                 # Interactive mode - select files to stage
gx a -i
```

**Flags:**

- `-i`, `--interactive`: Select files interactively

### Commit

Create a new commit.

```bash
gx commit                 # Opens editor for message
gx commit "message"       # Use provided message
gx commit -m "message"
gx c -m "message"
gx commit --amend         # Amend previous commit
gx commit --ai            # Generate commit message using AI
gx commit --no-edit       # Amend without editing message
```

**Flags:**

- `-m`, `--message`: Commit message
- `--amend`: Amend the previous commit
- `--no-edit`: Use existing commit message without editing
- `--ai`: Generate commit message using AI

### Push

Push commits to the remote repository.

```bash
gx push
gx p
gx push --force
gx push --force-dangerously
```

**Flags:**

- `-f`, `--force`: Force push with lease (safer)
- `--force-dangerously`: Force push without lease (dangerous)

### Stash

Stash changes with various subcommands.

```bash
gx stash                 # Interactive stash picker
gx st
gx stash push            # Push stash (default)
gx stash push -m "msg"   # Push stash with message
gx stash push -u         # Include untracked files
gx stash list            # List all stashes
gx stash pop             # Apply and remove latest stash
gx stash pop 0           # Apply and remove specific stash
gx stash apply           # Apply without removing
gx stash drop            # Drop latest stash
gx stash drop 0          # Drop specific stash
gx stash clear           # Remove all stashes
gx stash show            # Show diff of latest stash
gx stash show 0          # Show diff of specific stash
gx stash branch <name>   # Create branch from stash
```

**Stash Flags:**

- `-m`, `--message`: Stash message (push)
- `-u`, `--untracked`: Include untracked files (push)

### Log

View commit history.

```bash
gx log
gx l
gx log -n 10
gx log --limit 10
```

**Flags:**

- `-n`, `--limit`: Maximum number of commits to show

### Workspace

Manage workspaces (git worktrees): isolated checkouts of the same repository, each on its own branch. By default workspaces live in `~/gx/workspaces/<repo>/<name>`.

```bash
gx workspace               # Interactive workspace picker (TUI)
gx ws                      # Same, shorter

gx workspace new <name>            # Create workspace + branch <name>, run setup
gx workspace new <name> <base>     # Create the new branch from <base>
gx workspace new <name> -b <branch> # Check out an existing/specific branch
# Origin is fetched first, then: if <name> matches a remote branch
# (e.g. origin/<name>), the new branch is created from it and set up to
# track it. Otherwise the new branch is created from origin's default
# branch (e.g. origin/main).
# Names may contain '/' (e.g. feat/expose-rationale); the branch keeps the
# '/' while the workspace directory uses '-' (feat-expose-rationale).
gx workspace new <github-url> # Resolve a PR/branch URL (or '#13') to a branch
                              # and create a workspace named after that branch
gx workspace new <name> --no-setup # Skip copying setup files and setup script
gx workspace new <name> --no-cd    # Create the workspace but stay put (no shell
                                   # navigation; stdout stays empty for scripts)
gx workspace new <name> --no-fetch # Offline: resolve the base from local refs
                                   # only (skips fetching origin). If the base
                                   # can't be resolved locally, gx says so and
                                   # suggests retrying without --no-fetch.
gx workspace new feat/x --from-staged          # Extract work staged in the
                                               # current workspace: copy the
                                               # staged file contents into the
                                               # new one (the source is left
                                               # untouched)
gx workspace new feat/x --from-staged a.rs b.rs # ...limited to specific paths.
                                               # Staged deletions are skipped
                                               # (no content to copy).
gx workspace new <name> --detach   # Detached HEAD instead of a new branch
                                   # (mirrors 'git worktree add --detach'; pass
                                   # a <base> to detach at a specific commit)
gx workspace new <name> --track    # Set the base's remote branch as the new
                                   # branch's upstream (mirrors git tracking)
gx workspace new <name> --no-hooks # Skip the repo's pre/post-create hooks for
                                   # this creation (see Shared Workspace
                                   # Configuration)
# If a branch name collides with an existing one in git's ref namespace
# (e.g. 'foo/bar' when branch 'foo' exists), gx explains the conflict instead
# of letting 'git worktree add' fail cryptically. If the target path is already
# a worktree on a clean, different branch, gx safely switches it (or navigates
# to wherever the branch is already checked out).

gx workspace go [query]    # Switch to a workspace (fuzzy match, picker if omitted)
gx workspace go <github-url>       # Switch to the workspace on a PR/branch's branch
gx workspace list          # List all workspaces
gx workspace update [query]        # Fetch origin and rebase the workspace's branch
                                   # onto origin's default branch (current workspace
                                   # if no query)
gx workspace update [query] <base> # Rebase onto <base> instead
gx workspace remove [query] # Remove a workspace (asks for confirmation).
                            # Removing the workspace you are in moves you
                            # to the main workspace first.
gx workspace remove <name> --force # Remove even with uncommitted changes
gx workspace remove <name> --delete-branch # Also delete the local branch
gx workspace setup         # Re-run setup: copy files, then run setup script

gx workspace sync          # Copy configured setup files from main into the
                           # current workspace (manual copy tool)
gx workspace sync <target> # Copy into <target> (workspace name, branch, fuzzy
                           # query, or absolute path) instead of the current one
gx workspace sync <target> .env config/local.toml # Copy explicit paths
gx workspace sync <target> --from staging .env.local # Copy from another source
gx workspace sync <target> --dry-run               # Print what would be copied

gx workspace root          # Print the main worktree root, e.g. cd "$(gx workspace root)"
gx workspace move <query> <new-path> # Move a workspace to a new path (refuses the
                                     # main worktree and existing destinations)
gx workspace lock <query> [--reason <reason>]  # Lock a workspace so cleanup and
                                               # 'git worktree prune' skip it
gx workspace unlock <query> # Clear a workspace lock
gx workspace repair [query] # Repair worktree admin files after a move (all if omitted)

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

**`setup` vs `sync`:** `gx workspace setup` applies the configured policy (copy files plus the setup script) for the current workspace. `gx workspace sync` is the manual copy tool: it copies arbitrary paths (defaulting to the configured `copy_files`) from a source workspace (defaulting to the main worktree) into a target workspace (defaulting to the current one). Directories are copied recursively, parent directories are created as needed, and missing source paths are reported without aborting the rest of the sync.

**Interactive TUI** (`gx workspace`): fuzzy search across workspace names and branches, with `enter` to switch and `ctrl+n` to create a workspace named after the current query. The workspace list supports multi-select: `space` toggles a workspace, `ctrl+a` toggles all visible workspaces, and `ctrl+u` clears selections. When GitHub CLI (`gh`) is available, workspace rows show PR badges for open, draft, merged, and closed pull requests. Use `ctrl+d` to remove selected workspaces, or `ctrl+b` to remove them and delete their local branches after an inline confirmation. Bulk actions include `ctrl+r` to update/rebase selected workspaces and `ctrl+t` to re-copy setup files. Press `?` for the full help screen.

**Changing directories:** a child process can't change your shell's directory, so `cd`-on-switch is handled by the shell wrapper emitted by `gx setup`. With `eval "$(gx setup)"` in your shell config, `gx workspace go`, `gx workspace new`, and the TUI will land you directly in the workspace. Without it, the workspace path is printed so you can `cd "$(gx workspace go <query>)"` yourself.

**Setup files:** files like `.env` are usually gitignored, so a fresh worktree doesn't have them. When creating a workspace, gx copies the files configured in `workspace.copy_files` and the current repo's onboarding config from the main worktree into the new one (missing files are skipped), then runs the repo-specific setup script when one is configured. The setup script runs from the workspace root. If it fails, gx warns and still switches into the workspace. See [Workspace Configuration](#workspace-configuration) and [Repo Onboarding](#repo-onboarding).

**Cleanup safety:** `gx workspace clean` and `gx workspace prune` never remove the main worktree, the current worktree, locked worktrees, or protected branches. By default they also skip workspaces with uncommitted changes, untracked files, unpushed commits, or a branch that was never pushed (no upstream). `--force` relaxes only those content checks — the structural protections always hold. Protect a branch you want to keep with `gx workspace protect <branch>` (stored in `[workspace] protected_branches`), and lock an in-progress workspace with `gx workspace lock <query>` so cleanup and `git worktree prune` skip it. Use `--dry-run` to preview either command before it deletes anything.

### Pull Requests

An interactive dashboard of your open pull requests, grouped by review **state**
and by **repository**, with inline quick actions. Requires the GitHub CLI (`gh`)
installed and authenticated (`gh auth login`).

```bash
gx pr                      # Interactive PR dashboard (TUI)
gx prs                     # Same (aliases: pullrequest, pullrequests)
gx pr list                 # Non-interactive grouped listing (for non-TTY / piping)
```

The dashboard shows both PRs **you authored** and PRs where **review is requested
of you** (a "Needs your review" section), categorized into: Needs your review,
Waiting for review, Ready to merge, Changes requested, Drafts. PR status (review
decision, merge blockers, check rollup, requested reviewers) streams in the
background so the list renders immediately and resolves as `gh pr view` lands.

**Scope** defaults to the current repository when you run it inside one,
otherwise to all repositories `gh` can see. Press `ctrl+s` to cycle scope between
the current repo, your configured orgs (see configuration), and global.

**Quick actions** on the highlighted PR:

- `enter` / `ctrl+o` — open in your browser
- `ctrl+y` — copy the PR URL
- `ctrl+g` — merge (with a confirmation showing the target and method)
- `ctrl+d` — mark a draft ready for review
- `ctrl+v` — suggest reviewers (deterministic from CODEOWNERS + commit history,
  falling back to your configured AI agent when the signal is thin)
- `ctrl+w` — open the PR's branch in a workspace
- `ctrl+t` — troubleshoot: open the PR in a workspace and launch your AI agent to
  investigate
- `ctrl+r` refresh, `ctrl+s` switch scope, `?` help, `esc` quit

`ctrl+w` and `ctrl+t` operate on local worktrees, so they are enabled only for
PRs in the repository you launched `gx` from; PRs in other repositories are
marked with `⧉` and those actions are disabled. Fork PRs cannot be opened in a
workspace. The troubleshoot action treats the PR's branch contents as untrusted
and asks for confirmation before launching the agent against a PR you did not
author. Like `gx workspace`, the workspace actions rely on the `gx setup` shell
wrapper to `cd` you into the workspace.

### Repo Onboarding

Configure setup that belongs to the current repository:

```bash
gx onboarding
gx onboard
```

The onboarding TUI lets you select repo files/directories to copy into each new workspace, then asks **where** the configuration should be saved:

- **Shared repo config** (`.gx/workspace.toml`) is committable, so the whole team inherits the same workspace defaults. gx also writes a `.gx/.gitignore` (keeping the local override and transient state out of version control) and can optionally create a git-ignored `.gx/workspace.local.toml` for machine-specific overrides. A shared setup script is saved as `.gx/setup-workspace.sh`.
- **Personal config** stays on this machine under `~/.config/gx/repos/<repo>/` — good for secrets and local-only scripts. It is shared by all git worktrees from the same repository, matching Git's worktree behavior, but never committed. A personal setup script is saved as `setup.sh` there.

If you choose to define a setup script, gx creates it and opens it with `$VISUAL`, `$EDITOR`, or `vi`. Setup scripts are executed with the new workspace root as the current directory, so they can contain commands like:

```bash
npm install
npx vercel link
```

See [Shared Workspace Configuration](#shared-workspace-configuration) for the full `.gx/workspace.toml` schema, including pre/post-create hooks.

### Setup

Generate shell integration: aliases, the workspace `cd` wrapper, and shell
completions. Supports zsh, bash, and fish.

```bash
gx setup                      # auto-detects your shell from $SHELL
gx setup --shell zsh          # force a specific shell (zsh|bash|fish)
gx setup --shell bash
gx setup --shell fish
gx setup --completions zsh    # emit only the static completion script
gx setup --name gx-dev --command /path/to/gx   # custom wrapper name/binary
```

The script emits the aliases from your config, the shell wrapper used for the
workspace `cd` integration, and a completion hookup. The zsh wrapper uses
`noglob` so branch-name arguments containing glob characters (e.g.
`gx workspace remove feat/*` or `gx checkout users/[id]`) are passed through
literally instead of being expanded by the shell.

Completion is generated via `clap_complete` for command and flag completion,
plus dynamic helpers for workspace names, branch names, remote branch names,
and stash refs.

`--name`/`--command` let you install integration for a custom wrapper name
pointing at a specific binary — useful when developing `gx` locally while
keeping the installed release available as `gx`:

```bash
eval "$(gx setup --name gx-dev --command /Users/me/dev/gx/target/debug/gx)"
```

### External

Pass-through to git for unrecognized commands.

```bash
gx git <command>
gx remote -v
```

## Configuration

GX uses a configuration file stored at `~/.config/gx/config.toml`. It allows for easy aliasing and customizations. Add `eval "$(gx setup)"` to your shell configuration to load the aliases you configured in the config file.

### AI Configuration

You can configure the AI agent and model used for AI-generated commit messages:

```toml
[ai]
agent = "opencode"  # Options: "opencode" or "claude"
model = "opencode/big-pickle"  # Model to use
```

For Claude, the default model you should use is "haiku". You can configure the agent and model to your preference.

### Workspace Configuration

```toml
[workspace]
# Where workspaces are created. "{repo}" is replaced with the repository
# directory name. Supports "~" for the home directory and absolute paths;
# relative paths are resolved against the main worktree root.
root = "~/gx/workspaces/{repo}"

# Files copied from the main worktree into new workspaces.
# Paths are relative to the repo root. "*" / "?" match within one path
# component; "**" matches zero or more path components. Directories are
# copied recursively, missing entries are skipped.
copy_files = [".env"]

# Branches cleanup must never remove, on top of the always-protected set
# (the default branch, "main", "master", the current branch, and any branch
# checked out in a worktree). Managed by `gx workspace protect`/`unprotect`.
protected_branches = []

[workspace.clean]
# A workspace counts as "stale" once it is at least this many days old.
# Only consulted by `gx workspace clean --auto --use-threshold`.
threshold_days = 7

# When true, a bare `gx workspace clean` behaves like `--auto`.
auto = false
```

Example with more setup files:

```toml
[workspace]
copy_files = [".env*", "**/.env.local", "config/local.toml", ".vscode"]
```

### Shared Workspace Configuration

Beyond the global `~/.config/gx/config.toml`, a repository can ship a committable workspace policy in `.gx/workspace.toml` (created by `gx onboarding`). It is shared by every worktree of the repo and lets a team standardize how new workspaces are set up. A git-ignored `.gx/workspace.local.toml` can override it per machine.

```toml
# .gx/workspace.toml
version = 1

[workspace]
# Repo-relative globs copied into each new workspace. Unioned with the global
# config and personal profile copy_files (additive, not a replacement).
copy_files = [".env.example", "config/local.example.toml"]

# Setup script run after creation, resolved against the repo root.
setup_script = ".gx/setup-workspace.sh"

[workspace.hooks]
# Commands run before the worktree is created. A non-zero exit aborts creation.
pre_create = ["test -f package.json"]
# Commands run after the worktree exists. A failure only warns.
post_create = ["pnpm install"]
```

Hooks run via `sh -c` from the new workspace root and can use the `{workspace}`, `{workspace_path}`, `{main_root}`, and `{branch}` placeholders (also exported as `GX_WORKSPACE`, `GX_WORKSPACE_PATH`, `GX_MAIN_ROOT`, and `GX_BRANCH`). Skip them for a single creation with `gx workspace new <name> --no-hooks`.

Configuration layers merge lowest-to-highest: built-in defaults, the global config, the personal repo profile, `.gx/workspace.toml`, then `.gx/workspace.local.toml`, with CLI flags applied last. `copy_files` is additive across layers; other fields are replaced by the highest layer that sets them.

### PR Dashboard Configuration

```toml
[pr]
# Orgs offered in the dashboard's "org" scope (ctrl+s cycles through scopes).
# Each entry becomes a `gh search --owner <org>` qualifier. Empty by default,
# which omits the org scope from the cycle.
orgs = []

# Default merge method used by the merge action: "squash", "merge", or "rebase".
merge_method = "squash"

# Whether reviewer suggestion falls back to the configured AI agent when the
# deterministic (CODEOWNERS + commit history) signal is thin.
reviewer_ai_fallback = true
```

Example scoping the org filter to your org:

```toml
[pr]
orgs = ["dash0hq"]
```

## License

MIT
