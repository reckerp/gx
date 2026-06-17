use crate::git::{self, GitError, worktree::Worktree};
use crate::repo_setup;
use crate::ui;
use miette::{Diagnostic, Result};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum OnboardingError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::onboarding::git_error))]
    GitError(#[from] GitError),

    #[error("TUI error: {0}")]
    #[diagnostic(code(gx::onboarding::tui_error))]
    TuiError(String),
}

pub fn run() -> Result<()> {
    let worktrees = git::worktree::list().map_err(OnboardingError::GitError)?;
    let main_root = main_worktree_root(&worktrees)?;
    let mut profile = repo_setup::profile_for_repo(&main_root)?;
    let candidates = repo_setup::discover_copy_candidates(&main_root)?;

    let mut terminal =
        ui::terminal::setup_terminal().map_err(|e| OnboardingError::TuiError(e.to_string()))?;
    let selected =
        ui::setup_file_picker::run(&mut terminal, &candidates, &profile.config.copy_files);
    ui::terminal::restore_terminal(terminal)
        .map_err(|e| OnboardingError::TuiError(e.to_string()))?;

    let Some(copy_files) = selected? else {
        eprintln!("Cancelled");
        return Ok(());
    };

    profile.config.copy_files = copy_files;

    let has_script = profile.config.setup_script.is_some();
    let wants_script = if has_script {
        ui::confirm::run("Edit existing setup script?")?
    } else {
        ui::confirm::run("Define a setup script?")?
    };

    if wants_script {
        let script_path = repo_setup::setup_script_path(&profile)
            .unwrap_or_else(|| repo_setup::default_setup_script_path(&profile));
        repo_setup::create_default_setup_script(&script_path)?;
        profile.config.setup_script = script_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        repo_setup::save_profile(&profile)?;
        repo_setup::open_in_editor(&script_path)?;
    } else {
        repo_setup::save_profile(&profile)?;
    }

    eprintln!("Saved repo setup config: {}", profile.config_path.display());
    if let Some(script_path) = repo_setup::setup_script_path(&profile) {
        eprintln!("Setup script: {}", script_path.display());
    }

    Ok(())
}

fn main_worktree_root(worktrees: &[Worktree]) -> Result<std::path::PathBuf, OnboardingError> {
    worktrees
        .iter()
        .find(|w| w.is_main)
        .map(|w| w.path.clone())
        .ok_or(OnboardingError::GitError(GitError::NotInRepo))
}
