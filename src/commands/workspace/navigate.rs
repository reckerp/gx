//! Navigating between workspaces: `gx workspace go`, `list`, the interactive
//! picker (the default `gx workspace`), and launching an editor.

use super::{
    NewWorkspaceOptions, WorkspaceError, display_exit_status, load_worktrees, remove_worktrees,
    resolve_target, run_new, setup_worktrees, update_worktrees,
};
use crate::git::{self, worktree::Worktree};
use crate::output;
use crate::ui;
use crate::ui::workspace_picker::WorkspaceAction;
use miette::Result;
use std::io::{self, Write};

/// Resolve a workspace by query (or interactively) and print its path to
/// stdout for the shell wrapper to cd into.
pub fn run_go(query: Option<String>) -> Result<()> {
    let worktrees = load_worktrees()?;

    let target = match query {
        Some(q) => {
            // A GitHub pull-request/branch URL (or '#123') resolves to a branch;
            // switch to the workspace checked out on it.
            let q = match git::github::parse_ref(&q) {
                Some(gh_ref) => git::github::resolve_branch(&gh_ref)?,
                None => q,
            };
            resolve_target(&q, &worktrees)?
        }
        None => {
            let Some(action) = pick_workspace(&worktrees)? else {
                output::cancelled();
                return Ok(());
            };
            match action {
                WorkspaceAction::Go(w) => w,
                WorkspaceAction::Remove {
                    worktrees: worktrees_to_remove,
                    delete_branches,
                    confirmed,
                    dirty_paths,
                } => {
                    return remove_worktrees(
                        &worktrees_to_remove,
                        &worktrees,
                        false,
                        delete_branches,
                        confirmed,
                        &dirty_paths,
                    );
                }
                WorkspaceAction::Update(worktrees_to_update) => {
                    return update_worktrees(&worktrees_to_update, None);
                }
                WorkspaceAction::Setup(worktrees_to_setup) => {
                    return setup_worktrees(&worktrees_to_setup, &worktrees);
                }
                WorkspaceAction::OpenEditor(worktree) => return open_in_editor(&worktree),
                WorkspaceAction::Create { name } => return create_from_picker(name),
            }
        }
    };

    eprintln!("Switching to workspace '{}'", target.name);
    output::nav_path(&target.path);
    Ok(())
}

pub fn run_list() -> Result<()> {
    let worktrees = load_worktrees()?;

    if worktrees.is_empty() {
        eprintln!("No workspaces found");
        return Ok(());
    }

    let name_width = worktrees.iter().map(|w| w.name.len()).max().unwrap_or(0);
    let branch_width = worktrees
        .iter()
        .map(|w| w.branch.as_deref().unwrap_or("(detached)").len())
        .max()
        .unwrap_or(0);

    for w in &worktrees {
        let branch = w.branch.as_deref().unwrap_or("(detached)");
        let mut markers = Vec::new();
        if w.is_main {
            markers.push("main");
        }
        if w.is_current {
            markers.push("current");
        }
        if w.is_locked {
            markers.push("locked");
        }
        let markers = if markers.is_empty() {
            String::new()
        } else {
            format!(" ({})", markers.join(", "))
        };

        println!(
            "{:<name_width$}  [{:<branch_width$}]  {}{}",
            w.name,
            branch,
            w.path.display(),
            markers,
        );
    }

    Ok(())
}

/// Interactive workspace manager (default when no subcommand is given).
pub fn run_interactive() -> Result<()> {
    let worktrees = load_worktrees()?;

    let Some(action) = pick_workspace(&worktrees)? else {
        output::cancelled();
        return Ok(());
    };

    match action {
        WorkspaceAction::Go(w) => {
            eprintln!("Switching to workspace '{}'", w.name);
            output::nav_path(&w.path);
            Ok(())
        }
        WorkspaceAction::Remove {
            worktrees: worktrees_to_remove,
            delete_branches,
            confirmed,
            dirty_paths,
        } => remove_worktrees(
            &worktrees_to_remove,
            &worktrees,
            false,
            delete_branches,
            confirmed,
            &dirty_paths,
        ),
        WorkspaceAction::Update(worktrees_to_update) => {
            update_worktrees(&worktrees_to_update, None)
        }
        WorkspaceAction::Setup(worktrees_to_setup) => {
            setup_worktrees(&worktrees_to_setup, &worktrees)
        }
        WorkspaceAction::OpenEditor(worktree) => open_in_editor(&worktree),
        WorkspaceAction::Create { name } => create_from_picker(name),
    }
}

