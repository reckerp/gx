---
name: gx-onboarding-workflow
description: Use gx onboarding and workspace setup to make autonomous workspaces reproducible; trigger this when a repo needs setup files, env files, install commands, Vercel linking, bootstrap scripts, pre/post-create hooks, shared repo workspace config (.gx/workspace.toml), or when gx workspace setup/new/sync leaves missing local configuration.
---

# GX Onboarding Workflow

Use gx onboarding to make each new workspace start from the same local baseline. Treat copied files and setup scripts as local developer configuration, especially when secrets or machine-specific paths are involved.

## Diagnose

1. Verify gx is available with `command -v gx`.
2. Run `gx workspace setup` in the current workspace when it is missing local files or dependencies, or `gx workspace sync [target] [paths...]` to copy specific files in from another workspace (defaults: source = main worktree, target = current). Add `--dry-run` to preview before writing.
3. Inspect repo setup docs and package metadata before changing onboarding. Prefer existing setup commands over inventing new ones.
4. Use `gx status` to ensure onboarding changes do not accidentally stage local secrets.

## Configure

- Use `gx onboarding` when a human can drive the interactive picker. It selects files/directories to copy and then asks **where** the config should live.
- Choose the save location deliberately:
  - **Shared repo config** (`.gx/workspace.toml`) is committable and gives the whole team the same workspace defaults (copy files, setup script, hooks). Use it for non-secret, repo-wide setup. gx also writes a `.gx/.gitignore` and can add a git-ignored `.gx/workspace.local.toml` for machine-specific overrides.
  - **Personal config** (`~/.config/gx/repos/<repo>/`) stays on this machine and is never committed. Use it for secrets and local-only scripts.
- Add copy files for gitignored but necessary local state such as `.env`, `.env.local`, `.vercel/`, `config/local.*`, or editor settings only when appropriate for the repo.
- Put repeatable commands in the setup script. It runs from the new workspace root.
- Keep secrets out of tracked files and out of shared `.gx/workspace.toml`. Copy secrets through personal config, never committed shared config.

## Setup Script Patterns

Choose commands that match the repo:

```bash
npm install
npx vercel link
```

```bash
pnpm install
pnpm build
```

```bash
cargo fetch
cargo build
```

Use guarded commands when optional tools may be missing:

```bash
command -v direnv >/dev/null 2>&1 && direnv allow || true
```

## Hooks

Shared repo config (`.gx/workspace.toml`) can run commands around workspace creation, which is useful when setup must happen before or after the worktree exists:

```toml
[workspace.hooks]
pre_create = ["test -f package.json"]   # non-zero exit aborts creation
post_create = ["pnpm install"]          # failure only warns
```

Hooks run via `sh -c` from the new workspace root and can use `{workspace}`, `{workspace_path}`, `{main_root}`, and `{branch}` (also exported as `GX_WORKSPACE`, `GX_WORKSPACE_PATH`, `GX_MAIN_ROOT`, and `GX_BRANCH`). Skip them for a single creation with `gx workspace new <name> --no-hooks`.

## Validate

- Re-run `gx workspace setup` after configuring onboarding.
- Create a disposable workspace for a full smoke test only when the user approves the extra branch/worktree.
- Report copied-file patterns, setup commands, validation results, and any commands the user still needs to run manually.
