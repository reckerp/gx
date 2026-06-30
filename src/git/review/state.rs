//! In-memory review state: the line comments a reviewer has left.
//!
//! Comments anchor to `(file, side, line-range)` and carry a snapshot of the
//! anchored line (`anchor_text`) so a later session can re-attach them even if
//! the line moved (persistence and re-anchoring land in U7). The `Serialize`
//! derives are present now so U7 can persist this type unchanged.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Which side of the diff a comment is anchored to. New-side anchors carry the
/// post-change line number an agent can act on; old-side anchors mark removed
/// or pre-change lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Old,
    New,
}

/// A single review comment, possibly spanning several lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub file: String,
    pub side: Side,
    pub start_line: usize,
    pub end_line: usize,
    /// Snapshot of the first anchored line, for re-anchoring on resume (U7).
    pub anchor_text: String,
    pub body: String,
}

/// All comments left in the current review session.
#[derive(Debug, Default)]
pub struct ReviewState {
    pub comments: Vec<Comment>,
}

/// Line numbers, per side, that carry a comment in one file — used to draw
/// gutter markers.
#[derive(Debug, Default)]
pub struct Marks {
    pub old: HashSet<usize>,
    pub new: HashSet<usize>,
}

impl ReviewState {
    pub fn add(&mut self, comment: Comment) {
        self.comments.push(comment);
    }

    pub fn total(&self) -> usize {
        self.comments.len()
    }

    pub fn count_for_file(&self, file: &str) -> usize {
        self.comments.iter().filter(|c| c.file == file).count()
    }

    /// Index of a comment whose range covers `(file, side, line)`, if any —
    /// used to find the comment under the cursor for edit/delete.
    pub fn index_at(&self, file: &str, side: Side, line: usize) -> Option<usize> {
        self.comments.iter().position(|c| {
            c.file == file && c.side == side && line >= c.start_line && line <= c.end_line
        })
    }

    pub fn set_body(&mut self, idx: usize, body: String) {
        if let Some(c) = self.comments.get_mut(idx) {
            c.body = body;
        }
    }

    pub fn remove(&mut self, idx: usize) {
        if idx < self.comments.len() {
            self.comments.remove(idx);
        }
    }

    /// Lines (per side) carrying a comment in `file`, for gutter markers.
    pub fn marks_for(&self, file: &str) -> Marks {
        let mut marks = Marks::default();
        for c in self.comments.iter().filter(|c| c.file == file) {
            let set = match c.side {
                Side::Old => &mut marks.old,
                Side::New => &mut marks.new,
            };
            for line in c.start_line..=c.end_line {
                set.insert(line);
            }
        }
        marks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(file: &str, side: Side, start: usize, end: usize) -> Comment {
        Comment {
            file: file.into(),
            side,
            start_line: start,
            end_line: end,
            anchor_text: "x".into(),
            body: "note".into(),
        }
    }

    #[test]
    fn add_and_count_per_file() {
        let mut s = ReviewState::default();
        s.add(comment("a.rs", Side::New, 1, 1));
        s.add(comment("a.rs", Side::New, 5, 5));
        s.add(comment("b.rs", Side::Old, 2, 2));
        assert_eq!(s.total(), 3);
        assert_eq!(s.count_for_file("a.rs"), 2);
        assert_eq!(s.count_for_file("b.rs"), 1);
    }

    #[test]
    fn index_at_matches_within_range_and_side() {
        let mut s = ReviewState::default();
        s.add(comment("a.rs", Side::New, 10, 12));
        assert_eq!(s.index_at("a.rs", Side::New, 11), Some(0));
        assert_eq!(s.index_at("a.rs", Side::New, 13), None);
        assert_eq!(s.index_at("a.rs", Side::Old, 11), None);
        assert_eq!(s.index_at("other.rs", Side::New, 11), None);
    }

    #[test]
    fn set_body_and_remove() {
        let mut s = ReviewState::default();
        s.add(comment("a.rs", Side::New, 1, 1));
        s.set_body(0, "edited".into());
        assert_eq!(s.comments[0].body, "edited");
        s.remove(0);
        assert_eq!(s.total(), 0);
    }

    #[test]
    fn marks_cover_the_full_range_per_side() {
        let mut s = ReviewState::default();
        s.add(comment("a.rs", Side::New, 3, 5));
        s.add(comment("a.rs", Side::Old, 9, 9));
        let marks = s.marks_for("a.rs");
        assert!(marks.new.contains(&3) && marks.new.contains(&5));
        assert!(!marks.new.contains(&2));
        assert!(marks.old.contains(&9));
    }
}
