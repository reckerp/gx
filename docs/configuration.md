# Configuration

[← Docs index](README.md)

GX uses a global configuration file at `~/.config/gx/config.toml`. It controls
aliases, the AI agent, workspace defaults, and the PR dashboard. Add
`eval "$(gx setup)"` to your shell config to load the aliases you define here
(see [Shell Integration](shell-integration.md)).

> Looking for **per-repository** workspace setup (`.gx/workspace.toml`, copy
> files, setup scripts, hooks)? That lives with [Repo Onboarding](onboarding.md).

- [AI configuration](#ai-configuration)
- [Workspace configuration](#workspace-configuration)
- [PR dashboard configuration](#pr-dashboard-configuration)

## AI configuration

Configure the AI agent and model used for AI-generated commit messages
([`gx commit --ai`](commands.md#commit)) and reviewer suggestions.

```toml
[ai]
agent = "opencode"  # Options: "opencode" or "claude"
model = "opencode/big-pickle"  # Model to use
```

For Claude, the default model you should use is `"haiku"`. You can configure the
agent and model to your preference.

## Workspace configuration

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

See [Workspaces](workspaces.md) for the commands these settings affect, and
[Repo Onboarding](onboarding.md) for committable, per-repo workspace policy.

## PR dashboard configuration

Settings for the [pull-request dashboard](pull-requests.md).

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

---

[← Shell Integration](shell-integration.md) · [Docs index](README.md) · [Agent Skills →](skills.md)
