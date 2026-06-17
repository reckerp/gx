use super::{TermStderr, render_help_bar};
use crate::git::worktree::{Worktree, WorktreeSummary};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use fuzzy_matcher::skim::SkimMatcherV2;
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum WorkspaceAction {
    /// cd into the selected workspace
    Go(Worktree),
    /// Remove the selected workspaces
    Remove {
        worktrees: Vec<Worktree>,
        delete_branches: bool,
        confirmed: bool,
    },
    /// Update selected workspaces
    Update(Vec<Worktree>),
    /// Re-copy setup files into selected workspaces
    Setup(Vec<Worktree>),
    /// Create a new workspace; `name` is pre-filled from the search query
    Create { name: String },
}

#[derive(Debug, Clone)]
enum Mode {
    List,
    ConfirmRemove {
        worktrees: Vec<Worktree>,
        delete_branches: bool,
    },
    Help,
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
    scroll_offset: usize,
    visible_height: usize,
    selected_paths: &HashSet<PathBuf>,
    summaries: &HashMap<PathBuf, WorktreeSummary>,
) -> List<'a> {
    let items: Vec<ListItem> = worktrees
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
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

            if let Some(summary) = summaries.get(&w.path) {
                spans.extend(render_summary_badges(summary));
            }

            let line = Line::from(spans);
            if is_selected {
                ListItem::new(line).style(Style::default().bg(Color::DarkGray))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let range = visible_range(worktrees.len(), scroll_offset, visible_height);
    let title = if selected_paths.is_empty() {
        format!(" Workspaces ({}, {}) ", worktrees.len(), range)
    } else {
        format!(
            " Workspaces ({}, {} selected) ",
            range,
            selected_paths.len()
        )
    };
    List::new(items).block(Block::default().borders(Borders::ALL).title(title))
}

