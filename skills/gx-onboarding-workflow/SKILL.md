---
name: gx-onboarding-workflow
description: Use gx onboarding and workspace setup to make autonomous workspaces reproducible; trigger this when a repo needs setup files, env files, install commands, Vercel linking, bootstrap scripts, or when gx workspace setup/new leaves missing local configuration.
---

# GX Onboarding Workflow

Use gx onboarding to make each new workspace start from the same local baseline. Treat copied files and setup scripts as local developer configuration, especially when secrets or machine-specific paths are involved.

## Diagnose

1. Verify gx is available with `command -v gx`.
2. Run `gx workspace setup` in the current workspace when a workspace is missing local files or dependencies.
3. Inspect repo setup docs and package metadata before changing onboarding. Prefer existing setup commands over inventing new ones.
4. Use `gx status` to ensure onboarding changes do not accidentally stage local secrets.

## Configure

- Use `gx onboarding` when a human can drive the interactive picker.
- Add copy files for gitignored but necessary local state such as `.env`, `.env.local`, `.vercel/`, `config/local.*`, or editor settings only when appropriate for the repo.
- Put repeatable commands in the repo onboarding setup script. It runs from the new workspace root.
- Keep secrets out of tracked files. Copy them locally through gx onboarding rather than committing them.

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

## Validate

- Re-run `gx workspace setup` after configuring onboarding.
- Create a disposable workspace for a full smoke test only when the user approves the extra branch/worktree.
- Report copied-file patterns, setup commands, validation results, and any commands the user still needs to run manually.
