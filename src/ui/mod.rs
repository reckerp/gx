pub mod branch_picker;
pub mod confirm;
pub mod file_picker;
pub mod status;
pub mod terminal;

use ratatui::prelude::*;
use ratatui::widgets::*;
use std::io::Stdout;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn render_help_bar(hints: &[(&str, &str)]) -> Paragraph<'static> {
    let mut spans = Vec::new();

    for (i, (key, action)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!(" {} ", key),
            Style::default().fg(Color::Yellow),
        ));
        spans.push(Span::styled(
            (*action).to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL).title(" Help "))
}

pub fn status_color(status: crate::git::status::FileStatus) -> Color {
    match status {
        crate::git::status::FileStatus::New => Color::Green,
        crate::git::status::FileStatus::Modified => Color::Yellow,
        crate::git::status::FileStatus::Deleted => Color::Red,
        crate::git::status::FileStatus::Renamed => Color::Cyan,
        crate::git::status::FileStatus::Typechange => Color::Magenta,
    }
}

pub fn status_char(status: crate::git::status::FileStatus) -> char {
    match status {
        crate::git::status::FileStatus::New => 'A',
        crate::git::status::FileStatus::Modified => 'M',
        crate::git::status::FileStatus::Deleted => 'D',
        crate::git::status::FileStatus::Renamed => 'R',
        crate::git::status::FileStatus::Typechange => 'T',
    }
}
