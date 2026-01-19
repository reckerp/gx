use super::GitError;
use super::branch::get_current_branch;
use super::git_exec::{ExecOptions, exec};

#[derive(Default)]
pub struct PushOptions {
    pub force: bool,
    pub force_dangerously: bool,
}

pub fn push(options: PushOptions) -> Result<String, GitError> {
    let mut args = vec!["push".to_string()];

    if options.force_dangerously {
        args.push("--force".to_string());
    } else if options.force {
        let branch = get_current_branch()?;
        args.push(format!("--force-with-lease={}", branch.name));
    }

    exec(args, ExecOptions::default())
}
