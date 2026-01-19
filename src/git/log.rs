use super::{GitError, get_repo};
use crate::git::time;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub oid: git2::Oid,
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub time_relative: String,
    pub is_merge: bool,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LogGraph {
    pub entries: Vec<LogEntry>,
    pub graph_lines: Vec<String>,
}

pub fn get_log(limit: usize) -> Result<LogGraph, GitError> {
    let repo = get_repo()?;
    let mut revwalk = repo.revwalk()?;

    revwalk.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)?;

    if let Ok(head) = repo.head()
        && let Some(oid) = head.target()
    {
        revwalk.push(oid)?;
    }

    let ref_map = build_ref_map(&repo)?;
    let mut entries = Vec::new();
    let mut parent_map: HashMap<git2::Oid, Vec<git2::Oid>> = HashMap::new();

    for oid_result in revwalk.take(limit) {
        let oid = oid_result?;
        let commit = repo.find_commit(oid)?;

        let short_id = commit
            .as_object()
            .short_id()?
            .as_str()
            .unwrap_or("")
            .to_string();

        let summary = commit.summary().unwrap_or("").to_string();
        let author = commit.author();
        let author_name = author.name().unwrap_or("Unknown").to_string();
        let commit_time = commit.time().seconds();
        let time_relative = time::format_relative(time::now_secs() - commit_time);
        let is_merge = commit.parent_count() > 1;

        let refs = ref_map.get(&oid).cloned().unwrap_or_default();

        let parent_oids: Vec<git2::Oid> = commit.parent_ids().collect();
        parent_map.insert(oid, parent_oids);

        entries.push(LogEntry {
            oid,
            short_id,
            summary,
            author_name,
            time_relative,
            is_merge,
            refs,
        });
    }

    let graph_lines = build_graph(&entries, &parent_map);

    Ok(LogGraph {
        entries,
        graph_lines,
    })
}

fn build_ref_map(repo: &git2::Repository) -> Result<HashMap<git2::Oid, Vec<String>>, GitError> {
    let mut ref_map: HashMap<git2::Oid, Vec<String>> = HashMap::new();

    for reference in repo.references()? {
        let reference = reference?;
        if let Some(name) = reference.shorthand() {
            if let Some(oid) = reference.target() {
                ref_map.entry(oid).or_default().push(name.to_string());
            } else if let Ok(resolved) = reference.resolve()
                && let Some(oid) = resolved.target()
            {
                ref_map.entry(oid).or_default().push(name.to_string());
            }
        }
    }

    Ok(ref_map)
}

fn build_graph(
    entries: &[LogEntry],
    parent_map: &HashMap<git2::Oid, Vec<git2::Oid>>,
) -> Vec<String> {
    let mut graph_lines = Vec::with_capacity(entries.len());
    let mut lanes: Vec<Option<git2::Oid>> = Vec::new();

    for entry in entries {
        let oid = entry.oid;
        let parents = parent_map.get(&oid).cloned().unwrap_or_default();

        // Find all lanes waiting for this commit
        let matching_lanes: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| if *l == Some(oid) { Some(i) } else { None })
            .collect();

        // Commit appears in first matching lane, or find/create new one
        let commit_lane = if let Some(&first) = matching_lanes.first() {
            first
        } else if let Some(free) = lanes.iter().position(|l| l.is_none()) {
            lanes[free] = Some(oid);
            free
        } else {
            lanes.push(Some(oid));
            lanes.len() - 1
        };

        // Other matching lanes are closing (merging into this commit)
        let closing_lanes: Vec<usize> = matching_lanes
            .iter()
            .filter(|&&i| i != commit_lane)
            .copied()
            .collect();

        // For merge commits, add new lanes at the end (don't reuse closing lanes)
        // This allows us to show both / (merge) and \ (new branch)
        let mut new_branches: Vec<(usize, git2::Oid)> = Vec::new();
        if parents.len() > 1 {
            for &parent in parents.iter().skip(1) {
                let already_tracked = lanes.contains(&Some(parent));
                if !already_tracked {
                    let pos = lanes.len() + new_branches.len();
                    new_branches.push((pos, parent));
                }
            }
        }

        // Build the graph line
        let width = lanes
            .len()
            .max(new_branches.iter().map(|(p, _)| p + 1).max().unwrap_or(0));

        let mut line = String::new();
        for i in 0..width {
            let is_closing = closing_lanes.contains(&i);
            let is_new_branch = new_branches.iter().any(|(p, _)| *p == i);

            if i == commit_lane {
                line.push('*');
            } else if is_closing {
                // Lane merging in - show / regardless of position
                line.push('/');
            } else if is_new_branch {
                // New lane branching out
                line.push('\\');
            } else if i < lanes.len() && lanes[i].is_some() {
                line.push('|');
            } else {
                line.push(' ');
            }
        }

        graph_lines.push(line.trim_end().to_string());

        // Update lanes for next iteration
        // 1. Clear closing lanes
        for &i in &closing_lanes {
            if i < lanes.len() {
                lanes[i] = None;
            }
        }

        // 2. Update commit lane with first parent
        if parents.is_empty() {
            lanes[commit_lane] = None;
        } else {
            lanes[commit_lane] = Some(parents[0]);
        }

        // 3. Add new lanes for additional parents
        for (pos, parent) in &new_branches {
            while lanes.len() <= *pos {
                lanes.push(None);
            }
            lanes[*pos] = Some(*parent);
        }

        // 4. Compact: remove all None lanes and shift remaining left
        lanes = lanes.into_iter().flatten().map(Some).collect();
    }

    graph_lines
}

pub fn get_commit_details(oid: git2::Oid) -> Result<CommitDetails, GitError> {
    let repo = get_repo()?;
    let commit = repo.find_commit(oid)?;

    let full_id = oid.to_string();
    let summary = commit.summary().unwrap_or("").to_string();
    let body = commit.body().map(|s| s.to_string());
    let author = commit.author();
    let author_name = author.name().unwrap_or("Unknown").to_string();
    let author_email = author.email().unwrap_or("").to_string();
    let commit_time = commit.time().seconds();
    let time_relative = time::format_relative(time::now_secs() - commit_time);

    let parent_ids: Vec<String> = commit
        .parent_ids()
        .map(|p| {
            repo.find_commit(p)
                .ok()
                .and_then(|c| c.as_object().short_id().ok())
                .and_then(|s| s.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| p.to_string()[..7].to_string())
        })
        .collect();

    let ref_map = build_ref_map(&repo)?;
    let refs = ref_map.get(&oid).cloned().unwrap_or_default();

    let mut files_changed = 0;
    let mut insertions = 0;
    let mut deletions = 0;

    if let Ok(tree) = commit.tree()
        && let Ok(parent) = commit.parent(0)
        && let Ok(parent_tree) = parent.tree()
        && let Ok(diff) = repo.diff_tree_to_tree(Some(&parent_tree), Some(&tree), None)
        && let Ok(stats) = diff.stats()
    {
        files_changed = stats.files_changed();
        insertions = stats.insertions();
        deletions = stats.deletions();
    }

    Ok(CommitDetails {
        full_id,
        summary,
        body,
        author_name,
        author_email,
        time_relative,
        parent_ids,
        refs,
        files_changed,
        insertions,
        deletions,
    })
}

#[derive(Debug, Clone)]
pub struct CommitDetails {
    pub full_id: String,
    pub summary: String,
    pub body: Option<String>,
    pub author_name: String,
    pub author_email: String,
    pub time_relative: String,
    pub parent_ids: Vec<String>,
    pub refs: Vec<String>,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}
