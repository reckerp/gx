//! Inspecting the staged (index) contents of a specific worktree, via
//! `git -C <root> diff --cached` and `git -C <root> show :<path>`.

use crate::git::GitError;
use crate::git::git_exec::{self, ExecOptions};
use std::path::Path;

/// A file staged in the index, as reported by `git diff --cached --name-status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedEntry {
    /// The current (post-rename) path of the staged file.
    pub path: String,
    pub status: StagedStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedStatus {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    Copied { from: String },
    Other(char),
}

/// List the files staged in the index of the worktree at `root`.
/// Runs `git -C <root> diff --cached --name-status -z` and parses the
/// NUL-delimited porcelain output (NUL-delimited keeps paths unambiguous and
/// gives rename/copy entries their old+new path as separate fields).
pub fn staged_entries(root: &Path) -> Result<Vec<StagedEntry>, GitError> {
    let output = git_exec::exec_in(
        root,
        &["diff", "--cached", "--name-status", "-z"],
        ExecOptions::capture(),
    )?;
    Ok(parse_name_status(&output))
}

/// Parse the NUL-delimited output of `git diff --cached --name-status -z`.
/// Each record is a status field (e.g. "A", "M", "R100") followed by one path,
/// or two paths for renames/copies (old then new). Fields are NUL-separated.
fn parse_name_status(out: &str) -> Vec<StagedEntry> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut entries = Vec::new();

    while let Some(status_field) = fields.next() {
        let code = status_field.chars().next().unwrap_or(' ');
        match code {
            'R' | 'C' => {
                let Some(from) = fields.next() else { break };
                let Some(to) = fields.next() else { break };
                let status = if code == 'R' {
                    StagedStatus::Renamed {
                        from: from.to_string(),
                    }
                } else {
                    StagedStatus::Copied {
                        from: from.to_string(),
                    }
                };
                entries.push(StagedEntry {
                    path: to.to_string(),
                    status,
                });
            }
            other => {
                let Some(path) = fields.next() else { break };
                let status = match other {
                    'A' => StagedStatus::Added,
                    'M' => StagedStatus::Modified,
                    'D' => StagedStatus::Deleted,
                    c => StagedStatus::Other(c),
                };
                entries.push(StagedEntry {
                    path: path.to_string(),
                    status,
                });
            }
        }
    }

    entries
}

/// Read the staged (index) contents of `path` in the worktree at `root` via
/// `git -C <root> show :<path>`. Returns raw bytes so binary and
/// whitespace-significant files round-trip exactly.
pub fn show_staged(root: &Path, path: &str) -> Result<Vec<u8>, GitError> {
    let spec = format!(":{}", path);
    git_exec::exec_bytes_in(root, &["show", &spec], ExecOptions::capture())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_name_status_added_modified_deleted() {
        // NUL-delimited: each record is "<status>\0<path>\0"
        let out = "A\0src/new.rs\0M\0src/main.rs\0D\0old.rs\0";
        let entries = parse_name_status(out);

        assert_eq!(
            entries,
            vec![
                StagedEntry {
                    path: "src/new.rs".to_string(),
                    status: StagedStatus::Added,
                },
                StagedEntry {
                    path: "src/main.rs".to_string(),
                    status: StagedStatus::Modified,
                },
                StagedEntry {
                    path: "old.rs".to_string(),
                    status: StagedStatus::Deleted,
                },
            ]
        );
    }

    #[test]
    fn test_parse_name_status_rename_has_from_and_to() {
        // Renames carry a similarity score (R100) then old\0new.
        let out = "R100\0src/old.rs\0src/new.rs\0M\0other.rs\0";
        let entries = parse_name_status(out);

        assert_eq!(
            entries,
            vec![
                StagedEntry {
                    path: "src/new.rs".to_string(),
                    status: StagedStatus::Renamed {
                        from: "src/old.rs".to_string(),
                    },
                },
                StagedEntry {
                    path: "other.rs".to_string(),
                    status: StagedStatus::Modified,
                },
            ]
        );
    }

    #[test]
    fn test_parse_name_status_copy_has_from_and_to() {
        let out = "C75\0src/template.rs\0src/copy.rs\0";
        let entries = parse_name_status(out);

        assert_eq!(
            entries,
            vec![StagedEntry {
                path: "src/copy.rs".to_string(),
                status: StagedStatus::Copied {
                    from: "src/template.rs".to_string(),
                },
            }]
        );
    }

    #[test]
    fn test_parse_name_status_empty() {
        assert!(parse_name_status("").is_empty());
        assert!(parse_name_status("\0").is_empty());
    }
}
