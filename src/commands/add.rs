use crate::git;
use crate::git::GitError;
use crate::ui;
use miette::{Diagnostic, Result};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum AddError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::add::git_error), help("Are you in a git repository?"))]
    GitError(#[from] GitError),

    #[error("TUI error: {0}")]
    TuiError(String),
}

pub fn run(interactive: bool, paths: Vec<String>) -> Result<()> {
    if interactive {
        return run_interactive();
    }

    if paths.is_empty() {
        let staged = git::staging::stage_all().map_err(AddError::GitError)?;
        if staged.is_empty() {
            println!("Nothing to add.");
        } else {
            println!("Staged {} file(s):", staged.len());
            staged.iter().for_each(|f| println!("{}", f));
        }
    } else {
        let staged = git::staging::stage_paths(&paths).map_err(AddError::GitError)?;
        for path in &staged {
            println!("add '{}'", path);
        }
    }

    Ok(())
}

fn run_interactive() -> Result<()> {
    let (staged, unstaged) = git::status::get_status_files().map_err(AddError::GitError)?;

    if staged.is_empty() && unstaged.is_empty() {
        println!("Nothing to add.");
        return Ok(());
    }

    let mut terminal =
        ui::terminal::setup_terminal().map_err(|e| AddError::TuiError(e.to_string()))?;

    let selection = ui::file_picker::run(&mut terminal, &staged, &unstaged);

    ui::terminal::restore_terminal(terminal).map_err(|e| AddError::TuiError(e.to_string()))?;

    match selection? {
        Some(result) => {
            if !result.to_unstage.is_empty() {
                git::staging::unstage_paths(&result.to_unstage).map_err(AddError::GitError)?;
                for path in &result.to_unstage {
                    println!("unstage '{}'", path);
                }
            }
            if !result.to_stage.is_empty() {
                git::staging::stage_paths(&result.to_stage).map_err(AddError::GitError)?;
                for path in &result.to_stage {
                    println!("add '{}'", path);
                }
            }
            if result.to_stage.is_empty() && result.to_unstage.is_empty() {
                println!("No changes.");
            }
        }
        None => {
            println!("Cancelled.");
        }
    }

    Ok(())
}
