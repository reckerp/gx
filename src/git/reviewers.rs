//! Deterministic reviewer suggestion: rank likely reviewers from CODEOWNERS and
//! per-file commit history, excluding the PR author, bots, and already-requested
//! reviewers. When the deterministic signal is `Thin`, the caller (U8) falls back
//! to the configured AI agent.
//!
//! The pure cores ([`matches_codeowner`], [`parse_codeowners`], [`owners_for_file`],
//! [`is_bot`], [`suggest`], [`urlencode`]) are unit-tested without touching the
//! network; the `gh`-driven gatherers wrap them.

use super::gh;
use super::pr_search::{self, PrError};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// Number of distinct non-author/non-bot committers below which (with no
/// CODEOWNERS match) the signal is considered `Thin` and the AI fallback fires.
const THIN_COMMITTER_THRESHOLD: usize = 2;

/// Max changed files to sample for commit history (cost bound).
const MAX_HISTORY_FILES: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Strong,
    Thin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Bare handle (login for a user, `org/team` for a team).
    pub handle: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendations {
    /// Ranked individual reviewers (top 1-3).
    pub suggestions: Vec<Suggestion>,
    /// CODEOWNERS teams whose globs matched (surfaced, not ranked).
    pub teams: Vec<String>,
    pub confidence: Confidence,
}

/// What a PR touches, for reviewer ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFootprint {
    pub author: String,
    pub files: Vec<String>,
    /// Logins / team slugs already requested.
    pub already_requested: Vec<String>,
    pub already_reviewed: Vec<String>,
}

// ----- Pure cores --------------------------------------------------------------

/// True if `login` is a known automation account.
pub fn is_bot(login: &str) -> bool {
    let l = login.to_ascii_lowercase();
    l.ends_with("[bot]")
        || matches!(
            l.as_str(),
            "cursor" | "dependabot" | "renovate" | "github-actions" | "copilot"
        )
}

/// Glob match supporting `*` (within a path segment) and `**` (across segments).
fn glob_match(pattern: &str, text: &str) -> bool {
    glob_inner(pattern.as_bytes(), text.as_bytes())
}

fn glob_inner(pat: &[u8], text: &[u8]) -> bool {
    if pat.is_empty() {
        return text.is_empty();
    }
    if pat[0] == b'*' {
        if pat.len() >= 2 && pat[1] == b'*' {
            // `**` — consume any sequence including '/'.
            let rest = &pat[2..];
            // Skip an optional following '/'.
            let rest = rest.strip_prefix(b"/").unwrap_or(rest);
            if glob_inner(rest, text) {
                return true;
            }
            for i in 0..text.len() {
                if glob_inner(rest, &text[i + 1..]) {
                    return true;
                }
            }
            return false;
        }
        // `*` — consume any sequence not crossing '/'.
        let rest = &pat[1..];
        if glob_inner(rest, text) {
            return true;
        }
        for (i, &c) in text.iter().enumerate() {
            if c == b'/' {
                break;
            }
            if glob_inner(rest, &text[i + 1..]) {
                return true;
            }
        }
        false
    } else if pat[0] == b'?' {
        !text.is_empty() && text[0] != b'/' && glob_inner(&pat[1..], &text[1..])
    } else {
        !text.is_empty() && pat[0] == text[0] && glob_inner(&pat[1..], &text[1..])
    }
}

/// Match a CODEOWNERS pattern against a changed path (gitignore-ish subset).
pub fn matches_codeowner(pattern: &str, path: &str) -> bool {
    let pat = pattern.trim();
    if pat.is_empty() {
        return false;
    }
    if pat == "*" || pat == "**" {
        return true;
    }

    let pat = pat.strip_prefix('/').unwrap_or(pat);
    let (core, _dir_only) = match pat.strip_suffix('/') {
        Some(p) => (p, true),
        None => (pat, false),
    };

    if !core.contains('/') {
        // Unanchored: matches against any single path segment (basename rule).
        return path.split('/').any(|seg| glob_match(core, seg));
    }

    // Anchored to repo root: match the whole path, allowing a directory pattern
    // to match everything beneath it.
    if core.ends_with("**") {
        glob_match(core, path)
    } else {
        glob_match(core, path) || glob_match(&format!("{core}/**"), path)
    }
}

/// Parse CODEOWNERS text into `(pattern, owners)` rules. Owners have their `@`
/// stripped; a team owner keeps its `org/team` slash.
pub fn parse_codeowners(text: &str) -> Vec<(String, Vec<String>)> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.split_whitespace();
            let pattern = parts.next()?.to_string();
            let owners: Vec<String> = parts
                .map(|owner| owner.trim_start_matches('@').to_string())
                .collect();
            if owners.is_empty() {
                None
            } else {
                Some((pattern, owners))
            }
        })
        .collect()
}

