//! `gx review` orchestration: resolve the diff range, then (in later units)
//! build the diff model and launch the review TUI. For now this prints the
//! resolved range so the wiring can be exercised end-to-end.

use crate::git::review::{diff, range};
use miette::Result;

pub fn run(target: Option<String>, base: Option<String>) -> Result<()> {
    let range = range::resolve(target, base)?;
    let files = diff::changed_files(&range)?;

    println!(
        "gx review — {} (mode {:?}, scope {})",
        range.label, range.mode, range.scope_id
    );
    println!("{} file(s) changed", files.len());
    for f in &files {
        let fd = f.build(range.to)?;
        let (mut adds, mut dels) = (0usize, 0usize);
        for hunk in &fd.hunks {
            for row in &hunk.rows {
                match row.kind {
                    diff::RowKind::Added => adds += 1,
                    diff::RowKind::Removed => dels += 1,
                    diff::RowKind::Context => {}
                }
            }
        }
        let name = match &f.old_path {
            Some(old) => format!("{old} -> {}", f.path),
            None => f.path.clone(),
        };
        let tag = if fd.is_binary {
            " (binary)".to_string()
        } else if fd.too_large {
            " (too large)".to_string()
        } else {
            format!(" +{adds} -{dels}")
        };
        println!("  {name}{tag}");
    }
    println!("(diff viewer not yet wired — coming in U3–U4)");

    Ok(())
}
