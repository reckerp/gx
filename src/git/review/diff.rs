//! The structured diff model `gx review` renders.
//!
//! Changed files are enumerated once via git2 (cheap — paths + blob oids), then
//! each file's hunks are built lazily on first view: old and new contents are
//! loaded and run through [`similar`], which yields line operations plus
//! word-level inline changes (the `inline` feature). The model carries raw line
//! text and byte-range emphasis; turning that into colored spans is the
//! highlighter's job (U3), and pairing removed/added runs into side-by-side
//! columns is the widget's job (U4).

use super::range::{Endpoint, ReviewRange};
use crate::git::status::FileStatus;
use crate::git::{GitError, get_repo};
use git2::{DiffFindOptions, DiffOptions, Oid};
use similar::{ChangeTag, TextDiff};

/// Lines of unchanged context kept around each change when grouping into hunks.
const CONTEXT_LINES: usize = 3;
/// Files larger than this (on either side) skip diffing and render a placeholder
/// rather than stalling the UI on a huge generated/vendored file.
const MAX_LINES: usize = 5000;
/// Byte ceiling checked before any content scan, so a huge blob is not fully
/// UTF-8-converted and line-scanned just to discover it is over the line cap.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// What kind of line a row represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Context,
    Added,
    Removed,
}

/// One diff line. `old_no`/`new_no` are 1-based and present only on the side(s)
/// the line exists in (Context has both, Added only new, Removed only old).
/// `emphasis` holds byte ranges within `text` that changed at word level.
#[derive(Debug, Clone)]
pub struct Row {
    pub kind: RowKind,
    pub old_no: Option<usize>,
    pub new_no: Option<usize>,
    pub text: String,
    pub emphasis: Vec<(usize, usize)>,
}

/// A contiguous block of changes with surrounding context.
#[derive(Debug, Clone)]
pub struct Hunk {
    pub header: String,
    pub rows: Vec<Row>,
}

/// The diff for a single file.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub is_binary: bool,
    /// True when the file exceeded [`MAX_LINES`] and its body was omitted.
    pub too_large: bool,
    pub hunks: Vec<Hunk>,
}

/// A changed file as enumerated from the range, before its hunks are built.
/// Carries the blob oids so the per-file diff can be built later without
/// re-running the whole tree diff.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    old_id: Oid,
    new_id: Oid,
}

impl ChangedFile {
    /// Build the full [`FileDiff`] for this file, loading content and running
    /// the line/word diff. `to` is the range's "to" endpoint (so working-tree
    /// content is read from disk when the blob isn't in the object database).
    pub fn build(&self, to: Endpoint) -> Result<FileDiff, GitError> {
        let repo = get_repo()?;
        let (old_bytes, new_bytes) = self.load_raw(&repo, to)?;

        // Bail on oversized blobs before scanning/decoding them.
        if old_bytes.len() > MAX_BYTES || new_bytes.len() > MAX_BYTES {
            return Ok(self.empty_diff(false, true));
        }
        if looks_binary(&old_bytes) || looks_binary(&new_bytes) {
            return Ok(self.empty_diff(true, false));
        }

        let old = String::from_utf8_lossy(&old_bytes);
        let new = String::from_utf8_lossy(&new_bytes);
        if old.lines().count() > MAX_LINES || new.lines().count() > MAX_LINES {
            return Ok(self.empty_diff(false, true));
        }

        Ok(FileDiff {
            path: self.path.clone(),
            old_path: self.old_path.clone(),
            status: self.status,
            is_binary: false,
            too_large: false,
            hunks: build_hunks(&old, &new),
        })
    }

    /// Load the raw old/new bytes for this file. New content comes from the
    /// object database for committed ranges, and from the working tree when the
    /// blob isn't yet hashed (worktree / untracked).
    fn load_raw(
        &self,
        repo: &git2::Repository,
        to: Endpoint,
    ) -> Result<(Vec<u8>, Vec<u8>), GitError> {
        let old = blob_bytes(repo, self.old_id)?;
        let new = match to {
            Endpoint::Commit(_) => blob_bytes(repo, self.new_id)?,
            Endpoint::WorkingTree => {
                if self.new_id.is_zero() {
                    read_workdir(repo, &self.path)?
                } else {
                    blob_bytes(repo, self.new_id)?
                }
            }
        };
        Ok((old, new))
    }

