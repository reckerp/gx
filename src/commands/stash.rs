use crate::git;
use crate::git::GitError;
use crate::ui;
use crate::ui::stash_picker::StashAction;
use miette::{Diagnostic, Result};
use std::io::{self, Write};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum StashError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::stash::git_error))]
    GitError(#[from] GitError),

    #[error("TUI error: {0}")]
    #[diagnostic(code(gx::stash::tui_error))]
    TuiError(String),

    #[error("Invalid stash reference: {0}")]
    #[diagnostic(
        code(gx::stash::invalid_ref),
        help("Use 'gx stash list' to see available stashes")
    )]
    InvalidRef(String),

    #[error("No stashes found")]
    #[diagnostic(
        code(gx::stash::no_stashes),
        help("Create a stash with 'gx stash' or 'gx stash push'")
    )]
    NoStashes,
}

pub fn run_push(message: Option<String>, include_untracked: bool) -> Result<()> {
    let (staged, unstaged) = git::status::get_status_files().map_err(StashError::GitError)?;

    if staged.is_empty() && unstaged.is_empty() {
        println!("No local changes to save");
        return Ok(());
    }

    let oid =
        git::stash::save(message.as_deref(), include_untracked).map_err(StashError::GitError)?;
    let short_id = &oid.to_string()[..7];

    println!(
        "Saved working directory and index state {}",
        message.as_deref().unwrap_or("WIP")
    );
    println!("  stash@{{0}}: {}", short_id);

    Ok(())
}

pub fn run_list() -> Result<()> {
    let stashes = git::stash::list().map_err(StashError::GitError)?;

    if stashes.is_empty() {
        println!("No stashes found");
        return Ok(());
    }

    for stash in stashes {
        println!(
            "stash@{{{}}} [{}]: {} ({})",
            stash.index, stash.branch, stash.message, stash.time_relative
        );
    }

    Ok(())
}

pub fn run_pop(stash_ref: Option<String>) -> Result<()> {
    let index = resolve_stash_index(stash_ref.as_deref())?;
    git::stash::pop(index).map_err(StashError::GitError)?;
    println!("Dropped stash@{{{}}}", index);
    println!("Applied stash and removed it from stash list");
    Ok(())
}

pub fn run_apply(stash_ref: Option<String>) -> Result<()> {
    let index = resolve_stash_index(stash_ref.as_deref())?;
    git::stash::apply(index).map_err(StashError::GitError)?;
    println!("Applied stash@{{{}}}", index);
    Ok(())
}

pub fn run_drop(stash_ref: Option<String>) -> Result<()> {
    let index = resolve_stash_index(stash_ref.as_deref())?;

    let confirmed = ui::confirm::run(&format!("Drop stash@{{{}}}?", index))?;
    if !confirmed {
        println!("Cancelled");
        return Ok(());
    }

    git::stash::drop(index).map_err(StashError::GitError)?;
    println!("Dropped stash@{{{}}}", index);
    Ok(())
}

pub fn run_clear() -> Result<()> {
    let stashes = git::stash::list().map_err(StashError::GitError)?;

    if stashes.is_empty() {
        println!("No stashes to clear");
        return Ok(());
    }

    let confirmed = ui::confirm::run(&format!("Clear all {} stashes?", stashes.len()))?;
    if !confirmed {
        println!("Cancelled");
        return Ok(());
    }

    let count = git::stash::clear().map_err(StashError::GitError)?;
    println!("Cleared {} stash(es)", count);
    Ok(())
}

pub fn run_show(stash_ref: Option<String>) -> Result<()> {
    let index = resolve_stash_index(stash_ref.as_deref())?;
    let diff = git::stash::show(index).map_err(StashError::GitError)?;

    if diff.is_empty() {
        println!("No changes in stash@{{{}}}", index);
    } else {
        print!("{}", diff);
    }

    Ok(())
}

pub fn run_branch(name: String, stash_ref: Option<String>) -> Result<()> {
    let index = resolve_stash_index(stash_ref.as_deref())?;
    git::stash::branch(&name, index).map_err(StashError::GitError)?;
    println!("Switched to a new branch '{}'", name);
    Ok(())
}

pub fn run_interactive() -> Result<()> {
    let stashes = git::stash::list().map_err(StashError::GitError)?;

    if stashes.is_empty() {
        println!("No stashes found");
        return Ok(());
    }

    let mut terminal =
        ui::terminal::setup_terminal().map_err(|e| StashError::TuiError(e.to_string()))?;
    let result = ui::stash_picker::run(&mut terminal, &stashes);
    ui::terminal::restore_terminal(terminal).map_err(|e| StashError::TuiError(e.to_string()))?;

    let Some(selection) = result? else {
        println!("Cancelled");
        return Ok(());
    };

    match selection.action {
        StashAction::Pop => {
            git::stash::pop(selection.entry.index).map_err(StashError::GitError)?;
            println!("Popped stash@{{{}}}", selection.entry.index);
        }
        StashAction::Apply => {
            git::stash::apply(selection.entry.index).map_err(StashError::GitError)?;
            println!("Applied stash@{{{}}}", selection.entry.index);
        }
        StashAction::Drop => {
            let confirmed =
                ui::confirm::run(&format!("Drop stash@{{{}}}?", selection.entry.index))?;
            if confirmed {
                git::stash::drop(selection.entry.index).map_err(StashError::GitError)?;
                println!("Dropped stash@{{{}}}", selection.entry.index);
            } else {
                println!("Cancelled");
            }
        }
        StashAction::Show => {
            let diff = git::stash::show(selection.entry.index).map_err(StashError::GitError)?;
            if diff.is_empty() {
                println!("No changes in stash@{{{}}}", selection.entry.index);
            } else {
                print!("{}", diff);
            }
        }
        StashAction::Branch => {
            print!("Branch name: ");
            io::stdout().flush().ok();
            let mut branch_name = String::new();
            io::stdin()
                .read_line(&mut branch_name)
                .map_err(|e| StashError::TuiError(e.to_string()))?;
            let branch_name = branch_name.trim();

            if branch_name.is_empty() {
                println!("Cancelled");
                return Ok(());
            }

            git::stash::branch(branch_name, selection.entry.index).map_err(StashError::GitError)?;
            println!("Switched to a new branch '{}'", branch_name);
        }
    }

    Ok(())
}

fn resolve_stash_index(stash_ref: Option<&str>) -> Result<usize> {
    let stashes = git::stash::list().map_err(StashError::GitError)?;

    if stashes.is_empty() {
        return Err(StashError::NoStashes.into());
    }

    let Some(reference) = stash_ref else {
        return Ok(0);
    };

    if let Ok(index) = reference.parse::<usize>() {
        if index < stashes.len() {
            return Ok(index);
        }
        return Err(StashError::InvalidRef(reference.to_string()).into());
    }

    if reference.starts_with("stash@{") && reference.ends_with('}') {
        let inner = &reference[7..reference.len() - 1];
        if let Ok(index) = inner.parse::<usize>() {
            if index < stashes.len() {
                return Ok(index);
            }
        }
    }

    Err(StashError::InvalidRef(reference.to_string()).into())
}
