use super::{TermStderr, render_help_bar};
use crate::git::worktree::Worktree;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use fuzzy_matcher::skim::SkimMatcherV2;
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum WorkspaceAction {
    /// cd into the selected workspace
    Go(Worktree),
    /// Remove the selected workspaces
    Remove(Vec<Worktree>),
    /// Create a new workspace; `name` is pre-filled from the search query
    Create { name: String },
}

fn filter_worktrees(worktrees: &[Worktree], query: &str) -> Vec<Worktree> {
    if query.is_empty() {
        return worktrees.to_vec();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches: Vec<_> = worktrees
        .iter()
        .filter_map(|w| w.match_score(&matcher, query).map(|score| (score, w)))
        .collect();

    matches.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    matches.into_iter().map(|(_, w)| w.clone()).collect()
}

fn render_search_bar(query: &str) -> Paragraph<'_> {
    Paragraph::new(query).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Fuzzy Search "),
    )
}

fn render_workspace_list<'a>(
    worktrees: &'a [Worktree],
    selected: usize,
    selected_paths: &HashSet<PathBuf>,
) -> List<'a> {
    let items: Vec<ListItem> = worktrees
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let is_selected = i == selected;
            let is_marked = selected_paths.contains(&w.path);
            let checkbox = if w.is_main {
                " - "
            } else if is_marked {
                "[x]"
            } else {
                "[ ]"
            };

            let mut spans = vec![
                Span::styled(
                    format!("{} ", checkbox),
                    if w.is_main {
                        Style::default().fg(Color::DarkGray)
                    } else if is_marked {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(
                    w.name.clone(),
                    if is_selected {
                        Style::default().fg(Color::Yellow).bold()
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
            ];

            if let Some(branch) = &w.branch {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("[{}]", branch),
                    Style::default().fg(Color::Cyan),
                ));
            }

            if w.is_main {
                spans.push(Span::styled(" (main)", Style::default().fg(Color::Green)));
            }
            if w.is_current {
                spans.push(Span::styled(
                    " (current)",
                    Style::default().fg(Color::Magenta),
                ));
            }
            if w.is_locked {
                spans.push(Span::styled(" (locked)", Style::default().fg(Color::Red)));
            }

            let line = Line::from(spans);
            if is_selected {
                ListItem::new(line).style(Style::default().bg(Color::DarkGray))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let title = if selected_paths.is_empty() {
        format!(" Workspaces ({}) ", worktrees.len())
    } else {
        format!(
            " Workspaces ({} shown, {} selected) ",
            worktrees.len(),
            selected_paths.len()
        )
    };
    List::new(items).block(Block::default().borders(Borders::ALL).title(title))
}

fn render_info_pane<'a>(worktree: Option<&Worktree>) -> Paragraph<'a> {
    let content = if let Some(w) = worktree {
        let mut lines = vec![w.name.clone(), String::new()];

        if let Some(branch) = &w.branch {
            lines.push(format!("Branch: {}", branch));
        } else if w.is_bare {
            lines.push("Bare repository".to_string());
        } else {
            lines.push("Detached HEAD".to_string());
        }

        if let Some(head) = &w.head {
            lines.push(format!("HEAD:   {}", head));
        }

        lines.push(String::new());
        lines.push("Path:".to_string());
        lines.push(format!("  {}", w.path.display()));

        if w.is_main {
            lines.push(String::new());
            lines.push("This is the main worktree.".to_string());
        }

        lines.join("\n")
    } else {
        "No workspace matches the query.\n\nPress ctrl+n to create one.".to_string()
    };

    Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Workspace Info "),
        )
        .wrap(Wrap { trim: false })
}

fn selected_worktrees(worktrees: &[Worktree], selected_paths: &HashSet<PathBuf>) -> Vec<Worktree> {
    worktrees
        .iter()
        .filter(|w| selected_paths.contains(&w.path))
        .cloned()
        .collect()
}

fn toggle_selection(worktree: Option<&Worktree>, selected_paths: &mut HashSet<PathBuf>) {
    let Some(worktree) = worktree else {
        return;
    };

    if worktree.is_main {
        return;
    }

    if !selected_paths.insert(worktree.path.clone()) {
        selected_paths.remove(&worktree.path);
    }
}

fn toggle_visible_selection(worktrees: &[Worktree], selected_paths: &mut HashSet<PathBuf>) {
    let removable_paths: Vec<&Path> = worktrees
        .iter()
        .filter(|w| !w.is_main)
        .map(|w| w.path.as_path())
        .collect();

    if removable_paths.is_empty() {
        return;
    }

    let all_visible_selected = removable_paths
        .iter()
        .all(|path| selected_paths.contains(*path));

    if all_visible_selected {
        for path in removable_paths {
            selected_paths.remove(path);
        }
    } else {
        selected_paths.extend(removable_paths.into_iter().map(Path::to_path_buf));
    }
}

pub fn run(
    terminal: &mut TermStderr,
    worktrees: &[Worktree],
) -> miette::Result<Option<WorkspaceAction>> {
    let mut query = String::new();
    let mut selected_index = 0;
    let mut selected_paths = HashSet::new();

    loop {
        let filtered = filter_worktrees(worktrees, &query);

        if selected_index >= filtered.len() && !filtered.is_empty() {
            selected_index = filtered.len() - 1;
        }

        terminal
            .draw(|f| {
                let main_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(0),
                        Constraint::Length(3),
                    ])
                    .split(f.area());

                let middle_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(main_chunks[1]);

                f.render_widget(render_search_bar(&query), main_chunks[0]);
                f.render_widget(
                    render_workspace_list(&filtered, selected_index, &selected_paths),
                    middle_chunks[0],
                );
                f.render_widget(
                    render_info_pane(filtered.get(selected_index)),
                    middle_chunks[1],
                );
                f.render_widget(
                    render_help_bar(&[
                        ("^/k", "Up"),
                        ("v/j", "Down"),
                        ("Enter", "Go"),
                        ("Space", "Select"),
                        ("^a", "All shown"),
                        ("ctrl+n", "New"),
                        ("ctrl+d", "Remove selected"),
                        ("Esc", "Cancel"),
                    ]),
                    main_chunks[2],
                );
            })
            .into_diagnostic()?;

        if event::poll(Duration::from_millis(50)).into_diagnostic()?
            && let Event::Key(key) = event::read().into_diagnostic()?
        {
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                    return Ok(Some(WorkspaceAction::Create {
                        name: query.trim().to_string(),
                    }));
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    let selected = selected_worktrees(worktrees, &selected_paths);
                    if !selected.is_empty() {
                        return Ok(Some(WorkspaceAction::Remove(selected)));
                    }

                    if let Some(w) = filtered.get(selected_index)
                        && !w.is_main
                    {
                        return Ok(Some(WorkspaceAction::Remove(vec![w.clone()])));
                    }
                }
                (KeyCode::Char(' '), _) => {
                    toggle_selection(filtered.get(selected_index), &mut selected_paths);
                }
                (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                    toggle_visible_selection(&filtered, &mut selected_paths);
                }
                (KeyCode::Enter, _) => {
                    if let Some(w) = filtered.get(selected_index) {
                        return Ok(Some(WorkspaceAction::Go(w.clone())));
                    }
                }
                (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                    selected_index = selected_index.saturating_sub(1);
                }
                (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                    if selected_index + 1 < filtered.len() {
                        selected_index += 1;
                    }
                }
                (KeyCode::Backspace, _) => {
                    query.pop();
                    selected_index = 0;
                }
                (KeyCode::Char(c), _) => {
                    query.push(c);
                    selected_index = 0;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(name: &str, is_main: bool) -> Worktree {
        Worktree {
            name: name.to_string(),
            path: PathBuf::from(format!("/ws/{}", name)),
            branch: Some(name.to_string()),
            head: None,
            is_main,
            is_current: false,
            is_bare: false,
            is_locked: false,
        }
    }

    #[test]
    fn test_toggle_selection_ignores_main_worktree() {
        let main = worktree("repo", true);
        let feature = worktree("feature", false);
        let mut selected = HashSet::new();

        toggle_selection(Some(&main), &mut selected);
        assert!(selected.is_empty());

        toggle_selection(Some(&feature), &mut selected);
        assert!(selected.contains(&feature.path));

        toggle_selection(Some(&feature), &mut selected);
        assert!(selected.is_empty());
    }

    #[test]
    fn test_toggle_visible_selection_selects_only_removable_visible_worktrees() {
        let main = worktree("repo", true);
        let feature = worktree("feature", false);
        let fix = worktree("fix", false);
        let mut selected = HashSet::new();

        toggle_visible_selection(&[main.clone(), feature.clone(), fix.clone()], &mut selected);
        assert!(!selected.contains(&main.path));
        assert!(selected.contains(&feature.path));
        assert!(selected.contains(&fix.path));

        toggle_visible_selection(&[main, feature.clone()], &mut selected);
        assert!(!selected.contains(&feature.path));
        assert!(selected.contains(&fix.path));
    }

    #[test]
    fn test_selected_worktrees_preserves_workspace_order() {
        let main = worktree("repo", true);
        let feature = worktree("feature", false);
        let fix = worktree("fix", false);
        let selected = HashSet::from([fix.path.clone(), feature.path.clone()]);

        let selected_names: Vec<String> = selected_worktrees(&[main, feature, fix], &selected)
            .into_iter()
            .map(|w| w.name)
            .collect();

        assert_eq!(selected_names, vec!["feature", "fix"]);
    }
}
