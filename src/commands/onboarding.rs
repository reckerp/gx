use crate::commands::workspace::main_worktree_root;
use crate::git::{self, GitError};
use crate::output;
use crate::repo_config::{self, RepoConfigFile, RepoWorkspaceSection};
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

    let selected = ui::terminal::with_terminal(|t| {
        ui::setup_file_picker::run(t, &candidates, &profile.config.copy_files)
    })
    .map_err(|e| OnboardingError::TuiError(e.to_string()))?;

    let Some(copy_files) = selected? else {
        output::cancelled();
        return Ok(());
    };

    profile.config.copy_files = copy_files;

    // Where should this setup live? `ui::confirm` is yes/no, so ask
    // sequentially. The three outcomes map to the plan's Personal / Shared /
    // Shared+local choices.
    eprintln!();
    eprintln!("Where should this setup be saved?");
    eprintln!("  - Shared repo config is committable (.gx/workspace.toml) and gives the team a default.");
    eprintln!("  - Personal config stays on this machine (good for secrets and local-only scripts).");
    let shared = ui::confirm::run("Save as shared repo config (.gx/workspace.toml)?")?;

    if shared {
        let with_local = ui::confirm::run("Also create a local override (.gx/workspace.local.toml)?")?;
        save_shared(&main_root, profile.config.copy_files.clone(), with_local)
    } else {
        save_personal(profile)
    }
}

/// Existing personal-only save path: persist the profile under confy, optionally
/// authoring a setup script. Unchanged behavior.
fn save_personal(mut profile: repo_setup::RepoSetupProfile) -> Result<()> {
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

/// Shared save path: write `.gx/workspace.toml` (and `.gx/.gitignore`, plus an
/// optional local override) under the main worktree root, optionally authoring
/// a setup script at `.gx/setup-workspace.sh`.
fn save_shared(
    main_root: &std::path::Path,
    copy_files: Vec<String>,
    with_local: bool,
) -> Result<()> {
    let gx_dir = repo_config::ensure_gx_dir(main_root)?;

    let setup_script = {
        let wants_script = ui::confirm::run("Define a setup script (.gx/setup-workspace.sh)?")?;
        if wants_script {
            let script_path = gx_dir.join("setup-workspace.sh");
            repo_setup::create_default_setup_script(&script_path)?;
            repo_setup::open_in_editor(&script_path)?;
            // Stored relative to main_root, matching the plan example.
            Some(".gx/setup-workspace.sh".to_string())
        } else {
            None
        }
    };

    let config = RepoConfigFile {
        version: Some(repo_config::SUPPORTED_VERSION),
        default_branch: None,
        workspace: RepoWorkspaceSection {
            copy_files: Some(copy_files),
            setup_script,
            ..Default::default()
        },
    };

    let shared_path = gx_dir.join(repo_config::SHARED_FILE);
    repo_config::write_config_file(&shared_path, &config)?;
    eprintln!("Saved shared repo config: {}", shared_path.display());

    if repo_config::ensure_gitignore(&gx_dir)? {
        eprintln!("Wrote {}", gx_dir.join(".gitignore").display());
    }

    if with_local && repo_config::ensure_local_override(&gx_dir)? {
        eprintln!(
            "Created local override: {}",
            gx_dir.join(repo_config::LOCAL_FILE).display()
        );
    }

    Ok(())
}
