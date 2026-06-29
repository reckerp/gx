# Getting Started

[← Docs index](README.md)

## Prerequisites

- **Git** — gx is a layer on top of git.
- **GitHub CLI (`gh`)** — required for the
  [pull-request dashboard](pull-requests.md) and for resolving GitHub URLs in
  [`checkout`](commands.md#checkout) and [`workspace`](workspaces.md) commands.
  Install it and run `gh auth login`.

## Installation

**Homebrew**

```bash
brew install reckerp/tap/gx
```

**From source**

```bash
cargo install --path .
```

## Shell integration

Add this to your shell config (`~/.zshrc`, `~/.bashrc`, or
`~/.config/fish/config.fish`):

```bash
eval "$(gx setup)"
```

This loads your aliases, the wrapper that lets `gx workspace` change your shell's
directory, and shell completions. See [Shell Integration](shell-integration.md)
for details and customization.

> Without the wrapper, gx still works — it just prints the workspace path so you
> can `cd "$(gx workspace go <query>)"` yourself.

## Your first workflow

```bash
# Stage and commit interactively
gx status            # see what changed
gx add -i            # pick files to stage
gx commit --ai       # generate a commit message with AI (or: gx commit -m "...")
gx push

# Start an isolated task in its own workspace (git worktree)
gx workspace new feat/awesome
# ... edit, commit, and push from inside the workspace ...

# Switch back to another workspace later
gx workspace go      # fuzzy-pick, or pass a query: gx workspace go main

# Review and act on your pull requests
gx pr
```

## Where to go next

- [Core Commands](commands.md) — the day-to-day git wrappers.
- [Workspaces](workspaces.md) — get the most out of git worktrees.
- [Repo Onboarding](onboarding.md) — make new workspaces reproducible.
- [Configuration](configuration.md) — customize aliases, AI, and defaults.

---

[Docs index](README.md) · [Core Commands →](commands.md)
