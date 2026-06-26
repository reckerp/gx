//! PR dashboard data layer: search for the current user's open pull requests
//! across a configurable scope, and the data model the TUI renders.
//!
//! Two phases (see the plan's KTD1): [`search`] returns the PR list quickly from
//! `gh search prs` (a fixed, flat field set), then per-PR enrichment (in this
//! module's U3 half) fills in review/merge/check status via `gh pr view`.

use super::github;
use miette::Diagnostic;
use serde::Deserialize;
use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
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

/// Result of a combined (authored + review-requested) search.
pub type SearchResult = Result<Vec<DashboardPr>, PrError>;

/// Merge authored + review-requested results, dropping duplicates by [`PrId`].
/// A PR appearing in both relations keeps the `ReviewRequested` relation so it
/// short-circuits to `NeedsYourReview` (KTD3).
pub fn dedup_prs(
    authored: Vec<DashboardPr>,
    review_requested: Vec<DashboardPr>,
) -> Vec<DashboardPr> {
    let mut seen: HashSet<PrId> = HashSet::new();
    let mut out = Vec::new();
    for pr in review_requested.into_iter().chain(authored.into_iter()) {
        if seen.insert(pr.id()) {
            out.push(pr);
        }
    }
    out
}

/// Run both relation searches for `scope` on a background thread, returning the
/// combined, deduped list over a channel (sent once). Swapping the returned
/// receiver on a scope change is what makes stale results harmless — the old
/// receiver is dropped and its send discarded.
pub fn spawn_search(scope: Scope) -> Receiver<SearchResult> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let combined = (|| {
            let authored = search(&scope, Relation::Authored)?;
            let review_requested = search(&scope, Relation::ReviewRequested)?;
            Ok(dedup_prs(authored, review_requested))
        })();
        let _ = tx.send(combined);
    });
    rx
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

// ===== Enrichment + categorization (U3) =====

/// Review/merge bucket a PR falls into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Someone asked you to review (the `ReviewRequested` relation).
    NeedsYourReview,
    WaitingForReview,
    ReadyToMerge,
    ChangesRequested,
    Drafts,
    /// Not yet enriched, or enrichment failed.
    Unknown,
}

/// Pure categorization per the plan's KTD3 decision tree. Review-requested PRs
/// short-circuit to `NeedsYourReview`; drafts are known from search alone, so a
/// draft is bucketed before enrichment resolves.
pub fn categorize(pr: &DashboardPr) -> Category {
    if pr.relation == Relation::ReviewRequested {
        return Category::NeedsYourReview;
    }
    if pr.is_draft {
        return Category::Drafts;
    }
    match &pr.status {
        EnrichStatus::Loading | EnrichStatus::Failed => Category::Unknown,
        EnrichStatus::Ready(enriched) => categorize_enriched(enriched),
    }
}

fn categorize_enriched(e: &EnrichedStatus) -> Category {
    let has_changes_requested = e.review_decision == Some(ReviewDecision::ChangesRequested)
        || (e.review_decision.is_none()
            && e.latest_reviews
                .iter()
                .any(|r| r.state == ReviewState::ChangesRequested));
    if has_changes_requested {
        return Category::ChangesRequested;
    }

    let approved = e.review_decision == Some(ReviewDecision::Approved)
        || (e.review_decision.is_none()
            && e.latest_reviews
                .iter()
                .any(|r| r.state == ReviewState::Approved));
    if approved {
        return Category::ReadyToMerge;
    }

    Category::WaitingForReview
}

/// Short flag describing why a ready-to-merge PR can't merge yet, or `None` when
/// the merge state is clean or unknown.
pub fn merge_blocker_label(state: MergeState) -> Option<&'static str> {
    match state {
        MergeState::Clean | MergeState::Unknown => None,
        MergeState::Behind => Some("out of date"),
        MergeState::Dirty => Some("conflicts"),
        MergeState::Unstable => Some("failing/pending checks"),
        MergeState::Blocked => Some("blocked"),
    }
}

const ENRICH_WORKERS: usize = 8;

