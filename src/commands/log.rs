use crate::git;
use crate::git::GitError;
use crate::ui;
use crate::ui::log_viewer::LogAction;
use miette::{Diagnostic, Result};
use thiserror::Error;

const DEFAULT_LIMIT: usize = 500;

#[derive(Error, Debug, Diagnostic)]
pub enum LogError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::log::git_error))]
    GitError(#[from] GitError),

    #[error("TUI error: {0}")]
    #[diagnostic(code(gx::log::tui_error))]
    TuiError(String),
}

pub fn run(limit: Option<usize>) -> Result<()> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let log = git::log::get_log(limit).map_err(LogError::GitError)?;

    if log.entries.is_empty() {
        println!("No commits found");
        return Ok(());
    }

    let mut terminal =
        ui::terminal::setup_terminal().map_err(|e| LogError::TuiError(e.to_string()))?;

    let result = ui::log_viewer::run(&mut terminal, &log);

    ui::terminal::restore_terminal(terminal).map_err(|e| LogError::TuiError(e.to_string()))?;

    match result? {
        LogAction::Checkout(oid) => {
            let short_id =
                git::commit::checkout_commit(&oid.to_string()).map_err(LogError::GitError)?;
            println!("Checked out commit {}", short_id);
        }
        LogAction::Quit => {}
    }

    Ok(())
}
