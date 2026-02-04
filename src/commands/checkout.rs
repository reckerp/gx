use crate::git::{branch, commit, fetch, GitError};
use crate::ui;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
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

pub fn run(create_branch: Option<String>, query: Option<String>) -> Result<()> {
    if let Some(new_branch_name) = create_branch {
        branch::create_branch(&new_branch_name, query.as_deref())
            .map_err(CheckoutError::GitError)?;

        branch::checkout_branch(&new_branch_name)?;
        println!("Switched to a new branch '{}'", new_branch_name);
        return Ok(());
    }

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

            result.ok_or(CheckoutError::NoMatch { query: q })?
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
