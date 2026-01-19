# GX - Smart Git CLI

GX is a smart Git CLI that provides a streamlined interface for common Git operations. It offers interactive pickers, fuzzy matching, and intuitive commands for checkout, status, staging, committing, pushing, stashing, and viewing log history.

## Installation

**Homebrew:**
```bash
brew tap reckerp/tap
brew install gx
```

**Source:**
```bash
cargo install --path .
```

## Development

### Building

```bash
# Build in debug mode
cargo build

# Run the CLI
cargo run -- <arguments>
```


## Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `checkout` | `co`, `switch` | Checkout/Switch a branch/commit/tag |
| `status` | `s` | Show repository status |
| `add` | `a` | Stage files for commit |
| `commit` | `c` | Create a commit |
| `push` | `p` | Push commits to remote |
| `stash` | `st` | Stash changes |
| `log` | `l` | View commit history |
| `setup` | - | Generate shell aliases from config |

### Checkout

Switch to a branch, commit, or tag.

```bash
gx checkout <query>
gx co <query>
gx switch <query>
```

**Arguments:**
- `query` (optional): Branch/commit/tag to checkout (supports fuzzy matching)

### Status

Show the current repository status with an interactive TUI.

```bash
gx status
gx s
```

### Add

Stage files for commit.

```bash
gx add                    # Stage all files
gx add <paths...>         # Stage specific files
gx add -i                 # Interactive mode - select files to stage
gx a -i
```

**Flags:**
- `-i`, `--interactive`: Select files interactively

### Commit

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

**Flags:**
- `-m`, `--message`: Commit message
- `--amend`: Amend the previous commit
- `--no-edit`: Use existing commit message without editing
- `--ai`: Generate commit message using AI

### Push

Push commits to the remote repository.

```bash
gx push
gx p
gx push --force
gx push --force_dangerously
```

**Flags:**
- `-f`, `--force`: Force push with lease (safer)
- `--force_dangerously`: Force push without lease (dangerous)

### Stash

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

**Stash Flags:**
- `-m`, `--message`: Stash message (push)
- `-u`, `--untracked`: Include untracked files (push)

### Log

View commit history.

```bash
gx log
gx l
gx log -n 10
gx log --limit 10
```

**Flags:**
- `-n`, `--limit`: Maximum number of commits to show

### Setup

Generate shell aliases from configuration.

```bash
gx setup
```

### External

Pass-through to git for unrecognized commands.

```bash
gx git <command>
gx remote -v
```

## Configuration

GX uses a configuration file stored at `~/.config/gx/config.toml`. It allows for easy aliasing and customizations. Add `eval "$(gx setup)"` to your shell configuration to load the aliases you configured in the config file. Furthermore, you can change the [opencode](https://opencode.ai) model which is used for the AI commit message generation.

## License

MIT
