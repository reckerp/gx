use super::{Term, render_help_bar};
use crate::git::branch::BranchInfo;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::time::{Duration, Instant};

const DEBOUNCE_MS: u64 = 150;

fn filter_branches(branches: &[String], query: &str) -> Vec<String> {
    if query.is_empty() {
        return branches.to_vec();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches: Vec<_> = branches
        .iter()
        .filter_map(|b| matcher.fuzzy_match(b, query).map(|score| (score, b)))
        .collect();

    matches.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    matches.into_iter().map(|(_, b)| b.clone()).collect()
}

fn render_search_bar(query: &str) -> Paragraph<'_> {
    Paragraph::new(query).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Fuzzy Search "),
    )
}

fn render_branch_list(branches: &[String], selected: usize) -> List<'_> {
    let items: Vec<ListItem> = branches
        .iter()
        .enumerate()
        .map(|(i, branch)| {
            let style = if i == selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(branch.as_str()).style(style)
        })
        .collect();

    List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Branches "))
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol(">> ")
}

fn format_relative_time(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let diff = now - timestamp;

    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{} min{} ago", mins, if mins == 1 { "" } else { "s" })
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else if diff < 604800 {
        let days = diff / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    } else if diff < 2592000 {
        let weeks = diff / 604800;
        format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" })
    } else {
        let months = diff / 2592000;
        format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
    }
}

fn render_info_pane<'a>(info: Option<&BranchInfo>, loading: bool) -> Paragraph<'a> {
    let content = if loading {
        "Loading...".to_string()
    } else if let Some(info) = info {
        let mut lines = Vec::new();

        if info.is_current {
            lines.push(format!("{} (current)", info.name));
        } else {
            lines.push(info.name.clone());
        }
        lines.push(String::new());

        // ahead/behind info
        if let Some((ahead, behind)) = info.ahead_behind {
            if ahead > 0 || behind > 0 {
                let mut parts = Vec::new();
                if ahead > 0 {
                    parts.push(format!("+{} ahead", ahead));
                }
                if behind > 0 {
                    parts.push(format!("-{} behind", behind));
                }
                lines.push(parts.join(", "));
                lines.push(String::new());
            }
        }

        // Latest commit
        lines.push("Latest commit:".to_string());
        lines.push(format!("  {} {}", info.short_id, info.summary));
        lines.push(format!("  {} <{}>", info.author_name, info.author_email));
        lines.push(format!("  {}", format_relative_time(info.commit_time)));

        // Recent commits
        if info.recent_commits.len() > 1 {
            lines.push(String::new());
            lines.push("Recent commits:".to_string());
            for (_, msg) in info.recent_commits.iter().skip(1).take(4).enumerate() {
                lines.push(format!("  > {}", msg));
            }
        }

        lines.join("\n")
    } else {
        "Select a branch to view details".to_string()
    };

    Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Branch Info "),
        )
        .wrap(Wrap { trim: false })
}

pub fn run(terminal: &mut Term, all_branches: &[String]) -> miette::Result<Option<String>> {
    let mut query = String::new();
    let mut selected_index = 0;
    let mut last_selected: Option<String> = None;
    let mut branch_info: Option<BranchInfo> = None;
    let mut info_loading = false;
    let mut last_selection_change = Instant::now();
    let mut pending_info_fetch = false;

    loop {
        let filtered = filter_branches(all_branches, &query);

        if selected_index >= filtered.len() && !filtered.is_empty() {
            selected_index = filtered.len() - 1;
        }

        let current_selected = filtered.get(selected_index).cloned();

        // check selection changed
        if current_selected != last_selected {
            last_selected = current_selected.clone();
            pending_info_fetch = true;
            last_selection_change = Instant::now();
            info_loading = true;
            branch_info = None;
        }

        // check should fetch info (debounce expired)
        if pending_info_fetch
            && last_selection_change.elapsed() >= Duration::from_millis(DEBOUNCE_MS)
        {
            pending_info_fetch = false;
            if let Some(ref branch_name) = last_selected {
                match BranchInfo::fetch(branch_name) {
                    Ok(info) => {
                        branch_info = Some(info);
                    }
                    Err(_) => {
                        // keep branch_info as None on error
                    }
                }
            }
            info_loading = false;
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
                    .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
                    .split(main_chunks[1]);

                f.render_widget(render_search_bar(&query), main_chunks[0]);
                f.render_widget(
                    render_branch_list(&filtered, selected_index),
                    middle_chunks[0],
                );
                f.render_widget(
                    render_info_pane(branch_info.as_ref(), info_loading),
                    middle_chunks[1],
                );
                f.render_widget(
                    render_help_bar(&[
                        ("^/k", "Up"),
                        ("v/j", "Down"),
                        ("Enter", "Select"),
                        ("Esc", "Cancel"),
                    ]),
                    main_chunks[2],
                );
            })
            .into_diagnostic()?;

        if event::poll(Duration::from_millis(50)).into_diagnostic()? {
            if let Event::Key(key) = event::read().into_diagnostic()? {
                match (key.code, key.modifiers) {
                    (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        return Ok(None);
                    }
                    (KeyCode::Enter, _) => return Ok(filtered.get(selected_index).cloned()),
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
}
