# gx review

`gx review` opens a terminal UI for reading a diff and leaving line comments,
then copies those comments — wrapped as an instruction prompt — to your
clipboard so you can hand them to a coding agent.

```bash
gx review            # the current branch vs its base (origin's default branch)
gx rev               # alias
gx review --base main
gx review <commit>   # a single commit (<commit>^..<commit>)
gx review A..B       # an explicit commit range
```

## What it shows

- A **side-by-side, syntax-highlighted diff** with word-level emphasis on the
  parts of a line that changed. On narrow terminals it falls back to a unified
  single-column view (configurable; see below).
- A **file sidebar** listing the changed files, with a comment-count badge per
  file.

## Range modes

| Invocation | Diff |
| --- | --- |
| `gx review` | `merge-base(base, HEAD) … HEAD` — what your branch adds over its base |
| `gx review --base <ref>` | same, against an explicit base |
| `gx review <commit>` | a single commit's diff |
| `gx review A..B` / `A...B` | an explicit range |

The base defaults to `origin`'s default branch (falling back to `origin/main`
then `origin/master`).

## Keys

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | move the cursor |
| `Ctrl-d` / `Ctrl-u` | half-page down / up |
| `g` / `G` | top / bottom |
| `]c` / `[c` (or `}` / `{`) | next / previous hunk |
| `Tab` / `Shift-Tab` | next / previous file |
| `h` / `l`, `←` / `→` | scroll horizontally |
| `c` | comment on the current line |
| `V` then `j`/`k` then `c` | comment on a multi-line selection |
| `Enter` | edit the comment under the cursor |
| `D` | delete the comment under the cursor |
| `o` | list orphaned comments (see Persistence) |
| `F` | **finish**: copy the review to the clipboard |
| `X` (twice) | discard the saved review |
| `v` | toggle split / unified |
| `b` | toggle the sidebar |
| `?` | help overlay |
| `q` | quit (the review is saved for next time — it does **not** copy) |

Inside the comment popup: type your note, `Ctrl-s` to save, `Ctrl-e` to compose
in `$EDITOR` (or `$VISUAL`), `Enter` for a newline, `Esc` to cancel.

> **`q` saves but does not copy.** Pressing `q` keeps your comments for next time
> but does *not* put anything on the clipboard. Use `F` to hand the review off.

## Finishing

`F` builds a Markdown blob — a wrapping instruction, then each comment grouped by
file with a snippet of surrounding diff context and your note — and copies it to
the system clipboard. Paste it into your coding agent. (gx sets the clipboard
once; your clipboard manager keeps the history.)

## Persistence

Your review is saved to a temporary location keyed to the repository **and the
branch** (`{temp}/gx-review/…`), so quitting and relaunching resumes it. It is
**never committed**. When the underlying diff has changed between sessions,
comments re-anchor to where their line moved; any that no longer resolve are
collected in an **orphaned** list (`o`) instead of being dropped. `X` (pressed
twice) discards the saved review.

## Configuration

Under `[review]` in the gx config (`gx setup` shows the path):

| Key | Default | Meaning |
| --- | --- | --- |
| `appearance` | `auto` | `auto` detects the terminal background (light/dark) and picks a matching theme + diff palette; `light` / `dark` force it |
| `theme` | *(auto)* | syntect theme name; empty picks `InspiredGitHub` (light) or `base16-ocean.dark` (dark) from `appearance` |
| `side_by_side_min_width` | `120` | below this terminal width, use the unified view |
| `default_mode` | `branch` | default range mode |

The diff adapts to your terminal: on a light background it uses a light syntax
theme with pale add/remove tints; on a dark background, a dark theme with the
darker tints. If auto-detection guesses wrong (some terminals don't answer the
query), set `appearance` explicitly.