    /// Old and new file contents as lossy UTF-8, for whole-file syntax
    /// highlighting (the highlighter indexes lines back into the diff's rows).
    pub fn load_contents(&self, to: Endpoint) -> Result<(String, String), GitError> {
        let repo = get_repo()?;
        let (old, new) = self.load_raw(&repo, to)?;
        Ok((
            String::from_utf8_lossy(&old).into_owned(),
            String::from_utf8_lossy(&new).into_owned(),
        ))
    }

    fn empty_diff(&self, is_binary: bool, too_large: bool) -> FileDiff {
        FileDiff {
            path: self.path.clone(),
            old_path: self.old_path.clone(),
            status: self.status,
            is_binary,
            too_large,
            hunks: Vec::new(),
        }
    }
}

/// Enumerate the files changed by `range` (paths, status, rename source, and
/// blob oids). Hunks are not built here — call [`ChangedFile::build`] per file.
pub fn changed_files(range: &ReviewRange) -> Result<Vec<ChangedFile>, GitError> {
    let repo = get_repo()?;

    let old_tree = match range.from {
        Some(oid) => Some(repo.find_commit(oid)?.tree()?),
        None => None,
    };

    let mut opts = DiffOptions::new();
    let mut diff = match range.to {
        Endpoint::Commit(to_oid) => {
            let new_tree = repo.find_commit(to_oid)?.tree()?;
            repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))?
        }
        Endpoint::WorkingTree => {
            opts.include_untracked(true)
                .show_untracked_content(true)
                .recurse_untracked_dirs(true);
            repo.diff_tree_to_workdir_with_index(old_tree.as_ref(), Some(&mut opts))?
        }
    };

    // Coalesce add+delete pairs into renames for nicer display.
    let mut find = DiffFindOptions::new();
    find.renames(true);
    diff.find_similar(Some(&mut find))?;

    let mut files = Vec::new();
    for delta in diff.deltas() {
        let new_path = delta.new_file().path().map(path_to_string);
        let old_path_raw = delta.old_file().path().map(path_to_string);
        let path = new_path
            .clone()
            .or_else(|| old_path_raw.clone())
            .unwrap_or_default();

        let status = map_status(delta.status());
        let old_path = if status == FileStatus::Renamed {
            old_path_raw
        } else {
            None
        };

        files.push(ChangedFile {
            path,
            old_path,
            status,
            old_id: delta.old_file().id(),
            new_id: delta.new_file().id(),
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Group the line diff into hunks with [`CONTEXT_LINES`] of surrounding context,
/// carrying word-level emphasis ranges on changed lines.
fn build_hunks(old: &str, new: &str) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(CONTEXT_LINES) {
        let mut rows = Vec::new();

        for op in &group {
            for change in diff.iter_inline_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => RowKind::Context,
                    ChangeTag::Delete => RowKind::Removed,
                    ChangeTag::Insert => RowKind::Added,
                };

                let mut text = String::new();
                let mut emphasis = Vec::new();
                for (emphasized, value) in change.iter_strings_lossy() {
                    let start = text.len();
                    text.push_str(&value);
                    if emphasized {
                        emphasis.push((start, text.len()));
                    }
                }
                trim_line_endings(&mut text, &mut emphasis);

                rows.push(Row {
                    kind,
                    old_no: change.old_index().map(|i| i + 1),
                    new_no: change.new_index().map(|i| i + 1),
                    text,
                    emphasis,
                });
            }
        }

        if !rows.is_empty() {
            hunks.push(Hunk {
                header: hunk_header(&rows),
                rows,
            });
        }
    }

    hunks
}

/// Build a `@@ -old_start,old_len +new_start,new_len @@` header from a hunk's rows.
fn hunk_header(rows: &[Row]) -> String {
    let old_lines: Vec<usize> = rows.iter().filter_map(|r| r.old_no).collect();
    let new_lines: Vec<usize> = rows.iter().filter_map(|r| r.new_no).collect();

    let old_start = old_lines.first().copied().unwrap_or(0);
    let new_start = new_lines.first().copied().unwrap_or(0);
    format!(
        "@@ -{},{} +{},{} @@",
        old_start,
        old_lines.len(),
        new_start,
        new_lines.len()
    )
}

