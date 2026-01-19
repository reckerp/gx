use crate::config;
use crate::git;
use crate::git::GitError;
use crate::git::commit::CommitOptions;
use crate::ui;
use miette::{Diagnostic, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum CommitError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::commit::git_error), help("Are you in a git repository?"))]
    GitError(#[from] GitError),

    #[error("Commit aborted")]
    #[diagnostic(code(gx::commit::aborted))]
    Aborted,

    #[error("Nothing to commit")]
    #[diagnostic(
        code(gx::commit::nothing_to_commit),
        help("No staged or unstaged changes.")
    )]
    NothingToCommit,

    #[error("--no-edit can only be used with --amend")]
    #[diagnostic(
        code(gx::commit::no_edit_without_amend),
        help("Use --amend flag when using --no-edit")
    )]
    NoEditWithoutAmend,

    #[error("AI error: {0}")]
    #[diagnostic(
        code(gx::commit::ai_error),
        help("Ensure 'opencode' is installed and available in your PATH")
    )]
    AiError(String),
}

pub fn run(message: Option<String>, amend: bool, no_edit: bool, ai: bool) -> Result<()> {
    if no_edit && !amend {
        return Err(CommitError::NoEditWithoutAmend.into());
    }

    if !amend {
        let has_staged = git::status::has_staged_files().map_err(CommitError::GitError)?;

        if !has_staged {
            let (_, unstaged) = git::status::get_status_files().map_err(CommitError::GitError)?;

            if unstaged.is_empty() {
                return Err(CommitError::NothingToCommit.into());
            }

            let confirmed = ui::confirm::run("No staged files. Stage all changes?")?;

            if !confirmed {
                return Err(CommitError::Aborted.into());
            }

            git::staging::stage_all().map_err(CommitError::GitError)?;
        }
    }

    if ai {
        return run_ai_commit(amend);
    }

    let options = CommitOptions {
        message: message.as_deref(),
        amend,
        no_edit,
    };

    git::commit::create_commit(options).map_err(CommitError::GitError)?;

    Ok(())
}

fn run_ai_commit(amend: bool) -> Result<()> {
    let diff = git::staging::get_staged_diff().map_err(CommitError::GitError)?;

    if diff.is_empty() {
        return Err(CommitError::NothingToCommit.into());
    }

    let config = config::load()?;
    let model = &config.ai.model;

    let ai_message = generate_commit_message(&diff, model)?;

    println!("AI generated commit message:\n");
    println!("  {}\n", ai_message);

    let confirmed = ui::confirm::run("Use this commit message?")?;

    if confirmed {
        let options = CommitOptions {
            message: Some(&ai_message),
            amend,
            no_edit: false,
        };
        git::commit::create_commit(options).map_err(CommitError::GitError)?;
    } else {
        git::commit::create_commit_with_editor(&ai_message, amend)
            .map_err(CommitError::GitError)?;
    }

    Ok(())
}

fn generate_commit_message(diff: &str, model: &str) -> Result<String, CommitError> {
    let prompt = r#"Analyze this git diff and generate a conventional commit message following these rules:

- feat: NEW functionality or feature added
- fix: BUG fixes or corrections
- refactor: code restructuring WITHOUT behavior changes
- docs: documentation changes ONLY
- style: formatting, whitespace, missing semicolons (no code logic change)
- test: adding or updating tests
- perf: performance improvements
- chore: dependency updates, build config, tooling
- ci: CI/CD pipeline changes
- build: build system or external dependency changes

Carefully analyze what the diff actually does. Most changes are NOT features.

Output format: <type>: <imperative description>
Output ONLY the commit message, nothing else."#;

    let mut child = Command::new("opencode")
        .args(["run", prompt, "--model", model])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CommitError::AiError(format!("Failed to spawn opencode: {}", e)))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| CommitError::AiError("Failed to open stdin".to_string()))?;
        stdin
            .write_all(diff.as_bytes())
            .map_err(|e| CommitError::AiError(format!("Failed to write to stdin: {}", e)))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| CommitError::AiError(format!("Failed to wait for opencode: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CommitError::AiError(format!(
            "opencode failed: {}",
            stderr.trim()
        )));
    }

    let message = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if message.is_empty() {
        return Err(CommitError::AiError(
            "opencode returned empty message".to_string(),
        ));
    }

    Ok(message)
}
