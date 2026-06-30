# GX — Smart Git CLI

GX is a smart Git CLI that wraps everyday Git operations in fast, interactive
commands. It adds fuzzy-matching pickers, a workspace (git worktree) manager, a
pull-request dashboard, and shell integration — so common workflows take a
keystroke instead of a paragraph.

## Highlights

- **Interactive everything** — TUIs for status, staging, stashing, log, branch
  checkout, workspaces, and pull requests.
- **Workspaces** — create, switch, update, and clean up git worktrees with one
  command, each isolated on its own branch.
- **Pull-request dashboard** — review-state grouping, quick actions, and reviewer
  suggestions, powered by the GitHub CLI.
- **Shell integration** — aliases, `cd`-on-switch, and completions for zsh, bash,
  and fish.
- **Agent skills** — ships skills that let AI coding agents drive gx workspaces
  autonomously.

## Installation

**Homebrew**

```bash
brew install reckerp/tap/gx
```

**From source**

```bash
cargo install --path .
```

## Quick start

```bash
# 1. Wire up aliases, cd-on-switch, and completions (add to your shell config)
eval "$(gx setup)"

# 2. Everyday Git, but interactive
gx status                       # interactive status
gx add -i                       # pick files to stage
gx commit --ai                  # AI-generated commit message
gx push

# 3. Isolate a task in its own workspace (git worktree)
gx workspace new feat/login     # create + switch to a workspace on branch feat/login
gx workspace go                 # fuzzy-pick a workspace to return to

# 4. Manage your pull requests
gx pr                           # interactive PR dashboard
```

> `cd`-on-switch for workspaces requires the shell wrapper from
> `eval "$(gx setup)"`. See [Shell Integration](docs/shell-integration.md).

## Commands

| Command | Aliases | Description |
| --- | --- | --- |
| `checkout` | `co`, `switch` | Checkout/switch a branch, commit, or tag |
| `status` | `s` | Show repository status |
| `add` | `a` | Stage files for commit |
| `commit` | `c` | Create a commit |
| `push` | `p` | Push commits to remote |
| `stash` | `st` | Stash changes |
| `log` | `l` | View commit history |
| `workspace` | `ws` | Manage workspaces (git worktrees) |
| `pr` | `prs`, `pullrequest`, `pullrequests` | Dashboard of your open pull requests |
| `review` | `rev` | TUI diff reviewer with line comments for a coding agent |
| `onboarding` | `onboard` | Configure repo-specific setup |
| `setup` | — | Generate shell aliases, the `cd` wrapper, and completions |

Unrecognized commands pass through to `git` (e.g. `gx remote -v`).

## Documentation

Full documentation lives in [`docs/`](docs/README.md):

- [Getting Started](docs/getting-started.md) — install, shell integration, first workflow
- [Core Commands](docs/commands.md) — checkout, status, add, commit, push, stash, log
- [Workspaces](docs/workspaces.md) — git worktrees: create, switch, update, sync, clean up
- [Pull Requests](docs/pull-requests.md) — the interactive PR dashboard
- [Repo Onboarding](docs/onboarding.md) — shared & personal workspace setup, hooks
- [Shell Integration](docs/shell-integration.md) — aliases, `cd` wrapper, completions
- [Configuration](docs/configuration.md) — `config.toml` reference
- [Agent Skills](docs/skills.md) — using gx with autonomous AI agents

## Development

```bash
cargo build              # debug build
cargo run -- <args>      # run the CLI
cargo test               # run the test suite
```

## License

MIT
