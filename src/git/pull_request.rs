use super::worktree::Worktree;
use std::collections::{HashMap, HashSet};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestState {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PullRequestState {
    pub fn label(self) -> &'static str {
        match self {
            PullRequestState::Open => "open",
            PullRequestState::Draft => "draft",
            PullRequestState::Merged => "merged",
            PullRequestState::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestSummary {
    pub number: usize,
    pub state: PullRequestState,
    pub url: String,
}

#[derive(Debug)]
pub enum PullRequestLookupError {
    Io,
    CommandFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PullRequestCandidate {
    head_ref_name: String,
    summary: PullRequestSummary,
}

pub fn list_for_worktrees(
    worktrees: &[Worktree],
) -> Result<HashMap<String, PullRequestSummary>, PullRequestLookupError> {
    let branch_names: HashSet<&str> = worktrees
        .iter()
        .filter_map(|worktree| worktree.branch.as_deref())
        .collect();

    if branch_names.is_empty() {
        return Ok(HashMap::new());
    }

    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "500",
            "--json",
            "headRefName,isDraft,number,state,url",
            "--template",
            "{{range .}}{{.headRefName}}{{\"\\t\"}}{{.number}}{{\"\\t\"}}{{.state}}{{\"\\t\"}}{{.isDraft}}{{\"\\t\"}}{{.url}}{{\"\\n\"}}{{end}}",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|_| PullRequestLookupError::Io)?;

    if !output.status.success() {
        return Err(PullRequestLookupError::CommandFailed);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_pr_list(&stdout)
        .into_iter()
        .filter(|candidate| branch_names.contains(candidate.head_ref_name.as_str()))
        .fold(HashMap::new(), |mut summaries, candidate| {
            summaries
                .entry(candidate.head_ref_name)
                .and_modify(|existing| {
                    if should_replace(existing.state, candidate.summary.state) {
                        *existing = candidate.summary.clone();
                    }
                })
                .or_insert(candidate.summary);
            summaries
        }))
}

fn parse_pr_list(output: &str) -> Vec<PullRequestCandidate> {
    output.lines().filter_map(parse_pr_line).collect()
}

fn parse_pr_line(line: &str) -> Option<PullRequestCandidate> {
    let mut fields = line.splitn(5, '\t');
    let head_ref_name = fields.next()?.to_string();
    let number = fields.next()?.parse().ok()?;
    let raw_state = fields.next()?;
    let is_draft = fields.next()? == "true";
    let url = fields.next()?.to_string();

    Some(PullRequestCandidate {
        head_ref_name,
        summary: PullRequestSummary {
            number,
            state: parse_state(raw_state, is_draft)?,
            url,
        },
    })
}

fn parse_state(raw_state: &str, is_draft: bool) -> Option<PullRequestState> {
    match raw_state {
        "OPEN" if is_draft => Some(PullRequestState::Draft),
        "OPEN" => Some(PullRequestState::Open),
        "MERGED" => Some(PullRequestState::Merged),
        "CLOSED" => Some(PullRequestState::Closed),
        _ => None,
    }
}

fn should_replace(existing: PullRequestState, candidate: PullRequestState) -> bool {
    state_rank(candidate) > state_rank(existing)
}

fn state_rank(state: PullRequestState) -> u8 {
    match state {
        PullRequestState::Open | PullRequestState::Draft => 3,
        PullRequestState::Merged => 2,
        PullRequestState::Closed => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pr_list_maps_states() {
        let output = "\
feature\t12\tOPEN\tfalse\thttps://github.com/acme/repo/pull/12
draft-work\t13\tOPEN\ttrue\thttps://github.com/acme/repo/pull/13
merged-work\t14\tMERGED\tfalse\thttps://github.com/acme/repo/pull/14
closed-work\t15\tCLOSED\tfalse\thttps://github.com/acme/repo/pull/15
";

        let prs = parse_pr_list(output);

        assert_eq!(prs.len(), 4);
        assert_eq!(prs[0].summary.state, PullRequestState::Open);
        assert_eq!(prs[1].summary.state, PullRequestState::Draft);
        assert_eq!(prs[2].summary.state, PullRequestState::Merged);
        assert_eq!(prs[3].summary.state, PullRequestState::Closed);
    }

    #[test]
    fn test_should_replace_prefers_active_prs() {
        assert!(should_replace(
            PullRequestState::Merged,
            PullRequestState::Open
        ));
        assert!(should_replace(
            PullRequestState::Closed,
            PullRequestState::Draft
        ));
        assert!(!should_replace(
            PullRequestState::Open,
            PullRequestState::Merged
        ));
    }
}