/// Owners for a single path, applying CODEOWNERS last-match-wins precedence.
pub fn owners_for_file(rules: &[(String, Vec<String>)], path: &str) -> Vec<String> {
    rules
        .iter()
        .rev()
        .find(|(pattern, _)| matches_codeowner(pattern, path))
        .map(|(_, owners)| owners.clone())
        .unwrap_or_default()
}

fn is_team(owner: &str) -> bool {
    owner.contains('/')
}

/// Rank reviewers from CODEOWNERS owners + a precomputed commit-history weight
/// map, excluding the author, bots, and already-requested reviewers.
pub fn suggest(
    footprint: &PrFootprint,
    codeowner_owners: &[String],
    history: &HashMap<String, u32>,
) -> Recommendations {
    let excluded = |handle: &str| -> bool {
        handle.is_empty()
            || handle.eq_ignore_ascii_case(&footprint.author)
            || is_bot(handle)
            || footprint
                .already_requested
                .iter()
                .chain(footprint.already_reviewed.iter())
                .any(|r| r.eq_ignore_ascii_case(handle))
    };

    let teams: Vec<String> = codeowner_owners
        .iter()
        .filter(|o| is_team(o))
        .cloned()
        .collect();
    let individual_owners: Vec<String> = codeowner_owners
        .iter()
        .filter(|o| !is_team(o) && !excluded(o))
        .cloned()
        .collect();

    let mut scores: HashMap<String, u32> = HashMap::new();
    for owner in &individual_owners {
        *scores.entry(owner.clone()).or_default() += 100; // CODEOWNERS is the strongest signal
    }
    for (login, weight) in history {
        if excluded(login) {
            continue;
        }
        *scores.entry(login.clone()).or_default() += weight;
    }

    let mut ranked: Vec<(String, u32)> = scores.into_iter().collect();
    // Score descending, then handle ascending for deterministic output.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let suggestions: Vec<Suggestion> = ranked
        .iter()
        .take(3)
        .map(|(handle, _)| {
            let is_owner = individual_owners.iter().any(|o| o == handle);
            let commits = history.get(handle).copied().unwrap_or(0);
            Suggestion {
                handle: handle.clone(),
                evidence: evidence_for(is_owner, commits),
            }
        })
        .collect();

    let distinct_committers = history.keys().filter(|l| !excluded(l)).count();
    let confidence =
        if !codeowner_owners.is_empty() || distinct_committers >= THIN_COMMITTER_THRESHOLD {
            Confidence::Strong
        } else {
            Confidence::Thin
        };

    Recommendations {
        suggestions,
        teams,
        confidence,
    }
}

fn evidence_for(is_owner: bool, commits: u32) -> String {
    match (is_owner, commits > 0) {
        (true, true) => {
            format!("owns matching paths (CODEOWNERS); also a recent committer (weight {commits})")
        }
        (true, false) => "owns matching paths (CODEOWNERS)".to_string(),
        (false, _) => format!("recent committer to changed files (weight {commits})"),
    }
}

/// Percent-encode a path for use in a `gh api` query value. `/` is preserved so
/// path segments stay intact; spaces, `#`, `?`, `&` and other unsafe bytes are
/// encoded.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ----- gh-driven gatherers -----------------------------------------------------

#[derive(Deserialize)]
struct RawFile {
    path: String,
}

#[derive(Deserialize, Default)]
struct RawReview {
    #[serde(default)]
    author: gh::RawLogin,
}

#[derive(Deserialize)]
struct RawFootprint {
    #[serde(default)]
    author: gh::RawLogin,
    #[serde(default)]
    files: Vec<RawFile>,
    #[serde(rename = "reviewRequests", default)]
    review_requests: Vec<gh::RawReviewRequest>,
    #[serde(default)]
    reviews: Vec<RawReview>,
}

fn parse_footprint(json: &str) -> Result<PrFootprint, PrError> {
    let raw: RawFootprint =
        serde_json::from_str(json.trim()).map_err(|e| PrError::ParseFailed(e.to_string()))?;

    let mut already_reviewed = Vec::new();
    for review in &raw.reviews {
        let login = review.author.login.clone();
        if !login.is_empty() && !already_reviewed.contains(&login) {
            already_reviewed.push(login);
        }
    }

    Ok(PrFootprint {
        author: raw.author.login,
        files: raw.files.into_iter().map(|f| f.path).collect(),
        already_requested: raw
            .review_requests
            .iter()
            .filter_map(gh::RawReviewRequest::handle)
            .collect(),
        already_reviewed,
    })
}

