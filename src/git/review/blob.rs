//! Serialize a finished review into an agent-ready, prompt-wrapped Markdown
//! blob (copied to the clipboard by the "finish" action).
//!
//! The blob leads with a wrapping instruction, then groups comments by file,
//! each with a small diff snippet for context and the reviewer's note. Only the
//! commented regions are included — the agent reads the rest from disk.

use crate::git::review::diff::{FileDiff, RowKind};
use crate::git::review::state::Side;

/// Lines of surrounding diff context included on each side of a comment.
const CONTEXT: usize = 2;

const PROMPT_HEADER: &str = "You are addressing code-review feedback. Each item below is a comment on a specific line of a diff. Work through every comment: make the change it asks for, or briefly explain if you disagree. Line numbers refer to the post-change (new) side unless marked (old side).";

/// One comment ready to render: its file, a human location, a diff snippet, and
/// the reviewer's note.
pub struct CommentBlock {
    pub file: String,
    pub location: String,
    pub snippet: Vec<String>,
    pub body: String,
}

/// Render the wrapped Markdown blob. `blocks` should already be ordered by file
/// then line so the per-file grouping is contiguous.
pub fn build(label: &str, blocks: &[CommentBlock]) -> String {
    let mut out = String::new();
    out.push_str("# Code review feedback\n\n");
    out.push_str(PROMPT_HEADER);
    out.push_str(&format!("\n\n**Review range:** `{label}`\n"));

    let mut current_file: Option<&str> = None;
    for block in blocks {
        if current_file != Some(block.file.as_str()) {
            out.push_str(&format!("\n## `{}`\n", block.file));
            current_file = Some(block.file.as_str());
        }
        out.push_str(&format!("\n### {}\n", block.location));
        if !block.snippet.is_empty() {
            out.push_str("\n```diff\n");
            for line in &block.snippet {
                out.push_str(line);
                out.push('\n');
            }
            out.push_str("```\n");
        }
        out.push_str(&format!("\n{}\n", block.body.trim_end()));
    }

    out
}

/// A human-readable location string, e.g. `L42`, `L42-45`, or `L42 (old side)`.
pub fn location(side: Side, start: usize, end: usize) -> String {
    let range = if end > start {
        format!("L{start}-{end}")
    } else {
        format!("L{start}")
    };
    match side {
        Side::Old => format!("{range} (old side)"),
        Side::New => range,
    }
}

/// Extract a small diff snippet around `[start, end]` on `side`, each line
/// prefixed with `+`/`-`/space, for the blob's context block.
pub fn context_lines(diff: &FileDiff, side: Side, start: usize, end: usize) -> Vec<String> {
    let rows: Vec<&crate::git::review::diff::Row> =
        diff.hunks.iter().flat_map(|h| &h.rows).collect();

    let side_line = |r: &crate::git::review::diff::Row| match side {
        Side::New => r.new_no,
        Side::Old => r.old_no,
    };

    let mut first = None;
    let mut last = None;
    for (i, r) in rows.iter().enumerate() {
        if let Some(n) = side_line(r)
            && n >= start
            && n <= end
        {
            first.get_or_insert(i);
            last = Some(i);
        }
    }

    let (Some(f), Some(l)) = (first, last) else {
        return Vec::new();
    };
    let lo = f.saturating_sub(CONTEXT);
    let hi = (l + CONTEXT).min(rows.len().saturating_sub(1));

    rows[lo..=hi]
        .iter()
        .map(|r| {
            let sign = match r.kind {
                RowKind::Added => '+',
                RowKind::Removed => '-',
                RowKind::Context => ' ',
            };
            format!("{sign}{}", r.text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::review::diff::{Hunk, Row};
    use crate::git::status::FileStatus;

    fn row(kind: RowKind, old: Option<usize>, new: Option<usize>, text: &str) -> Row {
        Row {
            kind,
            old_no: old,
            new_no: new,
            text: text.to_string(),
            emphasis: Vec::new(),
        }
    }

    #[test]
    fn location_formats_range_and_side() {
        assert_eq!(location(Side::New, 42, 42), "L42");
        assert_eq!(location(Side::New, 42, 45), "L42-45");
        assert_eq!(location(Side::Old, 7, 7), "L7 (old side)");
    }

    #[test]
    fn build_groups_by_file_with_header_and_bodies() {
        let blocks = vec![
            CommentBlock {
                file: "a.rs".into(),
                location: "L1".into(),
                snippet: vec!["+let x = 1;".into()],
                body: "rename x".into(),
            },
            CommentBlock {
                file: "b.rs".into(),
                location: "L9 (old side)".into(),
                snippet: vec![],
                body: "why remove this?".into(),
            },
        ];
        let out = build("main...HEAD", &blocks);

        assert!(out.contains("# Code review feedback"));
        assert!(out.contains("**Review range:** `main...HEAD`"));
        assert!(out.contains("## `a.rs`"));
        assert!(out.contains("## `b.rs`"));
        assert!(out.contains("rename x"));
        assert!(out.contains("why remove this?"));
        assert!(out.contains("```diff"));
    }

    #[test]
    fn context_lines_window_around_target_line() {
        let diff = FileDiff {
            path: "x.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            too_large: false,
            hunks: vec![Hunk {
                header: "@@".into(),
                rows: vec![
                    row(RowKind::Context, Some(1), Some(1), "a"),
                    row(RowKind::Context, Some(2), Some(2), "b"),
                    row(RowKind::Added, None, Some(3), "c"),
                    row(RowKind::Context, Some(3), Some(4), "d"),
                    row(RowKind::Context, Some(4), Some(5), "e"),
                ],
            }],
        };
        // Comment on new-side line 3 (the added "c").
        let snippet = context_lines(&diff, Side::New, 3, 3);
        assert!(snippet.iter().any(|l| l == "+c"));
        // Includes up to CONTEXT lines of surrounding context.
        assert!(snippet.iter().any(|l| l == " b"));
        assert!(snippet.iter().any(|l| l == " d"));
    }

    #[test]
    fn context_lines_empty_when_line_absent() {
        let diff = FileDiff {
            path: "x.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            too_large: false,
            hunks: vec![Hunk {
                header: "@@".into(),
                rows: vec![row(RowKind::Context, Some(1), Some(1), "a")],
            }],
        };
        assert!(context_lines(&diff, Side::New, 99, 99).is_empty());
    }
}
