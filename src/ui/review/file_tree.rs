//! The changed-file sidebar as a collapsible nested tree with a fuzzy filter.
//!
//! The tree is derived from the changed-file paths on demand (cheap for the
//! file counts a review touches); collapse state is keyed by directory path so
//! it survives rebuilds. A non-empty filter narrows to fuzzy-matching files and
//! shows them fully expanded.

use crate::git::status::FileStatus;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::collections::{BTreeMap, HashSet};

/// What a visible tree row represents.
pub enum NodeKind {
    Dir,
    File { index: usize, status: FileStatus },
}

/// One row in the rendered tree.
pub struct Row {
    pub depth: usize,
    pub name: String,
    /// Directory path (for dirs) or full file path (for files).
    pub path: String,
    pub kind: NodeKind,
    pub collapsed: bool,
}

pub struct FileTree {
    files: Vec<(String, FileStatus)>,
    collapsed: HashSet<String>,
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<(usize, String, FileStatus)>,
}

impl FileTree {
    /// Build from `(path, status)` pairs (decoupled from `ChangedFile` so it is
    /// trivially testable).
    pub fn new<I: IntoIterator<Item = (String, FileStatus)>>(files: I) -> Self {
        FileTree {
            files: files.into_iter().collect(),
            collapsed: HashSet::new(),
        }
    }

    /// Toggle the collapsed state of a directory.
    pub fn toggle(&mut self, dir_path: &str) {
        if !self.collapsed.remove(dir_path) {
            self.collapsed.insert(dir_path.to_string());
        }
    }

    pub fn collapse(&mut self, dir_path: &str) {
        self.collapsed.insert(dir_path.to_string());
    }

    /// Visible rows in display order. With a non-empty `filter`, only
    /// fuzzy-matching files (and their ancestor directories) appear, fully
    /// expanded regardless of collapse state.
    pub fn rows(&self, filter: &str) -> Vec<Row> {
        let filtering = !filter.is_empty();
        let matcher = SkimMatcherV2::default();

        let mut root = Node::default();
        for (idx, (path, status)) in self.files.iter().enumerate() {
            if filtering && matcher.fuzzy_match(path, filter).is_none() {
                continue;
            }
            insert(&mut root, idx, path, *status);
        }

        let mut out = Vec::new();
        flatten(&root, "", 0, &self.collapsed, filtering, &mut out);
        out
    }
}

fn insert(root: &mut Node, idx: usize, path: &str, status: FileStatus) {
    let comps: Vec<&str> = path.split('/').collect();
    let (dirs, name) = comps.split_at(comps.len() - 1);
    let mut node = root;
    for dir in dirs {
        node = node.dirs.entry((*dir).to_string()).or_default();
    }
    node.files.push((idx, name[0].to_string(), status));
}

fn flatten(
    node: &Node,
    prefix: &str,
    depth: usize,
    collapsed: &HashSet<String>,
    filtering: bool,
    out: &mut Vec<Row>,
) {
    // Directories first, then files, each in sorted order.
    for (name, child) in &node.dirs {
        let dir_path = join(prefix, name);
        let is_collapsed = !filtering && collapsed.contains(&dir_path);
        out.push(Row {
            depth,
            name: name.clone(),
            path: dir_path.clone(),
            kind: NodeKind::Dir,
            collapsed: is_collapsed,
        });
        if !is_collapsed {
            flatten(child, &dir_path, depth + 1, collapsed, filtering, out);
        }
    }
    for (idx, name, status) in &node.files {
        out.push(Row {
            depth,
            name: name.clone(),
            path: join(prefix, name),
            kind: NodeKind::File {
                index: *idx,
                status: *status,
            },
            collapsed: false,
        });
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> FileTree {
        FileTree::new(paths.iter().map(|p| (p.to_string(), FileStatus::Modified)))
    }

    #[test]
    fn builds_nested_tree_dirs_before_files() {
        let tree = tree(&["a/b/c.rs", "a/d.rs", "e.rs"]);
        let rows = tree.rows("");
        // a/ (dir), a/b/ (dir), c.rs, d.rs, e.rs
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c.rs", "d.rs", "e.rs"]);
        assert_eq!(rows[0].depth, 0); // a
        assert_eq!(rows[1].depth, 1); // a/b
        assert_eq!(rows[2].depth, 2); // c.rs
        assert!(matches!(rows[0].kind, NodeKind::Dir));
        assert!(matches!(rows[2].kind, NodeKind::File { .. }));
    }

    #[test]
    fn collapsing_a_directory_hides_its_children() {
        let mut tree = tree(&["a/b/c.rs", "a/d.rs", "e.rs"]);
        tree.collapse("a");
        let rows = tree.rows("");
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        // a is shown collapsed; its children are hidden; e.rs still shows.
        assert_eq!(names, vec!["a", "e.rs"]);
        assert!(rows[0].collapsed);
    }

    #[test]
    fn filter_narrows_to_matches_with_ancestors() {
        let tree = tree(&["a/b/c.rs", "a/d.rs", "e.rs"]);
        let rows = tree.rows("c.rs");
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        // Only the path to c.rs survives.
        assert_eq!(names, vec!["a", "b", "c.rs"]);
    }

    #[test]
    fn toggle_flips_collapse() {
        let mut tree = tree(&["a/b.rs"]);
        assert!(!tree.rows("")[0].collapsed);
        tree.toggle("a");
        assert!(tree.rows("")[0].collapsed);
        tree.toggle("a");
        assert!(!tree.rows("")[0].collapsed);
    }
}