/// Fetch the PR footprint via `gh pr view`.
pub fn gather(owner: &str, repo: &str, number: u64) -> Result<PrFootprint, PrError> {
    let slug = format!("{owner}/{repo}");
    let num = number.to_string();
    let stdout = pr_search::gh_capture(&[
        "pr",
        "view",
        &num,
        "--repo",
        &slug,
        "--json",
        "author,files,reviewRequests,reviews",
    ])?;
    parse_footprint(&stdout)
}

// Best-effort: a missing CODEOWNERS file degrades to None rather than a surfaced
// error, so a failed `gh` call is just `.ok()`-discarded.
fn fetch_codeowners(owner: &str, repo: &str) -> Option<String> {
    for path in [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"] {
        let endpoint = format!("repos/{owner}/{repo}/contents/{path}");
        if let Ok(text) =
            gh::capture(&["api", "-H", "Accept: application/vnd.github.raw", &endpoint])
            && !text.trim().is_empty()
        {
            return Some(text);
        }
    }
    None
}

// Best-effort: degrades to an empty tally on error rather than aborting the
// whole suggestion.
fn fetch_commit_authors(owner: &str, repo: &str, path: &str) -> Vec<String> {
    let endpoint = format!(
        "repos/{owner}/{repo}/commits?path={}&per_page=30",
        urlencode(path)
    );
    let Ok(stdout) = gh::capture(&["api", &endpoint, "--jq", ".[] | .author.login // empty"])
    else {
        return Vec::new();
    };
    stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Tally per-file commit history into a login -> weight map. Weight combines
/// recency (newer commits weigh more) and breadth (files touched).
fn tally_history(owner: &str, repo: &str, files: &[String]) -> HashMap<String, u32> {
    let mut scores: HashMap<String, u32> = HashMap::new();
    let mut breadth: HashMap<String, u32> = HashMap::new();

    for file in files.iter().take(MAX_HISTORY_FILES) {
        let authors = fetch_commit_authors(owner, repo, file);
        let mut seen = HashSet::new();
        for (idx, login) in authors.iter().enumerate() {
            let recency = if idx < 5 {
                3
            } else if idx < 15 {
                2
            } else {
                1
            };
            *scores.entry(login.clone()).or_default() += recency;
            if seen.insert(login.clone()) {
                *breadth.entry(login.clone()).or_default() += 1;
            }
        }
    }

    for (login, count) in breadth {
        *scores.entry(login).or_default() += count * 5;
    }
    scores
}

/// Deterministic recommendation from an already-gathered footprint: resolve
/// CODEOWNERS for the changed files, tally commit history, and rank. Split out so
/// a caller that needs the footprint too (e.g. for an AI-fallback prompt) does
/// not pay a second `gh pr view`.
pub fn recommend_from_footprint(
    owner: &str,
    repo: &str,
    footprint: &PrFootprint,
) -> Recommendations {
    let codeowner_owners = match fetch_codeowners(owner, repo) {
        Some(text) => {
            let rules = parse_codeowners(&text);
            let mut owners = Vec::new();
            for file in &footprint.files {
                for owner in owners_for_file(&rules, file) {
                    if !owners.contains(&owner) {
                        owners.push(owner);
                    }
                }
            }
            owners
        }
        None => Vec::new(),
    };

    let history = tally_history(owner, repo, &footprint.files);
    suggest(footprint, &codeowner_owners, &history)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn footprint(author: &str, requested: &[&str]) -> PrFootprint {
        PrFootprint {
            author: author.to_string(),
            files: vec!["src/main.rs".to_string()],
            already_requested: requested.iter().map(|s| s.to_string()).collect(),
            already_reviewed: vec![],
        }
    }

    #[test]
    fn test_matches_codeowner() {
        assert!(matches_codeowner(
            "app/darkplane/**",
            "app/darkplane/ingest.ts"
        ));
        assert!(matches_codeowner("*.rs", "src/main.rs"));
        assert!(matches_codeowner("*", "anything/at/all.go"));
        assert!(matches_codeowner("/src/", "src/main.rs"));
        assert!(!matches_codeowner("app/darkplane/**", "src/main.rs"));
        assert!(!matches_codeowner("*.rs", "src/main.go"));
    }

    #[test]
    fn test_owners_for_file_last_match_wins() {
        let rules = vec![
            ("*".to_string(), vec!["default".to_string()]),
            ("src/**".to_string(), vec!["srcowner".to_string()]),
        ];
        assert_eq!(owners_for_file(&rules, "src/main.rs"), vec!["srcowner"]);
        assert_eq!(owners_for_file(&rules, "README.md"), vec!["default"]);
    }

    #[test]
    fn test_parse_codeowners_strips_comments_and_at() {
        let text = "# comment\n*.rs   @alice @dash0hq/backend\n\ndocs/  @bob # trailing";
        let rules = parse_codeowners(text);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].0, "*.rs");
        assert_eq!(rules[0].1, vec!["alice", "dash0hq/backend"]);
        assert_eq!(rules[1].1, vec!["bob"]);
    }

    #[test]
    fn test_is_bot() {
        assert!(is_bot("dependabot[bot]"));
        assert!(is_bot("renovate[bot]"));
        assert!(is_bot("cursor"));
        assert!(is_bot("github-actions[bot]"));
        assert!(!is_bot("alice"));
    }

    #[test]
    fn test_suggest_excludes_author() {
        let fp = footprint("alice", &[]);
        let mut history = HashMap::new();
        history.insert("alice".to_string(), 50);
        history.insert("bob".to_string(), 10);
        let rec = suggest(&fp, &[], &history);
        assert!(rec.suggestions.iter().all(|s| s.handle != "alice"));
        assert_eq!(rec.suggestions[0].handle, "bob");
    }

    #[test]
    fn test_suggest_ranks_breadth_depth_above_single_commit() {
        let fp = footprint("author", &[]);
        let mut history = HashMap::new();
        history.insert("heavy".to_string(), 40);
        history.insert("light".to_string(), 3);
        let rec = suggest(&fp, &[], &history);
        assert_eq!(rec.suggestions[0].handle, "heavy");
    }

    #[test]
    fn test_suggest_codeowner_outranks_history() {
        let fp = footprint("author", &[]);
        let mut history = HashMap::new();
        history.insert("committer".to_string(), 30);
        let rec = suggest(&fp, &["owner1".to_string()], &history);
        assert_eq!(rec.suggestions[0].handle, "owner1");
    }

    #[test]
    fn test_suggest_thin_when_no_codeowner_and_sparse_history() {
        let fp = footprint("author", &[]);
        let mut history = HashMap::new();
        history.insert("only-one".to_string(), 5);
        let rec = suggest(&fp, &[], &history);
        assert_eq!(rec.confidence, Confidence::Thin);
    }

    #[test]
    fn test_suggest_strong_with_two_committers() {
        let fp = footprint("author", &[]);
        let mut history = HashMap::new();
        history.insert("a".to_string(), 5);
        history.insert("b".to_string(), 5);
        let rec = suggest(&fp, &[], &history);
        assert_eq!(rec.confidence, Confidence::Strong);
    }

    #[test]
    fn test_suggest_dedupes_already_requested() {
        let fp = footprint("author", &["bob"]);
        let mut history = HashMap::new();
        history.insert("bob".to_string(), 50);
        history.insert("carol".to_string(), 10);
        let rec = suggest(&fp, &[], &history);
        assert!(rec.suggestions.iter().all(|s| s.handle != "bob"));
    }

    #[test]
    fn test_suggest_excludes_already_reviewed() {
        let fp = PrFootprint {
            author: "author".to_string(),
            files: vec![],
            already_requested: vec![],
            already_reviewed: vec!["dave".to_string()],
        };
        let mut history = HashMap::new();
        history.insert("dave".to_string(), 50);
        history.insert("erin".to_string(), 10);
        let rec = suggest(&fp, &[], &history);
        assert!(rec.suggestions.iter().all(|s| s.handle != "dave"));
    }

    #[test]
    fn test_suggest_teams_surface_and_history_still_flows() {
        let fp = footprint("author", &[]);
        let mut history = HashMap::new();
        history.insert("dev".to_string(), 8);
        // CODEOWNERS matched only a team.
        let rec = suggest(&fp, &["dash0hq/backend".to_string()], &history);
        assert_eq!(rec.teams, vec!["dash0hq/backend".to_string()]);
        assert_eq!(rec.suggestions[0].handle, "dev");
        assert_eq!(rec.confidence, Confidence::Strong); // a codeowner matched
    }

    #[test]
    fn test_urlencode_space_and_hash() {
        assert_eq!(urlencode("a b/c#d.rs"), "a%20b/c%23d.rs");
        assert_eq!(urlencode("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_parse_footprint() {
        let json = r#"{
          "author":{"login":"recker"},
          "files":[{"path":"src/a.rs"},{"path":"src/b.rs"}],
          "reviewRequests":[{"login":"alice"},{"slug":"backend"}],
          "reviews":[{"author":{"login":"bob"}},{"author":{"login":"bob"}}]
        }"#;
        let fp = parse_footprint(json).unwrap();
        assert_eq!(fp.author, "recker");
        assert_eq!(fp.files, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(fp.already_requested, vec!["alice", "backend"]);
        assert_eq!(fp.already_reviewed, vec!["bob"]);
    }
}
