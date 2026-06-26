//! PR write actions (merge, mark-ready) via the `gh` CLI. Every call pins an
//! explicit `--repo owner/repo` taken from the PR so a mutation can never target
//! the ambient working-directory repo.

use super::pr_search::{self, PrError};
use std::str::FromStr;

/// How a PR should be merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeMethod {
    #[default]
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    /// The `gh pr merge` flag for this method.
    pub fn flag(self) -> &'static str {
        match self {
            MergeMethod::Squash => "--squash",
            MergeMethod::Merge => "--merge",
            MergeMethod::Rebase => "--rebase",
        }
    }

    /// Human label (shown in the confirm modal).
    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Squash => "squash",
            MergeMethod::Merge => "merge",
            MergeMethod::Rebase => "rebase",
        }
    }
}

impl FromStr for MergeMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "squash" => Ok(MergeMethod::Squash),
            "merge" => Ok(MergeMethod::Merge),
            "rebase" => Ok(MergeMethod::Rebase),
            other => Err(format!(
                "unknown merge method '{other}' (expected squash, merge, or rebase)"
            )),
        }
    }
}

fn merge_args(owner: &str, repo: &str, number: u64, method: MergeMethod) -> Vec<String> {
    vec![
        "pr".to_string(),
        "merge".to_string(),
        number.to_string(),
        "--repo".to_string(),
        format!("{owner}/{repo}"),
        method.flag().to_string(),
    ]
}

fn ready_args(owner: &str, repo: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "ready".to_string(),
        number.to_string(),
        "--repo".to_string(),
        format!("{owner}/{repo}"),
    ]
}

fn run_gh(args: Vec<String>) -> Result<(), PrError> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    pr_search::gh_capture(&arg_refs).map(|_| ())
}

/// Merge the PR with the given method (`gh pr merge --<method>`).
pub fn merge(owner: &str, repo: &str, number: u64, method: MergeMethod) -> Result<(), PrError> {
    run_gh(merge_args(owner, repo, number, method))
}

/// Take the PR out of draft (`gh pr ready`).
pub fn mark_ready(owner: &str, repo: &str, number: u64) -> Result<(), PrError> {
    run_gh(ready_args(owner, repo, number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_args_squash() {
        let args = merge_args("dash0hq", "dash0", 42, MergeMethod::Squash);
        assert_eq!(
            args,
            vec![
                "pr".to_string(),
                "merge".to_string(),
                "42".to_string(),
                "--repo".to_string(),
                "dash0hq/dash0".to_string(),
                "--squash".to_string(),
            ]
        );
    }

    #[test]
    fn test_merge_args_method_flag_varies() {
        assert!(merge_args("o", "r", 1, MergeMethod::Merge).contains(&"--merge".to_string()));
        assert!(merge_args("o", "r", 1, MergeMethod::Rebase).contains(&"--rebase".to_string()));
    }

    #[test]
    fn test_ready_args() {
        let args = ready_args("reckerp", "gx", 7);
        assert_eq!(
            args,
            vec![
                "pr".to_string(),
                "ready".to_string(),
                "7".to_string(),
                "--repo".to_string(),
                "reckerp/gx".to_string(),
            ]
        );
    }

    #[test]
    fn test_merge_method_from_str() {
        assert_eq!("squash".parse::<MergeMethod>(), Ok(MergeMethod::Squash));
        assert_eq!("MERGE".parse::<MergeMethod>(), Ok(MergeMethod::Merge));
        assert_eq!(" rebase ".parse::<MergeMethod>(), Ok(MergeMethod::Rebase));
        assert!("bogus".parse::<MergeMethod>().is_err());
    }
}
