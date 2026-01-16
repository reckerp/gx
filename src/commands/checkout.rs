use crate::git::{GitError, branch, commit, fetch};
use crate::ui;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use miette::{Diagnostic, Result};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum CheckoutError {
    #[error("Could not read git branches")]
    #[diagnostic(code(gx::git::read_error), help("Are you in a git repository?"))]
    GitError(#[from] GitError),
    #[error("No branch or commit matches query: {query}")]
    #[diagnostic(
        code(gx::git::no_match),
        help(
            "The search string '{query}' didn't match any local or remote branches. Try 'gx checkout' to search for valid branches."
        )
    )]
    NoMatch { query: String },
    #[error("TUI Error: {0}")]
    TuiError(String),
}

enum CheckoutTarget {
    Branch(String),
    Commit(String),
}

pub fn run(query: Option<String>) -> Result<()> {
    let branches = branch::get_branches().map_err(CheckoutError::GitError)?;

    let target = match query {
        Some(q) => {
            let mut result: Option<CheckoutTarget> = None;
            let mut branches = branches;

            for attempt in 0..2 {
                if result.is_none() {
                    if let Some(branch) = fuzzy_match_branch(&q, &branches) {
                        result = Some(CheckoutTarget::Branch(branch));
                    } else if commit::is_valid_commit_ref(&q) {
                        result = Some(CheckoutTarget::Commit(q.clone()));
                    }
                }

                if result.is_some() {
                    break;
                }

                if attempt == 0 {
                    fetch::fetch().map_err(CheckoutError::GitError)?;
                    branches = branch::get_branches().map_err(CheckoutError::GitError)?;
                }
            }

            result.ok_or_else(|| CheckoutError::NoMatch { query: q })?
        }
        None => {
            let mut terminal = ui::terminal::setup_terminal()
                .map_err(|e| CheckoutError::TuiError(e.to_string()))?;

            let selection = ui::branch_picker::run(&mut terminal, &branches);

            // restore prev terminal state
            ui::terminal::restore_terminal(terminal)
                .map_err(|e| CheckoutError::TuiError(e.to_string()))?;

            match selection? {
                Some(branch) => CheckoutTarget::Branch(branch),
                None => {
                    println!("Checkout cancelled.");
                    return Ok(());
                }
            }
        }
    };

    match target {
        CheckoutTarget::Branch(branch_name) => {
            branch::checkout_branch(&branch_name)?;
            println!("Switched to branch '{}'", branch_name);
        }
        CheckoutTarget::Commit(commit_ref) => {
            let short_id = commit::checkout_commit(&commit_ref)?;
            println!("Switched to commit '{}'", short_id);
        }
    }

    Ok(())
}

fn fuzzy_match_branch(query: &str, branches: &[String]) -> Option<String> {
    let matcher = SkimMatcherV2::default();

    branches
        .iter()
        .filter_map(|branch| {
            if branch.eq_ignore_ascii_case(query) {
                Some((i64::MAX, branch)) // prioritize exact matches
            } else {
                matcher
                    .fuzzy_match(branch, query)
                    .map(|score| (score, branch))
            }
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, branch)| branch.clone())
}
