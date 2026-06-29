use super::{Term, adjust_scroll, render_help_bar};
use crate::repo_setup::CopyCandidate;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::HashSet;

pub fn run(
    terminal: &mut Term,
    candidates: &[CopyCandidate],
    initial_selection: &[String],
) -> miette::Result<Option<Vec<String>>> {
    if candidates.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let initial: HashSet<&str> = initial_selection.iter().map(String::as_str).collect();
    let mut selected_index = 0;
    let mut scroll_offset = 0;
    let mut selected_paths: HashSet<String> = candidates
        .iter()
        .filter(|candidate| initial.contains(candidate.path.as_str()))
        .map(|candidate| candidate.path.clone())
        .collect();

    loop {
        terminal
            .draw(|f| {
                let area = f.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)])
                    .split(area);

                let visible_height = chunks[0].height.saturating_sub(2) as usize;
                scroll_offset = adjust_scroll(selected_index, scroll_offset, visible_height);

                let items: Vec<ListItem> = candidates
                    .iter()
                    .enumerate()
                    .skip(scroll_offset)
                    .take(visible_height)
                    .map(|(i, candidate)| {
                        let is_selected = selected_paths.contains(&candidate.path);
                        let is_current = i == selected_index;
                        let checkbox = if is_selected { "[x]" } else { "[ ]" };
                        let display_path = if candidate.is_dir {
                            format!("{}/", candidate.path)
                        } else {
                            candidate.path.clone()
                        };

                        let line = Line::from(vec![
                            Span::styled(
                                format!("{} ", checkbox),
                                if is_selected {
                                    Style::default().fg(Color::Green)
                                } else {
                                    Style::default().fg(Color::DarkGray)
                                },
                            ),
                            Span::styled(
                                display_path,
                                if is_current {
                                    Style::default().fg(Color::White).bold()
                                } else if candidate.is_dir {
                                    Style::default().fg(Color::Cyan)
                                } else {
                                    Style::default()
                                },
                            ),
                        ]);

                        if is_current {
                            ListItem::new(line).style(Style::default().bg(Color::DarkGray))
                        } else {
                            ListItem::new(line)
                        }
                    })
                    .collect();

                let title = format!(" Setup files ({} selected) ", selected_paths.len());
                let list =
                    List::new(items).block(Block::default().borders(Borders::ALL).title(title));
                f.render_widget(list, chunks[0]);

                let help = render_help_bar(&[
                    ("j/k", "navigate"),
                    ("space", "toggle"),
                    ("a", "all"),
                    ("enter", "confirm"),
                    ("esc", "cancel"),
                ]);
                f.render_widget(help, chunks[1]);
            })
            .into_diagnostic()?;

        if let Event::Key(key) = event::read().into_diagnostic()? {
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                (KeyCode::Char('q'), _) => return Ok(None),
                (KeyCode::Enter, _) => {
                    let selected = candidates
                        .iter()
                        .filter(|candidate| selected_paths.contains(&candidate.path))
                        .map(|candidate| candidate.path.clone())
                        .collect();
                    return Ok(Some(selected));
                }
                (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                    selected_index = selected_index.saturating_sub(1);
                }
                (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                    if selected_index < candidates.len() - 1 {
                        selected_index += 1;
                    }
                }
                (KeyCode::Char(' '), _) => {
                    let path = &candidates[selected_index].path;
                    if selected_paths.contains(path) {
                        selected_paths.remove(path);
                    } else {
                        selected_paths.insert(path.clone());
                    }
                }
                (KeyCode::Char('a'), _) => {
                    if selected_paths.len() == candidates.len() {
                        selected_paths.clear();
                    } else {
                        selected_paths = candidates
                            .iter()
                            .map(|candidate| candidate.path.clone())
                            .collect();
                    }
                }
                _ => {}
            }
        }
    }
}
