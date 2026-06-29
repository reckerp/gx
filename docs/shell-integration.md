# Shell Integration

[← Docs index](README.md)

`gx setup` generates shell integration: the aliases from your config, the
workspace `cd` wrapper, and shell completions. It supports **zsh**, **bash**, and
**fish**.

Add this to your shell config (`~/.zshrc`, `~/.bashrc`, or
`~/.config/fish/config.fish`):

```bash
eval "$(gx setup)"
```

## Usage

```bash
gx setup                      # auto-detects your shell from $SHELL
gx setup --shell zsh          # force a specific shell (zsh|bash|fish)
gx setup --shell bash
gx setup --shell fish
gx setup --completions zsh    # emit only the static completion script
gx setup --name gx-dev --command /path/to/gx   # custom wrapper name/binary
```

## What it emits

The script emits three things:

1. **Aliases** — the aliases you configured in
   [`config.toml`](configuration.md).
2. **The workspace `cd` wrapper** — a child process can't change your shell's
   directory, so this wrapper is what lets `gx workspace go`/`new` and the
   [workspace picker](workspaces.md#changing-directories) land you directly in the
   workspace. The zsh wrapper uses `noglob` so branch-name arguments containing
   glob characters (e.g. `gx workspace remove feat/*` or `gx checkout users/[id]`)
   are passed through literally instead of being expanded by the shell.
3. **Completions** — generated via `clap_complete` for command and flag
   completion, plus dynamic helpers for workspace names, branch names, remote
   branch names, and stash refs.

## Custom wrapper name (local development)

`--name`/`--command` let you install integration for a custom wrapper name
pointing at a specific binary — useful when developing gx locally while keeping
the installed release available as `gx`:

```bash
eval "$(gx setup --name gx-dev --command /Users/me/dev/gx/target/debug/gx)"
```

---

[← Repo Onboarding](onboarding.md) · [Docs index](README.md) · [Configuration →](configuration.md)
