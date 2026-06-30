//! In-memory review state: the line comments a reviewer has left.
//!
//! Comments anchor to `(file, side, line-range)` and carry a snapshot of the
//! anchored line (`anchor_text`) so a later session can re-attach them even if
//! the line moved (persistence and re-anchoring land in U7). The `Serialize`
//! derives are present now so U7 can persist this type unchanged.

use crate::git::review::diff::{FileDiff, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

/// All comments left in the current review session, plus any that could not be
/// re-anchored to the current diff on resume.
#[derive(Debug, Default)]
pub struct ReviewState {
    pub comments: Vec<Comment>,
    pub orphaned: Vec<Comment>,
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

    /// Re-anchor this file's comments against its freshly-built diff. A comment
    /// whose anchored line still holds the same text stays; one whose
    /// `anchor_text` moved is re-anchored to the new line; one that no longer
    /// resolves is moved to `orphaned` rather than dropped.
    pub fn reanchor_file(&mut self, file: &str, diff: &FileDiff) {
        let rows: Vec<&Row> = diff.hunks.iter().flat_map(|h| &h.rows).collect();
        let line_of = |row: &Row, side: Side| match side {
            Side::New => row.new_no,
            Side::Old => row.old_no,
        };
        let text_at = |side: Side, line: usize| -> Option<String> {
            rows.iter()
                .find(|r| line_of(r, side) == Some(line))
                .map(|r| r.text.clone())
        };
        // Re-anchor a moved comment to the matching line *nearest* its original
        // position; when the two nearest matches are equidistant the choice is
        // ambiguous, so orphan rather than guess (duplicate `anchor_text` like
        // blank lines or `}` is common).
        let find_nearest = |side: Side, text: &str, origin: usize| -> Option<usize> {
            let mut candidates: Vec<usize> = rows
                .iter()
                .filter(|r| r.text == text)
                .filter_map(|r| line_of(r, side))
                .collect();
            if candidates.is_empty() {
                return None;
            }
            let dist = |l: usize| (l as isize - origin as isize).unsigned_abs();
            candidates.sort_by_key(|&l| dist(l));
            if candidates.len() >= 2 && dist(candidates[0]) == dist(candidates[1]) {
                return None; // ambiguous -> orphan
            }
            Some(candidates[0])
        };

        let (mine, rest): (Vec<Comment>, Vec<Comment>) =
            std::mem::take(&mut self.comments)
                .into_iter()
                .partition(|c| c.file == file);
        self.comments = rest;

        for mut c in mine {
            if text_at(c.side, c.start_line).as_deref() == Some(c.anchor_text.as_str()) {
                self.comments.push(c); // still anchored at the same line
            } else if let Some(new_line) = find_nearest(c.side, &c.anchor_text, c.start_line) {
                let span = c.end_line - c.start_line;
                c.start_line = new_line;
                c.end_line = new_line + span;
                self.comments.push(c);
            } else {
                self.orphaned.push(c);
            }
        }
    }
}

// --- Persistence (ephemeral, never committed) -------------------------------

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    comments: Vec<Comment>,
    #[serde(default)]
    orphaned: Vec<Comment>,
}

/// Stable storage key for a review: an FNV-1a digest of the clone's shared git
/// dir and the range scope. FNV-1a (not `DefaultHasher`) keeps the key stable
/// across Rust toolchains; the common git dir (not the per-worktree path) keeps
/// a branch's review identical across worktrees of one clone.
pub fn storage_key(common_git_dir: &Path, scope_id: &str) -> String {
    fnv1a_hex(&format!("{}\0{}", common_git_dir.display(), scope_id))
}

fn fnv1a_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn state_dir() -> PathBuf {
    std::env::temp_dir().join("gx-review")
}

fn state_path(key: &str) -> PathBuf {
    state_dir().join(format!("{key}.json"))
}

/// Load saved review state for `key`. Missing or unreadable state yields an
/// empty review rather than an error.
pub fn load(key: &str) -> ReviewState {
    let Ok(data) = std::fs::read_to_string(state_path(key)) else {
        return ReviewState::default();
    };
    match serde_json::from_str::<Persisted>(&data) {
        Ok(p) => ReviewState {
            comments: p.comments,
            orphaned: p.orphaned,
        },
        Err(_) => ReviewState::default(),
    }
}

/// Persist review state for `key` to the ephemeral temp location.
pub fn save(key: &str, state: &ReviewState) -> std::io::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    // The review dir may sit in a shared /tmp; restrict it to the current user
    // so other local users can't read review content.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let persisted = Persisted {
        comments: state.comments.clone(),
        orphaned: state.orphaned.clone(),
    };
    let json = serde_json::to_string_pretty(&persisted)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let path = state_path(key);
    // Concurrency safety net: keep the prior on-disk state as a single-level
    // .bak before overwriting, so a parallel review of the same scope can be
    // recovered instead of silently clobbered. (Best-effort, not a merge.)
    if let Ok(existing) = std::fs::read(&path)
        && existing != json.as_bytes()
    {
        let _ = std::fs::write(path.with_extension("json.bak"), &existing);
    }

    // Atomic write: a crash mid-write must not corrupt a resumable review.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)
}

