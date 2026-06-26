//! PR dashboard data layer: search for the current user's open pull requests
//! across a configurable scope, and the data model the TUI renders.
//!
//! Two phases (see the plan's KTD1): [`search`] returns the PR list quickly from
//! `gh search prs` (a fixed, flat field set), then per-PR enrichment (in this
//! module's U3 half) fills in review/merge/check status via `gh pr view`.

use super::github;
use miette::Diagnostic;
use serde::Deserialize;
use std::process::Command;
use thiserror::Error;

/// Which repositories the dashboard searches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// A single repository (`--repo owner/repo`).
    CurrentRepo { owner: String, repo: String },
    /// One or more orgs (`--owner` repeated).
    Orgs(Vec<String>),
    /// Everything `gh` can see (no repo/owner qualifier).
    Global,
}

impl Scope {
    /// Short human label for the list title.
    pub fn label(&self) -> String {
        match self {
            Scope::CurrentRepo { owner, repo } => format!("{owner}/{repo}"),
            Scope::Orgs(orgs) => format!("orgs: {}", orgs.join(", ")),
            Scope::Global => "all repos".to_string(),
        }
    }
}

/// Whether a PR was found because the user authored it or was asked to review it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    Authored,
    ReviewRequested,
}

/// Stable identity of a PR across the two searches and the enrichment stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrId {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

#[derive(Error, Debug, Diagnostic)]
pub enum PrError {
    #[error("'gh' executable not found")]
    #[diagnostic(
        code(gx::pr::gh_not_found),
        help("Install the GitHub CLI (https://cli.github.com) and run 'gh auth login'.")
    )]
    GhNotFound,

    #[error("gh command failed: {0}")]
    #[diagnostic(
        code(gx::pr::gh_failed),
        help("Check that 'gh' is authenticated ('gh auth login').")
    )]
    GhFailed(String),

    #[error("Failed to parse gh output: {0}")]
    #[diagnostic(code(gx::pr::parse_failed))]
    ParseFailed(String),
}

// ----- Enrichment data model (behavior lives in the U3 half of this module) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeState {
    Clean,
    Blocked,
    Behind,
    Dirty,
    Unstable,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChecksSummary {
    pub passing: u32,
    pub failing: u32,
    pub pending: u32,
}

impl ChecksSummary {
    pub fn total(&self) -> u32 {
        self.passing + self.failing + self.pending
    }
}

/// A requested reviewer, which may be an individual or a team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewerRef {
    User(String),
    Team(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestReview {
    pub author: String,
    pub state: ReviewState,
}

/// Per-PR status resolved by `gh pr view` (the enrichment phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichedStatus {
    pub review_decision: Option<ReviewDecision>,
    pub merge_state: MergeState,
    pub checks: ChecksSummary,
    pub review_requests: Vec<ReviewerRef>,
    pub latest_reviews: Vec<LatestReview>,
    pub head_branch: String,
    pub is_cross_repository: bool,
}

/// PR status as it moves through the background enrichment lookup.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EnrichStatus {
    #[default]
    Loading,
    Ready(EnrichedStatus),
    Failed,
}

/// One PR as shown in the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// Owner login (from `repository.nameWithOwner`).
    pub owner: String,
    /// Short repo name (`repository.name`).
    pub repo: String,
    pub is_draft: bool,
    /// ISO-8601 timestamp; used for ordering.
    pub updated_at: String,
    pub author: String,
    pub relation: Relation,
    pub status: EnrichStatus,
}

impl DashboardPr {
    pub fn id(&self) -> PrId {
        PrId {
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            number: self.number,
        }
    }
}

/// Default scope at startup: the current repo when detectable from `origin`,
/// otherwise everything.
pub fn default_scope() -> Scope {
    match github::origin_owner_repo() {
        Ok(Some((owner, repo))) => Scope::CurrentRepo { owner, repo },
        _ => Scope::Global,
    }
}

/// Build the `gh search prs` argument vector for a scope + relation. Pure, so it
/// is unit-tested without spawning `gh`.
fn search_args(scope: &Scope, relation: Relation) -> Vec<String> {
    let mut args = vec![
        "search".to_string(),
        "prs".to_string(),
        "--state".to_string(),
        "open".to_string(),
        "--limit".to_string(),
        "100".to_string(),
        "--json".to_string(),
        "number,title,url,repository,author,isDraft,createdAt,updatedAt".to_string(),
    ];

    match relation {
        Relation::Authored => {
            args.push("--author".to_string());
            args.push("@me".to_string());
        }
        Relation::ReviewRequested => {
            args.push("--review-requested".to_string());
            args.push("@me".to_string());
        }
    }

    match scope {
        Scope::CurrentRepo { owner, repo } => {
            args.push("--repo".to_string());
            args.push(format!("{owner}/{repo}"));
        }
        Scope::Orgs(orgs) => {
            for org in orgs {
                args.push("--owner".to_string());
                args.push(org.clone());
            }
        }
        Scope::Global => {}
    }

    args
}

