use super::{Term, render_help_bar};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;

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

pub fn run(terminal: &mut Term, all_branches: &[String]) -> miette::Result<Option<String>> {
    let mut query = String::new();
    let mut selected_index = 0;

    loop {
        let filtered = filter_branches(all_branches, &query);

        if selected_index >= filtered.len() && !filtered.is_empty() {
            selected_index = filtered.len() - 1;
        }

        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(0),
                        Constraint::Length(3),
                    ])
                    .split(f.area());

                f.render_widget(render_search_bar(&query), chunks[0]);
                f.render_widget(render_branch_list(&filtered, selected_index), chunks[1]);
                f.render_widget(
                    render_help_bar(&[
                        ("↑/k", "Up"),
                        ("↓/j", "Down"),
                        ("Enter", "Select"),
                        ("Esc", "Cancel"),
                    ]),
                    chunks[2],
                );
            })
            .into_diagnostic()?;

        if let Event::Key(key) = event::read().into_diagnostic()? {
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
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