/// Delete the saved review for `key` (the reset action).
pub fn reset(key: &str) -> std::io::Result<()> {
    let path = state_path(key);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
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

    #[test]
    fn storage_key_is_stable_and_scope_sensitive() {
        let dir = Path::new("/clones/gx/.git");
        let k1 = storage_key(dir, "branch:feat-review");
        // Deterministic across calls (FNV-1a, not DefaultHasher).
        assert_eq!(k1, storage_key(dir, "branch:feat-review"));
        // 16 hex chars.
        assert_eq!(k1.len(), 16);
        // Differs by scope and by clone dir.
        assert_ne!(k1, storage_key(dir, "branch:main"));
        assert_ne!(k1, storage_key(Path::new("/other/.git"), "branch:feat-review"));
    }

    #[test]
    fn save_load_reset_roundtrip() {
        // Unique key so the test doesn't collide with a real review or others.
        let key = storage_key(Path::new("/test/gx-state-test"), "branch:unit-test-xyz");
        let _ = reset(&key);

        let mut s = ReviewState::default();
        s.add(comment("a.rs", Side::New, 4, 4));
        save(&key, &s).expect("save");

        let loaded = load(&key);
        assert_eq!(loaded.total(), 1);
        assert_eq!(loaded.comments[0].file, "a.rs");

        reset(&key).expect("reset");
        assert_eq!(load(&key).total(), 0); // missing file -> empty
    }

    #[test]
    fn reanchor_keeps_moves_and_orphans() {
        use crate::git::review::diff::{Hunk, RowKind};
        use crate::git::status::FileStatus;

        let diff = FileDiff {
            path: "a.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            too_large: false,
            hunks: vec![Hunk {
                header: "@@".into(),
                rows: vec![
                    Row {
                        kind: RowKind::Context,
                        old_no: Some(1),
                        new_no: Some(1),
                        text: "alpha".into(),
                        emphasis: vec![],
                    },
                    Row {
                        kind: RowKind::Added,
                        old_no: None,
                        new_no: Some(2),
                        text: "beta".into(),
                        emphasis: vec![],
                    },
                ],
            }],
        };

        let mut s = ReviewState::default();
        // Stays: anchored at new line 1 with matching text.
        s.add(Comment {
            file: "a.rs".into(),
            side: Side::New,
            start_line: 1,
            end_line: 1,
            anchor_text: "alpha".into(),
            body: "keep".into(),
        });
        // Moves: text "beta" now lives on new line 2, comment thinks line 5.
        s.add(Comment {
            file: "a.rs".into(),
            side: Side::New,
            start_line: 5,
            end_line: 5,
            anchor_text: "beta".into(),
            body: "moved".into(),
        });
        // Orphans: anchor_text gone.
        s.add(Comment {
            file: "a.rs".into(),
            side: Side::New,
            start_line: 9,
            end_line: 9,
            anchor_text: "vanished".into(),
            body: "orphan".into(),
        });

        s.reanchor_file("a.rs", &diff);

        assert_eq!(s.comments.len(), 2);
        let moved = s.comments.iter().find(|c| c.body == "moved").unwrap();
        assert_eq!(moved.start_line, 2);
        assert_eq!(s.orphaned.len(), 1);
        assert_eq!(s.orphaned[0].body, "orphan");
    }

    #[test]
    fn reanchor_duplicate_text_picks_nearest_else_orphans() {
        use crate::git::review::diff::{Hunk, RowKind};
        use crate::git::status::FileStatus;

        let row = |new_no, text: &str| Row {
            kind: RowKind::Added,
            old_no: None,
            new_no: Some(new_no),
            text: text.into(),
            emphasis: vec![],
        };
        // "dup" appears on new lines 2 and 8.
        let diff = FileDiff {
            path: "a.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            too_large: false,
            hunks: vec![Hunk {
                header: "@@".into(),
                rows: vec![row(2, "dup"), row(5, "mid"), row(8, "dup")],
            }],
        };

        let mut s = ReviewState::default();
        // Was at line 7 -> nearest "dup" is line 8.
        s.add(Comment {
            file: "a.rs".into(),
            side: Side::New,
            start_line: 7,
            end_line: 7,
            anchor_text: "dup".into(),
            body: "near8".into(),
        });
        // Was at line 5 -> equidistant from 2 and 8 -> ambiguous -> orphan.
        s.add(Comment {
            file: "a.rs".into(),
            side: Side::New,
            start_line: 5,
            end_line: 5,
            anchor_text: "dup".into(),
            body: "ambiguous".into(),
        });

        s.reanchor_file("a.rs", &diff);

        let near = s.comments.iter().find(|c| c.body == "near8").unwrap();
        assert_eq!(near.start_line, 8);
        assert!(s.orphaned.iter().any(|c| c.body == "ambiguous"));
    }
}
