# Repo Onboarding

[← Docs index](README.md)

Onboarding makes every new [workspace](workspaces.md) start from the same local
baseline — the gitignored files it needs (like `.env`) and any setup commands
(installing dependencies, linking services, and so on).

- [Run onboarding](#run-onboarding)
- [Shared vs. personal config](#shared-vs-personal-config)
- [Shared workspace configuration](#shared-workspace-configuration)
- [Hooks](#hooks)
- [Configuration layering](#configuration-layering)

## Run onboarding

```bash
gx onboarding
gx onboard
```

The onboarding TUI lets you select repo files/directories to copy into each new
workspace, then asks **where** the configuration should be saved.

## Shared vs. personal config

- **Shared repo config** (`.gx/workspace.toml`) is committable, so the whole team
  inherits the same workspace defaults. gx also writes a `.gx/.gitignore`
  (keeping the local override and transient state out of version control) and can
  optionally create a git-ignored `.gx/workspace.local.toml` for machine-specific
  overrides. A shared setup script is saved as `.gx/setup-workspace.sh`.
- **Personal config** stays on this machine under `~/.config/gx/repos/<repo>/` —
  good for secrets and local-only scripts. It is shared by all git worktrees from
  the same repository, matching Git's worktree behavior, but never committed. A
  personal setup script is saved as `setup.sh` there.

If you choose to define a setup script, gx creates it and opens it with
`$VISUAL`, `$EDITOR`, or `vi`. Setup scripts are executed with the new workspace
root as the current directory, so they can contain commands like:

```bash
npm install
npx vercel link
```

> Keep secrets out of tracked files and out of shared `.gx/workspace.toml`. Copy
> secrets through personal config, never through committed shared config.

## Shared workspace configuration

Beyond the global [`~/.config/gx/config.toml`](configuration.md), a repository
can ship a committable workspace policy in `.gx/workspace.toml` (created by
`gx onboarding`). It is shared by every worktree of the repo and lets a team
standardize how new workspaces are set up. A git-ignored
`.gx/workspace.local.toml` can override it per machine.

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

## Hooks

Hooks let you run commands around workspace creation — useful when setup must
happen before the worktree exists (validation) or right after (installing
dependencies).

- `pre_create` runs **before** the worktree is created. A non-zero exit **aborts**
  creation.
- `post_create` runs **after** the worktree exists. A failure only **warns** and
  keeps the workspace.

Hooks run via `sh -c` from the new workspace root and can use the `{workspace}`,
`{workspace_path}`, `{main_root}`, and `{branch}` placeholders (also exported as
the `GX_WORKSPACE`, `GX_WORKSPACE_PATH`, `GX_MAIN_ROOT`, and `GX_BRANCH`
environment variables). Skip them for a single creation with
[`gx workspace new <name> --no-hooks`](workspaces.md#creating-a-workspace).

## Configuration layering

Configuration layers merge lowest-to-highest:

1. built-in defaults
2. the global config (`~/.config/gx/config.toml`)
3. the personal repo profile (`~/.config/gx/repos/<repo>/`)
4. shared `.gx/workspace.toml`
5. local `.gx/workspace.local.toml`

CLI flags are applied last. `copy_files` is **additive** across layers; other
fields are **replaced** by the highest layer that sets them.

---

[← Pull Requests](pull-requests.md) · [Docs index](README.md) · [Shell Integration →](shell-integration.md)