/// Enrich each PR via `gh pr view` on a detached coordinator thread that fans
/// work to a bounded worker pool, streaming `(PrId, result)` over a channel as
/// each completes. Modeled on [`crate::git::pull_request::spawn_lookup`] so the
/// caller (the TUI) never blocks — the `thread::scope` join happens on the
/// detached coordinator, not in this function.
pub fn spawn_enrichment(
    prs: &[DashboardPr],
) -> Receiver<(PrId, Result<EnrichedStatus, PrError>)> {
    let (tx, rx) = mpsc::channel();
    let ids: Vec<PrId> = prs.iter().map(DashboardPr::id).collect();

    thread::spawn(move || {
        if ids.is_empty() {
            return;
        }
        let next = AtomicUsize::new(0);
        let worker_count = ENRICH_WORKERS.min(ids.len());

        thread::scope(|scope| {
            for _ in 0..worker_count {
                let tx = tx.clone();
                let next = &next;
                let ids = &ids;
                scope.spawn(move || {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(id) = ids.get(i) else {
                            break;
                        };
                        let result = enrich_one(&id.owner, &id.repo, id.number);
                        // Receiver dropped (TUI closed) -> stop quietly.
                        if tx.send((id.clone(), result)).is_err() {
                            break;
                        }
                    }
                });
            }
        });
    });

    rx
}

/// Fetch one PR's enrichment via `gh pr view`. `headRefName`/`isCrossRepository`
/// ride along here because `gh search prs` cannot return them (KTD1).
pub fn enrich_one(owner: &str, repo: &str, number: u64) -> Result<EnrichedStatus, PrError> {
    let slug = format!("{owner}/{repo}");

    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &slug,
            "--json",
            "reviewDecision,mergeStateStatus,statusCheckRollup,reviewRequests,latestReviews,headRefName,isCrossRepository",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => PrError::GhNotFound,
            _ => PrError::GhFailed(e.to_string()),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(PrError::GhFailed(if stderr.is_empty() {
            format!("gh pr view {slug}#{number} failed")
        } else {
            stderr
        }));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_pr_view(&stdout)
}

#[derive(Deserialize)]
struct RawPrView {
    #[serde(rename = "reviewDecision", default)]
    review_decision: String,
    #[serde(rename = "mergeStateStatus", default)]
    merge_state_status: String,
    #[serde(rename = "statusCheckRollup", default)]
    status_check_rollup: Vec<RawCheck>,
    #[serde(rename = "reviewRequests", default)]
    review_requests: Vec<RawReviewRequest>,
    #[serde(rename = "latestReviews", default)]
    latest_reviews: Vec<RawLatestReview>,
    #[serde(rename = "headRefName", default)]
    head_ref_name: String,
    #[serde(rename = "isCrossRepository", default)]
    is_cross_repository: bool,
}

#[derive(Deserialize, Default)]
struct RawCheck {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    state: String,
}

#[derive(Deserialize, Default)]
struct RawReviewRequest {
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawLatestReview {
    #[serde(default)]
    author: RawAuthor,
    #[serde(default)]
    state: String,
}

fn parse_pr_view(json: &str) -> Result<EnrichedStatus, PrError> {
    let raw: RawPrView =
        serde_json::from_str(json.trim()).map_err(|e| PrError::ParseFailed(e.to_string()))?;

    Ok(EnrichedStatus {
        review_decision: parse_review_decision(&raw.review_decision),
        merge_state: parse_merge_state(&raw.merge_state_status),
        checks: summarize_checks(&raw.status_check_rollup),
        review_requests: raw.review_requests.iter().map(parse_reviewer_ref).collect(),
        latest_reviews: raw
            .latest_reviews
            .iter()
            .map(|r| LatestReview {
                author: r.author.login.clone(),
                state: parse_review_state(&r.state),
            })
            .collect(),
        head_branch: raw.head_ref_name,
        is_cross_repository: raw.is_cross_repository,
    })
}

fn parse_review_decision(s: &str) -> Option<ReviewDecision> {
    match s {
        "APPROVED" => Some(ReviewDecision::Approved),
        "CHANGES_REQUESTED" => Some(ReviewDecision::ChangesRequested),
        "REVIEW_REQUIRED" => Some(ReviewDecision::ReviewRequired),
        _ => None,
    }
}

fn parse_merge_state(s: &str) -> MergeState {
    match s {
        "CLEAN" => MergeState::Clean,
        "BLOCKED" => MergeState::Blocked,
        "BEHIND" => MergeState::Behind,
        "DIRTY" => MergeState::Dirty,
        "UNSTABLE" => MergeState::Unstable,
        _ => MergeState::Unknown,
    }
}

fn parse_review_state(s: &str) -> ReviewState {
    match s {
        "APPROVED" => ReviewState::Approved,
        "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
        "COMMENTED" => ReviewState::Commented,
        "DISMISSED" => ReviewState::Dismissed,
        _ => ReviewState::Pending,
    }
}

fn parse_reviewer_ref(r: &RawReviewRequest) -> ReviewerRef {
    if let Some(login) = &r.login {
        ReviewerRef::User(login.clone())
    } else if let Some(slug) = &r.slug {
        ReviewerRef::Team(slug.clone())
    } else if let Some(name) = &r.name {
        ReviewerRef::Team(name.clone())
    } else {
        ReviewerRef::User(String::new())
    }
}

enum CheckOutcome {
    Passing,
    Failing,
    Pending,
}

fn classify_check(c: &RawCheck) -> CheckOutcome {
    // StatusContext entries carry `state`; CheckRun entries carry status+conclusion.
    if !c.state.is_empty() {
        return match c.state.as_str() {
            "SUCCESS" => CheckOutcome::Passing,
            "FAILURE" | "ERROR" => CheckOutcome::Failing,
            _ => CheckOutcome::Pending,
        };
    }
    if c.status != "COMPLETED" {
        return CheckOutcome::Pending;
    }
    match c.conclusion.as_str() {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => CheckOutcome::Passing,
        "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
            CheckOutcome::Failing
        }
        _ => CheckOutcome::Pending,
    }
}

