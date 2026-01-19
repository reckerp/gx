use super::{Term, render_help_bar};
use crate::git::log::{CommitDetails, LogGraph};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::time::{Duration, Instant};

const DEBOUNCE_MS: u64 = 100;

pub enum LogAction {
    Checkout(git2::Oid),
    Quit,
}

pub fn run(terminal: &mut Term, log: &LogGraph) -> miette::Result<LogAction> {
    if log.entries.is_empty() {
        return Ok(LogAction::Quit);
    }

    let mut selected_index = 0;
    let mut scroll_offset = 0;
    let mut details: Option<CommitDetails> = None;
    let mut last_selected_oid: Option<git2::Oid> = None;
    let mut last_selection_change = Instant::now();
    let mut pending_fetch = false;

    loop {
        let current_oid = log.entries.get(selected_index).map(|e| e.oid);

        if current_oid != last_selected_oid {
            last_selected_oid = current_oid;
            pending_fetch = true;
            last_selection_change = Instant::now();
            details = None;
        }

        if pending_fetch && last_selection_change.elapsed() >= Duration::from_millis(DEBOUNCE_MS) {
            pending_fetch = false;
            if let Some(oid) = current_oid {
                details = crate::git::log::get_commit_details(oid).ok();
            }
        }

        terminal
            .draw(|f| {
                let area = f.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)])
                    .split(area);

                let main_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
                    .split(chunks[0]);

                let visible_height = main_chunks[0].height.saturating_sub(2) as usize;

                if selected_index >= scroll_offset + visible_height {
                    scroll_offset = selected_index.saturating_sub(visible_height - 1);
                }
                if selected_index < scroll_offset {
                    scroll_offset = selected_index;
                }

                render_log_list(f, main_chunks[0], log, selected_index, scroll_offset);
                render_details_pane(f, main_chunks[1], details.as_ref());

                let help = render_help_bar(&[
                    ("j/k", "navigate"),
                    ("enter/c", "checkout"),
                    ("q/esc", "quit"),
                ]);
                f.render_widget(help, chunks[1]);
            })
            .into_diagnostic()?;

        if event::poll(Duration::from_millis(50)).into_diagnostic()?
            && let Event::Key(key) = event::read().into_diagnostic()?
        {
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _)
                | (KeyCode::Char('q'), _)
                | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    return Ok(LogAction::Quit);
                }
                (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                    selected_index = selected_index.saturating_sub(1);
                }
                (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                    if selected_index + 1 < log.entries.len() {
                        selected_index += 1;
                    }
                }
                (KeyCode::PageUp, _) => {
                    selected_index = selected_index.saturating_sub(10);
                }
                (KeyCode::PageDown, _) => {
                    selected_index = (selected_index + 10).min(log.entries.len().saturating_sub(1));
                }
                (KeyCode::Home, _) | (KeyCode::Char('g'), _) => {
                    selected_index = 0;
                }
                (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
                    selected_index = log.entries.len().saturating_sub(1);
                }
                (KeyCode::Enter, _) | (KeyCode::Char('c'), KeyModifiers::NONE) => {
                    if let Some(entry) = log.entries.get(selected_index) {
                        return Ok(LogAction::Checkout(entry.oid));
                    }
                }
                _ => {}
            }
        }
    }
}

fn render_log_list(
    f: &mut ratatui::Frame,
    area: Rect,
    log: &LogGraph,
    selected: usize,
    scroll_offset: usize,
) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let available_width = area.width.saturating_sub(2) as usize;

    let visible_entries: Vec<Line> = log
        .entries
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(i, entry)| {
            let is_selected = i == selected;
            let graph = log.graph_lines.get(i).map(|s| s.as_str()).unwrap_or("");

            let mut spans = Vec::new();

            // Render graph with spacing between characters for readability
            for c in graph.chars() {
                let span = match c {
                    '*' => Span::styled("* ", Style::default().fg(Color::Green)),
                    '|' => Span::styled("| ", Style::default().fg(Color::Blue)),
                    '\\' => Span::styled("\\ ", Style::default().fg(Color::Magenta)),
                    '/' => Span::styled("/ ", Style::default().fg(Color::Magenta)),
                    _ => Span::raw("  "),
                };
                spans.push(span);
            }

            spans.push(Span::styled(
                format!("{} ", entry.short_id),
                Style::default().fg(Color::Yellow),
            ));

            if entry.is_merge {
                spans.push(Span::styled("Merge ", Style::default().fg(Color::Magenta)));
            }

            if !entry.refs.is_empty() {
                let ref_str = entry.refs.join(", ");
                spans.push(Span::styled(
                    format!("({}) ", ref_str),
                    Style::default().fg(Color::Cyan).bold(),
                ));
            }

            let prefix_len: usize = graph.len() * 2  // graph chars + spaces
                + entry.short_id.len()
                + 1
                + if entry.is_merge { 6 } else { 0 }
                + if entry.refs.is_empty() {
                    0
                } else {
                    entry.refs.join(", ").len() + 3
                };

            let author_time = format!(" - {} {}", entry.author_name, entry.time_relative);
            let author_time_len = author_time.len();

            let max_summary = available_width
                .saturating_sub(prefix_len)
                .saturating_sub(author_time_len)
                .max(10);

            let summary_style = if is_selected {
                Style::default().fg(Color::White).bold()
            } else {
                Style::default()
            };
            spans.push(Span::styled(
                truncate(&entry.summary, max_summary),
                summary_style,
            ));

            spans.push(Span::styled(
                format!(" - {}", entry.author_name),
                Style::default().fg(Color::Blue),
            ));

            spans.push(Span::styled(
                format!(" {}", entry.time_relative),
                Style::default().fg(Color::DarkGray),
            ));

            let line = Line::from(spans);

            if is_selected {
                line.style(Style::default().bg(Color::DarkGray))
            } else {
                line
            }
        })
        .collect();

    let title = format!(" Log ({} commits) ", log.entries.len());
    let paragraph =
        Paragraph::new(visible_entries).block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(paragraph, area);
}

fn render_details_pane(f: &mut ratatui::Frame, area: Rect, details: Option<&CommitDetails>) {
    let content = if let Some(d) = details {
        let mut lines = Vec::new();

        lines.push(Line::from(vec![
            Span::styled("Commit: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&d.full_id, Style::default().fg(Color::Yellow)),
        ]));

        if !d.refs.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Refs: ", Style::default().fg(Color::DarkGray)),
                Span::styled(d.refs.join(", "), Style::default().fg(Color::Cyan)),
            ]));
        }

        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled(
            &d.summary,
            Style::default().bold(),
        )));

        if let Some(ref body) = d.body {
            lines.push(Line::from(""));
            for line in body.lines().take(5) {
                lines.push(Line::from(Span::styled(
                    line,
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        lines.push(Line::from(""));

        lines.push(Line::from(vec![
            Span::styled("Author: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&d.author_name),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Email: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&d.author_email),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Date: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&d.time_relative),
        ]));

        if !d.parent_ids.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Parents: ", Style::default().fg(Color::DarkGray)),
                Span::raw(d.parent_ids.join(", ")),
            ]));
        }

        lines.push(Line::from(""));

        lines.push(Line::from(vec![
            Span::styled("Changes: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("+{}", d.insertions),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" "),
            Span::styled(format!("-{}", d.deletions), Style::default().fg(Color::Red)),
            Span::styled(
                format!(" ({} files)", d.files_changed),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        lines
    } else {
        vec![Line::from("Loading...")]
    };

    let paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Commit Details "),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    } else {
        s.to_string()
    }
}