fn render_summary_badges(summary: &WorktreeSummary) -> Vec<Span<'static>> {
    let mut badges = Vec::new();

    if summary.tracked_changes > 0 {
        badges.push(Span::styled(
            format!(" dirty:{}", summary.tracked_changes),
            Style::default().fg(Color::Yellow),
        ));
    }

    if summary.untracked_changes > 0 {
        badges.push(Span::styled(
            format!(" untracked:{}", summary.untracked_changes),
            Style::default().fg(Color::Red),
        ));
    }

    if let Some(ahead) = summary.ahead
        && ahead > 0
    {
        badges.push(Span::styled(
            format!(" +{}", ahead),
            Style::default().fg(Color::Green),
        ));
    }

    if let Some(behind) = summary.behind
        && behind > 0
    {
        badges.push(Span::styled(
            format!(" -{}", behind),
            Style::default().fg(Color::Blue),
        ));
    }

    if summary.status_error {
        badges.push(Span::styled(
            " status?".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    badges
}

fn render_info_pane<'a>(
    worktree: Option<&Worktree>,
    all_worktrees: &[Worktree],
    selected_paths: &HashSet<PathBuf>,
    summaries: &HashMap<PathBuf, WorktreeSummary>,
) -> Paragraph<'a> {
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

        if let Some(summary) = summaries.get(&w.path) {
            lines.push(String::new());
            lines.push("Status:".to_string());
            if summary.status_error {
                lines.push("  Could not read workspace status".to_string());
            } else if !summary.has_changes() && !summary.has_unpushed_commits() {
                lines.push("  Clean".to_string());
            } else {
                if summary.tracked_changes > 0 {
                    lines.push(format!("  {} tracked change(s)", summary.tracked_changes));
                }
                if summary.untracked_changes > 0 {
                    lines.push(format!("  {} untracked file(s)", summary.untracked_changes));
                }
                if summary.ahead.unwrap_or(0) > 0 || summary.behind.unwrap_or(0) > 0 {
                    lines.push(format!(
                        "  {} ahead, {} behind upstream",
                        summary.ahead.unwrap_or(0),
                        summary.behind.unwrap_or(0)
                    ));
                }
            }
        }

        if w.is_main {
            lines.push(String::new());
            lines.push("This is the main worktree.".to_string());
        }

        let selected = selected_worktrees(all_worktrees, selected_paths);
        if !selected.is_empty() {
            lines.push(String::new());
            lines.push(format!("Selected across filters: {}", selected.len()));
            for worktree in selected.iter().take(8) {
                lines.push(format!("  - {}", worktree.name));
            }
            if selected.len() > 8 {
                lines.push(format!("  ... and {} more", selected.len() - 8));
            }
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

fn action_targets(
    all_worktrees: &[Worktree],
    filtered: &[Worktree],
    selected_index: usize,
    selected_paths: &HashSet<PathBuf>,
    include_main_if_highlighted: bool,
) -> Vec<Worktree> {
    let selected = selected_worktrees(all_worktrees, selected_paths);
    if !selected.is_empty() {
        return selected;
    }

    filtered
        .get(selected_index)
        .filter(|w| include_main_if_highlighted || !w.is_main)
        .cloned()
        .into_iter()
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

fn adjust_scroll(selected: usize, scroll_offset: usize, visible_height: usize) -> usize {
    if visible_height == 0 {
        return scroll_offset;
    }

    if selected >= scroll_offset + visible_height {
        selected.saturating_sub(visible_height - 1)
    } else if selected < scroll_offset {
        selected
    } else {
        scroll_offset
    }
}

fn visible_range(total: usize, scroll_offset: usize, visible_height: usize) -> String {
    if total == 0 || visible_height == 0 {
        return "0 shown".to_string();
    }

    let start = scroll_offset.min(total - 1) + 1;
    let end = (scroll_offset + visible_height).min(total);
    format!("{}-{} of {}", start, end, total)
}

fn render_help_modal(area: Rect) -> Paragraph<'static> {
    let content = vec![
        Line::from(vec![
            Span::styled("Navigation", Style::default().bold()),
            Span::raw(": j/k, arrows, page up/down, home/end"),
        ]),
        Line::from(vec![
            Span::styled("Search", Style::default().bold()),
            Span::raw(": type to fuzzy filter, backspace to edit"),
        ]),
        Line::from(vec![
            Span::styled("Selection", Style::default().bold()),
            Span::raw(": space toggles, ctrl+a all shown, ctrl+u clears all"),
        ]),
        Line::from(vec![
            Span::styled("Actions", Style::default().bold()),
            Span::raw(": enter go, ctrl+n new, ctrl+d remove, ctrl+b remove + delete branches"),
        ]),
        Line::from(vec![
            Span::styled("Bulk actions", Style::default().bold()),
            Span::raw(": ctrl+r updates selected, ctrl+t re-copies setup files"),
        ]),
        Line::from(""),
        Line::from("Selections persist while filtering, so you can search/select several groups."),
        Line::from("Press esc or ? to close this help."),
    ];

    Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Workspace Help ({}x{}) ", area.width, area.height)),
        )
        .wrap(Wrap { trim: false })
}

fn render_remove_confirmation<'a>(
    worktrees: &'a [Worktree],
    delete_branches: bool,
) -> Paragraph<'a> {
    let mut lines = vec![Line::from(Span::styled(
        if delete_branches {
            format!(
                "Remove {} workspace(s) and delete their local branches?",
                worktrees.len()
            )
        } else {
            format!("Remove {} workspace(s)?", worktrees.len())
        },
        Style::default().fg(Color::Red).bold(),
    ))];

    if worktrees.iter().any(|w| w.is_current) {
        lines.push(Line::from(
            "The current workspace is selected; gx will switch you to the main workspace.",
        ));
    }

    lines.push(Line::from(""));
    for worktree in worktrees.iter().take(12) {
        let branch = worktree
            .branch
            .as_deref()
            .map(|branch| format!(" [{}]", branch))
            .unwrap_or_default();
        lines.push(Line::from(format!(
            "  - {}{} ({})",
            worktree.name,
            branch,
            worktree.path.display()
        )));
    }
    if worktrees.len() > 12 {
        lines.push(Line::from(format!(
            "  ... and {} more",
            worktrees.len() - 12
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Press enter/y to confirm, esc/n to go back."));

    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm Remove "),
        )
        .wrap(Wrap { trim: false })
}

pub fn run(
    terminal: &mut TermStderr,
    worktrees: &[Worktree],
    summaries: &HashMap<PathBuf, WorktreeSummary>,
) -> miette::Result<Option<WorkspaceAction>> {
    let mut query = String::new();
    let mut selected_index = 0;
    let mut scroll_offset = 0;
    let mut selected_paths = HashSet::new();
    let mut mode = Mode::List;

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

                match &mode {
                    Mode::List => {
                        let visible_height = middle_chunks[0].height.saturating_sub(2) as usize;
                        scroll_offset =
                            adjust_scroll(selected_index, scroll_offset, visible_height);

                        f.render_widget(render_search_bar(&query), main_chunks[0]);
                        f.render_widget(
                            render_workspace_list(
                                &filtered,
                                selected_index,
                                scroll_offset,
                                visible_height,
                                &selected_paths,
                                summaries,
                            ),
                            middle_chunks[0],
                        );
                        f.render_widget(
                            render_info_pane(
                                filtered.get(selected_index),
                                worktrees,
                                &selected_paths,
                                summaries,
                            ),
                            middle_chunks[1],
                        );
                        f.render_widget(
                            render_help_bar(&[
                                ("^/k", "Up"),
                                ("v/j", "Down"),
                                ("Space", "Select"),
                                ("^a", "All shown"),
                                ("^u", "Clear"),
                                ("Enter", "Go"),
                                ("^r", "Update"),
                                ("^t", "Setup"),
                                ("^d", "Remove"),
                                ("^b", "Remove+branch"),
                                ("?", "Help"),
                            ]),
                            main_chunks[2],
                        );
                    }
                    Mode::ConfirmRemove {
                        worktrees,
                        delete_branches,
                    } => {
                        f.render_widget(
                            render_remove_confirmation(worktrees, *delete_branches),
                            main_chunks[1],
                        );
                        f.render_widget(
                            render_help_bar(&[("Enter/y", "Confirm"), ("Esc/n", "Back")]),
                            main_chunks[2],
                        );
                    }
                    Mode::Help => {
                        f.render_widget(render_help_modal(main_chunks[1]), main_chunks[1]);
                        f.render_widget(render_help_bar(&[("Esc/?", "Back")]), main_chunks[2]);
                    }
                }
            })
            .into_diagnostic()?;

        if event::poll(Duration::from_millis(50)).into_diagnostic()?
            && let Event::Key(key) = event::read().into_diagnostic()?
        {
            match mode.clone() {
                Mode::List => match (key.code, key.modifiers) {
                    (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        return Ok(None);
                    }
                    (KeyCode::Char('?'), _) => {
                        mode = Mode::Help;
                    }
                    (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                        return Ok(Some(WorkspaceAction::Create {
                            name: query.trim().to_string(),
                        }));
                    }
                    (KeyCode::Char('d'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                        let delete_branches = matches!(
                            (key.code, key.modifiers),
                            (KeyCode::Char('b'), KeyModifiers::CONTROL)
                        );
                        let targets = action_targets(
                            worktrees,
                            &filtered,
                            selected_index,
                            &selected_paths,
                            false,
                        );
                        if !targets.is_empty() {
                            mode = Mode::ConfirmRemove {
                                worktrees: targets,
                                delete_branches,
                            };
                        }
                    }
                    (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                        let targets = action_targets(
                            worktrees,
                            &filtered,
                            selected_index,
                            &selected_paths,
                            true,
                        );
                        if !targets.is_empty() {
                            return Ok(Some(WorkspaceAction::Update(targets)));
                        }
                    }
                    (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                        let targets = action_targets(
                            worktrees,
                            &filtered,
                            selected_index,
                            &selected_paths,
                            true,
                        );
                        if !targets.is_empty() {
                            return Ok(Some(WorkspaceAction::Setup(targets)));
                        }
                    }
                    (KeyCode::Char(' '), _) => {
                        toggle_selection(filtered.get(selected_index), &mut selected_paths);
                    }
                    (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                        toggle_visible_selection(&filtered, &mut selected_paths);
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                        selected_paths.clear();
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
                    (KeyCode::PageUp, _) => {
                        selected_index = selected_index.saturating_sub(10);
                    }
                    (KeyCode::PageDown, _) => {
                        selected_index =
                            (selected_index + 10).min(filtered.len().saturating_sub(1));
                    }
                    (KeyCode::Home, _) => {
                        selected_index = 0;
                    }
                    (KeyCode::End, _) => {
                        selected_index = filtered.len().saturating_sub(1);
                    }
                    (KeyCode::Backspace, _) => {
                        query.pop();
                        selected_index = 0;
                        scroll_offset = 0;
                    }
                    (KeyCode::Char(c), _) => {
                        query.push(c);
                        selected_index = 0;
                        scroll_offset = 0;
                    }
                    _ => {}
                },
                Mode::ConfirmRemove {
                    worktrees,
                    delete_branches,
                } => match (key.code, key.modifiers) {
                    (KeyCode::Esc, _)
                    | (KeyCode::Char('n'), _)
                    | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        mode = Mode::List;
                    }
                    (KeyCode::Enter, _) | (KeyCode::Char('y'), _) => {
                        return Ok(Some(WorkspaceAction::Remove {
                            worktrees,
                            delete_branches,
                            confirmed: true,
                        }));
                    }
                    _ => {}
                },
                Mode::Help => match (key.code, key.modifiers) {
                    (KeyCode::Esc, _) | (KeyCode::Char('?'), _) => {
                        mode = Mode::List;
                    }
                    _ => {}
                },
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

    #[test]
    fn test_action_targets_falls_back_to_highlighted_workspace() {
        let main = worktree("repo", true);
        let feature = worktree("feature", false);
        let selected = HashSet::new();

        assert!(
            action_targets(
                &[main.clone(), feature.clone()],
                &[main.clone(), feature.clone()],
                0,
                &selected,
                false
            )
            .is_empty()
        );

        let targets = action_targets(
            &[main.clone(), feature.clone()],
            &[main, feature],
            0,
            &selected,
            true,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "repo");
    }

    #[test]
    fn test_adjust_scroll_keeps_selection_visible() {
        assert_eq!(adjust_scroll(0, 0, 5), 0);
        assert_eq!(adjust_scroll(5, 0, 5), 1);
        assert_eq!(adjust_scroll(2, 5, 5), 2);
    }

    #[test]
    fn test_visible_range() {
        assert_eq!(visible_range(0, 0, 5), "0 shown");
        assert_eq!(visible_range(12, 0, 5), "1-5 of 12");
        assert_eq!(visible_range(12, 10, 5), "11-12 of 12");
    }
}
