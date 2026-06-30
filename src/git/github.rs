//! Resolve GitHub references (pull-request URLs, branch/tree URLs, and the
//! `#123` shorthand) to a branch name.
//!
//! Commands like `gx checkout` and `gx workspace new|go` accept these in place
//! of a plain branch/workspace name. A reference is only accepted when it
//! belongs to the current repository's `origin` remote; pull requests are then
//! resolved to their head branch via the `gh` CLI.

use super::{GitError, get_repo, gh};
use miette::Diagnostic;
use thiserror::Error;

/// A parsed GitHub reference, before it is validated against `origin` or
/// resolved to a concrete branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubRef {
    /// A pull request URL: `https://github.com/<owner>/<repo>/pull/<number>`.
    Pull {
        owner: String,
        repo: String,
        number: u64,
    },
    /// A branch URL: `https://github.com/<owner>/<repo>/tree/<branch>`.
    Tree {
        owner: String,
        repo: String,
        branch: String,
    },
    /// The `#<number>` shorthand for a pull request in the current repository.
    PullShort { number: u64 },
}

#[derive(Error, Debug, Diagnostic)]
pub enum GitHubError {
    #[error("Git error: {0}")]
    #[diagnostic(code(gx::github::git_error))]
    Git(#[from] GitError),

    #[error("No 'origin' remote pointing at GitHub was found")]
    #[diagnostic(
        code(gx::github::no_origin),
        help(
            "gx resolves GitHub URLs against your 'origin' remote. Add one with 'git remote add origin <url>'."
        )
    )]
    NoOrigin,

    #[error("'{url_repo}' does not belong to this repository (origin is '{origin_repo}')")]
    #[diagnostic(
        code(gx::github::wrong_repo),
        help("Run this from a clone of '{url_repo}', or pass a reference from '{origin_repo}'.")
    )]
    WrongRepo {
        url_repo: String,
        origin_repo: String,
    },

    #[error("Pull request #{number} in '{repo}' comes from a fork")]
    #[diagnostic(
        code(gx::github::fork_pr),
        help(
            "gx can only check out pull requests whose branch lives in '{repo}'. Use 'gh pr checkout {number}' to fetch the fork's branch instead."
        )
    )]
    ForkPr { number: u64, repo: String },

    #[error("'gh' executable not found")]
    #[diagnostic(
        code(gx::github::gh_not_found),
        help(
            "Install the GitHub CLI (https://cli.github.com) and run 'gh auth login' to resolve pull-request references."
        )
    )]
    GhNotFound,

    #[error("Could not look up pull request: {0}")]
    #[diagnostic(
        code(gx::github::gh_failed),
        help("Check the pull-request number and that 'gh' is authenticated ('gh auth login').")
    )]
    GhFailed(String),
}

/// Parse `query` as a GitHub reference. Returns `None` when it is a plain
/// branch/workspace name (the common case), so callers can fall back to their
/// usual lookup.
pub fn parse_ref(query: &str) -> Option<GitHubRef> {
    let query = query.trim();

    // `#123` shorthand for a PR in the current repo.
    if let Some(rest) = query.strip_prefix('#') {
        return rest
            .parse::<u64>()
            .ok()
            .map(|number| GitHubRef::PullShort { number });
    }

    parse_github_url(query)
}

fn parse_github_url(input: &str) -> Option<GitHubRef> {
    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
        .unwrap_or(input);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.strip_prefix("github.com/")?;

    // Drop any query string or fragment (e.g. '/pull/13#issuecomment-...').
    let path = rest.split(['?', '#']).next().unwrap_or(rest);

    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let owner = segments.next()?.to_string();
    let repo = segments.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo).to_string();

    match segments.next()? {
        "pull" => {
            let number = segments.next()?.parse().ok()?;
            Some(GitHubRef::Pull {
                owner,
                repo,
                number,
            })
        }
        "tree" => {
            // Branch names may contain '/', so rejoin the remaining segments.
            let branch = segments.collect::<Vec<_>>().join("/");
            if branch.is_empty() {
                None
            } else {
                Some(GitHubRef::Tree {
                    owner,
                    repo,
                    branch,
                })
            }
        }
        _ => None,
    }
}

