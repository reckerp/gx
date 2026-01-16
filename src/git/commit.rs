use crate::git::GitError;

use super::get_repo;

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