fn summarize_checks(checks: &[RawCheck]) -> ChecksSummary {
    let mut summary = ChecksSummary::default();
    for check in checks {
        match classify_check(check) {
            CheckOutcome::Passing => summary.passing += 1,
            CheckOutcome::Failing => summary.failing += 1,
            CheckOutcome::Pending => summary.pending += 1,
        }
    }
    summary
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

    #[test]
    fn test_dedup_prs_prefers_review_requested() {
        let make = |number: u64, relation: Relation| DashboardPr {
            number,
            title: "t".to_string(),
            url: "u".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            is_draft: false,
            updated_at: "2026-06-26T00:00:00Z".to_string(),
            author: "me".to_string(),
            relation,
            status: EnrichStatus::Loading,
        };
        // #1 is in both relations; #2 authored only; #3 review-requested only.
        let authored = vec![make(1, Relation::Authored), make(2, Relation::Authored)];
        let review = vec![make(1, Relation::ReviewRequested), make(3, Relation::ReviewRequested)];

        let merged = dedup_prs(authored, review);
        assert_eq!(merged.len(), 3);
        let one = merged.iter().find(|p| p.number == 1).unwrap();
        assert_eq!(one.relation, Relation::ReviewRequested);
    }

    // ----- U3: enrichment + categorization -----

    fn pr(relation: Relation, is_draft: bool, status: EnrichStatus) -> DashboardPr {
        DashboardPr {
            number: 1,
            title: "t".to_string(),
            url: "u".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            is_draft,
            updated_at: "2026-06-26T00:00:00Z".to_string(),
            author: "me".to_string(),
            relation,
            status,
        }
    }

    fn enriched(decision: Option<ReviewDecision>, reviews: Vec<ReviewState>) -> EnrichStatus {
        EnrichStatus::Ready(EnrichedStatus {
            review_decision: decision,
            merge_state: MergeState::Clean,
            checks: ChecksSummary::default(),
            review_requests: vec![],
            latest_reviews: reviews
                .into_iter()
                .map(|state| LatestReview {
                    author: "rev".to_string(),
                    state,
                })
                .collect(),
            head_branch: "feat/x".to_string(),
            is_cross_repository: false,
        })
    }

    #[test]
    fn test_categorize_draft_even_if_approved() {
        let p = pr(
            Relation::Authored,
            true,
            enriched(Some(ReviewDecision::Approved), vec![]),
        );
        assert_eq!(categorize(&p), Category::Drafts);
    }

    #[test]
    fn test_categorize_changes_requested_via_decision() {
        let p = pr(
            Relation::Authored,
            false,
            enriched(Some(ReviewDecision::ChangesRequested), vec![]),
        );
        assert_eq!(categorize(&p), Category::ChangesRequested);
    }

    #[test]
    fn test_categorize_approved_via_decision() {
        let p = pr(
            Relation::Authored,
            false,
            enriched(Some(ReviewDecision::Approved), vec![]),
        );
        assert_eq!(categorize(&p), Category::ReadyToMerge);
    }

    #[test]
    fn test_categorize_null_decision_falls_back_to_latest_reviews_approved() {
        let p = pr(
            Relation::Authored,
            false,
            enriched(None, vec![ReviewState::Approved]),
        );
        assert_eq!(categorize(&p), Category::ReadyToMerge);
    }

    #[test]
    fn test_categorize_null_decision_changes_requested_wins() {
        let p = pr(
            Relation::Authored,
            false,
            enriched(None, vec![ReviewState::Approved, ReviewState::ChangesRequested]),
        );
        assert_eq!(categorize(&p), Category::ChangesRequested);
    }

    #[test]
    fn test_categorize_review_required_waits() {
        let p = pr(
            Relation::Authored,
            false,
            enriched(Some(ReviewDecision::ReviewRequired), vec![]),
        );
        assert_eq!(categorize(&p), Category::WaitingForReview);
    }

    #[test]
    fn test_categorize_review_requested_relation_short_circuits() {
        let p = pr(
            Relation::ReviewRequested,
            false,
            enriched(Some(ReviewDecision::Approved), vec![]),
        );
        assert_eq!(categorize(&p), Category::NeedsYourReview);
    }

    #[test]
    fn test_categorize_loading_non_draft_is_unknown() {
        let p = pr(Relation::Authored, false, EnrichStatus::Loading);
        assert_eq!(categorize(&p), Category::Unknown);
        let p = pr(Relation::Authored, false, EnrichStatus::Failed);
        assert_eq!(categorize(&p), Category::Unknown);
    }

    #[test]
    fn test_merge_blocker_label() {
        assert_eq!(merge_blocker_label(MergeState::Behind), Some("out of date"));
        assert_eq!(merge_blocker_label(MergeState::Dirty), Some("conflicts"));
        assert_eq!(
            merge_blocker_label(MergeState::Unstable),
            Some("failing/pending checks")
        );
        assert_eq!(merge_blocker_label(MergeState::Clean), None);
        assert_eq!(merge_blocker_label(MergeState::Unknown), None);
    }

    #[test]
    fn test_parse_merge_state_unknown_does_not_panic() {
        assert_eq!(parse_merge_state("UNKNOWN"), MergeState::Unknown);
        assert_eq!(parse_merge_state("HAS_HOOKS"), MergeState::Unknown);
        assert_eq!(parse_merge_state("CLEAN"), MergeState::Clean);
    }

    #[test]
    fn test_summarize_checks_mixed_and_empty() {
        let checks = vec![
            RawCheck {
                status: "COMPLETED".to_string(),
                conclusion: "SUCCESS".to_string(),
                state: String::new(),
            },
            RawCheck {
                status: "COMPLETED".to_string(),
                conclusion: "FAILURE".to_string(),
                state: String::new(),
            },
            RawCheck {
                status: "IN_PROGRESS".to_string(),
                conclusion: String::new(),
                state: String::new(),
            },
            RawCheck {
                status: String::new(),
                conclusion: String::new(),
                state: "PENDING".to_string(),
            },
        ];
        let summary = summarize_checks(&checks);
        assert_eq!(summary.passing, 1);
        assert_eq!(summary.failing, 1);
        assert_eq!(summary.pending, 2);

        assert_eq!(summarize_checks(&[]), ChecksSummary::default());
    }

    #[test]
    fn test_parse_pr_view_full_payload() {
        let json = r#"{
          "reviewDecision":"APPROVED",
          "mergeStateStatus":"BEHIND",
          "statusCheckRollup":[
            {"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS","name":"build"},
            {"__typename":"StatusContext","state":"PENDING","context":"ci"}
          ],
          "reviewRequests":[{"__typename":"User","login":"alice"},{"__typename":"Team","slug":"backend","name":"Backend"}],
          "latestReviews":[{"author":{"login":"bob"},"state":"APPROVED"}],
          "headRefName":"feat/expose",
          "isCrossRepository":true
        }"#;

        let e = parse_pr_view(json).unwrap();
        assert_eq!(e.review_decision, Some(ReviewDecision::Approved));
        assert_eq!(e.merge_state, MergeState::Behind);
        assert_eq!(e.checks.passing, 1);
        assert_eq!(e.checks.pending, 1);
        assert_eq!(
            e.review_requests,
            vec![
                ReviewerRef::User("alice".to_string()),
                ReviewerRef::Team("backend".to_string())
            ]
        );
        assert_eq!(e.latest_reviews.len(), 1);
        assert_eq!(e.latest_reviews[0].state, ReviewState::Approved);
        assert_eq!(e.head_branch, "feat/expose");
        assert!(e.is_cross_repository);
    }

    #[test]
    fn test_parse_pr_view_missing_rollup_is_zero() {
        let json = r#"{"reviewDecision":"","mergeStateStatus":"UNKNOWN","headRefName":"main","isCrossRepository":false}"#;
        let e = parse_pr_view(json).unwrap();
        assert_eq!(e.review_decision, None);
        assert_eq!(e.merge_state, MergeState::Unknown);
        assert_eq!(e.checks, ChecksSummary::default());
        assert!(e.review_requests.is_empty());
    }
}