/// Validate `gh_ref` against the `origin` remote and resolve it to a branch
/// name. Pull requests are resolved through the `gh` CLI; branch URLs return
/// their branch directly.
pub fn resolve_branch(gh_ref: &GitHubRef) -> Result<String, GitHubError> {
    let origin = origin_owner_repo()?.ok_or(GitHubError::NoOrigin)?;

    match gh_ref {
        GitHubRef::Pull {
            owner,
            repo,
            number,
        } => {
            ensure_same_repo(&origin, owner, repo)?;
            resolve_pr_branch(owner, repo, *number)
        }
        GitHubRef::PullShort { number } => {
            let (owner, repo) = &origin;
            resolve_pr_branch(owner, repo, *number)
        }
        GitHubRef::Tree {
            owner,
            repo,
            branch,
        } => {
            ensure_same_repo(&origin, owner, repo)?;
            Ok(branch.clone())
        }
    }
}

fn ensure_same_repo(origin: &(String, String), owner: &str, repo: &str) -> Result<(), GitHubError> {
    let (origin_owner, origin_repo) = origin;
    if origin_owner.eq_ignore_ascii_case(owner) && origin_repo.eq_ignore_ascii_case(repo) {
        Ok(())
    } else {
        Err(GitHubError::WrongRepo {
            url_repo: format!("{owner}/{repo}"),
            origin_repo: format!("{origin_owner}/{origin_repo}"),
        })
    }
}

/// Owner/repo of the `origin` remote, or `None` when there is no `origin`
/// remote pointing at github.com.
pub(crate) fn origin_owner_repo() -> Result<Option<(String, String)>, GitHubError> {
    let repo = get_repo()?;
    let Ok(remote) = repo.find_remote("origin") else {
        return Ok(None);
    };
    Ok(remote.url().and_then(parse_owner_repo))
}

/// Extract `(owner, repo)` from a github.com remote URL, supporting the
/// scp-like (`git@github.com:owner/repo.git`), HTTPS, and SSH URL forms.
pub(crate) fn parse_owner_repo(url: &str) -> Option<(String, String)> {
    let idx = url.find("github.com")?;
    let after = &url[idx + "github.com".len()..];
    // After the host comes ':owner/repo.git' (scp-like) or '/owner/repo.git'.
    let path = after.trim_start_matches([':', '/']);
    let path = path.split(['?', '#']).next().unwrap_or(path);

    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let owner = segments.next()?.to_string();
    let repo = segments.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo).to_string();

    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some((owner, repo))
    }
}

