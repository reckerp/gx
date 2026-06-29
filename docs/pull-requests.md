# Pull Requests

[← Docs index](README.md)

An interactive dashboard of your open pull requests, grouped by review **state**
and by **repository**, with inline quick actions.

> **Requires the GitHub CLI (`gh`)** installed and authenticated
> (`gh auth login`).

```bash
gx pr                      # Interactive PR dashboard (TUI)
gx prs                     # Same (aliases: pullrequest, pullrequests)
gx pr list                 # Non-interactive grouped listing (for non-TTY / piping)
```

## What it shows

The dashboard shows both PRs **you authored** and PRs where **review is requested
of you** (a "Needs your review" section), categorized into:

- Needs your review
- Waiting for review
- Ready to merge
- Changes requested
- Drafts

PR status (review decision, merge blockers, check rollup, requested reviewers)
streams in the background, so the list renders immediately and resolves as
`gh pr view` lands.

## Scope

**Scope** defaults to the current repository when you run gx inside one,
otherwise to all repositories `gh` can see. Press `ctrl+s` to cycle scope between
the current repo, your configured orgs (see
[PR dashboard configuration](configuration.md#pr-dashboard-configuration)), and
global.

## Quick actions

Actions operate on the highlighted PR:

- `enter` / `ctrl+o` — open in your browser
- `ctrl+y` — copy the PR URL
- `ctrl+g` — merge (with a confirmation showing the target and method)
- `ctrl+d` — mark a draft ready for review
- `ctrl+v` — suggest reviewers (deterministic from CODEOWNERS + commit history,
  falling back to your configured AI agent when the signal is thin)
- `ctrl+w` — open the PR's branch in a [workspace](workspaces.md)
- `ctrl+t` — troubleshoot: open the PR in a workspace and launch your AI agent to
  investigate
- `ctrl+r` refresh, `ctrl+s` switch scope, `?` help, `esc` quit

`ctrl+w` and `ctrl+t` operate on local worktrees, so they are enabled only for
PRs in the repository you launched gx from; PRs in other repositories are marked
with `⧉` and those actions are disabled. Fork PRs cannot be opened in a
workspace. The troubleshoot action treats the PR's branch contents as untrusted
and asks for confirmation before launching the agent against a PR you did not
author. Like `gx workspace`, the workspace actions rely on the
[`gx setup` shell wrapper](shell-integration.md) to `cd` you into the workspace.

---

[← Workspaces](workspaces.md) · [Docs index](README.md) · [Repo Onboarding →](onboarding.md)
