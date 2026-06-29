# Core Commands

[← Docs index](README.md)

Everyday Git operations, made interactive. Every command has short aliases, and
unrecognized commands pass straight through to `git`.

- [Checkout](#checkout)
- [Status](#status)
- [Add](#add)
- [Commit](#commit)
- [Push](#push)
- [Stash](#stash)
- [Log](#log)
- [Git pass-through](#git-pass-through)

## Checkout

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

**Arguments**

- `query` (optional): branch/commit/tag to checkout (supports fuzzy matching).

**GitHub references:** in place of a query you can pass a GitHub pull-request
URL, a branch (`/tree/...`) URL, or the `#<number>` PR shorthand. gx verifies the
reference belongs to this repository's `origin` remote (erroring otherwise),
resolves pull requests to their head branch via the GitHub CLI (`gh`), and
checks it out. Pull requests opened from a fork are not supported. The same
references also work for [`gx workspace new`](workspaces.md#creating-a-workspace)
(the workspace is named after the resolved branch) and `gx workspace go`.

## Status

Show the current repository status with an interactive TUI.

```bash
gx status
gx s
```

## Add

Stage files for commit.

```bash
gx add                    # Stage all files
gx add <paths...>         # Stage specific files
gx add -i                 # Interactive mode - select files to stage
gx a -i
```

**Flags**

- `-i`, `--interactive`: select files interactively.

## Commit

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

**Flags**

- `-m`, `--message`: commit message.
- `--amend`: amend the previous commit.
- `--no-edit`: use the existing commit message without editing.
- `--ai`: generate a commit message using AI (see [AI configuration](configuration.md#ai-configuration)).

## Push

Push commits to the remote repository.

```bash
gx push
gx p
gx push --force
gx push --force-dangerously
```

**Flags**

- `-f`, `--force`: force push with lease (safer).
- `--force-dangerously`: force push without lease (dangerous).

## Stash

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

**Flags**

- `-m`, `--message`: stash message (`push`).
- `-u`, `--untracked`: include untracked files (`push`).

## Log

View commit history.

```bash
gx log
gx l
gx log -n 10
gx log --limit 10
```

**Flags**

- `-n`, `--limit`: maximum number of commits to show.

## Git pass-through

Any command gx doesn't recognize is passed through to `git`, so you can keep
using gx as your everyday git entry point.

```bash
gx git <command>
gx remote -v
```

---

[← Docs index](README.md) · [Workspaces →](workspaces.md)
