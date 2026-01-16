use super::GitError;
use std::io::Write;
use std::process::{Command, Stdio};

use miette::Result;

pub struct ExecOptions {
    pub silent: bool,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self { silent: false }
    }
}

pub fn exec(args: Vec<String>, options: ExecOptions) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(&args);

    if options.silent {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    } else {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    let output = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => GitError::NotFound(e),
        _ => GitError::IoError(e),
    })?;

    if !options.silent {
        print!("{}", String::from_utf8_lossy(&output.stdout));
        let _ = std::io::stdout().flush();
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(map_git_error(stderr).into());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
