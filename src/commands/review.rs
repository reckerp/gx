//! `gx review` orchestration: resolve the diff range, then (in later units)
//! build the diff model and launch the review TUI. For now this prints the
//! resolved range so the wiring can be exercised end-to-end.

use crate::git::review::range::{self, Endpoint};
use miette::Result;

pub fn run(target: Option<String>, base: Option<String>) -> Result<()> {
    let range = range::resolve(target, base)?;

    let to = match range.to {
        Endpoint::Commit(_) => "commit",
        Endpoint::WorkingTree => "working tree",
    };
    println!(
        "gx review — {} (mode {:?}, to {to}, scope {})",
        range.label, range.mode, range.scope_id
    );
    println!("(diff viewer not yet wired — coming in U2–U4)");

    Ok(())
}
