pub mod branch_picker;
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
