//! Thin wrapper around the GitHub CLI (`gh`), analogous to [`super::git_exec`]
//! for `git`. Every gh-backed feature (the PR dashboard, PR write actions,
//! reviewer suggestions, ref resolution) spawns through [`capture`] so the
//! prompt-disabling env and the missing-binary-vs-failed-exit mapping live in
//! one place instead of being re-implemented per call site.

use serde::Deserialize;
use std::process::Command;

/// Failure spawning or running `gh`.
#[derive(Debug)]
pub enum GhError {
    /// The `gh` binary was not found on PATH.
    NotFound,
    /// `gh` ran but exited non-zero; carries the trimmed stderr (or a fallback
    /// message when stderr was empty).
    Failed(String),
}

/// Run `gh` with `args` and return its stdout on success.
///
/// `GH_PROMPT_DISABLED` is set so a missing token or auth prompt fails fast
/// instead of blocking on a TTY. stdout is returned untrimmed (callers that
/// parse JSON trim themselves; raw-content callers must not lose whitespace).
pub fn capture(args: &[&str]) -> Result<String, GhError> {
    let output = Command::new("gh")
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => GhError::NotFound,
            _ => GhError::Failed(e.to_string()),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GhError::Failed(if stderr.is_empty() {
            "gh exited with an error".to_string()
        } else {
            stderr
        }));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// A GitHub user handle as serialized by `gh ... --json` (`{ "login": ... }`).
/// Shared by the PR dashboard and the reviewer-suggestion feature.
#[derive(Deserialize, Default)]
pub struct RawLogin {
    #[serde(default)]
    pub login: String,
}

/// A requested reviewer as serialized by `gh pr view --json reviewRequests`:
/// a user carries `login`; a team carries `slug`/`name`.
#[derive(Deserialize, Default)]
pub struct RawReviewRequest {
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl RawReviewRequest {
    /// The reviewer's handle, preferring a user login, then a team slug, then a
    /// team name. `None` when the entry carries none of them.
    pub fn handle(&self) -> Option<String> {
        self.login
            .clone()
            .or_else(|| self.slug.clone())
            .or_else(|| self.name.clone())
    }
}