/// Ask `gh` for a pull request's head branch, erroring when the PR originates
/// from a fork (its branch does not live in `owner/repo`).
fn resolve_pr_branch(owner: &str, repo: &str, number: u64) -> Result<String, GitHubError> {
    let slug = format!("{owner}/{repo}");

    let stdout = gh::capture(&[
        "pr",
        "view",
        &number.to_string(),
        "--repo",
        &slug,
        "--json",
        "headRefName,isCrossRepository",
        "--template",
        "{{.headRefName}}{{\"\\t\"}}{{.isCrossRepository}}",
    ])
    .map_err(|e| match e {
        gh::GhError::NotFound => GitHubError::GhNotFound,
        gh::GhError::Failed(detail) => {
            GitHubError::GhFailed(format!("#{number} in '{slug}': {detail}"))
        }
    })?;

    let mut fields = stdout.trim().splitn(2, '\t');
    let branch = fields.next().unwrap_or("").trim().to_string();
    let is_cross_repository = fields.next().map(str::trim) == Some("true");

    if branch.is_empty() {
        return Err(GitHubError::GhFailed(format!(
            "#{number} in '{slug}': gh returned an empty branch name"
        )));
    }

    if is_cross_repository {
        return Err(GitHubError::ForkPr { number, repo: slug });
    }

    Ok(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ref_pull_url() {
        assert_eq!(
            parse_ref("https://github.com/reckerp/gx/pull/13"),
            Some(GitHubRef::Pull {
                owner: "reckerp".to_string(),
                repo: "gx".to_string(),
                number: 13,
            })
        );
    }

    #[test]
    fn test_parse_ref_pull_url_with_trailing_path_and_fragment() {
        let expected = Some(GitHubRef::Pull {
            owner: "reckerp".to_string(),
            repo: "gx".to_string(),
            number: 13,
        });
        assert_eq!(
            parse_ref("https://github.com/reckerp/gx/pull/13/files"),
            expected
        );
        assert_eq!(
            parse_ref("https://github.com/reckerp/gx/pull/13#issuecomment-42"),
            expected
        );
    }

    #[test]
    fn test_parse_ref_pull_url_without_scheme_or_www() {
        let expected = Some(GitHubRef::Pull {
            owner: "reckerp".to_string(),
            repo: "gx".to_string(),
            number: 7,
        });
        assert_eq!(parse_ref("github.com/reckerp/gx/pull/7"), expected);
        assert_eq!(
            parse_ref("https://www.github.com/reckerp/gx/pull/7"),
            expected
        );
    }

    #[test]
    fn test_parse_ref_tree_url() {
        assert_eq!(
            parse_ref("https://github.com/reckerp/gx/tree/feat/expose-rationale"),
            Some(GitHubRef::Tree {
                owner: "reckerp".to_string(),
                repo: "gx".to_string(),
                branch: "feat/expose-rationale".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_ref_tree_url_repo_with_git_suffix() {
        assert_eq!(
            parse_ref("https://github.com/reckerp/gx.git/tree/main"),
            Some(GitHubRef::Tree {
                owner: "reckerp".to_string(),
                repo: "gx".to_string(),
                branch: "main".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_ref_pr_shorthand() {
        assert_eq!(parse_ref("#42"), Some(GitHubRef::PullShort { number: 42 }));
        assert_eq!(
            parse_ref("  #42  "),
            Some(GitHubRef::PullShort { number: 42 })
        );
    }

    #[test]
    fn test_parse_ref_plain_names_are_not_github_refs() {
        assert_eq!(parse_ref("main"), None);
        assert_eq!(parse_ref("feat/expose-rationale"), None);
        assert_eq!(parse_ref("#not-a-number"), None);
        assert_eq!(parse_ref("https://github.com/reckerp/gx"), None);
        assert_eq!(parse_ref("https://github.com/reckerp/gx/issues/13"), None);
        assert_eq!(parse_ref("https://example.com/reckerp/gx/pull/13"), None);
    }

    #[test]
    fn test_parse_owner_repo_scp_like() {
        assert_eq!(
            parse_owner_repo("git@github.com:reckerp/gx.git"),
            Some(("reckerp".to_string(), "gx".to_string()))
        );
    }

    #[test]
    fn test_parse_owner_repo_https() {
        assert_eq!(
            parse_owner_repo("https://github.com/reckerp/gx.git"),
            Some(("reckerp".to_string(), "gx".to_string()))
        );
        assert_eq!(
            parse_owner_repo("https://github.com/reckerp/gx"),
            Some(("reckerp".to_string(), "gx".to_string()))
        );
    }

    #[test]
    fn test_parse_owner_repo_ssh() {
        assert_eq!(
            parse_owner_repo("ssh://git@github.com/reckerp/gx.git"),
            Some(("reckerp".to_string(), "gx".to_string()))
        );
    }

    #[test]
    fn test_parse_owner_repo_non_github() {
        assert_eq!(parse_owner_repo("git@gitlab.com:reckerp/gx.git"), None);
    }

    #[test]
    fn test_ensure_same_repo_is_case_insensitive() {
        let origin = ("reckerp".to_string(), "gx".to_string());
        assert!(ensure_same_repo(&origin, "RecKerp", "GX").is_ok());
        assert!(ensure_same_repo(&origin, "someone", "gx").is_err());
    }
}