pub(crate) fn pick_workspace(worktrees: &[Worktree]) -> Result<Option<WorkspaceAction>> {
    let summary_lookup = git::worktree::spawn_summary_lookup(worktrees);
    let pull_requests = git::pull_request::spawn_lookup(worktrees);
    ui::terminal::with_terminal_stderr(|t| {
        ui::workspace_picker::run(t, worktrees, summary_lookup, pull_requests)
    })
    .map_err(|e| WorkspaceError::TuiError(e.to_string()))?
}

pub(crate) fn create_from_picker(name: String) -> Result<()> {
    let name = if name.is_empty() {
        eprint!("Workspace name: ");
        io::stderr().flush().ok();
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| WorkspaceError::TuiError(e.to_string()))?;
        input.trim().to_string()
    } else {
        name
    };

    if name.is_empty() {
        output::cancelled();
        return Ok(());
    }

    run_new(name, NewWorkspaceOptions::default())
}

/// Open a workspace in the user's editor, resolved from `$VISUAL`, then
/// `$EDITOR`, with a platform fallback. The editor runs attached to the
/// terminal and gx waits for it, which is correct for terminal editors
/// (vim, nano, …) and for GUI editors invoked with a blocking flag
/// (e.g. `code --wait`).
pub(crate) fn open_in_editor(worktree: &Worktree) -> Result<()> {
    let editor = editor_command();
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| WorkspaceError::EditorFailed("no editor configured".to_string()))?;
    let args: Vec<&str> = parts.collect();

    eprintln!("Opening '{}' in {}", worktree.name, program);

    let mut command = std::process::Command::new(program);
    command
        .args(&args)
        .arg(&worktree.path)
        .current_dir(&worktree.path);

    // The picker renders to stderr because gx's stdout may be captured by the
    // `gx setup` shell wrapper's `$(...)`. A terminal editor needs a real tty
    // on stdout, so point it at the controlling terminal when ours is not one;
    // stdin and stderr are already the terminal, so they stay inherited.
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        if !io::stdout().is_terminal()
            && let Ok(tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty")
        {
            command.stdout(tty);
        }
    }

    let status = command.status().map_err(|e| {
        WorkspaceError::EditorFailed(format!("could not launch '{}': {}", program, e))
    })?;

    if !status.success() {
        return Err(WorkspaceError::EditorFailed(format!(
            "{} exited with {}",
            program,
            display_exit_status(&status)
        ))
        .into());
    }

    Ok(())
}

/// The editor command to use, from `$VISUAL` then `$EDITOR`, falling back to a
/// platform default. May contain arguments (e.g. "code --wait").
fn editor_command() -> String {
    resolve_editor(std::env::var("VISUAL").ok(), std::env::var("EDITOR").ok())
}

fn resolve_editor(visual: Option<String>, editor: Option<String>) -> String {
    visual
        .filter(|s| !s.trim().is_empty())
        .or_else(|| editor.filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(default_editor)
}

fn default_editor() -> String {
    if cfg!(windows) {
        "notepad".to_string()
    } else {
        "vi".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_editor_prefers_visual_then_editor() {
        assert_eq!(
            resolve_editor(Some("code --wait".to_string()), Some("vim".to_string())),
            "code --wait"
        );
        assert_eq!(resolve_editor(None, Some("vim".to_string())), "vim");
    }

    #[test]
    fn test_resolve_editor_ignores_blank_and_falls_back_to_default() {
        // A blank $VISUAL is skipped in favour of $EDITOR.
        assert_eq!(
            resolve_editor(Some("   ".to_string()), Some("nano".to_string())),
            "nano"
        );
        // Nothing usable set -> platform default.
        assert_eq!(resolve_editor(None, None), default_editor());
        assert_eq!(resolve_editor(Some(String::new()), None), default_editor());
    }
}
