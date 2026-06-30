//! The worktree data model, fuzzy matching, and parsing of
//! `git worktree list --porcelain` output.

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Worktree {
    /// Directory name of the worktree (last path component)
    pub name: String,
    pub path: PathBuf,
    /// Checked-out branch, None when HEAD is detached or the worktree is bare
    pub branch: Option<String>,
    /// Short HEAD commit id
    pub head: Option<String>,
    /// The main worktree (the original repository checkout)
    pub is_main: bool,
    /// The worktree the command is currently running in
    pub is_current: bool,
    pub is_bare: bool,
    pub is_locked: bool,
}

/// Replace '/' with '-': branch names may contain '/', workspace directory
/// names may not, so the two are treated as interchangeable when matching.
pub fn flatten_slashes(name: &str) -> String {
    name.replace('/', "-")
}

impl Worktree {
    /// True when `query` is exactly the worktree's name or branch,
    /// treating '/' and '-' as interchangeable.
    pub fn matches_exactly(&self, query: &str) -> bool {
        let query = flatten_slashes(query);
        self.name.eq_ignore_ascii_case(&query)
            || self
                .branch
                .as_deref()
                .is_some_and(|b| flatten_slashes(b).eq_ignore_ascii_case(&query))
    }

    /// Fuzzy score of `query` against the worktree's name and branch, with
    /// '/' and '-' interchangeable: 'feat/x' matches directory 'feat-x' and
    /// 'feat-x' matches branch 'feat/x'.
    pub fn match_score(&self, matcher: &SkimMatcherV2, query: &str) -> Option<i64> {
        let flattened = flatten_slashes(query);

        let name_score = matcher
            .fuzzy_match(&self.name, query)
            .into_iter()
            .chain(matcher.fuzzy_match(&self.name, &flattened))
            .max();

        let branch_score = self.branch.as_deref().and_then(|b| {
            matcher
                .fuzzy_match(b, query)
                .into_iter()
                .chain(matcher.fuzzy_match(&flatten_slashes(b), &flattened))
                .max()
        });

        name_score.into_iter().chain(branch_score).max()
    }
}

/// Parse `git worktree list --porcelain` into [`Worktree`]s. The first entry is
/// always the main worktree. `current_root` (canonicalized when possible) marks
/// which entry the command is currently running in.
pub(crate) fn parse_porcelain(output: &str, current_root: Option<&Path>) -> Vec<Worktree> {
    let current_canonical = current_root.and_then(|p| p.canonicalize().ok());
    let mut worktrees = Vec::new();

    for entry in output.split("\n\n") {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        let mut path: Option<PathBuf> = None;
        let mut branch: Option<String> = None;
        let mut head: Option<String> = None;
        let mut is_bare = false;
        let mut is_locked = false;

        for line in entry.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                head = Some(h.chars().take(7).collect());
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if line == "bare" {
                is_bare = true;
            } else if line == "locked" || line.starts_with("locked ") {
                is_locked = true;
            }
        }

        let Some(path) = path else { continue };

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        let is_current = match (&current_canonical, path.canonicalize()) {
            (Some(current), Ok(canonical)) => canonical == *current,
            _ => current_root == Some(path.as_path()),
        };

        worktrees.push(Worktree {
            name,
            path,
            branch,
            head,
            is_main: worktrees.is_empty(),
            is_current,
            is_bare,
            is_locked,
        });
    }

    worktrees
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::worktree::test_support::worktree;

    #[test]
    fn test_parse_porcelain_basic() {
        let output = "worktree /home/user/proj\nHEAD 1234567890abcdef\nbranch refs/heads/main\n\nworktree /home/user/proj-workspaces/feature\nHEAD fedcba0987654321\nbranch refs/heads/feature";
        let worktrees = parse_porcelain(output, Some(Path::new("/home/user/proj")));

        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].name, "proj");
        assert_eq!(worktrees[0].path, PathBuf::from("/home/user/proj"));
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[0].head.as_deref(), Some("1234567"));
        assert!(worktrees[0].is_main);
        assert!(worktrees[0].is_current);

        assert_eq!(worktrees[1].name, "feature");
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature"));
        assert!(!worktrees[1].is_main);
        assert!(!worktrees[1].is_current);
    }

    #[test]
    fn test_parse_porcelain_detached_and_locked() {
        let output = "worktree /repo\nHEAD aaaaaaa1111111\nbranch refs/heads/main\n\nworktree /repo-ws/hotfix\nHEAD bbbbbbb2222222\ndetached\nlocked reason here";
        let worktrees = parse_porcelain(output, None);

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[1].branch, None);
        assert!(worktrees[1].is_locked);
        assert!(!worktrees[1].is_main);
    }

    #[test]
    fn test_parse_porcelain_bare() {
        let output = "worktree /repo.git\nbare\n\nworktree /repo-ws/dev\nHEAD ccccccc3333333\nbranch refs/heads/dev";
        let worktrees = parse_porcelain(output, None);

        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[0].is_bare);
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].branch, None);
        assert!(!worktrees[1].is_bare);
    }

    #[test]
    fn test_parse_porcelain_empty() {
        assert!(parse_porcelain("", None).is_empty());
    }

    #[test]
    fn test_flatten_slashes() {
        assert_eq!(flatten_slashes("feature-x"), "feature-x");
        assert_eq!(
            flatten_slashes("feat/expose-rationale"),
            "feat-expose-rationale"
        );
        assert_eq!(flatten_slashes("a/b/c"), "a-b-c");
    }

    #[test]
    fn test_matches_exactly() {
        let w = worktree("feat-expose-rationale", Some("feat/expose-rationale"));
        assert!(w.matches_exactly("feat-expose-rationale"));
        assert!(w.matches_exactly("feat/expose-rationale"));
        assert!(w.matches_exactly("FEAT/EXPOSE-RATIONALE"));
        assert!(!w.matches_exactly("feat/expose"));

        // workspace dir differs from branch: both still match exactly
        let w = worktree("custom-dir", Some("fix/null-check"));
        assert!(w.matches_exactly("custom-dir"));
        assert!(w.matches_exactly("fix/null-check"));
        assert!(w.matches_exactly("fix-null-check"));

        let detached = worktree("hotfix", None);
        assert!(detached.matches_exactly("hotfix"));
        assert!(!detached.matches_exactly("fix/null-check"));
    }

    #[test]
    fn test_match_score_slash_dash_interchangeable() {
        let matcher = SkimMatcherV2::default();
        let w = worktree("feat-expose-rationale", Some("feat/expose-rationale"));

        assert!(w.match_score(&matcher, "feat/expo").is_some());
        assert!(w.match_score(&matcher, "feat-expo").is_some());
        assert!(w.match_score(&matcher, "expose").is_some());
        assert!(w.match_score(&matcher, "zzz-no-match").is_none());

        // dash query against a slash-only branch on a differently named dir
        let w = worktree("custom-dir", Some("fix/null-check"));
        assert!(w.match_score(&matcher, "fix-null").is_some());
        assert!(w.match_score(&matcher, "fix/null").is_some());
    }
}
