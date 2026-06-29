# Agent Skills

[← Docs index](README.md)

GX ships agent skills in the top-level [`skills/`](../skills) directory. They are
designed for autonomous workflows that use gx as the guide rail for
[workspaces](workspaces.md) and [repo onboarding](onboarding.md).

## Included skills

- **`gx-workspace-workflow`** — automatically use gx workspaces for new
  autonomous coding tasks and for any `git worktree` request, including cleanup,
  pruning, and protecting/locking workspaces.
- **`gx-onboarding-workflow`** — configure repeatable workspace setup (copy
  files, setup scripts, and hooks) for autonomous agents.

## Installing

Install them with the Vercel skills CLI:

```bash
# List skills available from this repo
npx skills add reckerp/gx --list

# Install the main workspace skill globally
npx skills add reckerp/gx --skill gx-workspace-workflow --global

# Install both gx skills globally
npx skills add reckerp/gx --skill gx-workspace-workflow --skill gx-onboarding-workflow --global
```

The CLI detects supported agents automatically. To target one explicitly, add
`--agent opencode`, `--agent claude-code`, `--agent cursor`, or another supported
agent. Omit `--global` to install into the current project instead of the
user-level agent skills directory.

## Developing locally

When working in this repo:

```bash
npx skills add . --list
npx skills add . --skill gx-workspace-workflow --global
```

You can also use a skill once without installing it:

```bash
npx skills use reckerp/gx --skill gx-workspace-workflow
```

---

[← Configuration](configuration.md) · [Docs index](README.md)
