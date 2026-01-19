use super::{Term, render_help_bar, status_char, status_color};
use crate::git::status::StatusFile;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::HashSet;

pub struct FilePickerResult {
    pub to_stage: Vec<String>,
    pub to_unstage: Vec<String>,
}

pub fn run(
    terminal: &mut Term,
    staged: &[StatusFile],
    unstaged: &[StatusFile],
) -> miette::Result<Option<FilePickerResult>> {
    let mut all_files: Vec<(&StatusFile, bool)> = Vec::new();

    for file in staged {
        all_files.push((file, true));
    }

    for file in unstaged {
        all_files.push((file, false));
    }

    let mut selected_index = 0;
    let mut selected_files: HashSet<usize> = all_files
        .iter()
        .enumerate()
        .filter(|(_, (_, is_staged))| *is_staged)
        .map(|(i, _)| i)
        .collect();

    if all_files.is_empty() {
        return Ok(None);
    }

    let initial_staged: HashSet<usize> = selected_files.clone();

    loop {
        let selected_count = selected_files.len();

        terminal
            .draw(|f| {
                let area = f.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)])
                    .split(area);

                let items: Vec<ListItem> = all_files
                    .iter()
                    .enumerate()
                    .map(|(i, (file, is_staged))| {
                        let is_selected = selected_files.contains(&i);
                        let checkbox = if is_selected { "[x]" } else { "[ ]" };
                        let status_ch = status_char(file.status);
                        let color = status_color(file.status);

                        let staged_indicator = if *is_staged {
                            Span::styled(" [staged] ", Style::default().fg(Color::Cyan))
                        } else {
                            Span::raw("")
                        };

                        let is_current = i == selected_index;
                        let line = Line::from(vec![
                            Span::styled(
                                format!("{} ", checkbox),
                                if is_selected {
                                    Style::default().fg(Color::Green)
                                } else {
                                    Style::default().fg(Color::DarkGray)
                                },
                            ),
                            Span::styled(format!("{} ", status_ch), Style::default().fg(color)),
                            Span::styled(
                                &file.path,
                                if is_current {
                                    Style::default().fg(Color::White).bold()
                                } else {
                                    Style::default()
                                },
                            ),
                            staged_indicator,
                        ]);

                        if is_current {
                            ListItem::new(line).style(Style::default().bg(Color::DarkGray))
                        } else {
                            ListItem::new(line)
                        }
                    })
                    .collect();

                let title = format!(" Stage Files ({} selected) ", selected_count);
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
                (KeyCode::Char('q'), _) => {
                    return Ok(None);
                }
                (KeyCode::Enter, _) => {
                    let to_stage: Vec<String> = selected_files
                        .iter()
                        .filter(|&&i| !initial_staged.contains(&i) && !all_files[i].1)
                        .map(|&i| all_files[i].0.path.clone())
                        .collect();

                    let to_unstage: Vec<String> = initial_staged
                        .iter()
                        .filter(|&&i| !selected_files.contains(&i) && all_files[i].1)
                        .map(|&i| all_files[i].0.path.clone())
                        .collect();

                    return Ok(Some(FilePickerResult {
                        to_stage,
                        to_unstage,
                    }));
                }
                (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                    selected_index = selected_index.saturating_sub(1);
                }
                (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                    if selected_index < all_files.len() - 1 {
                        selected_index += 1;
                    }
                }
                (KeyCode::Char(' '), _) => {
                    if selected_files.contains(&selected_index) {
                        selected_files.remove(&selected_index);
                    } else {
                        selected_files.insert(selected_index);
                    }
                }
                (KeyCode::Char('a'), _) => {
                    if selected_files.len() == all_files.len() {
                        selected_files.clear();
                    } else {
                        selected_files = (0..all_files.len()).collect();
                    }
                }
                _ => {}
            }
        }
    }
}