/// Strip a trailing `\n`/`\r\n` from a line and clamp emphasis ranges that
/// pointed past the new end.
fn trim_line_endings(text: &mut String, emphasis: &mut Vec<(usize, usize)>) {
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    let len = text.len();
    emphasis.retain(|&(s, _)| s < len);
    for range in emphasis.iter_mut() {
        if range.1 > len {
            range.1 = len;
        }
    }
}

fn blob_bytes(repo: &git2::Repository, oid: Oid) -> Result<Vec<u8>, GitError> {
    if oid.is_zero() {
        return Ok(Vec::new());
    }
    // A non-zero oid that fails to load is a real error (corrupt/missing
    // object); surface it instead of silently rendering an empty/wrong diff.
    Ok(repo.find_blob(oid)?.content().to_vec())
}

fn read_workdir(repo: &git2::Repository, path: &str) -> Result<Vec<u8>, GitError> {
    let Some(root) = repo.workdir() else {
        return Ok(Vec::new());
    };
    match std::fs::read(root.join(path)) {
        Ok(bytes) => Ok(bytes),
        // A missing working-tree file is an expected empty side (e.g. deleted).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(GitError::IoError(e)),
    }
}

/// Classic git heuristic: content with a NUL byte is treated as binary.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn path_to_string(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn map_status(delta: git2::Delta) -> FileStatus {
    use git2::Delta;
    match delta {
        Delta::Added | Delta::Untracked | Delta::Copied => FileStatus::New,
        Delta::Deleted => FileStatus::Deleted,
        Delta::Renamed => FileStatus::Renamed,
        Delta::Typechange => FileStatus::Typechange,
        _ => FileStatus::Modified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(old: &str, new: &str) -> Vec<Row> {
        build_hunks(old, new).into_iter().flat_map(|h| h.rows).collect()
    }

    #[test]
    fn modified_line_yields_context_removed_added_with_numbers() {
        let rs = rows("a\nb\nc\n", "a\nB\nc\n");
        // a (context), b (removed), B (added), c (context)
        assert_eq!(rs[0].kind, RowKind::Context);
        assert_eq!(rs[0].old_no, Some(1));
        assert_eq!(rs[0].new_no, Some(1));

        let removed = rs.iter().find(|r| r.kind == RowKind::Removed).unwrap();
        assert_eq!(removed.text, "b");
        assert_eq!(removed.old_no, Some(2));
        assert_eq!(removed.new_no, None);

        let added = rs.iter().find(|r| r.kind == RowKind::Added).unwrap();
        assert_eq!(added.text, "B");
        assert_eq!(added.new_no, Some(2));
        assert_eq!(added.old_no, None);
    }

    #[test]
    fn pure_addition_has_only_added_rows() {
        let rs = rows("", "x\ny\n");
        assert!(rs.iter().all(|r| r.kind == RowKind::Added));
        assert!(rs.iter().all(|r| r.old_no.is_none()));
        assert_eq!(rs.len(), 2);
    }

    #[test]
    fn pure_deletion_has_only_removed_rows() {
        let rs = rows("x\ny\n", "");
        assert!(rs.iter().all(|r| r.kind == RowKind::Removed));
        assert!(rs.iter().all(|r| r.new_no.is_none()));
        assert_eq!(rs.len(), 2);
    }

    #[test]
    fn word_change_emphasizes_only_changed_token() {
        let rs = rows("foo bar baz\n", "foo qux baz\n");
        let removed = rs.iter().find(|r| r.kind == RowKind::Removed).unwrap();
        let added = rs.iter().find(|r| r.kind == RowKind::Added).unwrap();

        // Emphasis must cover the changed word, not the whole line.
        assert!(!removed.emphasis.is_empty());
        assert!(!added.emphasis.is_empty());
        let emph_text: String = added
            .emphasis
            .iter()
            .map(|&(s, e)| &added.text[s..e])
            .collect();
        assert!(emph_text.contains("qux"));
        assert!(!emph_text.contains("foo"));
    }

    #[test]
    fn trims_trailing_newline_and_keeps_emphasis_in_bounds() {
        let mut text = "abc\n".to_string();
        let mut emphasis = vec![(0, 4)];
        trim_line_endings(&mut text, &mut emphasis);
        assert_eq!(text, "abc");
        assert_eq!(emphasis, vec![(0, 3)]);
    }

    #[test]
    fn detects_binary_via_nul_byte() {
        assert!(looks_binary(&[b'a', 0, b'b']));
        assert!(!looks_binary(b"plain text"));
    }
}