/// Run a single search and return the matching PRs (status `Loading`).
pub fn search(scope: &Scope, relation: Relation) -> Result<Vec<DashboardPr>, PrError> {
    let args = search_args(scope, relation);

    let output = Command::new("gh")
        .args(&args)
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => PrError::GhNotFound,
            _ => PrError::GhFailed(e.to_string()),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(PrError::GhFailed(if stderr.is_empty() {
            "gh exited with an error".to_string()
        } else {
            stderr
        }));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_search(&stdout, relation)
}

// `gh search prs --json` shapes (only the fields we consume).
#[derive(Deserialize)]
struct RawSearchPr {
    number: u64,
    title: String,
    url: String,
    repository: RawRepository,
    #[serde(default)]
    author: RawAuthor,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Deserialize)]
struct RawRepository {
    name: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize, Default)]
struct RawAuthor {
    #[serde(default)]
    login: String,
}

/// Owner from `repository.nameWithOwner` (split on the first `/`). A name with
/// no slash yields an empty owner rather than panicking.
fn split_owner(name_with_owner: &str) -> String {
    name_with_owner
        .split_once('/')
        .map(|(owner, _)| owner.to_string())
        .unwrap_or_default()
}

fn parse_search(json: &str, relation: Relation) -> Result<Vec<DashboardPr>, PrError> {
    let raws: Vec<RawSearchPr> =
        serde_json::from_str(json.trim()).map_err(|e| PrError::ParseFailed(e.to_string()))?;

    Ok(raws
        .into_iter()
        .map(|r| DashboardPr {
            number: r.number,
            title: r.title,
            url: r.url,
            owner: split_owner(&r.repository.name_with_owner),
            repo: r.repository.name,
            is_draft: r.is_draft,
            updated_at: r.updated_at,
            author: r.author.login,
            relation,
            status: EnrichStatus::Loading,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_current_repo_authored() {
        let args = search_args(
            &Scope::CurrentRepo {
                owner: "dash0hq".to_string(),
                repo: "dash0".to_string(),
            },
            Relation::Authored,
        );
        assert!(args.windows(2).any(|w| w == ["--author", "@me"]));
        assert!(args.windows(2).any(|w| w == ["--repo", "dash0hq/dash0"]));
        assert!(!args.iter().any(|a| a == "--owner"));
    }

    #[test]
    fn test_search_args_orgs_review_requested() {
        let args = search_args(
            &Scope::Orgs(vec!["a".to_string(), "b".to_string()]),
            Relation::ReviewRequested,
        );
        assert!(args.windows(2).any(|w| w == ["--review-requested", "@me"]));
        assert!(args.windows(2).any(|w| w == ["--owner", "a"]));
        assert!(args.windows(2).any(|w| w == ["--owner", "b"]));
        assert!(!args.iter().any(|a| a == "--repo"));
    }

    #[test]
    fn test_search_args_global_has_no_scope_qualifier() {
        let args = search_args(&Scope::Global, Relation::Authored);
        assert!(!args.iter().any(|a| a == "--repo" || a == "--owner"));
    }

    #[test]
    fn test_split_owner() {
        assert_eq!(split_owner("dash0hq/dash0"), "dash0hq");
        assert_eq!(split_owner("reckerp/gx"), "reckerp");
        // A name with no slash does not panic.
        assert_eq!(split_owner("weird"), "");
    }

    #[test]
    fn test_parse_search_maps_repository_and_relation() {
        let json = r#"[
          {"number":14359,"title":"feat: webhook ingestion","url":"https://github.com/dash0hq/dash0/pull/14359",
           "repository":{"name":"dash0","nameWithOwner":"dash0hq/dash0"},
           "author":{"login":"recker"},"isDraft":false,"updatedAt":"2026-06-26T10:00:00Z"},
          {"number":198,"title":"docs: summary","url":"https://github.com/dash0hq/darkplane/pull/198",
           "repository":{"name":"darkplane","nameWithOwner":"dash0hq/darkplane"},
           "author":{"login":"recker"},"isDraft":true,"updatedAt":"2026-06-25T09:00:00Z"}
        ]"#;

        let prs = parse_search(json, Relation::Authored).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].owner, "dash0hq");
        assert_eq!(prs[0].repo, "dash0");
        assert_eq!(prs[0].number, 14359);
        assert_eq!(prs[0].relation, Relation::Authored);
        assert_eq!(prs[0].status, EnrichStatus::Loading);
        assert!(prs[1].is_draft);
        assert_eq!(prs[1].id().repo, "darkplane");
    }

    #[test]
    fn test_parse_search_empty_is_empty_vec() {
        assert!(parse_search("[]", Relation::Authored).unwrap().is_empty());
    }

    #[test]
    fn test_parse_search_invalid_json_errors() {
        assert!(matches!(
            parse_search("not json", Relation::Authored),
            Err(PrError::ParseFailed(_))
        ));
    }

    #[test]
    fn test_scope_label() {
        assert_eq!(
            Scope::CurrentRepo {
                owner: "a".to_string(),
                repo: "b".to_string()
            }
            .label(),
            "a/b"
        );
        assert_eq!(Scope::Global.label(), "all repos");
    }
}
