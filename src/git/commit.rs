use crate::git::GitError;
use crate::git::git_exec::{ExecOptions, exec};

use super::get_repo;

pub struct CommitOptions<'a> {
    pub message: Option<&'a str>,
    pub amend: bool,
    pub no_edit: bool,
}

pub fn create_commit(options: CommitOptions) -> Result<String, GitError> {
    let mut args = vec!["commit".to_string()];

    if options.amend {
        args.push("--amend".to_string());
        args.push("--date=now".to_string());
    }

    if options.no_edit {
        args.push("--no-edit".to_string());
    }

    if let Some(msg) = options.message {
        args.push("-m".to_string());
        args.push(msg.to_string());
    }

    exec(args, ExecOptions::default())
}

pub fn create_commit_with_editor(initial_message: &str, amend: bool) -> Result<String, GitError> {
    let repo = get_repo()?;
    let git_dir = repo.path();
    let commit_msg_path = git_dir.join("COMMIT_EDITMSG");

    std::fs::write(&commit_msg_path, initial_message)?;

    let mut args = vec!["commit".to_string()];

    if amend {
        args.push("--amend".to_string());
        args.push("--date=now".to_string());
    }

    args.push("-e".to_string());
    args.push("-F".to_string());
    args.push(commit_msg_path.to_string_lossy().to_string());

    exec(
        args,
        ExecOptions {
            inherit: true,
            ..Default::default()
        },
    )?;

    Ok("Commit created".to_string())
}

pub fn is_valid_commit_ref(commit_ish: &str) -> bool {
    if let Ok(repo) = get_repo() {
        repo.revparse_single(commit_ish)
            .and_then(|obj| obj.peel_to_commit())
            .is_ok()
    } else {
        false
    }
}

pub fn checkout_commit(commit_ish: &str) -> Result<String, GitError> {
    let repo = get_repo()?;
    let obj = repo.revparse_single(commit_ish)?;
    let commit = obj.peel_to_commit()?;
    let short_id = commit.as_object().short_id()?;

    repo.set_head_detached(commit.id())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::default().safe()))?;

    Ok(short_id.as_str().unwrap_or(commit_ish).to_string())
}
