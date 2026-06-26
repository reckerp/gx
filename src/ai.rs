//! Shared invocation of the configured AI agent (opencode / claude).
//!
//! Both `gx commit --ai` and the PR dashboard (reviewer suggestion and the
//! troubleshoot launch) drive the same agents, so the command-building and
//! process plumbing live here instead of being duplicated per command.

use crate::config::Agent;
use miette::Diagnostic;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum AiError {
    #[error("Failed to spawn {agent}: {source}")]
    #[diagnostic(
        code(gx::ai::spawn_failed),
        help("Ensure the configured AI agent is installed and available in your PATH")
    )]
    Spawn {
        agent: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{agent} failed: {message}")]
    #[diagnostic(code(gx::ai::agent_failed))]
    Failed { agent: String, message: String },

    #[error("{agent} returned an empty response")]
    #[diagnostic(code(gx::ai::empty_response))]
    Empty { agent: String },

    #[error("I/O error talking to {agent}: {source}")]
    #[diagnostic(code(gx::ai::io_error))]
    Io {
        agent: String,
        #[source]
        source: std::io::Error,
    },
}

/// Build the `(command, args)` needed to run `prompt` with the given agent and
/// model. The prompt is a parameter (unlike the old commit-only helper) so the
/// same builder serves commit messages, reviewer suggestions, and investigate
/// prompts.
pub fn agent_command(agent: &Agent, model: &str, prompt: &str) -> (String, Vec<String>) {
    match agent {
        Agent::OpenCode => (
            "opencode".to_string(),
            vec![
                "run".to_string(),
                prompt.to_string(),
                "--model".to_string(),
                model.to_string(),
            ],
        ),
        Agent::Claude => (
            "claude".to_string(),
            vec![
                "-p".to_string(),
                prompt.to_string(),
                "--model".to_string(),
                model.to_string(),
            ],
        ),
    }
}

/// Run the agent with `prompt`, optionally piping `stdin`, and capture stdout.
///
/// Both stdout and stderr are piped so the agent process can never write onto a
/// caller's alternate screen (the PR dashboard renders a TUI to stderr while
/// this runs on a background thread); stderr is surfaced only inside error
/// messages.
pub fn run_capturing(
    agent: &Agent,
    model: &str,
    prompt: &str,
    stdin: Option<&str>,
) -> Result<String, AiError> {
    let (command, args) = agent_command(agent, model, prompt);

    let mut child = Command::new(&command)
        .args(&args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| AiError::Spawn {
            agent: command.clone(),
            source,
        })?;

    if let Some(input) = stdin {
        // Take the handle and drop it after writing so the agent sees EOF.
        let mut handle = child.stdin.take().ok_or_else(|| AiError::Failed {
            agent: command.clone(),
            message: "failed to open stdin".to_string(),
        })?;
        handle
            .write_all(input.as_bytes())
            .map_err(|source| AiError::Io {
                agent: command.clone(),
                source,
            })?;
    }

    let output = child
        .wait_with_output()
        .map_err(|source| AiError::Io {
            agent: command.clone(),
            source,
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            "agent exited with an error".to_string()
        } else {
            stderr
        };
        return Err(AiError::Failed { agent: command, message });
    }

    let message = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if message.is_empty() {
        return Err(AiError::Empty { agent: command });
    }

    Ok(message)
}

/// Launch the agent interactively (inherited stdio) in `cwd`, for a hands-on
/// session such as the PR troubleshoot flow. The caller must have torn down any
/// TUI first, since the agent takes over the terminal.
pub fn launch_interactive(
    agent: &Agent,
    model: &str,
    prompt: &str,
    cwd: &Path,
) -> Result<ExitStatus, AiError> {
    let (command, args) = agent_command(agent, model, prompt);

    Command::new(&command)
        .args(&args)
        .current_dir(cwd)
        .status()
        .map_err(|source| AiError::Spawn { agent: command, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_command_opencode_uses_prompt() {
        let (command, args) = agent_command(&Agent::OpenCode, "opencode/big-pickle", "do the thing");
        assert_eq!(command, "opencode");
        assert_eq!(
            args,
            vec![
                "run".to_string(),
                "do the thing".to_string(),
                "--model".to_string(),
                "opencode/big-pickle".to_string(),
            ]
        );
    }

    #[test]
    fn test_agent_command_claude_uses_prompt() {
        let (command, args) = agent_command(&Agent::Claude, "haiku", "review this PR");
        assert_eq!(command, "claude");
        assert_eq!(
            args,
            vec![
                "-p".to_string(),
                "review this PR".to_string(),
                "--model".to_string(),
                "haiku".to_string(),
            ]
        );
    }

    #[test]
    fn test_agent_command_substitutes_prompt_verbatim() {
        // The prompt is a parameter, not a baked-in constant: whatever is passed
        // appears verbatim as the run argument.
        let (_, opencode_args) = agent_command(&Agent::OpenCode, "m", "PROMPT-A");
        let (_, claude_args) = agent_command(&Agent::Claude, "m", "PROMPT-A");
        assert!(opencode_args.contains(&"PROMPT-A".to_string()));
        assert!(claude_args.contains(&"PROMPT-A".to_string()));
    }
}
