# GX Documentation

GX is a smart Git CLI with interactive pickers, a workspace (git worktree)
manager, a pull-request dashboard, and shell integration.

New to gx? Start with **[Getting Started](getting-started.md)**.

## Contents

| Guide | What's inside |
| --- | --- |
| [Getting Started](getting-started.md) | Installation, prerequisites, shell integration, and a first workflow. |
| [Core Commands](commands.md) | Everyday Git, made interactive: `checkout`, `status`, `add`, `commit`, `push`, `stash`, `log`, and git pass-through. |
| [Workspaces](workspaces.md) | Manage git worktrees: create, switch, update, sync setup files, and clean up stale workspaces and branches. |
| [Pull Requests](pull-requests.md) | The interactive PR dashboard: review-state grouping, quick actions, reviewer suggestions. |
| [Repo Onboarding](onboarding.md) | Make new workspaces reproducible with shared (`.gx/workspace.toml`) or personal setup, including pre/post-create hooks. |
| [Shell Integration](shell-integration.md) | `gx setup`: aliases, the `cd`-on-switch wrapper, and completions for zsh, bash, and fish. |
| [Configuration](configuration.md) | The `~/.config/gx/config.toml` reference: AI, workspace, and PR-dashboard settings. |
| [Agent Skills](skills.md) | Ship gx as a guide rail for autonomous AI coding agents. |

## Command reference at a glance

| Command | Aliases | Documentation |
| --- | --- | --- |
| `checkout` | `co`, `switch` | [Core Commands → Checkout](commands.md#checkout) |
| `status` | `s` | [Core Commands → Status](commands.md#status) |
| `add` | `a` | [Core Commands → Add](commands.md#add) |
| `commit` | `c` | [Core Commands → Commit](commands.md#commit) |
| `push` | `p` | [Core Commands → Push](commands.md#push) |
| `stash` | `st` | [Core Commands → Stash](commands.md#stash) |
| `log` | `l` | [Core Commands → Log](commands.md#log) |
| `workspace` | `ws` | [Workspaces](workspaces.md) |
| `pr` | `prs`, `pullrequest`, `pullrequests` | [Pull Requests](pull-requests.md) |
| `onboarding` | `onboard` | [Repo Onboarding](onboarding.md) |
| `setup` | — | [Shell Integration](shell-integration.md) |

---

[← Back to the project README](../README.md)
