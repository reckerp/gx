use super::GitError;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use miette::Result;

#[derive(Default)]
pub struct ExecOptions {
    pub silent: bool,
    pub capture: bool,
    pub inherit: bool,
}

impl ExecOptions {
    /// Capture stdout for the caller without echoing it to the terminal.
    pub fn capture() -> Self {
        ExecOptions {
            capture: true,
            ..Default::default()
        }
    }

    /// Run without echoing stdout. Errors are still captured and reported.
    pub fn silent() -> Self {
        ExecOptions {
            silent: true,
            ..Default::default()
        }
    }
}

/// Run `git` with `args` and return its stdout as a trimmed, lossily-decoded
/// `String`. `args` accepts anything iterable of `OsStr`-like values, so callers
/// pass `&str` flags, owned `String`s, and `Path`s without spelling `.to_string()`.
pub fn exec(
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    options: ExecOptions,
) -> Result<String, GitError> {
    let stdout = exec_inner(args, options)?;
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

/// Like [`exec`], but runs git inside `dir` (via `-C <dir>`), so callers don't
/// re-spell the `["-C", <path>, ...]` clump every time they target a worktree.
pub fn exec_in(dir: &Path, args: &[&str], options: ExecOptions) -> Result<String, GitError> {
    exec(in_dir(dir, args), options)
}

/// Like [`exec`], but returns the command's stdout as raw bytes without
/// lossy UTF-8 conversion or trimming. Needed when the content must be
/// preserved exactly (e.g. `git show :<path>` for staged file contents,
/// which may be binary or whitespace-significant).
pub fn exec_bytes(
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    options: ExecOptions,
) -> Result<Vec<u8>, GitError> {
    exec_inner(args, options)
}

/// [`exec_bytes`] variant that runs git inside `dir`.
pub fn exec_bytes_in(dir: &Path, args: &[&str], options: ExecOptions) -> Result<Vec<u8>, GitError> {
    exec_bytes(in_dir(dir, args), options)
}

/// Build a `["-C", <dir>, ...args]` argument vector.
fn in_dir(dir: &Path, args: &[&str]) -> Vec<OsString> {
    let mut full = Vec::with_capacity(args.len() + 2);
    full.push(OsString::from("-C"));
    full.push(dir.as_os_str().to_os_string());
    full.extend(args.iter().map(|&a| OsString::from(a)));
    full
}

/// Shared implementation behind [`exec`] and [`exec_bytes`]: run git, honor
/// the `inherit`/`silent`/`capture` options, and return raw stdout bytes on
/// success or a mapped error from stderr on failure.
fn exec_inner(
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    options: ExecOptions,
) -> Result<Vec<u8>, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(args);

    if options.inherit {
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        let status = cmd.status()?;

        if !status.success() {
            // stdout/stderr were streamed straight through, so there's nothing
            // captured to report — surface the exit status instead.
            return Err(GitError::CommandFailed {
                stderr: match status.code() {
                    Some(code) => format!("git exited with status {code}"),
                    None => "git terminated by a signal".to_string(),
                },
                code: status.code(),
            });
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
        return Err(map_git_error(stderr, output.status.code()));
    }

    Ok(output.stdout)
}

fn map_git_error(stderr: String, code: Option<i32>) -> GitError {
    if stderr.contains("fatal: not a git repository") {
        GitError::NotInRepo
    } else {
        GitError::CommandFailed { stderr, code }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_flag() {
        let result = exec(["--version"], ExecOptions::default());
        assert!(result.is_ok());
    }

    #[test]
    fn test_exec_subcommand() {
        let result = exec(["status"], ExecOptions::default());
        assert!(result.is_ok());
    }

    #[test]
    fn test_exec_fail() {
        let result = exec(["notfound"], ExecOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_exec_bytes_preserves_raw_output() {
        // exec_bytes must not trim: --version output ends in a newline that
        // the String-returning exec() strips.
        let bytes = exec_bytes(["--version"], ExecOptions::capture()).unwrap();
        assert!(bytes.starts_with(b"git version"));
        assert!(bytes.ends_with(b"\n"));
    }

    #[test]
    fn test_exec_bytes_fail() {
        let result = exec_bytes(["notfound"], ExecOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_map_git_error_not_in_repo() {
        let stderr =
            "fatal: not a git repository (or any of the parent directories): .git".to_string();
        let error = map_git_error(stderr, Some(128));
        assert!(matches!(error, GitError::NotInRepo));
    }

    #[test]
    fn test_map_git_error_command_failed() {
        let stderr = "some other error".to_string();
        let error = map_git_error(stderr.clone(), Some(1));
        assert!(
            matches!(error, GitError::CommandFailed { stderr: s, code } if s == stderr && code == Some(1))
        );
    }
}
