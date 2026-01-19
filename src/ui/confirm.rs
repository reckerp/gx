use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use ratatui::{TerminalOptions, Viewport};
use std::io;

pub fn run(message: &str) -> miette::Result<bool> {
    let mut stdout = io::stdout();
    let backend = CrosstermBackend::new(&mut stdout);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(1),
        },
    )
    .into_diagnostic()?;

    enable_raw_mode().into_diagnostic()?;

    let mut selected = 0; // 0 = Yes, 1 = No

    loop {
        terminal
            .draw(|f| {
                let area = f.area();

                let yes_style = if selected == 0 {
                    Style::default().fg(Color::Black).bg(Color::Green)
                } else {
                    Style::default().fg(Color::Green)
                };

                let no_style = if selected == 1 {
                    Style::default().fg(Color::Black).bg(Color::Red)
                } else {
                    Style::default().fg(Color::Red)
                };

                let line = Line::from(vec![
                    Span::styled(format!("{} ", message), Style::default().fg(Color::Yellow)),
                    Span::styled(" y ", yes_style),
                    Span::raw(" "),
                    Span::styled(" n ", no_style),
                    Span::styled("  (y/n)", Style::default().fg(Color::DarkGray)),
                ]);

                f.render_widget(Paragraph::new(line), area);
            })
            .into_diagnostic()?;

        if let Event::Key(key) = event::read().into_diagnostic()? {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') => selected = 0,
                KeyCode::Right | KeyCode::Char('l') => selected = 1,
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    disable_raw_mode().ok();
                    println!();
                    return Ok(true);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    disable_raw_mode().ok();
                    println!();
                    return Ok(false);
                }
                KeyCode::Enter => {
                    disable_raw_mode().ok();
                    println!();
                    return Ok(selected == 0);
                }
                _ => {}
            }
        }
    }
}
