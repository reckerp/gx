//! Worktree (gx "workspace") support, split by responsibility:
//! - [`model`] — the [`Worktree`] data type, fuzzy matching, and porcelain parsing
//! - [`ops`] — filesystem/subprocess lifecycle (list/add/remove/move/lock/repair)
//! - [`summary`] — background dirty/ahead/behind + PR-state summarization
//! - [`staged`] — staged-index inspection for a specific worktree
//! - [`cleanup`] — branch-state queries and the age/orphan/gone heuristics
//!
//! The public API is re-exported here so callers use `git::worktree::*`
//! regardless of the internal split.

mod cleanup;
mod model;
mod ops;
mod staged;
mod summary;

pub use cleanup::*;
pub use model::*;
pub use ops::*;
pub use staged::*;
pub use summary::*;

#[cfg(test)]
pub(crate) mod test_support {
    use super::Worktree;
    use crate::git::pull_request::{PullRequestState, PullRequestSummary};
    use std::path::PathBuf;

    /// A bare [`Worktree`] fixture under `/ws/<name>` for the submodule tests.
    pub(crate) fn worktree(name: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            name: name.to_string(),
            path: PathBuf::from(format!("/ws/{}", name)),
            branch: branch.map(|b| b.to_string()),
            head: None,
            is_main: false,
            is_current: false,
            is_bare: false,
            is_locked: false,
        }
    }

    pub(crate) fn pr_summary(number: usize) -> PullRequestSummary {
        PullRequestSummary {
            number,
            state: PullRequestState::Open,
            url: format!("https://example.com/pull/{number}"),
        }
    }
}
