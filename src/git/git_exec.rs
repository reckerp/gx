use super::GitError;
use std::io::Write;
use std::process::{Command, Stdio};

use miette::Result;

#[derive(Default)]
pub struct ExecOptions {
    pub silent: bool,
    pub capture: bool,
    pub inherit: bool,
}

pub fn exec(args: Vec<String>, options: ExecOptions) -> Result<String, GitError> {
    let stdout = exec_inner(args, options)?;
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

/// Like [`exec`], but returns the command's stdout as raw bytes without
/// lossy UTF-8 conversion or trimming. Needed when the content must be
/// preserved exactly (e.g. `git show :<path>` for staged file contents,
/// which may be binary or whitespace-significant).
pub fn exec_bytes(args: Vec<String>, options: ExecOptions) -> Result<Vec<u8>, GitError> {
    exec_inner(args, options)
}

/// Shared implementation behind [`exec`] and [`exec_bytes`]: run git, honor
/// the `inherit`/`silent`/`capture` options, and return raw stdout bytes on
/// success or a mapped error from stderr on failure.
fn exec_inner(args: Vec<String>, options: ExecOptions) -> Result<Vec<u8>, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(&args);

    if options.inherit {
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        let status = cmd.status()?;

        if !status.success() {
            return Err(GitError::CommandFailed("Command failed".to_string()));
        }

        return Ok(Vec::new());
    }

    // Always capture output so error messages can be reported even in silent mode;
    // `silent` only suppresses printing below.
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => GitError::NotFound(e),
        _ => GitError::IoError(e),
    })?;

    if !options.silent && !options.capture {
        print!("{}", String::from_utf8_lossy(&output.stdout));
        let _ = std::io::stdout().flush();
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(map_git_error(stderr));
    }

    Ok(output.stdout)
}

fn map_git_error(stderr: String) -> GitError {
    match stderr.as_str() {
        s if s.contains("fatal: not a git repository") => GitError::NotInRepo,
        _ => GitError::CommandFailed(stderr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_flag() {
        let result = exec(vec!["--version".to_string()], ExecOptions::default());
        assert!(result.is_ok());
    }

    #[test]
    fn test_exec_subcommand() {
        let result = exec(vec!["status".to_string()], ExecOptions::default());
        assert!(result.is_ok());
    }

    #[test]
    fn test_exec_fail() {
        let result = exec(vec!["notfound".to_string()], ExecOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_exec_bytes_preserves_raw_output() {
        // exec_bytes must not trim: --version output ends in a newline that
        // the String-returning exec() strips.
        let bytes = exec_bytes(
            vec!["--version".to_string()],
            ExecOptions {
                capture: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(bytes.starts_with(b"git version"));
        assert!(bytes.ends_with(b"\n"));
    }

    #[test]
    fn test_exec_bytes_fail() {
        let result = exec_bytes(vec!["notfound".to_string()], ExecOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_map_git_error_not_in_repo() {
        let stderr =
            "fatal: not a git repository (or any of the parent directories): .git".to_string();
        let error = map_git_error(stderr);
        assert!(matches!(error, GitError::NotInRepo));
    }

    #[test]
    fn test_map_git_error_command_failed() {
        let stderr = "some other error".to_string();
        let error = map_git_error(stderr.clone());
        assert!(matches!(error, GitError::CommandFailed(msg) if msg == stderr));
    }
}
